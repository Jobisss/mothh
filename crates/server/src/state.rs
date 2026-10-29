//! Estado do servidor guardado em memória.
//!
//! Por enquanto tudo some quando o servidor reinicia. O próximo passo troca
//! este módulo por PostgreSQL (sqlx) mantendo a mesma interface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{
    ChatMessage, ConversationId, ConversationKind, ConversationSummary, MAX_MESSAGE_CHARS,
    ServerMessage, UserId,
};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

struct Conversation {
    name: String,
    kind: ConversationKind,
    messages: Vec<ChatMessage>,
}

/// Estado compartilhado entre todas as conexões.
///
/// `Clone` aqui é barato: só copia os `Arc`, que apontam para os mesmos dados.
#[derive(Clone)]
pub struct AppState {
    conversations: Arc<RwLock<HashMap<ConversationId, Conversation>>>,
    /// Canal que entrega cada mensagem nova para todas as conexões abertas.
    events: broadcast::Sender<ServerMessage>,
}

impl AppState {
    /// Cria o estado com uma conversa de grupo inicial, a "Sala do Grupo".
    pub fn with_default_group() -> Self {
        let (events, _) = broadcast::channel(256);
        let mut conversations = HashMap::new();
        conversations.insert(
            ConversationId(Uuid::new_v4()),
            Conversation {
                name: "Sala do Grupo".into(),
                kind: ConversationKind::Group,
                messages: Vec::new(),
            },
        );
        Self {
            conversations: Arc::new(RwLock::new(conversations)),
            events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.events.subscribe()
    }

    pub async fn summaries(&self) -> Vec<ConversationSummary> {
        let conversations = self.conversations.read().await;
        let mut list: Vec<_> = conversations
            .iter()
            .map(|(id, c)| ConversationSummary {
                id: *id,
                name: c.name.clone(),
                kind: c.kind,
                last_message: c.messages.last().cloned(),
            })
            .collect();
        // Mais recentes primeiro, como numa lista de conversas.
        list.sort_by_key(|s| std::cmp::Reverse(s.last_message.as_ref().map(|m| m.sent_at_ms)));
        list
    }

    pub async fn history(&self, id: ConversationId) -> Option<Vec<ChatMessage>> {
        let conversations = self.conversations.read().await;
        conversations.get(&id).map(|c| c.messages.clone())
    }

    /// Guarda a mensagem e avisa todas as conexões.
    pub async fn post(
        &self,
        conversation_id: ConversationId,
        author_id: UserId,
        author_name: &str,
        text: &str,
    ) -> Result<ChatMessage, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("A mensagem está vazia.".into());
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(format!(
                "A mensagem passou de {MAX_MESSAGE_CHARS} caracteres."
            ));
        }

        let message = ChatMessage {
            conversation_id,
            author_id,
            author_name: author_name.to_owned(),
            text: text.to_owned(),
            sent_at_ms: now_ms(),
        };

        let mut conversations = self.conversations.write().await;
        let conversation = conversations
            .get_mut(&conversation_id)
            .ok_or_else(|| "Essa conversa não existe.".to_string())?;
        conversation.messages.push(message.clone());
        drop(conversations);

        // `send` só falha quando ninguém está ouvindo, o que não é um erro aqui.
        let _ = self.events.send(ServerMessage::NewMessage {
            message: message.clone(),
        });
        Ok(message)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
