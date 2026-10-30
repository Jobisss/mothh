//! Token de chamada: o servidor assina um JWT que o LiveKit aceita.
//!
//! O LiveKit não sabe quem é membro de qual grupo; quem decide é o servidor,
//! que só assina o token depois de conferir no banco. O formato do token
//! segue o do LiveKit: HS256 com a chave da API em `iss`, a pessoa em `sub`
//! e as permissões em `video`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header};
use protocol::{ConversationId, ServerMessage, UserId};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Quanto tempo o token vale para entrar. Quem já está na chamada não cai
/// quando ele vence; o LiveKit renova sozinho durante a conexão.
pub const CALL_TOKEN_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Onde fica o LiveKit e com que chave assinar. Vem do ambiente.
#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    /// Endereço que o app usa para conectar, ex.: `ws://127.0.0.1:7880`.
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl LiveKitConfig {
    /// Lê `MOTHH_LIVEKIT_URL`, `MOTHH_LIVEKIT_KEY` e `MOTHH_LIVEKIT_SECRET`.
    /// `None` se faltar alguma: o servidor sobe, só não tem chamada.
    pub fn from_env() -> Option<Self> {
        let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
        Some(Self {
            url: var("MOTHH_LIVEKIT_URL")?,
            api_key: var("MOTHH_LIVEKIT_KEY")?,
            api_secret: var("MOTHH_LIVEKIT_SECRET")?,
        })
    }
}

/// O que vai dentro do token, com os nomes que o LiveKit espera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallClaims {
    /// Chave da API do LiveKit.
    pub iss: String,
    /// Identidade na sala: o id do usuário.
    pub sub: String,
    /// Nome mostrado para os outros.
    pub name: String,
    pub nbf: u64,
    pub exp: u64,
    pub video: VideoGrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoGrant {
    pub room: String,
    pub room_join: bool,
    pub can_publish: bool,
    pub can_subscribe: bool,
}

/// Nome da sala do LiveKit de uma conversa.
pub fn room_name(id: ConversationId) -> String {
    id.0.to_string()
}

impl LiveKitConfig {
    /// Assina o token que deixa `user` entrar na sala da conversa.
    pub fn token(&self, user: UserId, username: &str, id: ConversationId) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = CallClaims {
            iss: self.api_key.clone(),
            sub: user.0.to_string(),
            name: username.to_owned(),
            nbf: now,
            exp: now + CALL_TOKEN_TTL.as_secs(),
            video: VideoGrant {
                room: room_name(id),
                room_join: true,
                can_publish: true,
                can_subscribe: true,
            },
        };
        jsonwebtoken::encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.api_secret.as_bytes()),
        )
        .expect("HS256 com segredo em bytes sempre assina")
    }
}

impl AppState {
    /// Token da chamada, só para quem é membro da conversa.
    pub async fn join_call(
        &self,
        user: UserId,
        username: &str,
        id: ConversationId,
    ) -> Result<ServerMessage, String> {
        let Some(livekit) = &self.livekit else {
            return Err("Chamadas não estão configuradas neste servidor.".into());
        };
        // Mesma resposta de uma conversa que não existe: não conta a quem é
        // de fora que o grupo existe.
        if !self.is_member(user, id).await? {
            return Err("Essa conversa não existe.".into());
        }
        Ok(ServerMessage::CallToken {
            conversation_id: id,
            url: livekit.url.clone(),
            token: livekit.token(user, username, id),
        })
    }
}
