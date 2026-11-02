//! Token de chamada: o servidor assina um JWT que o LiveKit aceita.
//!
//! O LiveKit não sabe quem é membro de qual grupo; quem decide é o servidor,
//! que só assina o token depois de conferir no banco. O formato do token
//! segue o do LiveKit: HS256 com a chave da API em `iss`, a pessoa em `sub`
//! e as permissões em `video`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header};
use protocol::{ConversationId, Member, ServerMessage, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AppState;

/// Quanto tempo o token vale para entrar. Quem já está na chamada não cai
/// quando ele vence; o LiveKit renova sozinho durante a conexão.
pub const CALL_TOKEN_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Quem está em cada chamada, só em memória. Conta conexões por pessoa: a
/// mesma conta em dois computadores é uma pessoa só na sala. Se o servidor
/// reinicia, cada app reconecta e pede o token de novo, e a conta volta.
#[derive(Debug, Default)]
pub struct Presence {
    rooms: Mutex<HashMap<ConversationId, HashMap<UserId, usize>>>,
}

impl Presence {
    /// Mais uma conexão de `user` na chamada. Devolve quantas pessoas há nela.
    pub fn enter(&self, id: ConversationId, user: UserId) -> u32 {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        let room = rooms.entry(id).or_default();
        *room.entry(user).or_default() += 1;
        room.len() as u32
    }

    /// Uma conexão de `user` saiu da chamada. Devolve quantas pessoas sobram.
    pub fn leave(&self, id: ConversationId, user: UserId) -> u32 {
        let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        let Some(room) = rooms.get_mut(&id) else {
            return 0;
        };
        if let Some(connections) = room.get_mut(&user) {
            *connections -= 1;
            if *connections == 0 {
                room.remove(&user);
            }
        }
        let count = room.len() as u32;
        if count == 0 {
            rooms.remove(&id);
        }
        count
    }

    pub fn count(&self, id: ConversationId) -> u32 {
        let rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        rooms.get(&id).map_or(0, |room| room.len() as u32)
    }

    /// Quem está na chamada, sempre na mesma ordem.
    pub fn people(&self, id: ConversationId) -> Vec<UserId> {
        let rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
        let mut people: Vec<UserId> = rooms
            .get(&id)
            .map(|room| room.keys().copied().collect())
            .unwrap_or_default();
        people.sort_by_key(|u| u.0);
        people
    }
}

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
    /// Sala musical isolada: serviço publica, pessoas só assinam áudio.
    /// Nenhuma chave MLS ou permissão de acesso à sala privada é concedida.
    pub fn music_token(&self, session: Uuid, identity: &str, publish: bool) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = serde_json::json!({
            "iss": self.api_key,
            "sub": identity,
            "name": if publish { "música" } else { "ouvinte" },
            "nbf": now,
            "exp": now + 60,
            "video": {
                "room": format!("music-{session}"),
                "roomJoin": true,
                "canPublish": publish,
                "canPublishSources": if publish { vec!["microphone"] } else { vec![] },
                "canSubscribe": !publish,
                "canPublishData": false,
                "canUpdateOwnMetadata": false
            }
        });
        jsonwebtoken::encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.api_secret.as_bytes()),
        )
        .expect("HS256 com segredo em bytes sempre assina")
    }

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

impl AppState {
    /// Marca `user` na chamada da conversa e avisa os membros. Chamado só
    /// depois de o token sair, então a pessoa já foi conferida como membro.
    pub async fn enter_call(&self, user: UserId, id: ConversationId) {
        let count = self.calls.enter(id, user);
        self.announce_call(id, count).await;
    }

    /// Tira `user` da chamada (saiu, caiu ou fechou o app) e avisa os membros.
    pub async fn leave_call(&self, user: UserId, id: ConversationId) {
        let count = self.calls.leave(id, user);
        self.announce_call(id, count).await;
    }

    /// As chamadas que já estão acontecendo nestas conversas, para mandar a
    /// quem acabou de entrar.
    pub async fn call_presences(
        &self,
        ids: impl IntoIterator<Item = ConversationId>,
    ) -> Vec<ServerMessage> {
        let mut presences = Vec::new();
        for conversation_id in ids {
            let count = self.calls.count(conversation_id);
            if count > 0 {
                presences.push(self.presence_message(conversation_id, count).await);
            }
        }
        presences
    }

    async fn announce_call(&self, id: ConversationId, count: u32) {
        // Sem a lista de membros (banco fora), a lista dos outros só fica
        // desatualizada até a próxima mudança.
        if let Ok(recipients) = self.member_ids(id).await {
            let message = self.presence_message(id, count).await;
            self.emit(recipients, message);
        }
    }

    /// `CallPresence` com quem está na chamada. O nome vem do banco na hora
    /// (quem trocou de nome aparece com o novo); sem banco, vai só a contagem.
    async fn presence_message(&self, id: ConversationId, count: u32) -> ServerMessage {
        let ids: Vec<Uuid> = self.calls.people(id).into_iter().map(|u| u.0).collect();
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, username FROM users WHERE id = ANY($1) ORDER BY lower(username), id",
        )
        .bind(&ids)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();
        ServerMessage::CallPresence {
            conversation_id: id,
            count,
            people: rows
                .into_iter()
                .map(|(id, username)| Member {
                    user_id: UserId(id),
                    username,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_counts_people_not_connections() {
        let presence = Presence::default();
        let room = ConversationId(Uuid::new_v4());
        let (ana, bia) = (UserId(Uuid::new_v4()), UserId(Uuid::new_v4()));

        assert_eq!(presence.enter(room, ana), 1);
        // A mesma pessoa em outro computador.
        assert_eq!(presence.enter(room, ana), 1);
        assert_eq!(presence.enter(room, bia), 2);
        let mut both = vec![ana, bia];
        both.sort_by_key(|u| u.0);
        assert_eq!(presence.people(room), both);
        assert_eq!(presence.leave(room, ana), 2);
        assert_eq!(presence.leave(room, ana), 1);
        assert_eq!(presence.leave(room, bia), 0);
        assert_eq!(presence.count(room), 0);
        // Sair de novo não fica negativo.
        assert_eq!(presence.leave(room, bia), 0);
    }
}
