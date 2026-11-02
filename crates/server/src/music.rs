//! Fila transitória, autorização e supervisão do worker de música.
//! A mídia usa uma sala LiveKit exclusiva, sem as chaves MLS da conversa.
use crate::{
    calls::LiveKitConfig,
    state::{AppState, db_error},
};
use protocol::{
    ConversationId, MusicCommand, MusicItemStatus, MusicPlaybackState, MusicQueueItem,
    MusicSnapshot, MusicTrack, MusicWorkerEvent, MusicWorkerJob, ServerMessage, UserId,
};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, timeout},
};
use uuid::Uuid;

const MAX_QUEUE: usize = 50;
const MAX_PER_USER: usize = 5;
const MAX_DURATION_MS: u64 = 20 * 60 * 1000;
const MAX_ROOMS: usize = 64;
const IDLE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct MusicConfig {
    pub worker: PathBuf,
    pub yt_dlp: PathBuf,
    pub ffmpeg: PathBuf,
}

impl MusicConfig {
    /// Opt-in: sem MOTHH_MUSIC_WORKER o recurso fica desabilitado.
    pub fn from_env() -> Option<Self> {
        Some(Self {
            worker: std::env::var_os("MOTHH_MUSIC_WORKER")
                .filter(|p| !p.is_empty())?
                .into(),
            yt_dlp: std::env::var_os("MOTHH_YT_DLP")
                .unwrap_or_else(|| "yt-dlp".into())
                .into(),
            ffmpeg: std::env::var_os("MOTHH_FFMPEG")
                .unwrap_or_else(|| "ffmpeg".into())
                .into(),
        })
    }
}

pub struct MusicService {
    config: Option<MusicConfig>,
    rooms: Mutex<HashMap<ConversationId, RoomHandle>>,
    resolvers: Arc<Semaphore>,
    players: Arc<Semaphore>,
}

impl Default for MusicService {
    fn default() -> Self {
        Self::new(None)
    }
}

#[derive(Clone)]
struct RoomHandle {
    id: Uuid,
    tx: mpsc::Sender<Envelope>,
}
struct Envelope {
    user: Option<UserId>,
    command: MusicCommand,
    expected_revision: Option<u64>,
    reply: oneshot::Sender<ServerMessage>,
}
enum Internal {
    Resolved(Uuid, Result<MusicTrack, String>),
    Playback(u64, MusicWorkerEvent),
}

impl MusicService {
    fn new(config: Option<MusicConfig>) -> Self {
        Self {
            config,
            rooms: Mutex::new(HashMap::new()),
            resolvers: Arc::new(Semaphore::new(2)),
            players: Arc::new(Semaphore::new(4)),
        }
    }
}

impl AppState {
    pub fn with_music_config(mut self, config: MusicConfig) -> Self {
        self.music = Arc::new(MusicService::new(Some(config)));
        self
    }

    pub async fn music_command(
        &self,
        user: UserId,
        id: ConversationId,
        command: MusicCommand,
        expected_revision: Option<u64>,
    ) -> ServerMessage {
        let request_id = match &command {
            MusicCommand::Enqueue { request_id, .. } => Some(*request_id),
            _ => None,
        };
        let result = self
            .dispatch_music(user, id, command, expected_revision)
            .await;
        result.unwrap_or_else(|reason| ServerMessage::MusicError {
            conversation_id: id,
            request_id,
            reason,
        })
    }

    async fn dispatch_music(
        &self,
        user: UserId,
        id: ConversationId,
        command: MusicCommand,
        expected_revision: Option<u64>,
    ) -> Result<ServerMessage, String> {
        // Antes de criar ator ou revelar configuração do recurso.
        self.music_access(user, id).await?;
        let config = self
            .music
            .config
            .clone()
            .ok_or("Música não está habilitada neste servidor.")?;
        let livekit = self
            .livekit
            .as_ref()
            .ok_or("LiveKit não está configurado.")?
            .as_ref()
            .clone();
        let tx = {
            let mut rooms = self.music.rooms.lock().unwrap_or_else(|e| e.into_inner());
            if rooms.get(&id).is_some_and(|room| room.tx.is_closed()) {
                rooms.remove(&id);
            }
            if !rooms.contains_key(&id) {
                if rooms.len() >= MAX_ROOMS {
                    return Err("Limite de salas de música atingido.".into());
                }
                let (tx, rx) = mpsc::channel(64);
                let room = RoomHandle {
                    id: Uuid::new_v4(),
                    tx,
                };
                let actor = Actor::new(self.clone(), config, livekit, id, room.id);
                rooms.insert(id, room);
                tokio::spawn(actor.run(rx));
            }
            rooms[&id].tx.clone()
        };
        let (reply, response) = oneshot::channel();
        tx.try_send(Envelope {
            user: Some(user),
            command,
            expected_revision,
            reply,
        })
        .map_err(|_| "Sala ocupada; tente novamente.")?;
        timeout(Duration::from_secs(15), response)
            .await
            .map_err(|_| "A sala demorou para responder.")?
            .map_err(|_| "A sessão de música terminou.".into())
    }

    /// Dono do grupo é o administrador. Sala pública usa condutor por sessão.
    async fn music_access(
        &self,
        user: UserId,
        id: ConversationId,
    ) -> Result<Option<UserId>, String> {
        let row: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT c.owner_id FROM conversations c JOIN conversation_members cm ON cm.conversation_id = c.id WHERE c.id = $1 AND cm.user_id = $2 AND c.kind = 'group'"
        ).bind(id.0).bind(user.0).fetch_optional(&self.db).await.map_err(db_error)?;
        row.map(|(owner,)| owner.map(UserId))
            .ok_or_else(|| "Esse grupo não existe.".into())
    }

    /// Mudança de membros interrompe o serviço antes de uma sala nova ser criada.
    /// Tokens antigos apontam para uma sala que nunca será reutilizada.
    pub(crate) async fn reset_music(&self, id: ConversationId) {
        let tx = self
            .music
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(|r| r.tx.clone());
        if let Some(tx) = tx {
            let (reply, response) = oneshot::channel();
            // Comando interno não depende da identidade de um membro já removido.
            let envelope = Envelope {
                user: None,
                command: MusicCommand::Stop,
                expected_revision: None,
                reply,
            };
            // Sem identidade é um encerramento interno, nunca comando do WebSocket.
            if tx.send(envelope).await.is_ok() {
                let _ = response.await;
            }
        }
    }
}

struct Actor {
    app: AppState,
    config: MusicConfig,
    livekit: LiveKitConfig,
    snapshot: MusicSnapshot,
    internal_tx: mpsc::Sender<Internal>,
    internal_rx: mpsc::Receiver<Internal>,
    resolutions: HashMap<Uuid, JoinHandle<()>>,
    playback: Option<(watch::Sender<bool>, JoinHandle<()>)>,
    seen: VecDeque<(UserId, Uuid)>,
    last_request: HashMap<UserId, Instant>,
    conductor: Option<UserId>,
}

impl Actor {
    fn new(
        app: AppState,
        config: MusicConfig,
        livekit: LiveKitConfig,
        id: ConversationId,
        session_id: Uuid,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::channel(128);
        Self {
            app,
            config,
            livekit,
            snapshot: MusicSnapshot {
                conversation_id: id,
                session_id,
                revision: 0,
                generation: 0,
                controller_id: None,
                state: MusicPlaybackState::Idle,
                current: None,
                queue: vec![],
                position_ms: 0,
                updated_at_ms: now_ms(),
                error: None,
            },
            internal_tx,
            internal_rx,
            resolutions: HashMap::new(),
            playback: None,
            seen: VecDeque::new(),
            last_request: HashMap::new(),
            conductor: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Envelope>) {
        let mut idle = tokio::time::interval(Duration::from_secs(10));
        let mut last_activity = Instant::now();
        let mut reset_reply = None;
        loop {
            tokio::select! {
                envelope = rx.recv() => {
                    let Some(envelope) = envelope else { break };
                    let Some(user) = envelope.user else {
                        reset_reply = Some(envelope.reply);
                        break;
                    };
                    last_activity = Instant::now();
                    let request_id = match &envelope.command { MusicCommand::Enqueue { request_id, .. } => Some(*request_id), _ => None };
                    let result = self.command(user, envelope.command, envelope.expected_revision).await;
                    let message = match result {
                        Ok(Some(message)) => message,
                        Ok(None) => { self.start_next(); self.publish().await; self.message() },
                        Err(reason) => ServerMessage::MusicError { conversation_id: self.snapshot.conversation_id, request_id, reason },
                    };
                    let _ = envelope.reply.send(message);
                }
                event = self.internal_rx.recv() => {
                    if let Some(event) = event { self.event(event).await; self.start_next(); self.publish().await; }
                }
                _ = idle.tick() => {
                    if self.playback.is_none() && last_activity.elapsed() >= IDLE_TTL { break; }
                }
            }
        }
        self.stop().await;
        let id = self.snapshot.conversation_id;
        let mut rooms = self
            .app
            .music
            .rooms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if rooms
            .get(&id)
            .is_some_and(|r| r.id == self.snapshot.session_id)
        {
            rooms.remove(&id);
        }
        drop(rooms);
        if let Some(reply) = reset_reply {
            let _ = reply.send(self.message());
        }
    }

    fn message(&self) -> ServerMessage {
        ServerMessage::MusicSnapshot {
            snapshot: self.snapshot.clone(),
        }
    }
    fn changed(&mut self) {
        self.snapshot.revision += 1;
        self.snapshot.updated_at_ms = now_ms();
    }
    async fn publish(&self) {
        if let Ok(recipients) = self.app.member_ids(self.snapshot.conversation_id).await {
            self.app.emit(recipients, self.message());
        }
    }

    async fn command(
        &mut self,
        user: UserId,
        command: MusicCommand,
        expected: Option<u64>,
    ) -> Result<Option<ServerMessage>, String> {
        // Revalidar no ator: o comando pode ter aguardado na fila interna.
        let owner = self
            .app
            .music_access(user, self.snapshot.conversation_id)
            .await?;
        let controller = owner.or(self.conductor);
        if self.snapshot.controller_id != controller {
            self.snapshot.controller_id = controller;
            self.changed();
        }
        if let MusicCommand::GetSnapshot = command {
            return Ok(Some(self.message()));
        }
        if let MusicCommand::Listen = command {
            let token =
                self.livekit
                    .music_token(self.snapshot.session_id, &user.0.to_string(), false);
            return Ok(Some(ServerMessage::MusicToken {
                conversation_id: self.snapshot.conversation_id,
                session_id: self.snapshot.session_id,
                url: self.livekit.url.clone(),
                token,
            }));
        }
        if let MusicCommand::Enqueue { request_id, .. } = &command
            && self.seen.contains(&(user, *request_id))
        {
            return Ok(Some(self.message()));
        }
        if expected.is_some_and(|r| r != self.snapshot.revision) {
            return Err("A fila mudou; peça um snapshot e tente novamente.".into());
        }
        let admin = owner.map_or(self.conductor == Some(user), |owner| owner == user);
        match command {
            MusicCommand::Enqueue { request_id, query } => {
                let query = normalize_query(&query)?;
                self.last_request
                    .retain(|_, at| at.elapsed() < Duration::from_secs(2));
                if self.snapshot.queue.len() + usize::from(self.snapshot.current.is_some())
                    >= MAX_QUEUE
                {
                    return Err("A fila está cheia (50 pedidos).".into());
                }
                if self
                    .snapshot
                    .queue
                    .iter()
                    .filter(|i| i.requested_by == user)
                    .count()
                    >= MAX_PER_USER
                {
                    return Err("Você já tem cinco pedidos na fila.".into());
                }
                if self
                    .last_request
                    .get(&user)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
                {
                    return Err("Espere dois segundos entre pedidos.".into());
                }
                let permit = self
                    .app
                    .music
                    .resolvers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| "Resolvedor ocupado; tente novamente.")?;
                let item_id = Uuid::new_v4();
                let tx = self.internal_tx.clone();
                let config = self.config.clone();
                self.resolutions.insert(
                    item_id,
                    tokio::spawn(async move {
                        let _permit = permit;
                        let result = resolve(&config, &query).await;
                        let _ = tx.send(Internal::Resolved(item_id, result)).await;
                    }),
                );
                self.snapshot.queue.push(MusicQueueItem {
                    item_id,
                    request_id,
                    requested_by: user,
                    status: MusicItemStatus::Resolving,
                    track: None,
                    error: None,
                });
                self.seen.push_back((user, request_id));
                if self.seen.len() > 512 {
                    self.seen.pop_front();
                }
                self.last_request.insert(user, Instant::now());
                self.conductor.get_or_insert(user);
                self.snapshot.controller_id = owner.or(self.conductor);
            }
            MusicCommand::Remove { item_id } => {
                let index = self
                    .snapshot
                    .queue
                    .iter()
                    .position(|i| i.item_id == item_id)
                    .ok_or("Pedido não está na fila.")?;
                if !admin && self.snapshot.queue[index].requested_by != user {
                    return Err("Você só pode remover seu próprio pedido.".into());
                }
                self.snapshot.queue.remove(index);
                if let Some(task) = self.resolutions.remove(&item_id) {
                    task.abort();
                }
            }
            _ if !admin => {
                return Err(
                    "Só o dono do grupo (ou condutor da sala pública) controla a reprodução."
                        .into(),
                );
            }
            MusicCommand::Pause => {
                if self.snapshot.current.is_none() {
                    return Err("Não há música tocando.".into());
                }
                self.cancel_playback().await;
                self.snapshot.state = MusicPlaybackState::Paused;
            }
            MusicCommand::Resume => {
                if !matches!(
                    self.snapshot.state,
                    MusicPlaybackState::Paused | MusicPlaybackState::Interrupted
                ) {
                    return Err("A reprodução não está pausada.".into());
                }
                self.snapshot.state = MusicPlaybackState::Idle;
            }
            MusicCommand::Skip => {
                self.cancel_playback().await;
                self.snapshot.current = None;
                self.snapshot.position_ms = 0;
                self.snapshot.state = MusicPlaybackState::Idle;
            }
            MusicCommand::Clear => {
                self.clear_pending();
            }
            MusicCommand::Stop => {
                self.stop().await;
            }
            MusicCommand::GetSnapshot | MusicCommand::Listen => unreachable!(),
        }
        self.changed();
        Ok(None)
    }

    fn clear_pending(&mut self) {
        for (_, task) in self.resolutions.drain() {
            task.abort();
        }
        self.snapshot.queue.clear();
    }
    async fn cancel_playback(&mut self) {
        self.snapshot.generation += 1;
        if let Some((cancel, mut task)) = self.playback.take() {
            let _ = cancel.send(true);
            if timeout(Duration::from_secs(4), &mut task).await.is_err() {
                task.abort();
            }
        }
    }
    async fn stop(&mut self) {
        self.cancel_playback().await;
        self.clear_pending();
        self.snapshot.current = None;
        self.snapshot.position_ms = 0;
        self.snapshot.state = MusicPlaybackState::Idle;
        self.changed();
        self.publish().await;
    }

    fn start_next(&mut self) {
        if self.playback.is_some()
            || matches!(
                self.snapshot.state,
                MusicPlaybackState::Paused | MusicPlaybackState::Interrupted
            )
        {
            return;
        }
        if self.snapshot.current.is_none() {
            // Falhos permanecem visíveis até remover/limpar. Não ultrapassar resolving.
            let next = self
                .snapshot
                .queue
                .iter()
                .position(|i| i.status != MusicItemStatus::Failed);
            let Some(index) = next else { return };
            if self.snapshot.queue[index].status != MusicItemStatus::Ready {
                return;
            }
            self.snapshot.current = Some(self.snapshot.queue.remove(index));
            self.snapshot.position_ms = 0;
        }
        let permit = match self.app.music.players.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.snapshot.state = MusicPlaybackState::Interrupted;
                self.snapshot.error =
                    Some("Limite de quatro players atingido; tente retomar depois.".into());
                self.changed();
                return;
            }
        };
        let track = self
            .snapshot
            .current
            .as_ref()
            .and_then(|i| i.track.as_ref())
            .expect("current pronto");
        self.snapshot.generation += 1;
        let generation = self.snapshot.generation;
        let job = MusicWorkerJob {
            url: self.livekit.url.clone(),
            token: self
                .livekit
                .music_token(self.snapshot.session_id, "music-worker", true),
            video_id: track.video_id.clone(),
            position_ms: self.snapshot.position_ms,
            duration_ms: track.duration_ms,
        };
        let config = self.config.clone();
        let tx = self.internal_tx.clone();
        let (cancel, rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let _permit = permit;
            run_worker(config, job, generation, tx, rx).await;
        });
        self.playback = Some((cancel, task));
        self.snapshot.state = MusicPlaybackState::Starting;
        self.snapshot.error = None;
        self.changed();
    }

    async fn event(&mut self, event: Internal) {
        match event {
            Internal::Resolved(id, result) => {
                self.resolutions.remove(&id);
                let Some(item) = self.snapshot.queue.iter_mut().find(|i| i.item_id == id) else {
                    return;
                };
                match result {
                    Ok(track) => {
                        item.track = Some(track);
                        item.status = MusicItemStatus::Ready;
                    }
                    Err(reason) => {
                        item.error = Some(reason);
                        item.status = MusicItemStatus::Failed;
                    }
                }
            }
            Internal::Playback(generation, event) => {
                if generation != self.snapshot.generation {
                    return;
                }
                match event {
                    MusicWorkerEvent::Playing => self.snapshot.state = MusicPlaybackState::Playing,
                    MusicWorkerEvent::Progress { position_ms } => {
                        if let Some(track) = self
                            .snapshot
                            .current
                            .as_ref()
                            .and_then(|i| i.track.as_ref())
                        {
                            self.snapshot.position_ms = position_ms.min(track.duration_ms);
                        }
                    }
                    MusicWorkerEvent::Finished => {
                        self.cancel_playback().await;
                        self.snapshot.current = None;
                        self.snapshot.position_ms = 0;
                        self.snapshot.state = MusicPlaybackState::Idle;
                    }
                    MusicWorkerEvent::Empty => {
                        self.stop().await;
                    }
                    MusicWorkerEvent::Failed { reason } => {
                        self.cancel_playback().await;
                        self.snapshot.error = Some(reason.clone());
                        if let Some(mut item) = self.snapshot.current.take() {
                            item.status = MusicItemStatus::Failed;
                            item.error = Some(reason);
                            self.snapshot.queue.push(item);
                        }
                        self.snapshot.position_ms = 0;
                        self.snapshot.state = MusicPlaybackState::Idle;
                    }
                }
            }
        }
        self.changed();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Só URLs de vídeos YouTube; nunca entregar URLs arbitrárias ao extrator.
fn normalize_query(query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() || query.len() > 300 || query.chars().any(char::is_control) {
        return Err("Informe nome ou link de até 300 bytes.".into());
    }
    if query.contains("://") || query.starts_with("www.") {
        // Parser URL padrão de reqwest/url não é necessário: rejeitar tudo fora
        // destes prefixos e reconstruir sempre uma URL com host fixo.
        let video = query
            .strip_prefix("https://youtu.be/")
            .or_else(|| query.strip_prefix("http://youtu.be/"))
            .map(|rest| rest.split(['?', '#']).next().unwrap_or_default())
            .or_else(|| {
                [
                    "https://www.youtube.com/watch?",
                    "https://youtube.com/watch?",
                    "https://m.youtube.com/watch?",
                ]
                .iter()
                .find_map(|prefix| {
                    query.strip_prefix(prefix).and_then(|rest| {
                        rest.split('&').find_map(|part| {
                            part.strip_prefix("v=")
                                .map(|v| v.split('#').next().unwrap_or_default())
                        })
                    })
                })
            })
            .or_else(|| {
                query
                    .strip_prefix("https://www.youtube.com/shorts/")
                    .map(|v| v.split(['?', '#']).next().unwrap_or_default())
            });
        let id = video
            .filter(|id| valid_video_id(id))
            .ok_or("Use um link de vídeo individual do YouTube.")?;
        Ok(format!("https://www.youtube.com/watch?v={id}"))
    } else {
        if query.contains(':') || query.starts_with(['/', '\\', '-']) {
            return Err("Pedido inválido; use nome da música ou link YouTube.".into());
        }
        Ok(format!("ytsearch1:{query}"))
    }
}

pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

async fn resolve(config: &MusicConfig, query: &str) -> Result<MusicTrack, String> {
    timeout(Duration::from_secs(30), async {
        let mut command = Command::new(&config.yt_dlp);
        isolate_environment(&mut command);
        let mut child = command
            .args([
                "--ignore-config",
                "--no-cache-dir",
                "--no-playlist",
                "--flat-playlist",
                "--dump-single-json",
                "--skip-download",
                "--no-warnings",
                "--socket-timeout",
                "10",
                "--retries",
                "0",
                "--",
                query,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "Não foi possível iniciar yt-dlp.")?;
        let mut bytes = vec![];
        child
            .stdout
            .take()
            .ok_or("Resolvedor sem saída.")?
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Falha ao ler resolvedor.")?;
        if bytes.len() > 256 * 1024 {
            return Err("Resposta do resolvedor muito grande.".into());
        }
        if !child
            .wait()
            .await
            .map_err(|_| "Falha no resolvedor.")?
            .success()
        {
            return Err("Fonte indisponível ou pedido não encontrado.".into());
        }
        parse_track(&bytes)
    })
    .await
    .map_err(|_| "Tempo limite ao buscar música.".to_string())?
}

fn parse_track(bytes: &[u8]) -> Result<MusicTrack, String> {
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Resposta inválida do resolvedor.")?;
    let item = if json.get("entries").is_some() {
        json["entries"]
            .as_array()
            .and_then(|v| v.first())
            .ok_or("Música não encontrada.")?
    } else {
        &json
    };
    let video_id = item["id"]
        .as_str()
        .filter(|id| valid_video_id(id))
        .ok_or("Vídeo inválido.")?;
    if item["is_live"].as_bool() == Some(true)
        || item["live_status"]
            .as_str()
            .is_some_and(|s| s == "is_live" || s == "is_upcoming")
    {
        return Err("Lives não são aceitas.".into());
    }
    let seconds = item["duration"]
        .as_f64()
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or("Não foi possível determinar a duração.")?;
    let duration_ms = (seconds * 1000.0) as u64;
    if duration_ms == 0 || duration_ms > MAX_DURATION_MS {
        return Err("Limite de 20 minutos por faixa.".into());
    }
    let title = item["title"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .ok_or("Faixa sem título.")?
        .chars()
        .take(300)
        .collect();
    let channel = item["channel"]
        .as_str()
        .or_else(|| item["uploader"].as_str())
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Ok(MusicTrack {
        video_id: video_id.into(),
        canonical_url: format!("https://www.youtube.com/watch?v={video_id}"),
        title,
        channel,
        duration_ms,
    })
}

async fn run_worker(
    config: MusicConfig,
    job: MusicWorkerJob,
    generation: u64,
    tx: mpsc::Sender<Internal>,
    mut cancel: watch::Receiver<bool>,
) {
    let result = async {
        let mut command = Command::new(config.worker);
        isolate_environment(&mut command);
        let mut child = command.env("MOTHH_YT_DLP", config.yt_dlp).env("MOTHH_FFMPEG", config.ffmpeg)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().map_err(|_| "Não foi possível iniciar music-worker.")?;
        let mut input = child.stdin.take().ok_or("Worker sem entrada.")?;
        let mut data = serde_json::to_vec(&job).map_err(|_| "Configuração inválida do worker.")?;
        data.push(b'\n');
        input.write_all(&data).await.map_err(|_| "Worker não recebeu configuração.")?;
        let mut output = BufReader::new(child.stdout.take().ok_or("Worker sem saída.")?);
        let deadline = tokio::time::sleep(Duration::from_millis(job.duration_ms.saturating_sub(job.position_ms)) + Duration::from_secs(90));
        tokio::pin!(deadline);
        loop {
            let mut line = vec![];
            let mut limited = (&mut output).take(8193);
            tokio::select! {
                _ = cancel.changed() => {
                    // EOF cancela o worker e seus subprocessos; kill como reserva.
                    drop(input);
                    if timeout(Duration::from_secs(3), child.wait()).await.is_err() { let _ = child.kill().await; }
                    return Ok(());
                }
                _ = &mut deadline => return Err("Tempo limite da reprodução."),
                read = limited.read_until(b'\n', &mut line) => {
                    if read.map_err(|_| "Falha na comunicação com worker.")? == 0 { return Err("Worker encerrou sem confirmar a reprodução."); }
                    if line.len() > 8192 { return Err("Evento do worker muito grande."); }
                    let event: MusicWorkerEvent = serde_json::from_slice(&line).map_err(|_| "Evento inválido do worker.")?;
                    let terminal = matches!(event, MusicWorkerEvent::Finished | MusicWorkerEvent::Failed { .. } | MusicWorkerEvent::Empty);
                    tx.send(Internal::Playback(generation, event)).await.map_err(|_| "Sessão encerrada.")?;
                    if terminal { drop(input); let _ = timeout(Duration::from_secs(2), child.wait()).await; return Ok(()); }
                }
            }
        }
    }.await;
    if let Err(reason) = result {
        let _ = tx
            .send(Internal::Playback(
                generation,
                MusicWorkerEvent::Failed {
                    reason: reason.into(),
                },
            ))
            .await;
    }
}

/// Não herdar DATABASE_URL, chave LiveKit ou outros segredos do servidor.
fn isolate_environment(command: &mut Command) {
    command.env_clear();
    for name in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LD_LIBRARY_PATH",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://test:test@127.0.0.1:1/test")
            .unwrap();
        Actor::new(
            AppState::new(db),
            MusicConfig {
                worker: "not-installed".into(),
                yt_dlp: "not-installed".into(),
                ffmpeg: "not-installed".into(),
            },
            LiveKitConfig {
                url: "ws://127.0.0.1:7880".into(),
                api_key: "test".into(),
                api_secret: "test-secret".into(),
            },
            ConversationId(Uuid::new_v4()),
            Uuid::new_v4(),
        )
    }

    fn item(status: MusicItemStatus) -> MusicQueueItem {
        MusicQueueItem {
            item_id: Uuid::new_v4(),
            request_id: Uuid::new_v4(),
            requested_by: UserId(Uuid::new_v4()),
            status,
            track: Some(MusicTrack {
                video_id: "abcdefghijk".into(),
                canonical_url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
                title: "test".into(),
                channel: "test".into(),
                duration_ms: 10_000,
            }),
            error: None,
        }
    }

    #[tokio::test]
    async fn late_worker_events_cannot_finish_or_move_a_new_track() {
        let mut actor = actor();
        actor.snapshot.generation = 4;
        actor.snapshot.current = Some(item(MusicItemStatus::Ready));
        actor.snapshot.state = MusicPlaybackState::Paused;
        actor.snapshot.position_ms = 2000;
        actor
            .event(Internal::Playback(3, MusicWorkerEvent::Finished))
            .await;
        actor
            .event(Internal::Playback(
                3,
                MusicWorkerEvent::Progress { position_ms: 9000 },
            ))
            .await;
        assert!(actor.snapshot.current.is_some());
        assert_eq!(actor.snapshot.state, MusicPlaybackState::Paused);
        assert_eq!(actor.snapshot.position_ms, 2000);
    }

    #[tokio::test]
    async fn pending_resolution_preserves_order_and_deleted_results_are_ignored() {
        let mut actor = actor();
        let first = item(MusicItemStatus::Resolving);
        let second = item(MusicItemStatus::Ready);
        let first_id = first.item_id;
        actor.snapshot.queue = vec![first, second];
        actor.start_next();
        assert!(actor.snapshot.current.is_none());
        actor.snapshot.queue.remove(0);
        actor
            .event(Internal::Resolved(first_id, Err("late".into())))
            .await;
        assert_eq!(actor.snapshot.queue.len(), 1);
        // Sem vaga global, preservar a faixa escolhida para retomar depois.
        let permits = actor
            .app
            .music
            .players
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();
        actor.start_next();
        assert!(actor.snapshot.current.is_some());
        assert_eq!(actor.snapshot.state, MusicPlaybackState::Interrupted);
        drop(permits);
    }

    #[test]
    fn music_tokens_only_grant_access_to_the_service_room() {
        let config = LiveKitConfig {
            url: "ws://localhost".into(),
            api_key: "test".into(),
            api_secret: "test-secret".into(),
        };
        let session = Uuid::new_v4();
        for publish in [true, false] {
            let token = config.music_token(session, "test", publish);
            let claims = jsonwebtoken::decode::<serde_json::Value>(
                &token,
                &jsonwebtoken::DecodingKey::from_secret(b"test-secret"),
                &jsonwebtoken::Validation::default(),
            )
            .unwrap()
            .claims;
            assert_eq!(claims["video"]["room"], format!("music-{session}"));
            assert_eq!(claims["video"]["canPublish"], publish);
            assert_eq!(claims["video"]["canSubscribe"], !publish);
            assert_eq!(claims["video"]["canPublishData"], false);
            assert_eq!(
                claims["exp"].as_u64().unwrap() - claims["nbf"].as_u64().unwrap(),
                60
            );
        }
    }
    #[test]
    fn normalizes_only_supported_urls_and_plain_searches() {
        assert_eq!(
            normalize_query(" artista música ").unwrap(),
            "ytsearch1:artista música"
        );
        assert_eq!(
            normalize_query("https://youtu.be/abcdefghijk?t=4").unwrap(),
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        assert_eq!(
            normalize_query("https://www.youtube.com/watch?list=x&v=abcdefghijk").unwrap(),
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        for bad in [
            "",
            "-exec evil",
            "file:/etc/passwd",
            "http://127.0.0.1/a",
            "https://youtube.com.evil/watch?v=abcdefghijk",
            "https://www.youtube.com/playlist?list=x",
            "ytsearch999:hello",
            "name\n--exec",
        ] {
            assert!(normalize_query(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn rejects_lives_missing_duration_and_oversized_tracks() {
        for json in [
            r#"{"id":"abcdefghijk","title":"x","duration":1201}"#,
            r#"{"id":"abcdefghijk","title":"x","duration":4,"is_live":true}"#,
            r#"{"id":"abcdefghijk","title":"x"}"#,
            r#"{"entries":[]}"#,
        ] {
            assert!(parse_track(json.as_bytes()).is_err());
        }
        let track = parse_track(
            br#"{"entries":[{"id":"abcdefghijk","title":"a","duration":180,"channel":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(track.duration_ms, 180_000);
        assert_eq!(track.channel, "b");
    }
}
