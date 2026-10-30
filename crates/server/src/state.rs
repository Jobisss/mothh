//! Estado do servidor: o banco PostgreSQL e o canal de eventos ao vivo.
//!
//! As consultas usam `sqlx::query`/`query_as` comuns (checadas ao rodar, não
//! ao compilar), assim o projeto compila sem precisar de um banco ligado.

use std::sync::Arc;

use protocol::{
    ChatMessage, ConversationId, ConversationKind, ConversationSummary, MAX_MESSAGE_CHARS,
    MessageId, OLDER_PAGE, ServerMessage, UserId,
};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::login_limit::{LoginLimit, LoginLimiter};

/// A "Sala do Grupo", criada pela primeira migração. Todo usuário novo entra nela.
pub const GROUP_ROOM: ConversationId = ConversationId(Uuid::from_u128(1));

/// Quantas mensagens o histórico de uma conversa devolve, das mais recentes.
pub const HISTORY_LIMIT: usize = 100;

/// Um pedaço do histórico, da mais antiga para a mais nova.
#[derive(Debug)]
pub struct Page {
    pub messages: Vec<ChatMessage>,
    /// Existem mensagens mais antigas que a primeira da página.
    pub has_more: bool,
}

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
    pub(crate) login_limiter: Arc<LoginLimiter>,
}

/// Uma linha da consulta de mensagens, antes de virar `ChatMessage`.
#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    conversation_id: Uuid,
    author_id: Uuid,
    author_name: String,
    text: String,
    sent_at_ms: i64,
}

impl From<MessageRow> for ChatMessage {
    fn from(row: MessageRow) -> Self {
        ChatMessage {
            id: MessageId(row.id),
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
    message_id: Option<i64>,
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
        Self::with_login_limit(db, LoginLimit::default())
    }

    /// Igual a `new`, com outro limite de tentativas de login (os testes usam
    /// prazos de segundos).
    pub fn with_login_limit(db: PgPool, limit: LoginLimit) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            db,
            events,
            login_limiter: Arc::new(LoginLimiter::new(limit)),
        }
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
                    last.id AS message_id, last.author_id, u.username AS author_name, last.text,
                    (EXTRACT(EPOCH FROM last.sent_at) * 1000)::BIGINT AS sent_at_ms
             FROM conversation_members cm
             JOIN conversations c ON c.id = cm.conversation_id
             LEFT JOIN LATERAL (
                 SELECT id, author_id, text, sent_at FROM messages
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
                let last_message = match (
                    row.message_id,
                    row.author_id,
                    row.author_name,
                    row.text,
                    row.sent_at_ms,
                ) {
                    (
                        Some(id),
                        Some(author_id),
                        Some(author_name),
                        Some(text),
                        Some(sent_at_ms),
                    ) => Some(ChatMessage::from(MessageRow {
                        id,
                        conversation_id: row.id,
                        author_id,
                        author_name,
                        text,
                        sent_at_ms,
                    })),
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

    /// As últimas `HISTORY_LIMIT` mensagens.
    /// `None` quando a conversa não existe ou o usuário não participa dela.
    pub async fn history(&self, user: UserId, id: ConversationId) -> Result<Option<Page>, String> {
        if !self.is_member(user, id).await? {
            return Ok(None);
        }
        self.page(id, None, HISTORY_LIMIT).await.map(Some)
    }

    /// Até `OLDER_PAGE` mensagens anteriores a `before`. `None` como em `history`.
    pub async fn older(
        &self,
        user: UserId,
        id: ConversationId,
        before: MessageId,
    ) -> Result<Option<Page>, String> {
        if !self.is_member(user, id).await? {
            return Ok(None);
        }
        self.page(id, Some(before), OLDER_PAGE).await.map(Some)
    }

    /// As `limit` mensagens mais novas da conversa com id menor que `before`
    /// (ou todas, sem `before`).
    async fn page(
        &self,
        id: ConversationId,
        before: Option<MessageId>,
        limit: usize,
    ) -> Result<Page, String> {
        // Pede uma a mais só para saber se ainda tem coisa antes.
        let mut rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT m.id, m.conversation_id, m.author_id, u.username AS author_name, m.text,
                    (EXTRACT(EPOCH FROM m.sent_at) * 1000)::BIGINT AS sent_at_ms
             FROM messages m
             JOIN users u ON u.id = m.author_id
             WHERE m.conversation_id = $1 AND ($2::BIGINT IS NULL OR m.id < $2)
             ORDER BY m.id DESC
             LIMIT $3",
        )
        .bind(id.0)
        .bind(before.map(|b| b.0))
        .bind(limit as i64 + 1)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        // Veio da mais nova para a mais antiga; a tela quer o contrário.
        rows.reverse();
        Ok(Page {
            messages: rows.into_iter().map(ChatMessage::from).collect(),
            has_more,
        })
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
                 RETURNING id, conversation_id, author_id, text, sent_at
             )
             SELECT new.id, new.conversation_id, new.author_id, u.username AS author_name, new.text,
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
