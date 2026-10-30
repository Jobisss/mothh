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

/// Identificador de um anexo (por enquanto, só mensagens de áudio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttachmentId(pub Uuid);

/// A "Sala do Grupo", onde todo mundo entra ao criar a conta. Não tem dono:
/// ninguém convida, remove nem sai dela.
pub const GROUP_ROOM: ConversationId = ConversationId(Uuid::from_u128(1));

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
    /// Numa conversa direta, o nome da outra pessoa.
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
    /// Numa mensagem de áudio, o anexo e a duração; o `text` fica vazio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Audio>,
}

/// O áudio de uma mensagem. O arquivo (Ogg Opus) é baixado à parte, por HTTP,
/// em `ATTACHMENTS_PATH/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    pub attachment_id: AttachmentId,
    /// Calculada pelo servidor a partir do arquivo.
    pub duration_ms: u32,
}

/// Resposta do servidor ao enviar um anexo por HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploaded {
    pub id: AttachmentId,
}

/// Uma pessoa dentro de um grupo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub user_id: UserId,
    pub username: String,
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
    /// Manda um áudio que já subiu por HTTP (`POST ATTACHMENTS_PATH`).
    /// Só quem enviou o arquivo pode usá-lo numa mensagem.
    SendAudio {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
    },
    /// Pede até `OLDER_PAGE` mensagens anteriores a `before`.
    LoadOlder {
        conversation_id: ConversationId,
        before: MessageId,
    },
    /// Abre a conversa direta com essa pessoa, criando se ainda não existir.
    OpenDirect {
        username: String,
    },
    /// Cria um grupo só com quem pediu, que vira o dono.
    CreateGroup {
        name: String,
    },
    /// Qualquer membro do grupo pode convidar outra pessoa pelo usuário.
    InviteToGroup {
        conversation_id: ConversationId,
        username: String,
    },
    /// Sai do grupo. Se o dono sair, o membro mais antigo vira o dono.
    LeaveGroup {
        conversation_id: ConversationId,
    },
    /// Só o dono pode remover alguém.
    RemoveFromGroup {
        conversation_id: ConversationId,
        user_id: UserId,
    },
    /// Pede a lista de membros (a resposta é um `Members`).
    ListMembers {
        conversation_id: ConversationId,
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
    /// Resposta a um `OpenDirect`: a conversa que o app deve abrir.
    DirectOpened {
        conversation: ConversationSummary,
    },
    /// Alguém abriu uma conversa direta com você ou te convidou para um
    /// grupo: ela entra na lista.
    ConversationAdded {
        conversation: ConversationSummary,
    },
    /// Resposta a um `CreateGroup`: o grupo novo, para o app já abrir.
    GroupCreated {
        conversation: ConversationSummary,
    },
    /// Você saiu ou foi removido: a conversa sai da lista.
    ConversationRemoved {
        conversation_id: ConversationId,
    },
    /// Quem está no grupo. Chega ao pedir (`ListMembers`) e de novo, para
    /// todos os membros, sempre que alguém entra ou sai.
    Members {
        conversation_id: ConversationId,
        /// `None` só na Sala do Grupo.
        owner_id: Option<UserId>,
        members: Vec<Member>,
    },
    Error {
        reason: String,
    },
}

/// Limite de tamanho de uma mensagem de texto, em caracteres.
pub const MAX_MESSAGE_CHARS: usize = 4000;

/// Quantas mensagens um `LoadOlder` devolve de uma vez.
pub const OLDER_PAGE: usize = 50;

/// Limite do nome de um grupo, em caracteres.
pub const MAX_GROUP_NAME_CHARS: usize = 40;

/// Rota HTTP dos anexos. `POST` com o arquivo no corpo envia (responde um
/// `Uploaded`); `GET /anexos/{id}` baixa. As duas pedem o token da sessão no
/// cabeçalho `Authorization: Bearer <token>`.
pub const ATTACHMENTS_PATH: &str = "/anexos";

/// Tipo do arquivo de áudio: Opus dentro de Ogg.
pub const AUDIO_CONTENT_TYPE: &str = "audio/ogg";

/// Áudio mais longo que o servidor aceita: 5 minutos.
pub const MAX_AUDIO_MS: u32 = 5 * 60 * 1000;

/// Tamanho máximo do arquivo de áudio. 5 minutos de voz em Opus a 24 kbit/s
/// dão uns 900 KB; sobra folga para o cabeçalho e taxas um pouco maiores.
pub const MAX_AUDIO_BYTES: usize = 2 * 1024 * 1024;

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
                audio: None,
            }],
            has_more: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"type\":\"older\""));
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn open_direct_round_trips_through_json() {
        let original = ClientMessage::OpenDirect {
            username: "nina".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"type":"open_direct","username":"nina"}"#);
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn direct_opened_and_conversation_added_round_trip_through_json() {
        let conversation = ConversationSummary {
            id: ConversationId(Uuid::new_v4()),
            name: "nina".into(),
            kind: ConversationKind::Direct,
            last_message: None,
        };
        for (original, tag) in [
            (
                ServerMessage::DirectOpened {
                    conversation: conversation.clone(),
                },
                "\"type\":\"direct_opened\"",
            ),
            (
                ServerMessage::ConversationAdded { conversation },
                "\"type\":\"conversation_added\"",
            ),
        ] {
            let json = serde_json::to_string(&original).unwrap();
            assert!(json.contains(tag), "{json}");
            assert!(json.contains("\"kind\":\"direct\""), "{json}");
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, original);
        }
    }

    #[test]
    fn group_commands_round_trip_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let originals = [
            ClientMessage::CreateGroup {
                name: "turma do cs".into(),
            },
            ClientMessage::InviteToGroup {
                conversation_id,
                username: "nina".into(),
            },
            ClientMessage::LeaveGroup { conversation_id },
            ClientMessage::RemoveFromGroup {
                conversation_id,
                user_id: UserId(Uuid::new_v4()),
            },
            ClientMessage::ListMembers { conversation_id },
        ];
        for original in originals {
            let json = serde_json::to_string(&original).unwrap();
            let back: ClientMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, original);
        }
        let json = serde_json::to_string(&ClientMessage::CreateGroup { name: "x".into() }).unwrap();
        assert_eq!(json, r#"{"type":"create_group","name":"x"}"#);
    }

    #[test]
    fn group_events_round_trip_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let owner = UserId(Uuid::new_v4());
        let summary = ConversationSummary {
            id: conversation_id,
            name: "turma do cs".into(),
            kind: ConversationKind::Group,
            last_message: None,
        };
        let originals = [
            ServerMessage::GroupCreated {
                conversation: summary.clone(),
            },
            ServerMessage::ConversationAdded {
                conversation: summary,
            },
            ServerMessage::ConversationRemoved { conversation_id },
            ServerMessage::Members {
                conversation_id,
                owner_id: Some(owner),
                members: vec![Member {
                    user_id: owner,
                    username: "jobiss".into(),
                }],
            },
        ];
        for original in originals {
            let json = serde_json::to_string(&original).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, original);
        }
    }

    #[test]
    fn text_message_has_no_audio_field() {
        let message = ChatMessage {
            id: MessageId(1),
            conversation_id: ConversationId(Uuid::new_v4()),
            author_id: UserId(Uuid::new_v4()),
            author_name: "nina".into(),
            text: "oi".into(),
            sent_at_ms: 0,
            audio: None,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(!json.contains("audio"), "{json}");
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn audio_message_round_trips_through_json() {
        let attachment_id = AttachmentId(Uuid::new_v4());
        let message = ChatMessage {
            id: MessageId(2),
            conversation_id: ConversationId(Uuid::new_v4()),
            author_id: UserId(Uuid::new_v4()),
            author_name: "jobiss".into(),
            text: String::new(),
            sent_at_ms: 0,
            audio: Some(Audio {
                attachment_id,
                duration_ms: 10_000,
            }),
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"duration_ms\":10000"), "{json}");
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);

        let send = ClientMessage::SendAudio {
            conversation_id: message.conversation_id,
            attachment_id,
        };
        let json = serde_json::to_string(&send).unwrap();
        assert!(json.contains("\"type\":\"send_audio\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), send);
    }

    #[test]
    fn logout_has_only_the_type_tag() {
        let json = serde_json::to_string(&ClientMessage::Logout).unwrap();
        assert_eq!(json, r#"{"type":"logout"}"#);
    }
}
