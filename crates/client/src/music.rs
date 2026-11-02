//! Estado do painel e ouvinte da sala musical (separada da chamada MLS).
use crate::{AppWindow, Command, MusicQueueRow, MusicViewData, volume};
use futures_util::StreamExt;
use livekit::{prelude::*, webrtc::audio_stream::native::NativeAudioStream};
use protocol::{
    ClientMessage, ConversationId, MusicCommand, MusicItemStatus, MusicPlaybackState,
    MusicSnapshot, UserId,
};
use slint::{ModelRc, VecModel};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc::UnboundedSender, watch},
    task::JoinHandle,
    time::{Instant, timeout},
};
use uuid::Uuid;

pub enum Event {
    Connected(Uuid),
    Ended(Uuid, Option<String>),
    Spectrum(Uuid, [f32; 32]),
}

struct Cached {
    snapshot: MusicSnapshot,
    received: Instant,
}
struct Listener {
    attempt: Uuid,
    session: Uuid,
    control: watch::Sender<i32>,
}
// Largar o último sender faz o ouvinte fechar a sala; não abortar antes de close().

pub struct Frontend {
    pub panel_open: bool,
    pub enabled: bool,
    pub online: bool,
    snapshots: HashMap<ConversationId, Cached>,
    retired: HashSet<Uuid>,
    errors: HashMap<ConversationId, String>,
    pending: HashMap<Uuid, (ConversationId, String)>,
    desired: Option<ConversationId>,
    listener: Option<Listener>,
    listening: bool,
    token_pending: bool,
    volume: i32,
    bands: [f32; 32],
}

impl Default for Frontend {
    fn default() -> Self {
        Self {
            panel_open: false,
            enabled: true,
            online: false,
            snapshots: HashMap::new(),
            retired: HashSet::new(),
            errors: HashMap::new(),
            pending: HashMap::new(),
            desired: None,
            listener: None,
            listening: false,
            token_pending: false,
            volume: volume::shared()
                .get("mothh:music", volume::Kind::Screen)
                .clamp(0, 100),
            bands: [0.0; 32],
        }
    }
}

fn message(id: ConversationId, command: MusicCommand) -> ClientMessage {
    ClientMessage::Music {
        conversation_id: id,
        command,
        expected_revision: None,
    }
}

impl Frontend {
    pub fn lost_connection(&mut self) {
        for cached in self.snapshots.values_mut() {
            if cached.snapshot.state == MusicPlaybackState::Playing {
                let duration = cached
                    .snapshot
                    .current
                    .as_ref()
                    .and_then(|item| item.track.as_ref())
                    .map_or(0, |track| track.duration_ms);
                cached.snapshot.position_ms = (cached.snapshot.position_ms
                    + cached.received.elapsed().as_millis() as u64)
                    .min(duration);
                cached.received = Instant::now();
            }
        }
        self.online = false;
        self.listener = None;
        self.listening = false;
        self.token_pending = false;
        self.bands = [0.0; 32];
        // Não reenviar mutações após reconectar: pedido pode ter sido aceito.
        self.pending.clear();
    }

    pub fn welcome(
        &mut self,
        groups: &[ConversationId],
        open: Option<ConversationId>,
        outgoing: &mut Vec<ClientMessage>,
    ) {
        self.online = true;
        self.snapshots.retain(|id, _| groups.contains(id));
        if self.desired.is_some_and(|id| !groups.contains(&id)) {
            self.stop_listening();
        }
        let mut wanted = vec![];
        if self.enabled
            && self.panel_open
            && let Some(id) = open.filter(|id| groups.contains(id))
        {
            wanted.push(id);
        }
        if let Some(id) = self.desired
            && !wanted.contains(&id)
        {
            wanted.push(id);
        }
        for id in wanted {
            outgoing.push(message(id, MusicCommand::GetSnapshot));
        }
        if self.enabled
            && let Some(id) = self.desired
        {
            self.request_token(id, outgoing);
        }
    }

    pub fn opened(&mut self, id: Option<ConversationId>, outgoing: &mut Vec<ClientMessage>) {
        if self.enabled
            && self.panel_open
            && self.online
            && let Some(id) = id
        {
            outgoing.push(message(id, MusicCommand::GetSnapshot));
        }
    }

    pub fn removed(&mut self, id: ConversationId) {
        self.snapshots.remove(&id);
        self.errors.remove(&id);
        self.pending.retain(|_, (room, _)| *room != id);
        if self.desired == Some(id) {
            self.stop_listening();
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.panel_open = false;
            self.stop_listening();
        }
    }

    fn stop_listening(&mut self) {
        self.listener = None;
        self.desired = None;
        self.listening = false;
        self.token_pending = false;
        self.bands = [0.0; 32];
    }

    fn request_token(&mut self, id: ConversationId, outgoing: &mut Vec<ClientMessage>) {
        if self.token_pending || !self.online || !self.enabled {
            return;
        }
        self.token_pending = true;
        outgoing.push(message(id, MusicCommand::Listen));
    }

    pub fn action(
        &mut self,
        open: Option<ConversationId>,
        me: Option<UserId>,
        action: &str,
        payload: &str,
        outgoing: &mut Vec<ClientMessage>,
    ) {
        if action == "volume" {
            if let Ok(v) = payload.parse::<i32>() {
                self.volume = v.clamp(0, 100);
                volume::shared().set("mothh:music", volume::Kind::Screen, self.volume);
                if let Some(listener) = &self.listener {
                    let _ = listener.control.send(self.volume);
                }
            }
            return;
        }
        if action == "leave" {
            self.stop_listening();
            return;
        }
        let Some(id) = open else {
            return;
        };
        if !self.enabled {
            return;
        }
        if action == "toggle" || action == "show" {
            self.panel_open = action == "show" || !self.panel_open;
            if self.panel_open && self.online {
                outgoing.push(message(id, MusicCommand::GetSnapshot));
            }
            return;
        }
        if action == "source" {
            if let Some(track) = self
                .snapshots
                .get(&id)
                .and_then(|c| c.snapshot.current.as_ref())
                .and_then(|i| i.track.as_ref())
            {
                // Reconstruir, não abrir uma URL arbitrária recebida pela rede.
                if valid_video_id(&track.video_id) {
                    crate::update::open_page(&format!(
                        "https://www.youtube.com/watch?v={}",
                        track.video_id
                    ));
                }
            }
            return;
        }
        if !self.online {
            self.errors.insert(
                id,
                "sem conexão: tente novamente quando o servidor voltar".into(),
            );
            return;
        }
        self.errors.remove(&id);
        let snapshot = self.snapshots.get(&id).map(|c| &c.snapshot);
        let controls = snapshot.is_some_and(|s| s.controller_id.is_some() && s.controller_id == me);
        let paused = snapshot.is_some_and(|s| {
            matches!(
                s.state,
                MusicPlaybackState::Paused | MusicPlaybackState::Interrupted
            )
        });
        let command = match action {
            "refresh" => Some(MusicCommand::GetSnapshot),
            "enqueue" => {
                let query = payload.trim();
                if query.is_empty() || query.len() > 300 {
                    self.errors
                        .insert(id, "informe nome ou link de até 300 bytes".into());
                    return;
                }
                if self.pending.values().any(|(room, _)| *room == id) {
                    return;
                }
                let request_id = Uuid::new_v4();
                self.pending.insert(request_id, (id, query.into()));
                // Pedir música é uma escolha explícita de entrar na escuta.
                if self.desired != Some(id) {
                    self.stop_listening();
                    self.desired = Some(id);
                }
                self.request_token(id, outgoing);
                Some(MusicCommand::Enqueue {
                    request_id,
                    query: query.into(),
                })
            }
            "listen" => {
                if self.desired == Some(id) {
                    self.stop_listening();
                } else {
                    self.stop_listening();
                    self.desired = Some(id);
                    self.request_token(id, outgoing);
                }
                None
            }
            "play" if self.desired != Some(id) => {
                self.stop_listening();
                self.desired = Some(id);
                self.request_token(id, outgoing);
                (controls && paused).then_some(MusicCommand::Resume)
            }
            "play" if !controls => {
                self.stop_listening();
                None
            }
            "play" => snapshot.map(|s| {
                if matches!(
                    s.state,
                    MusicPlaybackState::Paused | MusicPlaybackState::Interrupted
                ) {
                    MusicCommand::Resume
                } else {
                    MusicCommand::Pause
                }
            }),
            "skip" if controls => Some(MusicCommand::Skip),
            "clear" if controls => Some(MusicCommand::Clear),
            "stop" if controls => Some(MusicCommand::Stop),
            "remove" => payload
                .parse()
                .ok()
                .map(|item_id| MusicCommand::Remove { item_id }),
            _ => None,
        };
        if let Some(command) = command {
            outgoing.push(message(id, command));
        }
    }

    pub fn snapshot(
        &mut self,
        snapshot: MusicSnapshot,
        ui: &slint::Weak<AppWindow>,
        outgoing: &mut Vec<ClientMessage>,
    ) {
        if self.retired.contains(&snapshot.session_id) {
            return;
        }
        let id = snapshot.conversation_id;
        if let Some(old) = self.snapshots.get(&id) {
            if old.snapshot.session_id == snapshot.session_id
                && old.snapshot.revision >= snapshot.revision
            {
                return;
            }
            if old.snapshot.session_id != snapshot.session_id {
                self.retired.insert(old.snapshot.session_id);
                if self.retired.len() > 128 {
                    self.retired.clear();
                    self.retired.insert(old.snapshot.session_id);
                }
                if self.desired == Some(id) {
                    self.listener = None;
                    self.listening = false;
                    self.token_pending = false;
                    self.request_token(id, outgoing);
                }
            }
        }
        let accepted: Vec<_> = self
            .pending
            .iter()
            .filter(|(request, (room, _))| {
                *room == id
                    && snapshot
                        .queue
                        .iter()
                        .chain(snapshot.current.iter())
                        .any(|item| item.request_id == **request)
            })
            .map(|(key, (_, query))| (*key, query.clone()))
            .collect();
        for (request, query) in accepted {
            self.pending.remove(&request);
            let room = id.0.to_string();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                if ui.get_music_data().conversation_id.as_str() == room
                    && ui.get_music_query().as_str().trim() == query
                {
                    ui.set_music_query("".into());
                }
            });
        }
        self.snapshots.insert(
            id,
            Cached {
                snapshot,
                received: Instant::now(),
            },
        );
    }

    pub fn error(&mut self, id: ConversationId, request: Option<Uuid>, reason: String) {
        if let Some(request) = request {
            self.pending.remove(&request);
        }
        if self.desired == Some(id) && self.token_pending {
            self.stop_listening();
        }
        self.errors.insert(id, reason);
    }

    pub fn unsupported(&mut self, open: Option<ConversationId>, reason: &str) -> bool {
        let Some(id) = open else {
            return false;
        };
        if reason != "Mensagem inválida." || !self.panel_open || self.snapshots.contains_key(&id) {
            return false;
        }
        self.pending.retain(|_, (room, _)| *room != id);
        self.error(
            id,
            None,
            "este servidor ainda não tem música; é preciso atualizar o servidor".into(),
        );
        true
    }

    pub fn token(
        &mut self,
        id: ConversationId,
        session: Uuid,
        url: String,
        token: String,
        commands: &UnboundedSender<Command>,
        outgoing: &mut Vec<ClientMessage>,
    ) {
        if self.desired != Some(id) || !self.enabled || !self.online {
            return;
        }
        self.token_pending = false;
        if self.retired.contains(&session) {
            self.request_token(id, outgoing);
            return;
        }
        if self.listener.as_ref().is_some_and(|l| l.session == session) {
            return;
        }
        let attempt = Uuid::new_v4();
        let (control, rx) = watch::channel(self.volume);
        self.listener = Some(Listener {
            attempt,
            session,
            control,
        });
        self.listening = false;
        let sender = commands.clone();
        tokio::spawn(async move {
            let reason = listen(url, token, attempt, &sender, rx).await.err();
            let _ = sender.send(Command::MusicEvent(Event::Ended(attempt, reason)));
        });
    }

    /// Meter atualiza só as barras, evitando reconstruir o chat a 10 Hz.
    pub fn event(
        &mut self,
        event: Event,
        open: Option<ConversationId>,
        ui: &slint::Weak<AppWindow>,
    ) -> bool {
        let attempt = match &event {
            Event::Connected(id) | Event::Ended(id, _) | Event::Spectrum(id, _) => *id,
        };
        if self.listener.as_ref().is_none_or(|l| l.attempt != attempt) {
            return false;
        }
        match event {
            Event::Connected(_) => {
                self.listening = true;
                true
            }
            Event::Ended(_, reason) => {
                if let (Some(id), Some(reason)) = (self.desired, reason) {
                    self.errors.insert(id, reason);
                }
                self.stop_listening();
                true
            }
            Event::Spectrum(_, bands) => {
                self.bands = bands;
                if self.desired == open && self.panel_open {
                    let _ = ui.upgrade_in_event_loop(move |ui| {
                        ui.set_music_bands(ModelRc::new(VecModel::from(bands.to_vec())))
                    });
                }
                false
            }
        }
    }

    pub fn render(
        &self,
        open: Option<ConversationId>,
        me: Option<UserId>,
        names: &HashMap<UserId, String>,
        room_names: &HashMap<ConversationId, String>,
        ui: &slint::Weak<AppWindow>,
    ) {
        let cached = open.and_then(|id| self.snapshots.get(&id));
        let snapshot = cached.map(|c| &c.snapshot);
        let track = snapshot
            .and_then(|s| s.current.as_ref())
            .and_then(|i| i.track.as_ref());
        let playing =
            self.online && snapshot.is_some_and(|s| s.state == MusicPlaybackState::Playing);
        let can_control =
            snapshot.is_some_and(|s| s.controller_id.is_some() && s.controller_id == me);
        let listening = self.listening && self.desired == open;
        let connecting = self.desired == open && open.is_some() && !self.listening;
        let position = cached.map_or(0, |c| {
            c.snapshot.position_ms
                + if playing {
                    c.received.elapsed().as_millis() as u64
                } else {
                    0
                }
        });
        let error = open
            .and_then(|id| self.errors.get(&id))
            .cloned()
            .or_else(|| snapshot.and_then(|s| s.error.clone()))
            .or_else(|| {
                snapshot
                    .filter(|s| {
                        s.current.is_none()
                            && s.queue
                                .iter()
                                .all(|item| item.status == MusicItemStatus::Failed)
                    })
                    .and_then(|s| s.queue.last())
                    .and_then(|item| item.error.clone())
            })
            .unwrap_or_default();
        let status = if !self.online {
            "sem conexão"
        } else if connecting {
            "conectando ao som da sala..."
        } else if !error.is_empty() {
            "música indisponível"
        } else if snapshot.is_none() {
            "carregando a sala..."
        } else {
            match snapshot.unwrap().state {
                MusicPlaybackState::Idle
                    if snapshot.is_some_and(|s| {
                        s.queue
                            .iter()
                            .any(|item| item.status == MusicItemStatus::Resolving)
                    }) =>
                {
                    "buscando música..."
                }
                MusicPlaybackState::Idle => "adicione uma música para começar",
                MusicPlaybackState::Starting => "preparando a música...",
                MusicPlaybackState::Playing if listening => "ouvindo junto",
                MusicPlaybackState::Playing => "tocando na sala · clique em ouvir",
                MusicPlaybackState::Paused => "pausado",
                MusicPlaybackState::Interrupted => "reprodução interrompida",
            }
        };
        let detail = snapshot
            .and_then(|s| s.current.as_ref())
            .map(|item| {
                format!(
                    "adicionada por {}",
                    names
                        .get(&item.requested_by)
                        .map(String::as_str)
                        .unwrap_or("membro do grupo")
                )
            })
            .unwrap_or_else(|| "cole um link ou escreva o nome da música".into());
        let data = MusicViewData {
            video_id: track
                .map(|t| t.video_id.as_str())
                .unwrap_or_default()
                .into(),
            conversation_id: open.map(|id| id.0.to_string()).unwrap_or_default().into(),
            panel_open: self.panel_open && self.enabled,
            has_track: track.is_some(),
            title: track
                .map(|t| t.title.as_str())
                .unwrap_or("Som da sala")
                .into(),
            subtitle: track
                .map(|t| t.channel.as_str())
                .unwrap_or("Uma fila, todo mundo junto.")
                .into(),
            status: status.into(),
            detail: detail.into(),
            error: error.into(),
            playing,
            buffering: snapshot.is_some_and(|s| s.state == MusicPlaybackState::Starting),
            can_control,
            online: self.online,
            listening,
            connecting,
            submitting: open.is_some_and(|id| self.pending.values().any(|(room, _)| *room == id)),
            volume: self.volume,
            position_ms: position.min(track.map_or(0, |t| t.duration_ms)) as f32,
            duration_ms: track.map_or(0, |t| t.duration_ms) as f32,
            queued: snapshot.map_or(0, |s| {
                s.queue
                    .iter()
                    .filter(|item| item.status != MusicItemStatus::Failed)
                    .count()
            }) as i32,
        };
        let rows: Vec<_> = snapshot
            .map(|s| {
                s.queue
                    .iter()
                    .map(|item| {
                        let who = names
                            .get(&item.requested_by)
                            .map(String::as_str)
                            .unwrap_or("membro do grupo");
                        let status = match item.status {
                            MusicItemStatus::Resolving => "buscando música...".to_string(),
                            MusicItemStatus::Ready => format!(
                                "{} · {}",
                                who,
                                crate::audio::mm_ss(
                                    item.track.as_ref().map_or(0, |t| t.duration_ms)
                                )
                            ),
                            MusicItemStatus::Failed => item
                                .error
                                .clone()
                                .unwrap_or_else(|| "não foi possível tocar".into()),
                        };
                        MusicQueueRow {
                            id: item.item_id.to_string().into(),
                            title: item
                                .track
                                .as_ref()
                                .map(|t| t.title.as_str())
                                .unwrap_or("Resolvendo pedido")
                                .into(),
                            detail: status.into(),
                            failed: item.status == MusicItemStatus::Failed,
                            can_remove: can_control || Some(item.requested_by) == me,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bands = if listening && playing {
            self.bands
        } else {
            [0.0; 32]
        };
        let room = self
            .desired
            .and_then(|id| room_names.get(&id))
            .cloned()
            .unwrap_or_default();
        let room_id = self.desired.map(|id| id.0.to_string()).unwrap_or_default();
        let _ = ui.upgrade_in_event_loop(move |ui| {
            if ui.get_music_data().conversation_id != data.conversation_id {
                ui.set_music_query("".into());
            }
            ui.set_music_data(data);
            ui.set_music_queue(ModelRc::new(VecModel::from(rows)));
            ui.set_music_bands(ModelRc::new(VecModel::from(bands.to_vec())));
            ui.set_music_room_name(room.into());
            ui.set_music_room_id(room_id.into());
        });
    }
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

async fn listen(
    url: String,
    token: String,
    attempt: Uuid,
    net: &UnboundedSender<Command>,
    mut control: watch::Receiver<i32>,
) -> Result<(), String> {
    let connect = timeout(
        Duration::from_secs(15),
        Room::connect(&url, &token, RoomOptions::default()),
    );
    tokio::pin!(connect);
    let (room, mut events) = loop {
        tokio::select! {
            changed = control.changed() => { if changed.is_err() { return Ok(()); } }
            result = &mut connect => break result.map_err(|_| "o som da sala demorou para conectar")?.map_err(|_| "não foi possível conectar ao som da sala")?,
        }
    };
    let _ = net.send(Command::MusicEvent(Event::Connected(attempt)));
    let spectrum = Arc::new(Mutex::new([0.0f32; 32]));
    let mut meters: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            changed = control.changed() => {
                if changed.is_err() { break Ok(()); }
                let percent = *control.borrow_and_update();
                for participant in room.remote_participants().values() {
                    for publication in participant.track_publications().values() {
                        if let Some(RemoteTrack::Audio(track)) = publication.track() { volume::apply(&track.rtc_track(), volume::Kind::Screen, percent); }
                    }
                }
            }
            event = events.recv() => match event {
                None | Some(RoomEvent::Disconnected { .. }) => break Err("a conexão com o som da sala caiu; clique em ouvir para tentar novamente".into()),
                Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(track), publication, .. }) => {
                    volume::apply(&track.rtc_track(), volume::Kind::Screen, *control.borrow());
                    let shared = spectrum.clone();
                    let mut stream = NativeAudioStream::new(track.rtc_track(), 24_000, 1);
                    let task = tokio::spawn(async move {
                        let mut meter = Spectrum::default();
                        let mut since = 0usize;
                        while let Some(frame) = stream.next().await {
                            meter.push(&frame.data);
                            since += frame.data.len();
                            if since >= 2400 {
                                *shared.lock().unwrap_or_else(|e| e.into_inner()) = meter.bands();
                                since = 0;
                            }
                        }
                    });
                    if let Some(old) = meters.insert(publication.sid().to_string(), task) { old.abort(); }
                }
                Some(RoomEvent::TrackUnsubscribed { publication, .. }) => {
                    if let Some(task) = meters.remove(&publication.sid().to_string()) { task.abort(); }
                    *spectrum.lock().unwrap_or_else(|e| e.into_inner()) = [0.0; 32];
                }
                _ => {}
            },
            _ = tick.tick() => {
                let bands = *spectrum.lock().unwrap_or_else(|e| e.into_inner());
                if net.send(Command::MusicEvent(Event::Spectrum(attempt, bands))).is_err() { break Ok(()); }
            }
        }
    };
    for (_, task) in meters {
        task.abort();
    }
    let _ = room.close().await;
    result
}

/// Goertzel em 32 bandas logarítmicas: barras derivadas do áudio recebido.
#[derive(Default)]
struct Spectrum {
    samples: VecDeque<f32>,
}
impl Spectrum {
    fn push(&mut self, samples: &[i16]) {
        for sample in samples {
            if self.samples.len() == 1024 {
                self.samples.pop_front();
            }
            self.samples.push_back(f32::from(*sample) / 32768.0);
        }
    }
    fn bands(&self) -> [f32; 32] {
        if self.samples.len() < 128 {
            return [0.0; 32];
        }
        let count = self.samples.len() as f32;
        std::array::from_fn(|i| {
            let frequency = 80.0 * (10_000.0f32 / 80.0).powf(i as f32 / 31.0);
            let coefficient = 2.0 * (std::f32::consts::TAU * frequency / 24_000.0).cos();
            let (mut a, mut b) = (0.0, 0.0);
            for (index, sample) in self.samples.iter().enumerate() {
                let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / count).cos();
                let value = sample * window + coefficient * a - b;
                b = a;
                a = value;
            }
            let amplitude = (a * a + b * b - coefficient * a * b).max(0.0).sqrt() * 4.0 / count;
            ((20.0 * amplitude.max(0.00001).log10() + 65.0) / 65.0).clamp(0.0, 1.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: ConversationId, session: Uuid, revision: u64) -> MusicSnapshot {
        MusicSnapshot {
            conversation_id: id,
            session_id: session,
            revision,
            generation: 0,
            controller_id: None,
            state: MusicPlaybackState::Idle,
            current: None,
            queue: vec![],
            position_ms: 0,
            updated_at_ms: 0,
            error: None,
        }
    }

    #[test]
    fn ignores_duplicate_revisions_and_retired_rooms() {
        let mut front = Frontend::default();
        let id = ConversationId(Uuid::new_v4());
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let ui = slint::Weak::<AppWindow>::default();
        let mut outgoing = vec![];
        front.snapshot(snapshot(id, old, 5), &ui, &mut outgoing);
        front.snapshot(snapshot(id, old, 4), &ui, &mut outgoing);
        assert_eq!(front.snapshots[&id].snapshot.revision, 5);
        front.snapshot(snapshot(id, new, 0), &ui, &mut outgoing);
        front.snapshot(snapshot(id, old, 99), &ui, &mut outgoing);
        assert_eq!(front.snapshots[&id].snapshot.session_id, new);
    }

    #[test]
    fn stale_listener_cannot_stop_a_new_listener_and_disabling_disconnects() {
        let mut front = Frontend::default();
        let id = ConversationId(Uuid::new_v4());
        let attempt = Uuid::new_v4();
        let (control, mut receiver) = watch::channel(100);
        front.desired = Some(id);
        front.listener = Some(Listener {
            attempt,
            session: Uuid::new_v4(),
            control,
        });
        let ui = slint::Weak::<AppWindow>::default();
        assert!(!front.event(Event::Ended(Uuid::new_v4(), None), Some(id), &ui));
        assert!(front.listener.is_some());
        front.set_enabled(false);
        assert!(front.listener.is_none());
        assert!(front.desired.is_none());
        assert!(receiver.has_changed().is_err());
        assert_eq!(*receiver.borrow_and_update(), 100);
    }

    #[test]
    fn room_rotation_requests_fresh_credentials_only_after_explicit_listening() {
        let mut front = Frontend {
            online: true,
            ..Default::default()
        };
        let id = ConversationId(Uuid::new_v4());
        let ui = slint::Weak::<AppWindow>::default();
        let mut outgoing = vec![];
        front.snapshot(snapshot(id, Uuid::new_v4(), 1), &ui, &mut outgoing);
        front.snapshot(snapshot(id, Uuid::new_v4(), 1), &ui, &mut outgoing);
        assert!(outgoing.is_empty());
        front.desired = Some(id);
        front.snapshot(snapshot(id, Uuid::new_v4(), 0), &ui, &mut outgoing);
        assert!(matches!(
            &outgoing[..],
            [ClientMessage::Music {
                command: MusicCommand::Listen,
                ..
            }]
        ));
        front.lost_connection();
        front.action(Some(id), None, "enqueue", "track", &mut outgoing);
        assert!(front.pending.is_empty());
    }
    #[test]
    fn spectrum_has_silence_and_tracks_a_tone() {
        let mut meter = Spectrum::default();
        meter.push(&[0; 1024]);
        assert_eq!(meter.bands(), [0.0; 32]);
        let tone: Vec<_> = (0..1024)
            .map(|i| {
                ((std::f32::consts::TAU * 1000.0 * i as f32 / 24_000.0).sin() * 20_000.0) as i16
            })
            .collect();
        meter.push(&tone);
        let bands = meter.bands();
        let peak = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let frequency = 80.0 * (10_000.0f32 / 80.0).powf(peak as f32 / 31.0);
        assert!((frequency - 1000.0).abs() < 200.0);
        assert!(bands[peak] > 0.5);
        assert_eq!(meter.samples.len(), 1024);
    }
}
