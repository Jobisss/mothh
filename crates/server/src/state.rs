//! Estado do servidor: o banco PostgreSQL e o canal de eventos ao vivo.
//!
//! As consultas usam `sqlx::query`/`query_as` comuns (checadas ao rodar, não
//! ao compilar), assim o projeto compila sem precisar de um banco ligado.

use protocol::{
    ChatMessage, ConversationId, ConversationKind, ConversationSummary, MAX_MESSAGE_CHARS,
    ServerMessage, UserId,
};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

/// A "Sala do Grupo", criada pela primeira migração. Todo usuário novo entra nela.
pub const GROUP_ROOM: ConversationId = ConversationId(Uuid::from_u128(1));

/// Quantas mensagens o histórico de uma conversa devolve, das mais recentes.
pub const HISTORY_LIMIT: i64 = 100;

/// Um evento ao vivo e quem pode recebê-lo.
#[derive(Debug, Clone)]
pub struct Event {
    pub recipients: Vec<UserId>,
    pub message: ServerMessage,
}

/// Estado compartilhado entre todas as conexões.
///
/// `Clone` aqui é barato: `PgPool` e `Sender` são ponteiros para os mesmos dados.
#[derive(Clone)]
pub struct AppState {
    pub(crate) db: PgPool,
    /// Canal que entrega cada evento para todas as conexões abertas; cada uma
    /// descarta o que não for para ela.
    events: broadcast::Sender<Event>,
}

/// Uma linha da consulta de mensagens, antes de virar `ChatMessage`.
#[derive(sqlx::FromRow)]
struct MessageRow {
    conversation_id: Uuid,
    author_id: Uuid,
    author_name: String,
    text: String,
    sent_at_ms: i64,
}

impl From<MessageRow> for ChatMessage {
    fn from(row: MessageRow) -> Self {
        ChatMessage {
            conversation_id: ConversationId(row.conversation_id),
            author_id: UserId(row.author_id),
            author_name: row.author_name,
            text: row.text,
            sent_at_ms: row.sent_at_ms.max(0) as u64,
        }
    }
}

/// Uma conversa com a última mensagem, quando houver (`LEFT JOIN`).
#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    name: String,
    kind: String,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    text: Option<String>,
    sent_at_ms: Option<i64>,
}

/// Abre o pool de conexões e aplica as migrações que faltam.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPool::connect(database_url).await?;
    // `migrate!` embute os arquivos de `migrations/` no binário.
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// Registra o erro do banco no log e devolve uma frase para a pessoa.
pub(crate) fn db_error(err: sqlx::Error) -> String {
    tracing::error!(%err, "erro no banco");
    "Erro no servidor, tente de novo.".into()
}

impl AppState {
    pub fn new(db: PgPool) -> Self {
        let (events, _) = broadcast::channel(256);
        Self { db, events }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Conversas de que o usuário participa, as com mensagem mais nova primeiro.
    pub async fn summaries(&self, user: UserId) -> Result<Vec<ConversationSummary>, String> {
        // `LATERAL` deixa a subconsulta enxergar `c.id`: pega só a última mensagem
        // de cada conversa.
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT c.id, c.name, c.kind,
                    last.author_id, u.username AS author_name, last.text,
                    (EXTRACT(EPOCH FROM last.sent_at) * 1000)::BIGINT AS sent_at_ms
             FROM conversation_members cm
             JOIN conversations c ON c.id = cm.conversation_id
             LEFT JOIN LATERAL (
                 SELECT author_id, text, sent_at FROM messages
                 WHERE conversation_id = c.id
                 ORDER BY id DESC LIMIT 1
             ) last ON true
             LEFT JOIN users u ON u.id = last.author_id
             WHERE cm.user_id = $1
             ORDER BY last.sent_at DESC NULLS LAST, c.created_at",
        )
        .bind(user.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                // Os campos da mensagem vêm todos juntos ou todos nulos.
                let last_message = match (row.author_id, row.author_name, row.text, row.sent_at_ms)
                {
                    (Some(author_id), Some(author_name), Some(text), Some(sent_at_ms)) => {
                        Some(ChatMessage::from(MessageRow {
                            conversation_id: row.id,
                            author_id,
                            author_name,
                            text,
                            sent_at_ms,
                        }))
                    }
                    _ => None,
                };
                ConversationSummary {
                    id: ConversationId(row.id),
                    name: row.name,
                    kind: if row.kind == "direct" {
                        ConversationKind::Direct
                    } else {
                        ConversationKind::Group
                    },
                    last_message,
                }
            })
            .collect())
    }

    /// As últimas `HISTORY_LIMIT` mensagens, da mais antiga para a mais nova.
    /// `None` quando a conversa não existe ou o usuário não participa dela.
    pub async fn history(
        &self,
        user: UserId,
        id: ConversationId,
    ) -> Result<Option<Vec<ChatMessage>>, String> {
        if !self.is_member(user, id).await? {
            return Ok(None);
        }
        let mut rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT m.conversation_id, m.author_id, u.username AS author_name, m.text,
                    (EXTRACT(EPOCH FROM m.sent_at) * 1000)::BIGINT AS sent_at_ms
             FROM messages m
             JOIN users u ON u.id = m.author_id
             WHERE m.conversation_id = $1
             ORDER BY m.id DESC
             LIMIT $2",
        )
        .bind(id.0)
        .bind(HISTORY_LIMIT)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        // Veio da mais nova para a mais antiga; a tela quer o contrário.
        rows.reverse();
        Ok(Some(rows.into_iter().map(ChatMessage::from).collect()))
    }

    /// Guarda a mensagem e avisa as conexões de quem participa da conversa.
    pub async fn post(
        &self,
        conversation_id: ConversationId,
        author_id: UserId,
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
        if !self.is_member(author_id, conversation_id).await? {
            return Err("Essa conversa não existe.".into());
        }

        let row: MessageRow = sqlx::query_as(
            "WITH new AS (
                 INSERT INTO messages (conversation_id, author_id, text)
                 VALUES ($1, $2, $3)
                 RETURNING conversation_id, author_id, text, sent_at
             )
             SELECT new.conversation_id, new.author_id, u.username AS author_name, new.text,
                    (EXTRACT(EPOCH FROM new.sent_at) * 1000)::BIGINT AS sent_at_ms
             FROM new JOIN users u ON u.id = new.author_id",
        )
        .bind(conversation_id.0)
        .bind(author_id.0)
        .bind(text)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        let message = ChatMessage::from(row);

        let recipients: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM conversation_members WHERE conversation_id = $1")
                .bind(conversation_id.0)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;

        // `send` só falha quando ninguém está ouvindo, o que não é um erro aqui.
        let _ = self.events.send(Event {
            recipients: recipients.into_iter().map(|(id,)| UserId(id)).collect(),
            message: ServerMessage::NewMessage {
                message: message.clone(),
            },
        });
        Ok(message)
    }

    async fn is_member(&self, user: UserId, id: ConversationId) -> Result<bool, String> {
        let (member,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM conversation_members WHERE conversation_id = $1 AND user_id = $2
             )",
        )
        .bind(id.0)
        .bind(user.0)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        Ok(member)
    }
}

/// Coloca o usuário numa conversa; não faz nada se ele já estiver nela.
pub(crate) async fn add_member(
    conn: &mut sqlx::PgConnection,
    conversation: ConversationId,
    user: UserId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO conversation_members (conversation_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(conversation.0)
    .bind(user.0)
    .execute(conn)
    .await?;
    Ok(())
}
