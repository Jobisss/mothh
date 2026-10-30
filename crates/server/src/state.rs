//! Estado do servidor: o banco PostgreSQL e o canal de eventos ao vivo.
//!
//! As consultas usam `sqlx::query`/`query_as` comuns (checadas ao rodar, não
//! ao compilar), assim o projeto compila sem precisar de um banco ligado.

use std::path::PathBuf;
use std::sync::Arc;

use protocol::{
    AttachmentId, Audio, ChatMessage, ConversationId, ConversationKind, ConversationSummary,
    FileAttachment, MAX_MESSAGE_CHARS, MessageId, OLDER_PAGE, ServerMessage, UserId,
};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::normalize_username;
use crate::calls::{LiveKitConfig, Presence};
use crate::login_limit::{LoginLimit, LoginLimiter};

/// A "Sala do Grupo", criada pela primeira migração. Todo usuário novo entra nela.
pub const GROUP_ROOM: ConversationId = protocol::GROUP_ROOM;

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
    /// Pasta onde ficam os arquivos dos anexos.
    pub(crate) attachments_dir: Arc<PathBuf>,
    /// `None` quando o LiveKit não foi configurado: `JoinCall` responde erro.
    pub(crate) livekit: Option<Arc<LiveKitConfig>>,
    /// Quem está em cada chamada, para a lista mostrar "▪ 3 na sala".
    pub(crate) calls: Arc<Presence>,
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
    /// Preenchidos só numa mensagem de áudio.
    attachment_id: Option<Uuid>,
    duration_ms: Option<i32>,
    file_name: Option<String>,
    content_type: Option<String>,
    size_bytes: Option<i32>,
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
            audio: row
                .attachment_id
                .zip(row.duration_ms)
                .map(|(id, duration_ms)| Audio {
                    attachment_id: AttachmentId(id),
                    duration_ms: duration_ms.max(0) as u32,
                }),
            file: row
                .attachment_id
                .zip(row.file_name)
                .map(|(id, name)| FileAttachment {
                    attachment_id: AttachmentId(id),
                    name,
                    content_type: row.content_type.unwrap_or_default(),
                    size_bytes: row.size_bytes.unwrap_or_default().max(0) as u32,
                }),
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
    attachment_id: Option<Uuid>,
    duration_ms: Option<i32>,
    file_name: Option<String>,
    content_type: Option<String>,
    size_bytes: Option<i32>,
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
            attachments_dir: Arc::new(PathBuf::from("anexos")),
            livekit: None,
            calls: Arc::default(),
        }
    }

    /// Liga as chamadas, com o LiveKit e a chave que assina os tokens.
    pub fn with_livekit(mut self, config: LiveKitConfig) -> Self {
        self.livekit = Some(Arc::new(config));
        self
    }

    /// Troca a pasta dos anexos (padrão: `anexos/`, a partir de onde o
    /// servidor foi aberto).
    pub fn with_attachments_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.attachments_dir = Arc::new(dir.into());
        self
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Entrega um evento ao vivo para as conexões de `recipients`.
    pub(crate) fn emit(&self, recipients: Vec<UserId>, message: ServerMessage) {
        // `send` só falha quando ninguém está ouvindo, o que não é um erro aqui.
        let _ = self.events.send(Event {
            recipients,
            message,
        });
    }

    /// Conversas de que o usuário participa, as com mensagem mais nova primeiro.
    pub async fn summaries(&self, user: UserId) -> Result<Vec<ConversationSummary>, String> {
        self.query_summaries(user, None).await
    }

    /// Uma conversa só, como apareceria na lista do usuário.
    pub(crate) async fn summary(
        &self,
        user: UserId,
        id: ConversationId,
    ) -> Result<Option<ConversationSummary>, String> {
        Ok(self.query_summaries(user, Some(id)).await?.pop())
    }

    /// `only` filtra uma conversa; `None` traz todas.
    async fn query_summaries(
        &self,
        user: UserId,
        only: Option<ConversationId>,
    ) -> Result<Vec<ConversationSummary>, String> {
        // `LATERAL` deixa a subconsulta enxergar `c.id`: pega só a última mensagem
        // de cada conversa. Conversa direta não tem nome próprio: mostra o da
        // outra pessoa.
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT c.id, c.kind,
                    CASE WHEN c.kind = 'direct' THEN COALESCE((
                        SELECT o.username FROM conversation_members om
                        JOIN users o ON o.id = om.user_id
                        WHERE om.conversation_id = c.id AND om.user_id <> $1
                        LIMIT 1
                    ), c.name) ELSE c.name END AS name,
                    last.id AS message_id, last.author_id, u.username AS author_name, last.text,
                    (EXTRACT(EPOCH FROM last.sent_at) * 1000)::BIGINT AS sent_at_ms,
                    last.attachment_id, a.duration_ms, a.file_name, a.content_type, a.size_bytes
             FROM conversation_members cm
             JOIN conversations c ON c.id = cm.conversation_id
             LEFT JOIN LATERAL (
                 SELECT id, author_id, text, sent_at, attachment_id FROM messages
                 WHERE conversation_id = c.id
                 ORDER BY id DESC LIMIT 1
             ) last ON true
             LEFT JOIN users u ON u.id = last.author_id
             LEFT JOIN attachments a ON a.id = last.attachment_id
             WHERE cm.user_id = $1 AND ($2::UUID IS NULL OR c.id = $2)
             ORDER BY last.sent_at DESC NULLS LAST, c.created_at",
        )
        .bind(user.0)
        .bind(only.map(|id| id.0))
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
                        attachment_id: row.attachment_id,
                        duration_ms: row.duration_ms,
                        file_name: row.file_name,
                        content_type: row.content_type,
                        size_bytes: row.size_bytes,
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
                    (EXTRACT(EPOCH FROM m.sent_at) * 1000)::BIGINT AS sent_at_ms,
                    m.attachment_id, a.duration_ms, a.file_name, a.content_type, a.size_bytes
             FROM messages m
             JOIN users u ON u.id = m.author_id
             LEFT JOIN attachments a ON a.id = m.attachment_id
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
        self.insert_message(conversation_id, author_id, text, None)
            .await
    }

    /// Manda uma mensagem de áudio com um anexo que o próprio autor enviou.
    pub async fn post_audio(
        &self,
        conversation_id: ConversationId,
        author_id: UserId,
        attachment_id: AttachmentId,
    ) -> Result<ChatMessage, String> {
        if !self.is_member(author_id, conversation_id).await? {
            return Err("Essa conversa não existe.".into());
        }
        let (owned,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM attachments
                 WHERE id = $1 AND uploader_id = $2 AND duration_ms IS NOT NULL
             )",
        )
        .bind(attachment_id.0)
        .bind(author_id.0)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        if !owned {
            return Err("Esse áudio não existe.".into());
        }
        self.insert_message(conversation_id, author_id, "", Some(attachment_id))
            .await
    }

    /// Envia um arquivo já salvo por quem está nesta conversa.
    pub async fn post_file(
        &self,
        conversation_id: ConversationId,
        author_id: UserId,
        attachment_id: AttachmentId,
    ) -> Result<ChatMessage, String> {
        if !self.is_member(author_id, conversation_id).await? {
            return Err("Essa conversa não existe.".into());
        }
        let (owned,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM attachments
                 WHERE id = $1 AND uploader_id = $2 AND file_name IS NOT NULL
             )",
        )
        .bind(attachment_id.0)
        .bind(author_id.0)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        if !owned {
            return Err("Esse arquivo não existe.".into());
        }
        self.insert_message(conversation_id, author_id, "", Some(attachment_id))
            .await
    }

    /// Grava a mensagem (já conferida) e avisa quem participa da conversa.
    async fn insert_message(
        &self,
        conversation_id: ConversationId,
        author_id: UserId,
        text: &str,
        attachment: Option<AttachmentId>,
    ) -> Result<ChatMessage, String> {
        let row: MessageRow = sqlx::query_as(
            "WITH new AS (
                 INSERT INTO messages (conversation_id, author_id, text, attachment_id)
                 VALUES ($1, $2, $3, $4)
                 RETURNING id, conversation_id, author_id, text, attachment_id, sent_at
             )
             SELECT new.id, new.conversation_id, new.author_id, u.username AS author_name, new.text,
                    (EXTRACT(EPOCH FROM new.sent_at) * 1000)::BIGINT AS sent_at_ms,
                    new.attachment_id, a.duration_ms, a.file_name, a.content_type, a.size_bytes
             FROM new
             JOIN users u ON u.id = new.author_id
             LEFT JOIN attachments a ON a.id = new.attachment_id",
        )
        .bind(conversation_id.0)
        .bind(author_id.0)
        .bind(text)
        .bind(attachment.map(|id| id.0))
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        let message = ChatMessage::from(row);

        let recipients = self.member_ids(conversation_id).await?;
        self.emit(
            recipients,
            ServerMessage::NewMessage {
                message: message.clone(),
            },
        );
        Ok(message)
    }

    /// Abre a conversa direta entre `user` e a pessoa com esse usuário,
    /// criando na primeira vez. Quando cria, a outra pessoa recebe a conversa
    /// na lista ao vivo.
    pub async fn open_direct(
        &self,
        user: UserId,
        username: &str,
    ) -> Result<ConversationSummary, String> {
        const NOBODY: &str = "Ninguém com esse usuário.";
        let username = normalize_username(username).map_err(|_| NOBODY.to_string())?;
        let other: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM users WHERE lower(username) = $1")
                .bind(&username)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        let other = UserId(other.ok_or(NOBODY)?.0);
        if other == user {
            return Err("Essa é a sua conta; escolha outra pessoa.".into());
        }

        let (low, high) = if user.0 < other.0 {
            (user, other)
        } else {
            (other, user)
        };
        let (id, created) = match self.find_direct(low, high).await? {
            Some(id) => (id, false),
            None => match self.create_direct(low, high).await? {
                Some(id) => (id, true),
                // Os dois abriram ao mesmo tempo e o outro ganhou: usa a dele.
                None => (
                    self.find_direct(low, high)
                        .await?
                        .ok_or("Erro no servidor, tente de novo.")?,
                    false,
                ),
            },
        };

        if created && let Some(conversation) = self.summary(other, id).await? {
            let _ = self.events.send(Event {
                recipients: vec![other],
                message: ServerMessage::ConversationAdded { conversation },
            });
        }
        self.summary(user, id)
            .await?
            .ok_or_else(|| "Erro no servidor, tente de novo.".into())
    }

    async fn find_direct(
        &self,
        low: UserId,
        high: UserId,
    ) -> Result<Option<ConversationId>, String> {
        let found: Option<(Uuid,)> = sqlx::query_as(
            "SELECT conversation_id FROM direct_conversations
             WHERE user_low = $1 AND user_high = $2",
        )
        .bind(low.0)
        .bind(high.0)
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        Ok(found.map(|(id,)| ConversationId(id)))
    }

    /// Cria a conversa e põe os dois nela, tudo ou nada. `None` quando outra
    /// conexão criou a mesma conversa um instante antes.
    async fn create_direct(
        &self,
        low: UserId,
        high: UserId,
    ) -> Result<Option<ConversationId>, String> {
        let mut tx = self.db.begin().await.map_err(db_error)?;
        // O nome fica vazio: a lista mostra o nome da outra pessoa.
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO conversations (name, kind) VALUES ('', 'direct') RETURNING id",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let inserted = sqlx::query(
            "INSERT INTO direct_conversations (user_low, user_high, conversation_id)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(low.0)
        .bind(high.0)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        if inserted.rows_affected() == 0 {
            // Desfaz a conversa criada à toa.
            return Ok(None);
        }
        let id = ConversationId(id);
        for user in [low, high] {
            add_member(&mut tx, id, user).await.map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(Some(id))
    }

    /// Quem participa da conversa agora.
    pub(crate) async fn member_ids(&self, id: ConversationId) -> Result<Vec<UserId>, String> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM conversation_members WHERE conversation_id = $1")
                .bind(id.0)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        Ok(rows.into_iter().map(|(id,)| UserId(id)).collect())
    }

    pub(crate) async fn is_member(&self, user: UserId, id: ConversationId) -> Result<bool, String> {
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
