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
//! O volume de cada pessoa (a voz e o som da tela, `volume.rs`) é posto em
//! cada faixa de áudio quando ela chega e de novo quando alguém mexe.
//!
//! O microfone, a saída de áudio e a câmera são os escolhidos nas
//! configurações (`devices.rs`); trocar no meio da chamada vale na hora.
//!
//! Tudo o que sai (voz, câmera, tela e som da tela) vai cifrado ponta a
//! ponta (F2-09, `sframe.rs`): sem a chave a chamada nem começa.
//!
//! Banda: ninguém baixa nada que não pediu. A voz e as câmeras chegam
//! sozinhas; a tela de alguém (com o som dela) só depois de clicar em
//! "assistir" no tile dela (`Viewing`). As câmeras vêm na camada pequena
//! (180p) e só a ampliada ou em tela cheia vem inteira. Do outro lado, o
//! LiveKit avisa quem publica quais camadas alguém está vendo (dynacast) e
//! o resto para de ser codificado e enviado: tela que ninguém assiste não
//! sobe nada para a VPS.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use livekit::options::{DegradationPreference, TrackPublishOptions};
use livekit::prelude::*;
use livekit::track::VideoQuality;
use livekit::webrtc::native::frame_cryptor::EncryptionState;
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use protocol::ConversationId;
use tokio::sync::Notify;
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
use crate::volume::{self, Volumes};
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
    /// Está mandando o som da tela, que tem volume próprio.
    pub screen_audio: bool,
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
    screen_audio: &HashSet<String>,
    now: Instant,
) -> Vec<CallPerson> {
    let mut people: Vec<CallPerson> = names
        .iter()
        .map(|(id, name)| CallPerson {
            id: id.clone(),
            name: name.clone(),
            speaking: speaking.is_speaking(id, now),
            screen_audio: screen_audio.contains(id),
        })
        .collect();
    people.sort_by_cached_key(|p| (p.id != me, p.name.to_lowercase(), p.id.clone()));
    people
}

/// O que a pessoa quer ver na chamada: de quem está assistindo a tela e
/// quais vídeos estão ampliados ou em tela cheia. A tela mexe aqui direto
/// (sem passar pela rede) e a tarefa da chamada ouve.
pub struct Viewing {
    state: Mutex<View>,
    changed: Notify,
}

#[derive(Debug, Default, Clone)]
struct View {
    /// Pessoas de quem estamos assistindo a tela.
    screens: HashSet<String>,
    /// Tiles ampliados ou em tela cheia.
    focus: HashSet<String>,
}

static VIEWING: LazyLock<Viewing> = LazyLock::new(|| Viewing {
    state: Mutex::new(View::default()),
    changed: Notify::new(),
});

pub fn viewing() -> &'static Viewing {
    &VIEWING
}

impl Viewing {
    /// Começa ou para de assistir a tela de `person`.
    pub fn watch_screen(&self, person: &str, on: bool) {
        self.change(|view| {
            if on {
                view.screens.insert(person.to_owned());
            } else {
                view.screens.remove(person);
            }
        });
    }

    /// Os tiles ampliado e em tela cheia (vazio: nenhum).
    pub fn set_focus(&self, big: &str, full: &str) {
        self.change(|view| {
            view.focus = [big, full]
                .into_iter()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
        });
    }

    fn change(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
        // `notify_one` guarda o aviso se a chamada não estiver esperando
        // agora: nenhum clique se perde.
        self.changed.notify_one();
    }

    fn get(&self) -> View {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Chamada nova: ninguém está sendo assistido ainda.
    fn reset(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = View::default();
    }
}

/// Se baixamos uma faixa de `person`: voz e câmera sempre; a tela e o som
/// dela só de quem estamos assistindo.
fn wanted(source: TrackSource, person: &str, view: &View) -> bool {
    match source {
        TrackSource::Microphone | TrackSource::Camera => true,
        TrackSource::Screenshare | TrackSource::ScreenshareAudio => view.screens.contains(person),
        _ => false,
    }
}

/// A camada da câmera de `person`: a pequena num tile da fileira, a grande
/// quando ampliada ou em tela cheia.
fn camera_quality(person: &str, view: &View) -> VideoQuality {
    if view.focus.contains(person) {
        VideoQuality::High
    } else {
        VideoQuality::Low
    }
}

/// Assina (ou larga) cada faixa de quem está na sala conforme o que a
/// pessoa quer ver, e oferece as telas que ela ainda não assiste. Pode
/// rodar quantas vezes quiser: só mexe no que mudou.
fn sync_subscriptions(room: &Room, view: &View, ui: &slint::Weak<AppWindow>) {
    for participant in room.remote_participants().into_values() {
        let id = participant.identity().to_string();
        for publication in participant.track_publications().into_values() {
            let source = publication.source();
            let want = wanted(source, &id, view);
            if publication.is_desired() != want {
                publication.set_subscribed(want);
            }
            match source {
                TrackSource::Screenshare if !want => {
                    let tile = format!("{id}{}", screen::TILE_SUFFIX);
                    let name = format!("tela de {}", display_name(&participant));
                    video::offer_tile(ui, &tile, &name);
                }
                TrackSource::Camera if want => set_camera_quality(&publication, &id, view),
                _ => {}
            }
        }
    }
}

fn set_camera_quality(publication: &RemoteTrackPublication, person: &str, view: &View) {
    if publication.simulcasted() {
        publication.set_video_quality(camera_quality(person, view));
    }
}

/// A tela e o som dela saem calados (mudos) até alguém clicar em assistir:
/// o dynacast só desliga uma camada depois que alguém já viu e largou, e
/// antes disso quem mostra mandaria a tela inteira para ninguém.
fn hold_until_watched(track: &LocalTrack) {
    track.mute();
}

/// Alguém assinou uma faixa nossa: se é a tela, começa a mandar. Quando
/// todo mundo larga, o dynacast para a tela de novo.
fn someone_watching(track: &LocalTrack) {
    if matches!(
        track.source(),
        TrackSource::Screenshare | TrackSource::ScreenshareAudio
    ) {
        track.unmute();
    }
}

/// O nome para mostrar; sem nome, o id.
fn display_name(participant: &RemoteParticipant) -> String {
    match participant.name() {
        name if name.is_empty() => participant.identity().to_string(),
        name => name,
    }
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
    viewing().reset();
    tokio::spawn(async move {
        let mut video = Video::new(devices);
        let entry = Entry { url, token, key };
        let reason = run(
            conversation_id,
            &entry,
            control_rx,
            &net,
            &ui,
            &mut video,
            volume::shared(),
        )
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
    volumes: &Volumes,
) -> Result<(), String> {
    let devices = &video.devices.clone();
    // Precisa viver a chamada inteira: largar desliga microfone e som.
    let audio = PlatformAudio::new().map_err(|err| format!("o som da chamada não abriu: {err}"))?;
    if let Err(reason) = use_audio_devices(&audio, devices, false) {
        let _ = net.send(Command::CallWarning(reason));
    }
    let mut options = RoomOptions::default();
    options.encryption = Some(sframe::options(&entry.key));
    // Cada faixa é assinada à mão (`sync_subscriptions`), e quem publica só
    // manda as camadas que alguém está vendo.
    options.auto_subscribe = false;
    options.dynacast = true;
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
    let mut view = viewing().get();
    // Quem já estava na sala não dispara `TrackPublished`.
    sync_subscriptions(&room, &view, ui);
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
                    // Se voltar mostrando a tela, é outro convite.
                    viewing().watch_screen(&id, false);
                }
                Some(RoomEvent::TrackPublished { .. }) => sync_subscriptions(&room, &view, ui),
                Some(RoomEvent::LocalTrackSubscribed { track }) => someone_watching(&track),
                // Parou de mostrar (ou trocou de janela: vem outra faixa logo
                // depois, e quem assistia continua assistindo).
                Some(RoomEvent::TrackUnpublished { publication, participant })
                    if publication.source() == TrackSource::Screenshare =>
                {
                    video.unwatch(&publication.sid(), ui);
                    let tile = format!("{}{}", participant.identity(), screen::TILE_SUFFIX);
                    video::remove_tile(ui, &tile);
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
                // O áudio de quem entra é tocado sozinho pelo WebRTC; só
                // falta pôr o volume que a pessoa escolheu para ele.
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(track), publication, participant }) => {
                    if let Some(kind) = volume_kind(publication.source()) {
                        let level = volumes.get(&participant.identity().to_string(), kind);
                        volume::apply(&track.rtc_track(), level);
                    }
                }
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Video(track), publication, participant })
                    if publication.source() == TrackSource::Camera =>
                {
                    set_camera_quality(&publication, &participant.identity().to_string(), &view);
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
                Some(RoomEvent::TrackUnsubscribed { publication, participant, .. }) => {
                    video.unwatch(&publication.sid(), ui);
                    // Parou de assistir, mas a pessoa continua mostrando:
                    // o tile volta a ser o convite.
                    if participant.track_publications().contains_key(&publication.sid()) {
                        sync_subscriptions(&room, &view, ui);
                    }
                }
                // Quem entra e sai é ouvido sozinho; o resto não interessa aqui.
                Some(_) => {}
            },
            () = volumes.changed() => apply_volumes(&room, volumes),
            () = viewing().changed.notified() => {
                view = viewing().get();
                sync_subscriptions(&room, &view, ui);
            }
            // Também confere aqui quem saiu ou mudou acima: o envio fica num
            // lugar só, no máximo dez vezes por segundo.
            _ = tick.tick() => {}
        }
        let screen_audio = sharing_audio(&room);
        let now_people = people(&me_id, &names, &speaking, &screen_audio, Instant::now());
        if now_people != shown {
            shown = now_people;
            let _ = net.send(Command::CallPeople {
                conversation_id,
                people: shown.clone(),
            });
        }
    }
}

/// Que volume vale para uma faixa de áudio: voz ou som da tela.
fn volume_kind(source: TrackSource) -> Option<volume::Kind> {
    match source {
        TrackSource::Microphone => Some(volume::Kind::Voice),
        TrackSource::ScreenshareAudio => Some(volume::Kind::Screen),
        _ => None,
    }
}

/// Põe de novo o volume escolhido em toda faixa de áudio que está tocando.
fn apply_volumes(room: &Room, volumes: &Volumes) {
    for (identity, participant) in room.remote_participants() {
        let id = identity.to_string();
        for publication in participant.track_publications().into_values() {
            let (Some(kind), Some(RemoteTrack::Audio(track))) =
                (volume_kind(publication.source()), publication.track())
            else {
                continue;
            };
            volume::apply(&track.rtc_track(), volumes.get(&id, kind));
        }
    }
}

/// Quem está mandando o som da tela agora.
fn sharing_audio(room: &Room) -> HashSet<String> {
    room.remote_participants()
        .into_iter()
        .filter(|(_, p)| {
            p.track_publications()
                .values()
                .any(|publication| publication.source() == TrackSource::ScreenshareAudio)
        })
        .map(|(identity, _)| identity.to_string())
        .collect()
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
        let track = LocalTrack::Video(LocalVideoTrack::create_video_track(
            "tela",
            RtcVideoSource::Native(source.clone()),
        ));
        let publication = room
            .local_participant()
            .publish_track(
                track.clone(),
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
        hold_until_watched(&track);
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
        let track = LocalTrack::Audio(LocalAudioTrack::create_audio_track(
            "som da tela",
            RtcAudioSource::Native(source.clone()),
        ));
        let publication = room
            .local_participant()
            .publish_track(
                track.clone(),
                TrackPublishOptions {
                    source: TrackSource::ScreenshareAudio,
                    audio_encoding: Some(livekit::options::audio::MUSIC_STEREO.encoding),
                    dtx: false,
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("o som da tela não entrou na chamada: {err}"))?;
        hold_until_watched(&track);
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
        let name = display_name(participant);
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
    fn tela_so_baixa_de_quem_assistimos() {
        let mut view = View::default();
        assert!(wanted(TrackSource::Microphone, "ana", &view));
        assert!(wanted(TrackSource::Camera, "ana", &view));
        assert!(!wanted(TrackSource::Screenshare, "ana", &view));
        assert!(!wanted(TrackSource::ScreenshareAudio, "ana", &view));
        view.screens.insert("ana".into());
        assert!(wanted(TrackSource::Screenshare, "ana", &view));
        assert!(wanted(TrackSource::ScreenshareAudio, "ana", &view));
        assert!(!wanted(TrackSource::Screenshare, "bia", &view));
    }

    #[test]
    fn camera_grande_so_quando_ampliada() {
        let mut view = View::default();
        assert_eq!(camera_quality("ana", &view), VideoQuality::Low);
        view.focus.insert("ana".into());
        assert_eq!(camera_quality("ana", &view), VideoQuality::High);
        assert_eq!(camera_quality("bia", &view), VideoQuality::Low);
    }

    /// Com o LiveKit de verdade: quem mostra a tela não manda nada enquanto
    /// ninguém assiste, manda quando alguém clica em assistir e para de
    /// novo quando a pessoa larga. Mede os bytes que saem do app de quem
    /// mostra.
    ///
    /// `docker run --network host livekit/livekit-server --dev --bind 127.0.0.1`
    /// e `MOTHH_LIVEKIT_URL=ws://127.0.0.1:7880 MOTHH_LIVEKIT_KEY=devkey
    /// MOTHH_LIVEKIT_SECRET=secret`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de um LiveKit rodando"]
    async fn livekit_tela_so_sobe_com_alguem_assistindo() {
        use livekit::webrtc::prelude::{I420Buffer, VideoFrame, VideoRotation};
        use livekit::webrtc::stats::RtcStats;

        let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("falta {name}"));
        let url = var("MOTHH_LIVEKIT_URL");
        let api_key = var("MOTHH_LIVEKIT_KEY");
        let api_secret = var("MOTHH_LIVEKIT_SECRET");
        let room_name = format!("tela-{}", uuid::Uuid::new_v4());
        let join = |who: &str| {
            let token = livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
                .with_identity(who)
                .with_grants(livekit_api::access_token::VideoGrants {
                    room_join: true,
                    room: room_name.clone(),
                    ..Default::default()
                })
                .to_jwt()
                .unwrap();
            let url = url.clone();
            async move {
                // As mesmas opções da chamada de verdade (`run`).
                let mut options = RoomOptions::default();
                options.auto_subscribe = false;
                options.dynacast = true;
                Room::connect(&url, &token, options).await.unwrap()
            }
        };
        let (mostra, mut mostra_events) = join("mostra").await;
        let (assiste, mut events) = join("assiste").await;

        // A "tela": 1080p mudando a cada quadro, como `publish_screen`.
        let source = NativeVideoSource::new(
            VideoResolution {
                width: screen::WIDTH,
                height: screen::HEIGHT,
            },
            true,
        );
        let track =
            LocalVideoTrack::create_video_track("tela", RtcVideoSource::Native(source.clone()));
        mostra
            .local_participant()
            .publish_track(
                LocalTrack::Video(track.clone()),
                TrackPublishOptions {
                    source: TrackSource::Screenshare,
                    video_encoding: Some(screen::ENCODING),
                    simulcast: false,
                    degradation_preference: Some(DegradationPreference::Balanced),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        hold_until_watched(&LocalTrack::Video(track.clone()));
        let publisher = tokio::spawn(async move {
            while let Some(event) = mostra_events.recv().await {
                if let RoomEvent::LocalTrackSubscribed { track } = event {
                    someone_watching(&track);
                }
            }
        });
        let feed = tokio::spawn(async move {
            let mut n = 0u8;
            let mut tick = interval(Duration::from_millis(33));
            loop {
                tick.tick().await;
                let mut buffer = I420Buffer::new(screen::WIDTH, screen::HEIGHT);
                let (y, _, _) = buffer.data_mut();
                for (i, p) in y.iter_mut().enumerate() {
                    *p = (i as u8).wrapping_mul(7).wrapping_add(n);
                }
                n = n.wrapping_add(13);
                source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
            }
        });

        let sent = || {
            let track = track.clone();
            async move {
                track
                    .get_stats()
                    .await
                    .unwrap()
                    .iter()
                    .map(|s| match s {
                        RtcStats::OutboundRtp(o) => o.sent.bytes_sent,
                        _ => 0,
                    })
                    .sum::<u64>()
            }
        };
        // Quanto saiu em 3 s, depois de dar tempo ao LiveKit de avisar (ele
        // espera 5 s antes de desligar uma camada).
        let rate = || async {
            tokio::time::sleep(Duration::from_secs(7)).await;
            let before = sent().await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            (sent().await - before) / 3
        };

        let ui = slint::Weak::<AppWindow>::default();
        let mut view = View::default();
        sync_subscriptions(&assiste, &view, &ui);
        let ninguem = rate().await;
        eprintln!("ninguém assistindo: {ninguem} bytes/s");

        view.screens.insert("mostra".into());
        sync_subscriptions(&assiste, &view, &ui);
        let assistindo = rate().await;
        eprintln!("assistindo: {assistindo} bytes/s");
        // E o vídeo chegou mesmo.
        let chegou = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(RoomEvent::TrackSubscribed { publication, .. }) = events.recv().await
                    && publication.source() == TrackSource::Screenshare
                {
                    break;
                }
            }
        })
        .await;
        assert!(
            chegou.is_ok(),
            "a tela não chegou para quem clicou em assistir"
        );

        view.screens.clear();
        sync_subscriptions(&assiste, &view, &ui);
        let largou = rate().await;
        eprintln!("parou de assistir: {largou} bytes/s");

        feed.abort();
        publisher.abort();
        // Muda, a faixa ainda manda quadros pretos (uns 10 KB/s, contra uns
        // 500 KB/s da tela de verdade); depois que o dynacast desliga, nada.
        assert!(
            ninguem < 20_000,
            "mandou {ninguem} bytes/s sem ninguém assistindo"
        );
        assert!(
            assistindo > 50_000,
            "mandou só {assistindo} bytes/s com alguém assistindo"
        );
        assert!(largou < 5_000, "mandou {largou} bytes/s depois de largarem");
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
            people("1", &names, s, &HashSet::new(), t0)
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        speaking.update(set(&["2"]), t0);
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        assert!(people("1", &names, &speaking, &HashSet::new(), t0)[2].speaking);
    }
}
