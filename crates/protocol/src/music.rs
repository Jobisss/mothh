//! Contratos públicos e protocolo privado (JSON por stdin/stdout) do worker.
use crate::{ConversationId, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MusicCommand {
    Enqueue { request_id: Uuid, query: String },
    Remove { item_id: Uuid },
    Pause,
    Resume,
    Skip,
    Clear,
    Stop,
    GetSnapshot,
    Listen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicItemStatus {
    Resolving,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicPlaybackState {
    Idle,
    Starting,
    Playing,
    Paused,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicTrack {
    pub video_id: String,
    pub canonical_url: String,
    pub title: String,
    pub channel: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicQueueItem {
    pub item_id: Uuid,
    pub request_id: Uuid,
    pub requested_by: UserId,
    pub status: MusicItemStatus,
    pub track: Option<MusicTrack>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicSnapshot {
    pub conversation_id: ConversationId,
    pub session_id: Uuid,
    pub revision: u64,
    pub generation: u64,
    pub controller_id: Option<UserId>,
    pub state: MusicPlaybackState,
    pub current: Option<MusicQueueItem>,
    pub queue: Vec<MusicQueueItem>,
    /// Última posição confirmada pelo worker; UI pode interpolar enquanto toca.
    pub position_ms: u64,
    pub updated_at_ms: u64,
    pub error: Option<String>,
}

// Não implementar Debug: contém JWT de curta duração.
#[derive(Clone, Serialize, Deserialize)]
pub struct MusicWorkerJob {
    pub url: String,
    pub token: String,
    pub video_id: String,
    pub position_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MusicWorkerEvent {
    Playing,
    Progress { position_ms: u64 },
    Finished,
    Empty,
    Failed { reason: String },
}
