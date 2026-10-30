//! Tipos que trafegam pelo WebSocket entre cliente e servidor.
//!
//! Tudo aqui é serializado em JSON com `serde`. Como cliente e servidor usam
//! esta mesma crate, uma mudança no protocolo quebra a compilação dos dois
//! lados ao mesmo tempo, em vez de virar um bug em produção.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identificador de uma conversa (pessoa ou grupo).
///
/// É um "newtype": um `Uuid` embrulhado num tipo próprio, para o compilador
/// não deixar confundir o id de uma conversa com o id de um usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub Uuid);

/// Identificador de uma mensagem. Cresce na ordem em que as mensagens chegam
/// ao servidor, então serve para pedir "as anteriores a esta".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub i64);

/// Identificador de um usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Direct,
    Group,
}

/// Uma linha da lista de conversas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: ConversationId,
    pub name: String,
    pub kind: ConversationKind,
    pub last_message: Option<ChatMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub author_id: UserId,
    pub author_name: String,
    pub text: String,
    /// Milissegundos desde 1970 (horário do servidor).
    pub sent_at_ms: u64,
}

/// O que o cliente manda para o servidor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Cria uma conta e já entra nela.
    Register {
        username: String,
        password: String,
    },
    Login {
        username: String,
        password: String,
    },
    /// Entra de novo com o token de uma sessão anterior, sem senha.
    Resume {
        token: String,
    },
    /// Encerra a sessão; o token deixa de valer. A conexão volta para a tela
    /// de login e pode entrar com outra conta.
    Logout,
    /// Pede o histórico de uma conversa.
    OpenConversation {
        conversation_id: ConversationId,
    },
    SendMessage {
        conversation_id: ConversationId,
        text: String,
    },
    /// Pede até `OLDER_PAGE` mensagens anteriores a `before`.
    LoadOlder {
        conversation_id: ConversationId,
        before: MessageId,
    },
}

/// O que o servidor manda para o cliente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Resposta a um `Register`, `Login` ou `Resume` que deu certo.
    Welcome {
        user_id: UserId,
        username: String,
        /// Guarde para entrar depois com `Resume`.
        token: String,
        conversations: Vec<ConversationSummary>,
    },
    /// O login, a criação da conta ou o `Resume` não deu certo.
    AuthError {
        reason: String,
    },
    /// As mensagens mais recentes de uma conversa, da mais antiga para a mais nova.
    History {
        conversation_id: ConversationId,
        messages: Vec<ChatMessage>,
        /// Existem mensagens mais antigas que estas.
        has_more: bool,
    },
    /// Resposta a um `LoadOlder`, da mais antiga para a mais nova.
    Older {
        conversation_id: ConversationId,
        messages: Vec<ChatMessage>,
        has_more: bool,
    },
    NewMessage {
        message: ChatMessage,
    },
    Error {
        reason: String,
    },
}

/// Limite de tamanho de uma mensagem de texto, em caracteres.
pub const MAX_MESSAGE_CHARS: usize = 4000;

/// Quantas mensagens um `LoadOlder` devolve de uma vez.
pub const OLDER_PAGE: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_round_trips_through_json() {
        let original = ClientMessage::SendMessage {
            conversation_id: ConversationId(Uuid::new_v4()),
            text: "bora abrir uma caixa depois".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"type\":\"send_message\""));
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn load_older_round_trips_through_json() {
        let original = ClientMessage::LoadOlder {
            conversation_id: ConversationId(Uuid::new_v4()),
            before: MessageId(1234),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"type\":\"load_older\""));
        assert!(json.contains("\"before\":1234"));
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn older_round_trips_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let original = ServerMessage::Older {
            conversation_id,
            messages: vec![ChatMessage {
                id: MessageId(7),
                conversation_id,
                author_id: UserId(Uuid::new_v4()),
                author_name: "nina".into(),
                text: "lá de cima".into(),
                sent_at_ms: 1_700_000_000_000,
            }],
            has_more: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"type\":\"older\""));
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn logout_has_only_the_type_tag() {
        let json = serde_json::to_string(&ClientMessage::Logout).unwrap();
        assert_eq!(json, r#"{"type":"logout"}"#);
    }
}
