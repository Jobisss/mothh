//! Tipos que trafegam pelo WebSocket entre cliente e servidor.
//!
//! Tudo aqui é serializado em JSON com `serde`. Como cliente e servidor usam
//! esta mesma crate, uma mudança no protocolo quebra a compilação dos dois
//! lados ao mesmo tempo, em vez de virar um bug em produção.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod music;
pub use music::*;

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

/// Identificador de um anexo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AttachmentId(pub Uuid);

/// Bytes cifrados (MLS) que o servidor guarda e repassa sem conseguir ler.
/// No JSON viajam em base64.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Sealed(pub Vec<u8>);

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealed({} bytes)", self.0.len())
    }
}

impl Serialize for Sealed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        base64_serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Sealed {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        base64_deserialize(deserializer).map(Sealed)
    }
}

fn base64_serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn base64_deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    use base64::Engine;
    let text = String::deserialize(deserializer)?;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(serde::de::Error::custom)
}

/// Foto de perfil: um PNG quadrado de `PHOTO_SIZE` px, no máximo
/// `PHOTO_MAX_BYTES`. Não é cifrada (o perfil é público para quem divide uma
/// conversa com a pessoa). No JSON viaja em base64.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Photo(pub Vec<u8>);

impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Photo({} bytes)", self.0.len())
    }
}

impl Serialize for Photo {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        base64_serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for Photo {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        base64_deserialize(deserializer).map(Photo)
    }
}

/// Lado da foto de perfil em pixels (o app reduz antes de mandar).
pub const PHOTO_SIZE: u32 = 128;
/// Tamanho máximo do PNG da foto de perfil.
pub const PHOTO_MAX_BYTES: usize = 200_000;

/// As imagens da página de perfil. A foto pequena (`Photo`, PNG) continua
/// sendo a das listas; `Avatar` é a mesma foto em tamanho maior, que pode
/// ser um GIF, para a página e o cartão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileImage {
    Avatar,
    Background,
    Art,
    ArtSide,
}

impl ProfileImage {
    pub const ALL: [ProfileImage; 4] = [
        ProfileImage::Avatar,
        ProfileImage::Background,
        ProfileImage::Art,
        ProfileImage::ArtSide,
    ];

    /// O nome na rota HTTP (`/perfil/imagens/fundo`).
    pub fn slug(self) -> &'static str {
        match self {
            ProfileImage::Avatar => "foto",
            ProfileImage::Background => "fundo",
            ProfileImage::Art => "arte",
            ProfileImage::ArtSide => "arte-lateral",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == slug)
    }

    /// Tamanho máximo do arquivo.
    pub fn max_bytes(self) -> usize {
        match self {
            ProfileImage::Avatar => 2 * 1024 * 1024,
            ProfileImage::Background => 4 * 1024 * 1024,
            ProfileImage::Art => 5 * 1024 * 1024,
            ProfileImage::ArtSide => 3 * 1024 * 1024,
        }
    }

    /// Largura e altura máximas, em pixels (o app reduz antes de mandar).
    pub fn max_size(self) -> (u32, u32) {
        match self {
            ProfileImage::Avatar => (512, 512),
            ProfileImage::Background => (1920, 1080),
            ProfileImage::Art => (1200, 1200),
            ProfileImage::ArtSide => (600, 1200),
        }
    }
}

/// Os formatos aceitos nas imagens do perfil.
pub const PROFILE_IMAGE_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/gif"];

/// Rotas HTTP das imagens do perfil, com o token no `Authorization`:
/// `POST /perfil/imagens/{tipo}` envia a sua (o corpo é a imagem, com o
/// `Content-Type` dela), `DELETE /perfil/imagens/{tipo}` tira, e
/// `GET /perfil/{id da pessoa}/imagens/{tipo}` baixa a de alguém.
pub const PROFILE_PATH: &str = "/perfil";

/// Limites dos textos do perfil, em caracteres.
pub const MAX_PROFILE_LINE_CHARS: usize = 40;
pub const MAX_ABOUT_CHARS: usize = 400;

/// Uma vitrine da página de perfil, na ordem em que aparecem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Showcase {
    /// A arte em destaque: a imagem grande e a faixa estreita.
    Art,
    /// O texto "sobre mim".
    About,
}

/// O que a pessoa escreve e escolhe na página de perfil.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileText {
    /// Nome real (opcional) e de onde é.
    pub real_name: String,
    pub location: String,
    pub about: String,
    /// Cor de destaque, "#rrggbb" (vazio: a do app).
    pub accent: String,
    pub showcases: Vec<Showcase>,
}

impl Default for ProfileText {
    fn default() -> Self {
        ProfileText {
            real_name: String::new(),
            location: String::new(),
            about: String::new(),
            accent: String::new(),
            showcases: vec![Showcase::Art, Showcase::About],
        }
    }
}

/// Uma imagem que a pessoa pôs no perfil. Baixe de novo quando a versão mudar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImageInfo {
    pub kind: ProfileImage,
    pub version: i64,
    pub content_type: String,
}

/// A página de perfil de alguém.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub listening: Option<ListeningActivity>,
    pub user_id: UserId,
    pub username: String,
    /// Quando a conta foi criada, em ms desde 1970.
    pub member_since_ms: i64,
    pub text: ProfileText,
    pub images: Vec<ProfileImageInfo>,
    /// Sobe a cada mudança (texto ou imagem).
    pub version: i64,
}

/// Presença transitória; não contém áudio, credenciais ou URLs arbitrárias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningActivity {
    pub title: String,
    pub artist: String,
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Um pacote de chaves MLS de uma pessoa, para pô-la num grupo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageOf {
    pub user_id: UserId,
    pub package: Sealed,
}

/// Uma mudança aceita no grupo MLS, feita na época `epoch` (quem aplica
/// passa para `epoch + 1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochCommit {
    pub epoch: u64,
    pub commit: Sealed,
}

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
    /// Numa conversa direta, quem é a outra pessoa (para a foto dela na
    /// lista). `None` em grupos e vindo de servidor antigo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_id: Option<UserId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub author_id: UserId,
    pub author_name: String,
    /// O texto já decifrado. O servidor manda sempre vazio: quem preenche é
    /// o app, depois de abrir `sealed`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Numa mensagem de texto, o texto cifrado com o grupo MLS da conversa
    /// (F2-08). O servidor não consegue ler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
    /// O texto foi trocado depois de enviado (`EditMessage`): a tela marca
    /// "editada". Servidor antigo não manda: `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edited: bool,
    /// Milissegundos desde 1970 (horário do servidor).
    pub sent_at_ms: u64,
    /// Numa mensagem de áudio, o anexo e a duração; o `text` fica vazio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Audio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileAttachment>,
}

/// O áudio de uma mensagem. O arquivo (Ogg Opus) é baixado à parte, por HTTP,
/// em `ATTACHMENTS_PATH/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audio {
    pub attachment_id: AttachmentId,
    /// Calculada pelo servidor a partir do arquivo.
    pub duration_ms: u32,
}

/// Imagem ou arquivo enviado numa mensagem. O conteúdo é baixado por HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub attachment_id: AttachmentId,
    pub name: String,
    pub content_type: String,
    pub size_bytes: u32,
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
    /// Música usa sala de serviço separada da chamada cifrada.
    Music {
        conversation_id: ConversationId,
        command: MusicCommand,
        #[serde(default)]
        expected_revision: Option<u64>,
    },
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
    /// Uma mensagem de texto, já cifrada com o grupo MLS da conversa.
    SendMessage {
        conversation_id: ConversationId,
        sealed: Sealed,
    },
    /// Manda um áudio que já subiu por HTTP (`POST ATTACHMENTS_PATH`).
    /// Só quem enviou o arquivo pode usá-lo numa mensagem.
    SendAudio {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
    },
    SendFile {
        conversation_id: ConversationId,
        attachment_id: AttachmentId,
    },
    /// Troca o texto de uma mensagem sua (só de texto), já cifrado de novo
    /// com o grupo MLS. Todo mundo da conversa recebe um `MessageEdited`.
    EditMessage {
        conversation_id: ConversationId,
        message_id: MessageId,
        sealed: Sealed,
    },
    /// Apaga uma mensagem sua para todo mundo: some do histórico. Todo mundo
    /// da conversa recebe um `MessageDeleted`.
    DeleteMessage {
        conversation_id: ConversationId,
        message_id: MessageId,
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
    /// Troca a senha. Precisa da atual; as outras sessões da conta caem
    /// (este aparelho continua entrado). Resposta: `PasswordChanged`.
    ChangePassword {
        current: String,
        new: String,
    },
    /// Troca o nome de usuário (o mesmo do login). Quem divide uma conversa
    /// com você recebe um `UserRenamed` (você também).
    Rename {
        username: String,
    },
    /// Troca a sua foto de perfil. Quem divide uma conversa com você recebe
    /// um `PhotoChanged` (você também).
    SetPhoto {
        png: Photo,
    },
    /// Pede a foto de alguém que divide uma conversa com você (resposta: `Photo`).
    GetPhoto {
        user_id: UserId,
    },
    /// Pede a página de perfil de alguém que divide uma conversa com você
    /// (ou a sua). Resposta: `Profile`.
    GetProfile {
        user_id: UserId,
    },
    SetListeningActivity {
        activity: Option<ListeningActivity>,
    },
    /// Salva os textos e as vitrines da sua página. Quem divide uma conversa
    /// com você recebe um `ProfileChanged` (você também).
    SaveProfile {
        text: ProfileText,
    },
    /// Pede um token para entrar na chamada da conversa (resposta: `CallToken`).
    /// Só quem é membro recebe.
    JoinCall {
        conversation_id: ConversationId,
    },
    /// Pacotes de chaves novos deste aparelho, para os outros poderem pô-lo
    /// em grupos. Mande quando o `KeyPackageStock` estiver baixo.
    UploadKeyPackages {
        packages: Vec<Sealed>,
        /// Apaga antes os pacotes que já estavam no servidor. O app manda
        /// assim na primeira vez de cada sessão: os pacotes de outra
        /// instalação (ou de outro perfil com a mesma conta) não servem para
        /// este aparelho, e um welcome feito com eles não abre aqui.
        #[serde(default)]
        replace: bool,
    },
    /// Pede um pacote de chaves de cada pessoa, para pô-las no grupo MLS da
    /// conversa (resposta: `KeyPackages`). Cada pacote só é entregue uma vez.
    ClaimKeyPackages {
        conversation_id: ConversationId,
        user_ids: Vec<UserId>,
    },
    /// Uma mudança no grupo MLS da conversa, preparada na época `epoch`. O
    /// servidor aceita só se `epoch` for a época atual dele (resposta:
    /// `CommitAccepted` ou `CommitRejected`). Sem `commit`, registra que quem
    /// mandou criou o grupo (só vale quando ele ainda não existe).
    Commit {
        conversation_id: ConversationId,
        epoch: u64,
        commit: Option<Sealed>,
        /// Para quem entrou (`added`).
        welcome: Option<Sealed>,
        added: Vec<UserId>,
        removed: Vec<UserId>,
    },
    /// Pede as mudanças do grupo a partir da época `from` (resposta: `Commits`).
    FetchCommits {
        conversation_id: ConversationId,
        from: u64,
    },
    /// Este aparelho perdeu o grupo da conversa (app reinstalado, estado
    /// corrompido): pede para ser tirado e posto de novo.
    RejoinGroup {
        conversation_id: ConversationId,
    },
    /// Saiu da chamada (ou não conseguiu entrar). Fechar a conexão também tira
    /// a pessoa da chamada.
    LeaveCall {
        conversation_id: ConversationId,
    },
}

/// O que o servidor manda para o cliente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    MusicSnapshot {
        snapshot: MusicSnapshot,
    },
    MusicError {
        conversation_id: ConversationId,
        request_id: Option<Uuid>,
        reason: String,
    },
    /// Token só de escuta, sem a chave MLS da chamada.
    MusicToken {
        conversation_id: ConversationId,
        session_id: Uuid,
        url: String,
        token: String,
    },
    /// Resposta a um `Register`, `Login` ou `Resume` que deu certo.
    Welcome {
        user_id: UserId,
        username: String,
        /// Guarde para entrar depois com `Resume`.
        token: String,
        conversations: Vec<ConversationSummary>,
        /// Versão da sua foto de perfil no servidor (0: ainda sem foto).
        /// `None` vem de um servidor antigo, que não guarda fotos: o app
        /// então não manda nem pede foto nenhuma.
        #[serde(default)]
        photo_version: Option<i64>,
        /// O servidor guarda páginas de perfil (`GetProfile` e as rotas de
        /// `PROFILE_PATH`). Servidor antigo não manda: `false`.
        #[serde(default)]
        profiles: bool,
        #[serde(default)]
        listening_activity: bool,
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
    /// Alguém editou uma mensagem: a mesma `id`, com o `sealed` novo e
    /// `edited` ligado. Troca a que está na tela.
    MessageEdited {
        message: ChatMessage,
    },
    /// Alguém apagou uma mensagem: ela some da tela e do histórico de todos.
    MessageDeleted {
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    /// A linha da conversa na lista mudou sem mensagem nova (a última foi
    /// apagada, por exemplo): troca ela pela que veio.
    ConversationUpdated {
        conversation: ConversationSummary,
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
    /// A senha foi trocada (resposta a `ChangePassword`).
    PasswordChanged,
    /// Alguém trocou de nome; quem mostra o nome antigo troca pelo novo.
    UserRenamed {
        user_id: UserId,
        username: String,
    },
    /// Resposta a um `GetPhoto`. `png` vazio (e `version` 0): sem foto.
    Photo {
        user_id: UserId,
        version: i64,
        png: Option<Photo>,
    },
    /// Alguém trocou a foto de perfil; quem tem a antiga guardada pede de novo.
    PhotoChanged {
        user_id: UserId,
        version: i64,
    },
    /// Resposta a um `GetProfile`.
    Profile {
        profile: Profile,
    },
    /// Alguém mudou a página de perfil (texto ou imagem); quem está com ela
    /// aberta pede de novo.
    ProfileChanged {
        user_id: UserId,
        version: i64,
    },
    /// Resposta a um `JoinCall`: com `url` e `token` o app conecta no LiveKit.
    /// A sala do LiveKit tem o id da conversa como nome.
    CallToken {
        conversation_id: ConversationId,
        url: String,
        token: String,
    },
    /// Quantas pessoas estão na chamada da conversa. Chega para todos os
    /// membros quando alguém entra ou sai, e logo depois do `Welcome` para as
    /// chamadas que já estão acontecendo.
    CallPresence {
        conversation_id: ConversationId,
        count: u32,
        /// Quem está na chamada, para quem ainda não entrou ver as fotos e os
        /// nomes. Vazio quando vem de um servidor antigo (só tinha a contagem).
        #[serde(default)]
        people: Vec<Member>,
    },
    /// Quantos pacotes de chaves deste usuário o servidor ainda tem. Chega
    /// logo depois do `Welcome` e quando alguém usa um.
    KeyPackageStock {
        count: u32,
    },
    /// Como está o grupo MLS da conversa. Chega depois do `Welcome`, para cada
    /// conversa, e para todos os membros sempre que algo muda.
    GroupState {
        conversation_id: ConversationId,
        /// A época atual. 0 com `members` vazio: o grupo ainda não existe.
        epoch: u64,
        /// Quem está no grupo MLS.
        members: Vec<UserId>,
        /// Está na conversa e tem pacote de chaves, mas não no grupo MLS.
        pending_add: Vec<UserId>,
        /// Está no grupo MLS, mas saiu da conversa.
        pending_remove: Vec<UserId>,
        /// Quem deve fazer as mudanças pendentes agora (um membro online).
        /// Assim só um aparelho gasta os pacotes de chaves.
        committer: Option<UserId>,
    },
    /// Resposta a `ClaimKeyPackages`. Quem não tinha pacote fica de fora.
    KeyPackages {
        conversation_id: ConversationId,
        packages: Vec<KeyPackageOf>,
    },
    /// Alguém mudou o grupo MLS da conversa (para os outros membros).
    Commit {
        conversation_id: ConversationId,
        commit: EpochCommit,
    },
    /// Resposta a `FetchCommits`, em ordem de época.
    Commits {
        conversation_id: ConversationId,
        commits: Vec<EpochCommit>,
    },
    /// Você foi posto no grupo MLS da conversa, que já está na época `epoch`.
    GroupWelcome {
        conversation_id: ConversationId,
        epoch: u64,
        welcome: Sealed,
    },
    /// O servidor aceitou a sua mudança: o grupo está agora na época `epoch`.
    CommitAccepted {
        conversation_id: ConversationId,
        epoch: u64,
    },
    /// Outra mudança chegou antes; o grupo está na época `epoch`.
    CommitRejected {
        conversation_id: ConversationId,
        epoch: u64,
    },
    Error {
        reason: String,
    },
}

/// Limite de tamanho de uma mensagem de texto, em caracteres.
pub const MAX_MESSAGE_CHARS: usize = 4000;

/// Limite de uma mensagem cifrada, em bytes: os 4000 caracteres em UTF-8
/// (até 4 bytes cada) mais a sobra do MLS.
pub const MAX_SEALED_BYTES: usize = MAX_MESSAGE_CHARS * 4 + 1024;

/// Limite de uma mudança no grupo (commit ou welcome). Cresce com o número de
/// membros; a Sala do Grupo tem todo mundo.
pub const MAX_HANDSHAKE_BYTES: usize = 1024 * 1024;

/// Limite de um pacote de chaves, e quantos o servidor guarda por pessoa.
pub const MAX_KEY_PACKAGE_BYTES: usize = 4096;
pub const MAX_KEY_PACKAGES: u32 = 100;

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

/// Limite de um arquivo enviado (inclusive imagens).
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_FILE_NAME_CHARS: usize = 120;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_round_trips_through_json() {
        let original = ClientMessage::SendMessage {
            conversation_id: ConversationId(Uuid::new_v4()),
            sealed: Sealed(vec![0, 1, 2, 255]),
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"type\":\"send_message\""));
        assert!(json.contains("\"sealed\":\"AAEC/w==\""), "{json}");
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
    fn edit_and_delete_round_trip_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let edit = ClientMessage::EditMessage {
            conversation_id,
            message_id: MessageId(9),
            sealed: Sealed(b"novo".to_vec()),
        };
        let json = serde_json::to_string(&edit).unwrap();
        assert!(json.contains("\"type\":\"edit_message\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), edit);

        let delete = ClientMessage::DeleteMessage {
            conversation_id,
            message_id: MessageId(9),
        };
        let json = serde_json::to_string(&delete).unwrap();
        assert!(json.contains("\"type\":\"delete_message\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), delete);

        let edited = ServerMessage::MessageEdited {
            message: ChatMessage {
                id: MessageId(9),
                conversation_id,
                author_id: UserId(Uuid::new_v4()),
                author_name: "nina".into(),
                text: String::new(),
                sealed: Some(Sealed(b"novo".to_vec())),
                edited: true,
                sent_at_ms: 1_700_000_000_000,
                audio: None,
                file: None,
            },
        };
        let json = serde_json::to_string(&edited).unwrap();
        assert!(json.contains("\"type\":\"message_edited\""), "{json}");
        assert!(json.contains("\"edited\":true"), "{json}");
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), edited);

        // Sem o campo (servidor antigo), a mensagem vem como não editada.
        let ServerMessage::MessageEdited { message } = edited else {
            unreachable!()
        };
        let json = serde_json::to_string(&message)
            .unwrap()
            .replace("\"edited\":true,", "");
        let back: ChatMessage = serde_json::from_str(&json).unwrap();
        assert!(!back.edited);

        let deleted = ServerMessage::MessageDeleted {
            conversation_id,
            message_id: MessageId(9),
        };
        let json = serde_json::to_string(&deleted).unwrap();
        assert!(json.contains("\"type\":\"message_deleted\""), "{json}");
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), deleted);
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
                text: String::new(),
                sealed: Some(Sealed(b"cifrado".to_vec())),
                sent_at_ms: 1_700_000_000_000,
                edited: false,
                audio: None,
                file: None,
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
            other_id: Some(UserId(Uuid::new_v4())),
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
            other_id: None,
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
            text: String::new(),
            sealed: Some(Sealed(b"oi".to_vec())),
            sent_at_ms: 0,
            edited: false,
            audio: None,
            file: None,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(!json.contains("audio"), "{json}");
        assert!(!json.contains("\"text\""), "{json}");
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
            sealed: None,
            sent_at_ms: 0,
            edited: false,
            audio: Some(Audio {
                attachment_id,
                duration_ms: 10_000,
            }),
            file: None,
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
    fn file_message_round_trips_through_json() {
        let id = AttachmentId(Uuid::new_v4());
        let conversation_id = ConversationId(Uuid::new_v4());
        let sent = ClientMessage::SendFile {
            conversation_id,
            attachment_id: id,
        };
        let json = serde_json::to_string(&sent).unwrap();
        assert!(json.contains("\"type\":\"send_file\""));
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), sent);

        let message = ChatMessage {
            id: MessageId(3),
            conversation_id,
            author_id: UserId(Uuid::new_v4()),
            author_name: "nina".into(),
            text: String::new(),
            sealed: None,
            sent_at_ms: 1,
            edited: false,
            audio: None,
            file: Some(FileAttachment {
                attachment_id: id,
                name: "foto.png".into(),
                content_type: "image/png".into(),
                size_bytes: 123,
            }),
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<ChatMessage>(&json).unwrap(), message);
    }

    #[test]
    fn call_messages_round_trip_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let join = ClientMessage::JoinCall { conversation_id };
        let json = serde_json::to_string(&join).unwrap();
        assert!(json.contains("\"type\":\"join_call\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), join);

        let token = ServerMessage::CallToken {
            conversation_id,
            url: "ws://127.0.0.1:7880".into(),
            token: "abc.def.ghi".into(),
        };
        let json = serde_json::to_string(&token).unwrap();
        assert!(json.contains("\"type\":\"call_token\""), "{json}");
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), token);
    }

    #[test]
    fn call_presence_round_trips_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let leave = ClientMessage::LeaveCall { conversation_id };
        let json = serde_json::to_string(&leave).unwrap();
        assert!(json.contains("\"type\":\"leave_call\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), leave);

        let presence = ServerMessage::CallPresence {
            conversation_id,
            count: 1,
            people: vec![Member {
                user_id: UserId(Uuid::new_v4()),
                username: "ana".into(),
            }],
        };
        let json = serde_json::to_string(&presence).unwrap();
        assert!(json.contains("\"type\":\"call_presence\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            presence
        );
    }

    #[test]
    fn mls_messages_round_trip_through_json() {
        let conversation_id = ConversationId(Uuid::new_v4());
        let user = UserId(Uuid::new_v4());
        let clients = [
            ClientMessage::UploadKeyPackages {
                packages: vec![Sealed(vec![1]), Sealed(vec![2])],
                replace: true,
            },
            ClientMessage::ClaimKeyPackages {
                conversation_id,
                user_ids: vec![user],
            },
            ClientMessage::Commit {
                conversation_id,
                epoch: 3,
                commit: Some(Sealed(vec![9])),
                welcome: None,
                added: vec![],
                removed: vec![user],
            },
            ClientMessage::FetchCommits {
                conversation_id,
                from: 2,
            },
            ClientMessage::RejoinGroup { conversation_id },
        ];
        for original in clients {
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientMessage>(&json).unwrap(),
                original
            );
        }
        let servers = [
            ServerMessage::KeyPackageStock { count: 4 },
            ServerMessage::GroupState {
                conversation_id,
                epoch: 1,
                members: vec![user],
                pending_add: vec![],
                pending_remove: vec![],
                committer: Some(user),
            },
            ServerMessage::KeyPackages {
                conversation_id,
                packages: vec![KeyPackageOf {
                    user_id: user,
                    package: Sealed(vec![7; 300]),
                }],
            },
            ServerMessage::Commit {
                conversation_id,
                commit: EpochCommit {
                    epoch: 1,
                    commit: Sealed(vec![5]),
                },
            },
            ServerMessage::GroupWelcome {
                conversation_id,
                epoch: 2,
                welcome: Sealed(vec![6]),
            },
            ServerMessage::CommitAccepted {
                conversation_id,
                epoch: 2,
            },
            ServerMessage::CommitRejected {
                conversation_id,
                epoch: 2,
            },
        ];
        for original in servers {
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(
                serde_json::from_str::<ServerMessage>(&json).unwrap(),
                original
            );
        }
        assert!(serde_json::from_str::<Sealed>("\"não é base64\"").is_err());
    }

    #[test]
    fn logout_has_only_the_type_tag() {
        let json = serde_json::to_string(&ClientMessage::Logout).unwrap();
        assert_eq!(json, r#"{"type":"logout"}"#);
    }

    #[test]
    fn profile_messages_round_trip_through_json() {
        let user_id = UserId(Uuid::from_u128(7));
        let text = ProfileText {
            real_name: "João".into(),
            location: "Brasil".into(),
            about: "oi".into(),
            accent: "#7dff9b".into(),
            showcases: vec![Showcase::About],
        };
        let save = ClientMessage::SaveProfile { text: text.clone() };
        let json = serde_json::to_string(&save).unwrap();
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), save);
        let profile = ServerMessage::Profile {
            profile: Profile {
                listening: None,
                user_id,
                username: "ana".into(),
                member_since_ms: 1,
                text,
                images: vec![ProfileImageInfo {
                    kind: ProfileImage::ArtSide,
                    version: 2,
                    content_type: "image/gif".into(),
                }],
                version: 3,
            },
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("\"art_side\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ServerMessage>(&json).unwrap(),
            profile
        );
        for kind in ProfileImage::ALL {
            assert_eq!(ProfileImage::from_slug(kind.slug()), Some(kind));
        }
    }

    #[test]
    fn welcome_from_an_old_server_has_no_profiles() {
        let json = r#"{"type":"welcome","user_id":"00000000-0000-0000-0000-000000000001","username":"a","token":"t","conversations":[]}"#;
        match serde_json::from_str::<ServerMessage>(json).unwrap() {
            ServerMessage::Welcome {
                profiles,
                photo_version,
                ..
            } => {
                assert!(!profiles);
                assert_eq!(photo_version, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn photo_messages_round_trip_through_json() {
        let user_id = UserId(Uuid::from_u128(7));
        let set = ClientMessage::SetPhoto {
            png: Photo(vec![0x89, b'P', b'N', b'G']),
        };
        let json = serde_json::to_string(&set).unwrap();
        assert!(json.contains("\"png\":\"iVBORw==\""), "{json}");
        assert_eq!(serde_json::from_str::<ClientMessage>(&json).unwrap(), set);

        for message in [
            ServerMessage::Photo {
                user_id,
                version: 3,
                png: Some(Photo(vec![1, 2, 3])),
            },
            ServerMessage::Photo {
                user_id,
                version: 0,
                png: None,
            },
            ServerMessage::PhotoChanged {
                user_id,
                version: 4,
            },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ServerMessage>(&json).unwrap(),
                message
            );
        }
    }

    #[test]
    fn account_messages_round_trip_through_json() {
        for message in [
            ClientMessage::ChangePassword {
                current: "velha-123".into(),
                new: "nova-12345".into(),
            },
            ClientMessage::Rename {
                username: "lia2".into(),
            },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ClientMessage>(&json).unwrap(),
                message
            );
        }
        for message in [
            ServerMessage::PasswordChanged,
            ServerMessage::UserRenamed {
                user_id: UserId(Uuid::from_u128(3)),
                username: "lia2".into(),
            },
        ] {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<ServerMessage>(&json).unwrap(),
                message
            );
        }
    }

    /// Um servidor antigo não manda `photo_version`: o app entende como "sem fotos".
    #[test]
    fn welcome_from_an_old_server_has_no_photo_version() {
        let json = r#"{"type":"welcome","user_id":"00000000-0000-0000-0000-000000000001",
            "username":"lia","token":"t","conversations":[]}"#;
        let ServerMessage::Welcome { photo_version, .. } = serde_json::from_str(json).unwrap()
        else {
            panic!("não é welcome");
        };
        assert_eq!(photo_version, None);
    }
}
