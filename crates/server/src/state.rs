//! Estado do servidor: conversas e mensagens no PostgreSQL.
//!
//! O banco guarda tudo; o canal de eventos (`broadcast`) continua em memória,
//! porque só serve para avisar as conexões abertas agora.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{
    ChatMessage, ConversationId, ConversationKind, ConversationSummary, MAX_MESSAGE_CHARS,
    ServerMessage, UserId,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Quantas mensagens o histórico devolve de uma vez (as mais recentes).
const HISTORY_LIMIT: i64 = 100;

/// O grupo em que toda pessoa nova entra. O id é fixo e vem da migration.
pub const SALA_DO_GRUPO: ConversationId = ConversationId(Uuid::from_u128(1));

/// Um aviso para as conexões abertas, com a lista de quem pode recebê-lo.
#[derive(Debug, Clone)]
pub struct Event {
    /// Membros da conversa. As outras conexões ignoram o aviso.
    pub audience: Arc<[UserId]>,
    pub message: ServerMessage,
}

/// Estado compartilhado entre todas as conexões.
///
/// `Clone` aqui é barato: `PgPool` e `broadcast::Sender` já são ponteiros
/// compartilhados por dentro.
#[derive(Clone)]
pub struct AppState {
    pub(crate) db: PgPool,
    /// Canal que entrega cada mensagem nova para todas as conexões abertas.
    events: broadcast::Sender<Event>,
}

/// Uma linha de mensagem como vem do banco, já com o nome de quem escreveu.
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
            sent_at_ms: row.sent_at_ms as u64,
        }
    }
}

/// Uma linha da lista de conversas. As colunas da última mensagem vêm
/// vazias (`None`) quando a conversa ainda não tem nenhuma.
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

impl AppState {
    /// Conecta no banco e aplica as migrations que faltarem.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let db = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&db).await?;
        Ok(Self::from_pool(db))
    }

    /// Usa um pool que já existe e já tem as migrations (os testes usam isso).
    pub fn from_pool(db: PgPool) -> Self {
        let (events, _) = broadcast::channel(256);
        Self { db, events }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// As conversas de que a pessoa participa.
    pub async fn summaries(
        &self,
        user_id: UserId,
    ) -> Result<Vec<ConversationSummary>, sqlx::Error> {
        // `LATERAL` roda a subconsulta uma vez por conversa, pegando só a
        // última mensagem de cada uma.
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT c.id, c.name, c.kind,
                    m.author_id, u.username AS author_name, m.text, m.sent_at_ms
             FROM conversations c
             JOIN conversation_members cm ON cm.conversation_id = c.id AND cm.user_id = $1
             LEFT JOIN LATERAL (
                 SELECT author_id, text, sent_at_ms FROM messages
                 WHERE conversation_id = c.id
                 ORDER BY sent_at_ms DESC, id DESC
                 LIMIT 1
             ) m ON true
             LEFT JOIN users u ON u.id = m.author_id
             -- Mais recentes primeiro, como numa lista de conversas.
             ORDER BY m.sent_at_ms DESC NULLS LAST, c.created_at",
        )
        .bind(user_id.0)
        .fetch_all(&self.db)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
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
                    kind: parse_kind(&row.kind),
                    last_message,
                }
            })
            .collect())
    }

    /// As últimas mensagens da conversa, da mais antiga para a mais nova.
    /// `None` quando a conversa não existe ou a pessoa não participa dela.
    pub async fn history(
        &self,
        id: ConversationId,
        user_id: UserId,
    ) -> Result<Option<Vec<ChatMessage>>, sqlx::Error> {
        if !self.is_member(id, user_id).await? {
            return Ok(None);
        }

        // Pega as mais recentes e depois vira a ordem para a tela.
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT * FROM (
                 SELECT m.id, m.conversation_id, m.author_id, u.username AS author_name,
                        m.text, m.sent_at_ms
                 FROM messages m
                 JOIN users u ON u.id = m.author_id
                 WHERE m.conversation_id = $1
                 ORDER BY m.sent_at_ms DESC, m.id DESC
                 LIMIT $2
             ) recent
             ORDER BY sent_at_ms, id",
        )
        .bind(id.0)
        .bind(HISTORY_LIMIT)
        .fetch_all(&self.db)
        .await?;
        Ok(Some(rows.into_iter().map(ChatMessage::from).collect()))
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

        // O `WHERE EXISTS` só deixa gravar quem participa da conversa.
        let inserted = sqlx::query(
            "INSERT INTO messages (conversation_id, author_id, text, sent_at_ms)
             SELECT $1, $2, $3, $4
             WHERE EXISTS (
                 SELECT 1 FROM conversation_members
                 WHERE conversation_id = $1 AND user_id = $2
             )",
        )
        .bind(conversation_id.0)
        .bind(author_id.0)
        .bind(&message.text)
        .bind(message.sent_at_ms as i64)
        .execute(&self.db)
        .await
        .map_err(log_db_error)?;
        if inserted.rows_affected() == 0 {
            return Err("Essa conversa não existe.".into());
        }

        let members: Vec<Uuid> = sqlx::query_scalar(
            "SELECT user_id FROM conversation_members WHERE conversation_id = $1",
        )
        .bind(conversation_id.0)
        .fetch_all(&self.db)
        .await
        .map_err(log_db_error)?;
        let audience: Arc<[UserId]> = members.into_iter().map(UserId).collect();

        // `send` só falha quando ninguém está ouvindo, o que não é um erro aqui.
        let _ = self.events.send(Event {
            audience,
            message: ServerMessage::NewMessage {
                message: message.clone(),
            },
        });
        Ok(message)
    }

    async fn is_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM conversation_members
                 WHERE conversation_id = $1 AND user_id = $2
             )",
        )
        .bind(conversation_id.0)
        .bind(user_id.0)
        .fetch_one(&self.db)
        .await
    }
}

/// Coloca a pessoa na conversa (se ainda não estiver).
pub(crate) async fn join_conversation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation_id: ConversationId,
    user_id: UserId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO conversation_members (conversation_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(conversation_id.0)
    .bind(user_id.0)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Registra o erro no log e devolve a frase que a pessoa vê.
pub(crate) fn log_db_error(e: sqlx::Error) -> String {
    tracing::error!(error = %e, "falha no banco");
    DB_ERROR.into()
}

/// O que a pessoa vê quando o banco falha. O detalhe vai só para o log.
pub const DB_ERROR: &str = "O servidor não conseguiu acessar o banco. Tente de novo.";

fn parse_kind(kind: &str) -> ConversationKind {
    match kind {
        "direct" => ConversationKind::Direct,
        _ => ConversationKind::Group,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
