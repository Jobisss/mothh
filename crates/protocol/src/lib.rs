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
    History {
        conversation_id: ConversationId,
        messages: Vec<ChatMessage>,
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
    fn logout_has_only_the_type_tag() {
        let json = serde_json::to_string(&ClientMessage::Logout).unwrap();
        assert_eq!(json, r#"{"type":"logout"}"#);
    }
}
