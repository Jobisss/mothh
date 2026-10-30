//! Chamada de voz pelo LiveKit.
//!
//! O servidor do mothh só entrega o token (`CallToken`); a voz vai direto do
//! app para o LiveKit. O microfone e o alto-falante ficam com o próprio
//! WebRTC (`PlatformAudio`), que já faz cancelamento de eco, redução de
//! ruído e ganho automático, e toca sozinho o áudio de quem está na sala.
//!
//! A chamada roda numa tarefa Tokio própria. A rede manda ordens por um
//! `CallHandle` e fica sabendo do resultado pelo canal de comandos
//! (`Command::CallJoined`, `Command::CallPeople`, `Command::CallEnded` e
//! `Command::CameraFailed`).
//!
//! Quem está falando vem do LiveKit: o servidor dele mede o nível de áudio
//! de cada um e avisa quem passou do limiar ou voltou para baixo dele
//! (`ActiveSpeakersChanged`). Para
//! o destaque não piscar nas pausas entre palavras, quem para de falar fica
//! destacado mais um pouco (`HOLD`).
//!
//! A câmera (F2-05) e a tela compartilhada (F2-06) são faixas de vídeo
//! publicadas só enquanto estão ligadas; a captura da câmera e os tiles na
//! tela ficam em `video.rs`, a captura da tela em `screen.rs`. O som da
//! tela, quando a pessoa pede, é outra faixa (`share_audio.rs`).
//!
//! O microfone, a saída de áudio e a câmera são os escolhidos nas
//! configurações (`devices.rs`); trocar no meio da chamada vale na hora.
//!
//! Tudo o que sai (voz, câmera, tela e som da tela) vai cifrado ponta a
//! ponta (F2-09, `sframe.rs`): sem a chave a chamada nem começa.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_util::StreamExt;
use livekit::options::{DegradationPreference, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::native::frame_cryptor::EncryptionState;
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use protocol::ConversationId;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval};

use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};

use crate::devices::{self, Devices};
use crate::screen::{self, ScreenCapture};
use crate::sframe::{self, MediaKey};
use crate::share_audio::{self, SoundCapture};
use crate::video::{self, CameraCapture};
use crate::{AppWindow, Command};

/// Quanto tempo o destaque fica depois que o LiveKit diz que a pessoa parou
/// de falar. Cobre a pausa entre uma palavra e outra.
const HOLD: Duration = Duration::from_millis(500);

/// De quanto em quanto tempo confere se algum destaque venceu.
const TICK: Duration = Duration::from_millis(100);

/// Uma pessoa na chamada, como a tela mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPerson {
    /// Identidade no LiveKit: o id do usuário no mothh.
    pub id: String,
    pub name: String,
    pub speaking: bool,
}

/// Quem está falando, com o tempo extra do `HOLD` para não piscar.
#[derive(Debug, Default)]
struct Speaking {
    /// Quem o LiveKit diz que fala agora.
    active: HashSet<String>,
    /// Quem parou há pouco, e até quando continua destacado.
    until: HashMap<String, Instant>,
}

impl Speaking {
    /// Chegou a lista nova do LiveKit.
    fn update(&mut self, active: HashSet<String>, now: Instant) {
        for id in self.active.difference(&active) {
            self.until.insert(id.clone(), now + HOLD);
        }
        for id in &active {
            self.until.remove(id);
        }
        self.active = active;
    }

    /// Tira o destaque na hora (saiu da sala ou mutou).
    fn forget(&mut self, id: &str) {
        self.active.remove(id);
        self.until.remove(id);
    }

    fn is_speaking(&self, id: &str, now: Instant) -> bool {
        self.active.contains(id) || self.until.get(id).is_some_and(|&end| now < end)
    }
}

/// Quem está na sala e quem fala. Eu primeiro, os outros por nome, para
/// ninguém trocar de lugar quando começa a falar.
fn people(
    me: &str,
    names: &HashMap<String, String>,
    speaking: &Speaking,
    now: Instant,
) -> Vec<CallPerson> {
    let mut people: Vec<CallPerson> = names
        .iter()
        .map(|(id, name)| CallPerson {
            id: id.clone(),
            name: name.clone(),
            speaking: speaking.is_speaking(id, now),
        })
        .collect();
    people.sort_by_cached_key(|p| (p.id != me, p.name.to_lowercase(), p.id.clone()));
    people
}

/// O que a rede pede para a chamada em andamento.
enum Control {
    Mute(bool),
    Camera(bool),
    /// Compartilhar uma tela ou janela, ou parar (`None`).
    Share(Option<ShareRequest>),
    /// A pessoa trocou o microfone, a saída ou a câmera nas configurações.
    Devices,
    /// O grupo trocou de época: chave nova da mídia.
    Key(MediaKey),
    Leave,
}

/// O que compartilhar.
#[derive(Debug, Clone)]
pub struct ShareRequest {
    /// O id de um `screen::ShareSource`.
    pub source_id: String,
    /// Como a lista chama ("tela 1", o título da janela), para o tile.
    pub name: String,
    /// Levar o som junto.
    pub audio: bool,
}

/// Controle de uma chamada. Largar o `CallHandle` também desliga ela.
pub struct CallHandle {
    pub conversation_id: ConversationId,
    control: UnboundedSender<Control>,
}

impl CallHandle {
    pub fn set_muted(&self, muted: bool) {
        let _ = self.control.send(Control::Mute(muted));
    }

    pub fn set_camera(&self, on: bool) {
        let _ = self.control.send(Control::Camera(on));
    }

    pub fn set_share(&self, request: Option<ShareRequest>) {
        let _ = self.control.send(Control::Share(request));
    }

    pub fn devices_changed(&self) {
        let _ = self.control.send(Control::Devices);
    }

    /// O grupo MLS da conversa mudou de época: cifra com a chave nova.
    pub fn set_key(&self, key: MediaKey) {
        let _ = self.control.send(Control::Key(key));
    }

    pub fn leave(&self) {
        let _ = self.control.send(Control::Leave);
    }
}

/// Entra na sala do LiveKit com o token que o servidor mandou. `key` é a
/// chave de mídia da época atual do grupo.
pub fn start(
    conversation_id: ConversationId,
    url: String,
    token: String,
    key: MediaKey,
    net: UnboundedSender<Command>,
    ui: slint::Weak<AppWindow>,
    devices: Devices,
) -> CallHandle {
    let (control, control_rx) = unbounded_channel();
    tokio::spawn(async move {
        let mut video = Video::new(devices);
        let entry = Entry { url, token, key };
        let reason = run(conversation_id, &entry, control_rx, &net, &ui, &mut video)
            .await
            .err();
        video.stop_all();
        video::clear_tiles(&ui);
        let _ = net.send(Command::CallEnded {
            conversation_id,
            reason,
        });
    });
    CallHandle {
        conversation_id,
        control,
    }
}

/// O que é preciso para entrar na sala: endereço e token do LiveKit e a
/// chave de mídia da época atual.
struct Entry {
    url: String,
    token: String,
    key: MediaKey,
}

/// A chamada do começo ao fim. `Ok` quando a pessoa saiu; `Err` com uma
/// frase para a tela quando não deu para entrar ou a chamada caiu.
async fn run(
    conversation_id: ConversationId,
    entry: &Entry,
    mut control: UnboundedReceiver<Control>,
    net: &UnboundedSender<Command>,
    ui: &slint::Weak<AppWindow>,
    video: &mut Video,
) -> Result<(), String> {
    let devices = &video.devices.clone();
    // Precisa viver a chamada inteira: largar desliga microfone e som.
    let audio = PlatformAudio::new().map_err(|err| format!("o som da chamada não abriu: {err}"))?;
    if let Err(reason) = use_audio_devices(&audio, devices, false) {
        let _ = net.send(Command::CallWarning(reason));
    }
    let mut options = RoomOptions::default();
    options.encryption = Some(sframe::options(&entry.key));
    let (room, mut events) = Room::connect(&entry.url, &entry.token, options)
        .await
        .map_err(|err| format!("não deu para entrar na chamada: {err}"))?;

    let mic = LocalAudioTrack::create_audio_track("microfone", audio.rtc_source());
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(mic.clone()),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await
        .map_err(|err| format!("o microfone não entrou na chamada: {err}"))?;
    sframe::use_latest_key(&room);
    let _ = net.send(Command::CallJoined(conversation_id));

    let me = room.local_participant();
    let me_id = me.identity().to_string();
    let mut names: HashMap<String, String> = room
        .remote_participants()
        .into_values()
        .map(|p| (p.identity().to_string(), p.name()))
        .collect();
    names.insert(me_id.clone(), me.name());
    let mut speaking = Speaking::default();
    // O que a tela já tem; só manda de novo quando muda.
    let mut shown = Vec::new();
    let mut tick = interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            order = control.recv() => match order {
                Some(Control::Mute(true)) => {
                    mic.mute();
                    speaking.forget(&me_id);
                }
                Some(Control::Mute(false)) => mic.unmute(),
                Some(Control::Camera(true)) => {
                    if let Err(reason) = video.publish_camera(&room, net, ui).await {
                        let _ = net.send(Command::CameraFailed(reason));
                    }
                }
                Some(Control::Camera(false)) => video.unpublish_camera(&room, ui).await,
                Some(Control::Share(Some(request))) => {
                    if let Err(reason) = video.publish_screen(&room, request, net, ui).await {
                        let _ = net.send(Command::ShareFailed(reason));
                    }
                }
                Some(Control::Devices) => {
                    if let Err(reason) = use_audio_devices(&audio, devices, true) {
                        let _ = net.send(Command::CallWarning(reason));
                    }
                    // Câmera ligada e outra escolhida: desliga e liga com a nova.
                    let wanted = devices.get().camera.map(|c| c.name);
                    if video.camera.is_some() && video.camera_name != wanted {
                        video.unpublish_camera(&room, ui).await;
                        if let Err(reason) = video.publish_camera(&room, net, ui).await {
                            let _ = net.send(Command::CameraFailed(reason));
                        }
                    }
                }
                Some(Control::Share(None)) => video.unpublish_screen(&room, ui).await,
                Some(Control::Key(key)) => sframe::set_key(&room, &key),
                // `None`: a rede largou o controle, então sai também.
                Some(Control::Leave) | None => {
                    let _ = room.close().await;
                    return Ok(());
                }
            },
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("a chamada caiu ({reason:?})"));
                }
                None => return Err("a chamada caiu".into()),
                Some(RoomEvent::ParticipantConnected(p)) => {
                    names.insert(p.identity().to_string(), p.name());
                }
                Some(RoomEvent::ParticipantDisconnected(p)) => {
                    let id = p.identity().to_string();
                    names.remove(&id);
                    speaking.forget(&id);
                }
                // O evento traz só quem mudou desde o último aviso; quem
                // continua falando não vem nele. O estado de cada pessoa fica
                // guardado no próprio participante (`is_speaking`).
                Some(RoomEvent::ActiveSpeakersChanged { .. }) => {
                    let mut active: HashSet<String> = room
                        .remote_participants()
                        .into_values()
                        .filter(|p| p.is_speaking())
                        .map(|p| p.identity().to_string())
                        .collect();
                    if me.is_speaking() {
                        active.insert(me_id.clone());
                    }
                    speaking.update(active, Instant::now());
                }
                // O áudio de quem entra é tocado sozinho; o resto não interessa aqui.
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Video(track), publication, participant })
                    if publication.source() == TrackSource::Camera =>
                {
                    video.watch(track, &participant, "", "", false, ui);
                }
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Video(track), publication, participant })
                    if publication.source() == TrackSource::Screenshare =>
                {
                    video.watch(track, &participant, screen::TILE_SUFFIX, "tela de ", true, ui);
                }
                // Um quadro que não abriu: chave errada ou faltando. Só vai
                // para o log; a pessoa nota pelo silêncio.
                Some(RoomEvent::E2eeStateChanged { participant, state })
                    if !matches!(state, EncryptionState::New | EncryptionState::Ok) =>
                {
                    eprintln!("cifra da chamada com {}: {state:?}", participant.identity());
                }
                Some(RoomEvent::TrackUnsubscribed { publication, .. }) => {
                    video.unwatch(&publication.sid(), ui);
                }
                // Quem entra e sai é ouvido sozinho; o resto não interessa aqui.
                Some(_) => {}
            },
            // Também confere aqui quem saiu ou mudou acima: o envio fica num
            // lugar só, no máximo dez vezes por segundo.
            _ = tick.tick() => {}
        }
        let now_people = people(&me_id, &names, &speaking, Instant::now());
        if now_people != shown {
            shown = now_people;
            let _ = net.send(Command::CallPeople {
                conversation_id,
                people: shown.clone(),
            });
        }
    }
}

/// Põe o microfone e a saída escolhidos no som da chamada. Com `switch`, a
/// chamada já está tocando e o WebRTC precisa parar e religar o
/// dispositivo. Escolha desconectada vale como o padrão do sistema; o erro
/// é só quando o WebRTC recusa a troca.
fn use_audio_devices(audio: &PlatformAudio, devices: &Devices, switch: bool) -> Result<(), String> {
    let prefs = devices.get();
    let as_choice = |id: &RecordingOrPlayout| devices::Choice {
        id: id.0.clone(),
        name: id.1.clone(),
    };
    let mics: Vec<RecordingOrPlayout> = audio
        .recording_devices()
        .map(|d| (d.id.as_str().to_owned(), d.name))
        .collect();
    let speakers: Vec<RecordingOrPlayout> = audio
        .playout_devices()
        .map(|d| (d.id.as_str().to_owned(), d.name))
        .collect();
    let mic_list: Vec<_> = mics.iter().map(as_choice).collect();
    let speaker_list: Vec<_> = speakers.iter().map(as_choice).collect();

    if let Some(mic) = pick(&mic_list, &prefs.mic, devices::default_mic_name()) {
        let id = RecordingDeviceId::from_unchecked_guid(&mic.id);
        let done = if switch {
            audio.switch_recording_device(&id)
        } else {
            audio.set_recording_device(&id)
        };
        done.map_err(|err| format!("não deu para trocar o microfone: {err}"))?;
    }
    if let Some(speaker) = pick(
        &speaker_list,
        &prefs.speaker,
        devices::default_speaker_name(),
    ) {
        let id = PlayoutDeviceId::from_unchecked_guid(&speaker.id);
        let done = if switch {
            audio.switch_playout_device(&id)
        } else {
            audio.set_playout_device(&id)
        };
        done.map_err(|err| format!("não deu para trocar a saída de áudio: {err}"))?;
    }
    Ok(())
}

/// Id e nome de um dispositivo do WebRTC.
type RecordingOrPlayout = (String, String);

/// O dispositivo da lista do WebRTC a usar: o escolhido ou, no padrão do
/// sistema, o que o sistema diz que é o padrão (`default_name`, achado pelo
/// nome). `None`: não mexe, o WebRTC fica com o que já tem.
fn pick(
    list: &[devices::Choice],
    chosen: &Option<devices::Choice>,
    default_name: Option<String>,
) -> Option<devices::Choice> {
    match devices::position(list, chosen) {
        0 => {
            let name = default_name?;
            list.iter().find(|c| c.name == name).cloned()
        }
        i => list.get(i - 1).cloned(),
    }
}

/// Os vídeos de uma chamada: a nossa câmera e a nossa tela, se ligadas, e
/// os que assistimos.
struct Video {
    /// Qual câmera usar.
    devices: Devices,
    /// A faixa da câmera publicada e a captura que alimenta ela.
    camera: Option<(TrackSid, CameraCapture)>,
    /// O nome da câmera em uso (`None`: a do sistema), para saber se a
    /// escolha nas configurações mudou.
    camera_name: Option<String>,
    /// A faixa da tela publicada e a captura dela.
    screen: Option<(TrackSid, ScreenCapture)>,
    /// O som da tela, quando vai junto.
    screen_audio: Option<(TrackSid, SoundCapture)>,
    /// Vídeos dos outros: a tarefa que copia os quadros e o id do tile.
    watching: HashMap<TrackSid, (JoinHandle<()>, String)>,
}

impl Video {
    fn new(devices: Devices) -> Self {
        Self {
            devices,
            camera: None,
            camera_name: None,
            screen: None,
            screen_audio: None,
            watching: HashMap::new(),
        }
    }

    /// Liga a câmera e publica em 360p a 30 quadros. A captura avisa a rede se parar.
    async fn publish_camera(
        &mut self,
        room: &Room,
        net: &UnboundedSender<Command>,
        ui: &slint::Weak<AppWindow>,
    ) -> Result<(), String> {
        if self.camera.is_some() {
            return Ok(());
        }
        let source = NativeVideoSource::new(
            VideoResolution {
                width: video::WIDTH,
                height: video::HEIGHT,
            },
            false,
        );
        let track =
            LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));
        let publication = room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    source: TrackSource::Camera,
                    video_encoding: Some(video::ENCODING),
                    // Uma camada de 180p junto, para o LiveKit mandar a menor
                    // a quem tem a rede fraca sem a VPS recodificar nada.
                    simulcast: true,
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("a câmera não entrou na chamada: {err}"))?;
        sframe::use_latest_key(room);
        video::add_tile(ui, video::ME, "você", false);
        let chosen = self.devices.get().camera;
        let index = tokio::task::spawn_blocking({
            let chosen = chosen.clone();
            move || devices::camera_index(&chosen)
        })
        .await
        .unwrap_or(nokhwa::utils::CameraIndex::Index(0));
        let net = net.clone();
        let capture = video::start_camera(index, source, ui.clone(), move |reason| {
            let _ = net.send(Command::CameraFailed(reason));
        });
        self.camera = Some((publication.sid(), capture));
        self.camera_name = chosen.map(|c| c.name);
        Ok(())
    }

    async fn unpublish_camera(&mut self, room: &Room, ui: &slint::Weak<AppWindow>) {
        self.camera_name = None;
        if let Some((sid, capture)) = self.camera.take() {
            video::remove_tile(ui, video::ME);
            // A thread da câmera espera o próximo quadro para parar.
            let _ = tokio::task::spawn_blocking(move || drop(capture)).await;
            let _ = room.local_participant().unpublish_track(&sid).await;
        }
    }

    /// Compartilha a tela ou janela pedida em até 1080p a 30 quadros por
    /// segundo, e o som dela se for pedido. Trocar de tela para a anterior
    /// antes.
    async fn publish_screen(
        &mut self,
        room: &Room,
        request: ShareRequest,
        net: &UnboundedSender<Command>,
        ui: &slint::Weak<AppWindow>,
    ) -> Result<(), String> {
        self.unpublish_screen(room, ui).await;
        let source = NativeVideoSource::new(
            VideoResolution {
                width: screen::WIDTH,
                height: screen::HEIGHT,
            },
            true,
        );
        let track =
            LocalVideoTrack::create_video_track("tela", RtcVideoSource::Native(source.clone()));
        let publication = room
            .local_participant()
            .publish_track(
                LocalTrack::Video(track),
                TrackPublishOptions {
                    source: TrackSource::Screenshare,
                    // 1080p a 30 quadros. Com a rede ou a CPU no limite, o
                    // padrão da tela (manter a resolução) derrubava os
                    // quadros; o equilíbrio cede um pouco de cada.
                    video_encoding: Some(screen::ENCODING),
                    simulcast: false,
                    degradation_preference: Some(DegradationPreference::Balanced),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("a tela não entrou na chamada: {err}"))?;
        sframe::use_latest_key(room);
        let label = if request.source_id.starts_with("s:") {
            format!("você mostra: {}", request.name)
        } else {
            format!("você mostra a janela: {}", request.name)
        };
        video::add_tile(ui, screen::ME, &label, false);
        let failed = net.clone();
        let capture = screen::start_share(
            request.source_id.clone(),
            source,
            ui.clone(),
            move |reason| {
                let _ = failed.send(Command::ShareFailed(reason));
            },
        );
        self.screen = Some((publication.sid(), capture));
        if request.audio && share_audio::supported() {
            // Sem som a tela continua: só avisa.
            if let Err(reason) = self
                .publish_screen_audio(room, &request.source_id, net)
                .await
            {
                let _ = net.send(Command::CallWarning(reason));
            }
        }
        Ok(())
    }

    /// Publica o som da tela (`source_id` diz se é a tela toda ou uma
    /// janela) como música: estéreo, sem cancelamento de eco nem redução de
    /// ruído, sem cortar nos silêncios.
    async fn publish_screen_audio(
        &mut self,
        room: &Room,
        source_id: &str,
        net: &UnboundedSender<Command>,
    ) -> Result<(), String> {
        let target = match source_id.strip_prefix("w:").and_then(|id| id.parse().ok()) {
            Some(window) => share_audio::Target::Window(window),
            None => share_audio::Target::Screen,
        };
        let source = NativeAudioSource::new(
            AudioSourceOptions {
                echo_cancellation: false,
                noise_suppression: false,
                auto_gain_control: false,
            },
            share_audio::RATE,
            share_audio::CHANNELS,
            100,
        );
        let track = LocalAudioTrack::create_audio_track(
            "som da tela",
            RtcAudioSource::Native(source.clone()),
        );
        let publication = room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::ScreenshareAudio,
                    audio_encoding: Some(livekit::options::audio::MUSIC_STEREO.encoding),
                    dtx: false,
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("o som da tela não entrou na chamada: {err}"))?;
        sframe::use_latest_key(room);
        let net = net.clone();
        let capture = share_audio::start(
            target,
            source,
            tokio::runtime::Handle::current(),
            move |reason| {
                let _ = net.send(Command::CallWarning(reason));
            },
        );
        self.screen_audio = Some((publication.sid(), capture));
        Ok(())
    }

    async fn unpublish_screen(&mut self, room: &Room, ui: &slint::Weak<AppWindow>) {
        if let Some((sid, capture)) = self.screen_audio.take() {
            let _ = tokio::task::spawn_blocking(move || drop(capture)).await;
            let _ = room.local_participant().unpublish_track(&sid).await;
        }
        if let Some((sid, capture)) = self.screen.take() {
            video::remove_tile(ui, screen::ME);
            let _ = tokio::task::spawn_blocking(move || drop(capture)).await;
            let _ = room.local_participant().unpublish_track(&sid).await;
        }
    }

    /// Começa a mostrar um vídeo de alguém num tile. O id do tile é o da
    /// pessoa mais `suffix` (vazio para a câmera), e o nome ganha `prefix`.
    fn watch(
        &mut self,
        track: RemoteVideoTrack,
        participant: &RemoteParticipant,
        suffix: &str,
        prefix: &str,
        enlarge: bool,
        ui: &slint::Weak<AppWindow>,
    ) {
        let id = participant.identity().to_string();
        let name = match participant.name() {
            name if name.is_empty() => id.clone(),
            name => name,
        };
        let tile = format!("{id}{suffix}");
        let sid = track.sid();
        video::add_tile(ui, &tile, &format!("{prefix}{name}"), enlarge);
        let ui = ui.clone();
        let tile_id = tile.clone();
        let task = tokio::spawn(async move {
            let mut frames = NativeVideoStream::new(track.rtc_track());
            let tile = video::TileFeed::new(&ui, &tile_id);
            // Com `MOTHH_FPS=1`, escreve no terminal quantos quadros por
            // segundo chegam de cada vídeo, para medir sem cronômetro.
            let measure = std::env::var("MOTHH_FPS").is_ok_and(|v| v == "1");
            let (mut count, mut since) = (0u32, Instant::now());
            while let Some(frame) = frames.next().await {
                tile.show(video::frame_to_pixels(&frame));
                if measure {
                    count += 1;
                    if since.elapsed() >= Duration::from_secs(1) {
                        let (w, h) = (frame.buffer.width(), frame.buffer.height());
                        eprintln!("[fps] {tile_id}: {count} quadros/s em {w}x{h}");
                        (count, since) = (0, Instant::now());
                    }
                }
            }
        });
        if let Some((old, _)) = self.watching.insert(sid, (task, tile)) {
            old.abort();
        }
    }

    fn unwatch(&mut self, sid: &TrackSid, ui: &slint::Weak<AppWindow>) {
        if let Some((task, id)) = self.watching.remove(sid) {
            task.abort();
            video::remove_tile(ui, &id);
        }
    }

    /// Fim da chamada: para tudo. As capturas próprias param sozinhas ao
    /// largar.
    fn stop_all(&mut self) {
        for (task, _) in self.watching.values() {
            task.abort();
        }
        self.watching.clear();
        if let Some((_, capture)) = self.camera.take() {
            std::thread::spawn(move || drop(capture));
        }
        if let Some((_, capture)) = self.screen.take() {
            std::thread::spawn(move || drop(capture));
        }
        if let Some((_, capture)) = self.screen_audio.take() {
            std::thread::spawn(move || drop(capture));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn pausa_curta_nao_apaga_o_destaque() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["ana"]), t0);
        assert!(speaking.is_speaking("ana", t0));
        // O LiveKit diz que ela parou (pausa entre palavras)...
        speaking.update(set(&[]), t0 + Duration::from_millis(400));
        assert!(speaking.is_speaking("ana", t0 + Duration::from_millis(600)));
        // ...e que voltou antes do HOLD vencer: o destaque nunca apagou.
        speaking.update(set(&["ana"]), t0 + Duration::from_millis(800));
        assert!(speaking.is_speaking("ana", t0 + Duration::from_secs(5)));
    }

    #[test]
    fn destaque_some_depois_do_hold() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["ana", "bia"]), t0);
        speaking.update(set(&["bia"]), t0 + Duration::from_secs(3));
        let later = t0 + Duration::from_secs(3) + HOLD;
        assert!(!speaking.is_speaking("ana", later));
        assert!(speaking.is_speaking("bia", later));
    }

    #[test]
    fn mutar_tira_o_destaque_na_hora() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["eu"]), t0);
        speaking.forget("eu");
        assert!(!speaking.is_speaking("eu", t0));
    }

    #[test]
    fn eu_primeiro_e_ordem_nao_muda_ao_falar() {
        let t0 = Instant::now();
        let names: HashMap<String, String> = [("1", "zeca"), ("2", "Bia"), ("3", "ana")]
            .into_iter()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect();
        let mut speaking = Speaking::default();
        let order = |s: &Speaking| -> Vec<String> {
            people("1", &names, s, t0)
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        speaking.update(set(&["2"]), t0);
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        assert!(people("1", &names, &speaking, t0)[2].speaking);
    }
}
