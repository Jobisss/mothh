//! Conexão com o servidor e o estado local da conversa.
//!
//! Esta tarefa é a única dona do estado (lista de conversas, conversa aberta,
//! mensagens). A interface só recebe cópias prontas para desenhar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ATTACHMENTS_PATH, AUDIO_CONTENT_TYPE, AttachmentId, ChatMessage, ClientMessage, ConversationId,
    ConversationKind, ConversationSummary, GROUP_ROOM, MAX_FILE_BYTES, Member, MessageId,
    PROFILE_PATH, ProfileImage, ServerMessage, Uploaded, UserId,
};
use slint::{Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, interval, sleep_until};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::audio::{AudioHandle, mm_ss};
use crate::call::{self, CallHandle, CallPerson, ShareRequest};
use crate::devices::Devices;
use crate::e2ee::{self, E2ee, ServerGroup};
use crate::mentions;
use crate::photos::{self, Photos};
use crate::profiles;
use crate::screen::{Preview, ShareSource};
use crate::session::SavedSession;
use crate::sframe;
use crate::sound;
use crate::volume;
use crate::{
    AppWindow, CallFace, CallPersonRow, Command, ConversationRow, MemberRow, MessageRow,
    ShareSourceRow,
};

/// De quanto em quanto tempo mandamos um ping para o servidor.
const PING_EVERY: Duration = Duration::from_secs(15);
/// Tempo máximo para enviar ou baixar um áudio.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Sem ouvir nada do servidor por esse tempo, a conexão é dada como morta.
/// Pega o caso em que a rede some e o sistema não avisa que o TCP fechou.
const SILENCE_LIMIT: Duration = Duration::from_secs(45);
/// Quanto esperar o servidor aceitar a conexão. Sem limite, um servidor fora
/// do ar que não recusa (o pacote some no caminho) prende o app no
/// "conectando..." pelo tempo do TCP do sistema, e enquanto isso os cliques
/// da interface ficam na fila sem resposta.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct LocalState {
    listening_supported: bool,
    /// O servidor aceita as linhas de diagnóstico (`diag.rs`).
    diagnostics_supported: bool,
    publishing_listening: bool,
    /// Páginas de perfil vistas e o que está no cartão e na página.
    profiles: profiles::Profiles,
    /// O nome que a tela mostrava de cada pessoa aberta no cartão ou na
    /// página, enquanto o perfil não chega.
    profile_names: HashMap<UserId, String>,
    /// Mandamos salvar a página e esperamos: um `Error` agora é sobre ela.
    profile_saving: bool,
    /// O editor já foi preenchido com a sua página desde que entrou.
    profile_editor_filled: bool,
    conversations: Vec<ConversationSummary>,
    open: Option<ConversationId>,
    messages: Vec<ChatMessage>,
    /// O servidor tem mensagens mais antigas que a primeira de `messages`.
    has_older: bool,
    /// Já pedimos as anteriores e estamos esperando; evita pedir duas vezes.
    loading_older: bool,
    /// As últimas mudanças entraram no topo da lista: a tela deve manter o
    /// ponto de leitura em vez de pular. Volta a `false` depois de desenhar.
    prepended: bool,
    /// Token da sessão atual, guardado também em memória: se o disco falhar,
    /// a reconexão continua entrando sozinha.
    token: Option<String>,
    /// Já recebemos um `Welcome` e a tela de conversas está aberta.
    logged_in: bool,
    /// Mandamos um `Resume` com o token salvo e esperamos a resposta.
    resuming: bool,
    /// Pedimos uma conversa direta e esperamos a resposta: um erro agora é
    /// sobre a busca, e aparece embaixo do campo.
    opening_direct: bool,
    /// Quem está entrado, para saber se é o dono do grupo aberto.
    me: Option<UserId>,
    /// E o nome dele, para achar "@nome" nas mensagens.
    my_name: String,
    /// Conversas com uma mensagem que marcou a gente e ainda não foram
    /// abertas: a lista mostra um "@".
    mentioned: HashSet<ConversationId>,
    /// Mensagens de outras pessoas que chegaram em conversas fechadas desde
    /// que o app abriu. Some ao abrir a conversa.
    unread: HashMap<ConversationId, u32>,
    /// Quantas não lidas a conversa que acabou de abrir tinha: quando o
    /// histórico chega, vira `unread_mark`.
    unread_pending: u32,
    /// A primeira mensagem não lida da conversa aberta: a linha "novas"
    /// aparece em cima dela.
    unread_mark: Option<MessageId>,
    /// A coluna de membros do grupo aberto está na tela.
    members_open: bool,
    /// Membros e dono do grupo aberto, do último `Members`.
    members: Vec<Member>,
    owner: Option<UserId>,
    /// Pedimos algo de grupo e esperamos: um `Error` agora é a resposta dele.
    group_pending: bool,
    /// Áudios já baixados (ou enviados daqui), para tocar de novo sem baixar.
    audio_cache: HashMap<AttachmentId, Vec<u8>>,
    image_cache: HashMap<AttachmentId, Thumbnail>,
    pending_images: HashSet<AttachmentId>,
    /// A chamada em que estamos, ou entrando. Uma por vez; largar o
    /// `CallHandle` (sair da conta, por exemplo) também sai da chamada.
    call: Option<CallHandle>,
    /// Pedimos o token da chamada desta conversa e esperamos o `CallToken`.
    call_pending: Option<ConversationId>,
    /// Pedimos para entrar na chamada desta conversa, mas o aparelho ainda
    /// não tem a chave do grupo: entra sozinho quando ela chegar.
    call_waiting_key: Option<ConversationId>,
    /// A chamada já conectou no LiveKit; antes disso a tela diz "entrando".
    call_connected: bool,
    call_muted: bool,
    /// Quem está na chamada em que estamos, e quem está falando (F2-04).
    call_people: Vec<CallPerson>,
    call_camera: bool,
    /// Estamos compartilhando a tela na chamada (F2-06).
    call_sharing: bool,
    /// A lista de telas e janelas está aberta, e o que ela mostra (vazia
    /// enquanto `screen.rs` ainda procura).
    share_picker_open: bool,
    share_sources: Vec<ShareSource>,
    /// As fotos da lista, por id, conforme `screen.rs` tira.
    share_previews: HashMap<String, Preview>,
    /// Quantas pessoas há na chamada de cada conversa, do `CallPresence`.
    in_call: HashMap<ConversationId, u32>,
    /// E quem são (vazio com servidor antigo, que só manda a contagem).
    call_members: HashMap<ConversationId, Vec<Member>>,
    /// Acabamos de reconectar no meio de uma chamada: avisa o servidor de
    /// novo, porque ele tirou a gente da conta quando a conexão caiu.
    rejoin_call: bool,
    /// As chaves das conversas (F2-08). Abre no `Welcome`, com o id da conta.
    e2ee: Option<E2ee>,
    /// O que mandar para o servidor depois de tratar uma mensagem ou um
    /// comando, além da resposta direta (a troca de chaves gera várias).
    outgoing: Vec<ClientMessage>,
    /// Fotos de perfil dos outros: quais temos, quais pedimos.
    photos: Photos,
    /// Pedimos uma troca de nome ou de senha: um `Error` agora é a resposta.
    profile_pending: Option<ProfileOp>,
}

/// Resposta para quem tenta trocar nome ou senha num servidor antigo.
const OLD_SERVER: &str = "o servidor ainda não tem essa função: ele precisa ser atualizado";

/// O que a página de perfil pediu ao servidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileOp {
    Name,
    Password,
    Email,
}

/// Mostra na página de perfil como foi a troca (embaixo da seção certa).
fn profile_message(ui: &slint::Weak<AppWindow>, op: ProfileOp, text: String, error: bool) {
    let _ = ui.upgrade_in_event_loop(move |ui| match op {
        ProfileOp::Name => {
            ui.set_profile_name_message(text.into());
            ui.set_profile_name_error(error);
        }
        ProfileOp::Password => {
            ui.set_profile_password_message(text.into());
            ui.set_profile_password_error(error);
        }
        ProfileOp::Email => {
            ui.set_profile_email_message(text.into());
            ui.set_profile_email_error(error);
        }
    });
}

/// Primeira letra, maiúscula (para quem não tem foto).
pub fn initial_of(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

/// Alguém trocou de nome: troca em tudo que já está na tela.
fn renamed(state: &mut LocalState, user_id: UserId, username: &str) {
    if state.me == Some(user_id) {
        state.my_name = username.to_string();
    }
    for c in &mut state.conversations {
        if c.other_id == Some(user_id) {
            c.name = username.to_string();
        }
        if let Some(m) = &mut c.last_message
            && m.author_id == user_id
        {
            m.author_name = username.to_string();
        }
    }
    for m in &mut state.messages {
        if m.author_id == user_id {
            m.author_name = username.to_string();
        }
    }
    for m in state
        .members
        .iter_mut()
        .chain(state.call_members.values_mut().flatten())
    {
        if m.user_id == user_id {
            m.username = username.to_string();
        }
    }
    let id = user_id.0.to_string();
    for p in &mut state.call_people {
        if p.id == id {
            p.name = username.to_string();
        }
    }
}

#[derive(Clone)]
struct Thumbnail {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// O que os envios e downloads de áudio precisam. Eles rodam em tarefas
/// separadas, para a conversa não travar enquanto o arquivo vai e volta;
/// quando terminam, avisam a rede pelo próprio canal de comandos.
#[derive(Clone)]
struct Media {
    commands: UnboundedSender<Command>,
    audio: AudioHandle,
    /// Microfone, saída e câmera escolhidos, para a chamada.
    devices: Devices,
    http: reqwest::Client,
    /// "http://127.0.0.1:3000", tirado do endereço do WebSocket.
    base: String,
}

/// Por que uma conexão terminou.
enum Ended {
    /// O servidor caiu ou a rede sumiu: tenta de novo.
    Lost,
    /// A janela fechou: acabou o app.
    Quit,
}

/// Espera antes da tentativa número `attempt` (começando em 0): 1s, 2s, 4s,
/// 8s, 16s e depois sempre 30s.
fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(5);
    Duration::from_secs(secs.min(30))
}

pub async fn run(
    url: String,
    saved: SavedSession,
    ui: slint::Weak<AppWindow>,
    command_tx: UnboundedSender<Command>,
    mut commands: UnboundedReceiver<Command>,
    audio: AudioHandle,
    devices: Devices,
) {
    let mut state = LocalState {
        token: saved.load(),
        ..LocalState::default()
    };
    let media = Media {
        commands: command_tx,
        audio,
        devices,
        http: reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_default(),
        base: http_base(&url),
    };
    let mut attempt = 0;

    loop {
        if let Ok(Ok((socket, _))) =
            tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&url)).await
        {
            attempt = 0;
            match connection(socket, &mut state, &saved, &ui, &mut commands, &media).await {
                Ended::Quit => return,
                Ended::Lost => {
                    state.listening_supported = false;
                    state.profiles.clear_listening();
                    push_profile_views(&state, &ui);
                    render(&mut state, &ui);
                }
            }
        }

        let delay = backoff(attempt);
        attempt = attempt.saturating_add(1);
        show_offline(&state, &ui, delay);
        if !wait_offline(delay, &mut state, &saved, &ui, &mut commands, &media).await {
            return;
        }
    }
}

/// Uma conexão do começo ao fim. O estado local sobrevive entre conexões,
/// por isso a conversa aberta continua na tela enquanto o app reconecta.
async fn connection(
    socket: Socket,
    state: &mut LocalState,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
    commands: &mut UnboundedReceiver<Command>,
    media: &Media,
) -> Ended {
    let (mut sink, mut stream) = socket.split();

    // Com um token, tenta entrar direto; senão fica na tela de login.
    if let Some(token) = state.token.clone() {
        state.resuming = true;
        if send(&mut sink, &ClientMessage::Resume { token })
            .await
            .is_err()
        {
            return Ended::Lost;
        }
    } else {
        set_auth_error(ui, String::new());
        show_login(ui);
    }

    let mut ping = interval(PING_EVERY);
    let mut heard_at = Instant::now();

    loop {
        tokio::select! {
            frame = stream.next() => {
                let text = match frame {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ended::Lost,
                    // Ping, pong, binário: só conta como sinal de vida.
                    Some(Ok(_)) => {
                        heard_at = Instant::now();
                        continue;
                    }
                };
                heard_at = Instant::now();
                let reply = match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(msg) => {
                        let reply = apply(state, msg, saved, ui, media);
                        prefetch_images(state, media);
                        reply
                    },
                    Err(_) => {
                        set_status(ui, "o servidor mandou algo que não entendi".into());
                        None
                    }
                };
                if let Some(reply) = reply
                    && send(&mut sink, &reply).await.is_err()
                {
                    return Ended::Lost;
                }
                for msg in std::mem::take(&mut state.outgoing) {
                    if send(&mut sink, &msg).await.is_err() {
                        return Ended::Lost;
                    }
                }
                // O LiveKit não caiu junto com o servidor do mothh; só pede o
                // token de novo para voltar a contar na lista dos outros.
                if std::mem::take(&mut state.rejoin_call)
                    && let Some(conversation_id) = state.call.as_ref().map(|c| c.conversation_id)
                    && send(&mut sink, &ClientMessage::JoinCall { conversation_id }).await.is_err()
                {
                    return Ended::Lost;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ended::Quit };
                let outgoing = match command {
                    Command::ListeningSample(activity) => {
                        if state.logged_in && state.listening_supported {
                            for place in [profiles::Place::Card, profiles::Place::Page] {
                                if let Some(user_id) = state.profiles.showing(place) {
                                    state.outgoing.push(ClientMessage::GetProfile { user_id });
                                }
                            }
                            let publish = activity.is_some() || state.publishing_listening;
                            state.publishing_listening = activity.is_some();
                            publish.then_some(ClientMessage::SetListeningActivity { activity })
                        } else { None }
                    }
                    Command::Login { username, password } => {
                        Some(ClientMessage::Login { username, password })
                    }
                    Command::Register {
                        username,
                        email,
                        password,
                    } => Some(ClientMessage::Register {
                        username,
                        password,
                        email: Some(email),
                    }),
                    Command::RequestReset(email) => {
                        Some(ClientMessage::RequestPasswordReset { email })
                    }
                    Command::ResetPassword {
                        email,
                        code,
                        password,
                    } => Some(ClientMessage::ResetPassword {
                        email,
                        code,
                        password,
                    }),
                    Command::SetEmail(email) => {
                        state.profile_pending = Some(ProfileOp::Email);
                        Some(ClientMessage::SetEmail { email })
                    }
                    Command::Logout => {
                        forget_session(state, saved, ui);
                        Some(ClientMessage::Logout)
                    }
                    Command::Open(id) => open(state, &id, ui)
                        .map(|conversation_id| ClientMessage::OpenConversation { conversation_id }),
                    Command::LoadOlder => {
                        let msg = load_older(state);
                        // Mostra o "carregando..." no topo.
                        render(state, ui);
                        msg
                    }
                    Command::Send(text) => {
                        send_text(state, state.open, text, ui);
                        None
                    }
                    Command::EditMessage { id, text } => edit_text(state, &id, text, ui),
                    Command::DeleteMessage(id) => {
                        state.open.zip(parse_message_id(&id)).map(|(conversation_id, message_id)| {
                            ClientMessage::DeleteMessage {
                                conversation_id,
                                message_id,
                            }
                        })
                    }
                    Command::OpenCallChat => open_call_chat(state, ui)
                        .map(|conversation_id| ClientMessage::OpenConversation { conversation_id }),
                    Command::SendCall(text) => {
                        let conversation = call_conversation(state).or(state.open);
                        send_text(state, conversation, text, ui);
                        None
                    }
                    Command::OpenDirect(username) => {
                        let username = username.trim().to_string();
                        (!username.is_empty()).then(|| {
                            state.opening_direct = true;
                            set_direct_error(ui, String::new());
                            ClientMessage::OpenDirect { username }
                        })
                    }
                    Command::SendAudio(ogg) => {
                        upload_audio(state, media, ogg, ui);
                        None
                    }
                    Command::PlayAudio(id) => {
                        play_audio(state, media, &id, ui);
                        None
                    }
                    Command::AudioUploaded { conversation_id, attachment_id, ogg } => {
                        state.audio_cache.insert(attachment_id, ogg);
                        set_status(ui, "online".into());
                        Some(ClientMessage::SendAudio { conversation_id, attachment_id })
                    }
                    Command::AudioDownloaded { attachment_id, ogg } => {
                        audio_downloaded(state, media, attachment_id, ogg);
                        // Tira o "baixando áudio..." do rodapé.
                        set_status(ui, "online".into());
                        None
                    }
                    Command::AudioFailed(reason) => {
                        set_status(ui, reason);
                        None
                    }
                    Command::ViewImage(id, name) => {
                        if let Some(token) = state.token.clone() {
                            let media = media.clone();
                            let weak = ui.clone();
                            tokio::spawn(async move {
                                let result = async {
                                    let bytes = download(&media, &token, id).await?;
                                    tokio::task::spawn_blocking(move || {
                                        image::load_from_memory(&bytes).map(|image| image.to_rgba8()).map_err(|e| e.to_string())
                                    }).await.map_err(|e| e.to_string())?
                                }.await;
                                match result {
                                    Ok(image) => {
                                        let (w, h) = image.dimensions();
                                        let bytes = image.into_raw();
                                        let _ = weak.upgrade_in_event_loop(move |ui| {
                                            ui.set_viewer_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(&bytes, w, h)));
                                            ui.set_viewer_id(id.0.to_string().into());
                                            ui.set_viewer_name(name.into());
                                            ui.set_viewer_zoom(1.0);
                                            ui.set_image_open(true);
                                        });
                                    }
                                    Err(reason) => set_status(&weak, reason),
                                }
                            });
                        }
                        None
                    }
                    Command::SendFile(path, caption) => {
                        upload_file(state, media, path, caption, ui);
                        None
                    }
                    Command::FileUploaded { conversation_id, attachment_id, caption } => {
                        if !caption.trim().is_empty()
                            && let Some(e2ee) = &mut state.e2ee
                            && let Err(reason) = e2ee.send(conversation_id, caption, &mut state.outgoing)
                        {
                            set_status(ui, reason);
                        }
                        set_status(ui, "online".into());
                        Some(ClientMessage::SendFile { conversation_id, attachment_id })
                    }
                    Command::SaveFile { attachment_id, path } => {
                        save_file(state, media, attachment_id, path, ui);
                        None
                    }
                    Command::ImageReady { attachment_id, width, height, pixels } => {
                        state.pending_images.remove(&attachment_id);
                        state.image_cache.insert(attachment_id, Thumbnail { width, height, pixels });
                        render(state, ui);
                        None
                    }
                    Command::ImageFailed(id) => {
                        state.pending_images.remove(&id);
                        None
                    }
                    Command::JoinCall => join_call(state, ui),
                    command @ (Command::ShowProfile { .. }
                    | Command::HideProfile(_)
                    | Command::SaveProfile(_)
                    | Command::SetProfileImage { .. }
                    | Command::RemoveProfileImage(_)
                    | Command::ProfileImageDone(_)) => {
                        profile_command(state, command, media, ui, true);
                        None
                    }
                    Command::SetPhoto(png) => {
                        state.photos.set_mine(png, true, &mut state.outgoing);
                        None
                    }
                    // Servidor antigo responderia só "Mensagem inválida".
                    Command::Rename(_) | Command::ChangePassword { .. } if !state.photos.server_supports() => {
                        let op = if matches!(command, Command::Rename(_)) { ProfileOp::Name } else { ProfileOp::Password };
                        profile_message(ui, op, OLD_SERVER.into(), true);
                        None
                    }
                    Command::Rename(username) => {
                        state.profile_pending = Some(ProfileOp::Name);
                        Some(ClientMessage::Rename { username })
                    }
                    Command::ChangePassword { current, new } => {
                        state.profile_pending = Some(ProfileOp::Password);
                        Some(ClientMessage::ChangePassword { current, new })
                    }
                    command @ (Command::LeaveCall
                    | Command::ToggleMute
                    | Command::ToggleCamera
                    | Command::CameraFailed(_)
                    | Command::OpenSharePicker
                    | Command::CloseSharePicker
                    | Command::ShareSources(_)
                    | Command::SharePreview(_)
                    | Command::StartShare { .. }
                    | Command::PointAt { .. }
                    | Command::DevicesChanged
                    | Command::CallWarning(_)
                    | Command::StopShare
                    | Command::ShareFailed(_)
                    | Command::CallJoined(_)
                    | Command::CallPeople { .. }
                    | Command::CallEnded { .. }) => call_command(state, command, media, ui),
                    command => group_command(state, command, ui),
                };
                if let Some(msg) = outgoing
                    && send(&mut sink, &msg).await.is_err()
                {
                    return Ended::Lost;
                }
                for msg in std::mem::take(&mut state.outgoing) {
                    if send(&mut sink, &msg).await.is_err() {
                        return Ended::Lost;
                    }
                }
            }
            _ = ping.tick() => {
                if heard_at.elapsed() > SILENCE_LIMIT
                    || sink.send(Message::Ping(Default::default())).await.is_err()
                {
                    return Ended::Lost;
                }
                // Algo parado em "trocando chaves": cutuca o servidor, que
                // manda o estado do grupo de novo e, se quem devia mudar o
                // grupo não agiu, passa a vez a outro membro online.
                // As linhas de diagnóstico vão para o log do servidor.
                let lines = crate::diag::take();
                if state.logged_in && state.diagnostics_supported {
                    for text in lines {
                        if send(&mut sink, &ClientMessage::Diagnostic { text }).await.is_err() {
                            return Ended::Lost;
                        }
                    }
                }
                // Na chamada: confere se perdemos algum commit do grupo, para
                // a chave da mídia não ficar velha (ver `E2ee::catch_up`).
                if let Some(conversation_id) = state.call.as_ref().map(|c| c.conversation_id)
                    && let Some(e2ee) = &mut state.e2ee
                {
                    let mut out = Vec::new();
                    e2ee.catch_up(conversation_id, &mut out);
                    for msg in out {
                        if send(&mut sink, &msg).await.is_err() {
                            return Ended::Lost;
                        }
                    }
                }
                let stuck = state.e2ee.as_ref().map(|e| e.stuck()).unwrap_or_default();
                for conversation_id in stuck {
                    if send(&mut sink, &ClientMessage::NudgeGroup { conversation_id })
                        .await
                        .is_err()
                    {
                        return Ended::Lost;
                    }
                }
            }
        }
    }
}

/// Espera `delay` sem conexão, ainda atendendo a interface. `false` quando a
/// janela fechou.
async fn wait_offline(
    delay: Duration,
    state: &mut LocalState,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
    commands: &mut UnboundedReceiver<Command>,
    media: &Media,
) -> bool {
    let until = Instant::now() + delay;
    loop {
        tokio::select! {
            _ = sleep_until(until) => return true,
            command = commands.recv() => {
                let Some(command) = command else { return false };
                match command {
                    Command::ListeningSample(_) => {}
                    // Os botões ficam desligados sem conexão; isto é só garantia.
                    Command::Login { .. }
                    | Command::Register { .. }
                    | Command::RequestReset(_)
                    | Command::ResetPassword { .. } => {
                        set_auth_error(ui, "sem conexão com o servidor".into());
                    }
                    Command::SetEmail(_) => {
                        profile_message(ui, ProfileOp::Email, "sem conexão: tente de novo daqui a pouco".into(), true);
                    }
                    // O servidor só fica sabendo quando a sessão expirar, mas
                    // o token some deste computador na hora.
                    Command::Logout => {
                        forget_session(state, saved, ui);
                        show_offline(state, ui, until - Instant::now());
                    }
                    // Abre já com o que temos; o histórico vem ao reconectar.
                    Command::Open(id) => {
                        open(state, &id, ui);
                    }
                    // Pede de novo quando voltar, ao chegar no topo outra vez.
                    Command::LoadOlder => {}
                    Command::Send(_) | Command::SendCall(_) => {
                        set_status(ui, "sem conexão: a mensagem não foi enviada".into());
                    }
                    Command::EditMessage { .. } | Command::DeleteMessage(_) => {
                        set_status(ui, "sem conexão: a mensagem não mudou".into());
                    }
                    Command::OpenCallChat => {
                        open_call_chat(state, ui);
                    }
                    Command::OpenDirect(_) => {
                        set_direct_error(ui, "sem conexão com o servidor".into());
                    }
                    // Abrir e fechar a coluna funciona sem rede.
                    Command::ShowMembers(open) => {
                        state.members_open = open;
                        render(state, ui);
                    }
                    Command::CreateGroup(_)
                    | Command::Invite(_)
                    | Command::Remove(_)
                    | Command::Leave => {
                        set_group_error(ui, "sem conexão com o servidor".into());
                    }
                    Command::SendAudio(_) | Command::AudioUploaded { .. } => {
                        set_status(ui, "sem conexão: o áudio não foi enviado".into());
                    }
                    // Um áudio que já está aqui toca mesmo sem rede.
                    Command::PlayAudio(id) => {
                        if !play_cached(state, media, &id) {
                            set_status(ui, "sem conexão: o áudio não foi baixado".into());
                        }
                    }
                    Command::AudioDownloaded { attachment_id, ogg } => {
                        audio_downloaded(state, media, attachment_id, ogg);
                    }
                    Command::AudioFailed(reason) => set_status(ui, reason),
                    Command::SendFile(_, _) | Command::FileUploaded { .. } | Command::SaveFile { .. } | Command::ViewImage(_, _) => {
                        set_status(ui, "sem conexão: arquivo não enviado ou baixado".into());
                    }
                    Command::ImageReady { attachment_id, width, height, pixels } => {
                        state.pending_images.remove(&attachment_id);
                        state.image_cache.insert(attachment_id, Thumbnail { width, height, pixels });
                        render(state, ui);
                    }
                    Command::ImageFailed(id) => { state.pending_images.remove(&id); }
                    Command::JoinCall => {
                        set_status(ui, "sem conexão: não dá para entrar na chamada".into());
                    }
                    command @ (Command::ShowProfile { .. }
                    | Command::HideProfile(_)
                    | Command::SaveProfile(_)
                    | Command::SetProfileImage { .. }
                    | Command::RemoveProfileImage(_)
                    | Command::ProfileImageDone(_)) => {
                        profile_command(state, command, media, ui, false);
                    }
                    // Sobe quando a conexão voltar (no próximo `Welcome`).
                    Command::SetPhoto(png) => state.photos.set_mine(png, false, &mut state.outgoing),
                    Command::Rename(_) => {
                        profile_message(ui, ProfileOp::Name, "sem conexão: tente de novo daqui a pouco".into(), true);
                    }
                    Command::ChangePassword { .. } => {
                        profile_message(ui, ProfileOp::Password, "sem conexão: tente de novo daqui a pouco".into(), true);
                    }
                    // Uma chamada em andamento continua sem o servidor do
                    // mothh; o aviso de saída vai ao reconectar (ou o
                    // servidor já tirou a gente ao perder a conexão).
                    command @ (Command::LeaveCall
                    | Command::ToggleMute
                    | Command::ToggleCamera
                    | Command::CameraFailed(_)
                    | Command::OpenSharePicker
                    | Command::CloseSharePicker
                    | Command::ShareSources(_)
                    | Command::SharePreview(_)
                    | Command::StartShare { .. }
                    | Command::PointAt { .. }
                    | Command::DevicesChanged
                    | Command::CallWarning(_)
                    | Command::StopShare
                    | Command::ShareFailed(_)
                    | Command::CallJoined(_)
                    | Command::CallPeople { .. }
                    | Command::CallEnded { .. }) => {
                        call_command(state, command, media, ui);
                    }
                }
            }
        }
    }
}

/// Pede o token da chamada da conversa aberta, se for um grupo.
fn join_call(state: &mut LocalState, ui: &slint::Weak<AppWindow>) -> Option<ClientMessage> {
    let conversation_id = state.open.filter(|&id| can_call(state, id))?;
    if state
        .call
        .as_ref()
        .is_some_and(|c| c.conversation_id == conversation_id)
    {
        return None;
    }
    // Já pedido: cliques repetidos no botão não pedem de novo.
    if state.call_pending == Some(conversation_id) {
        return None;
    }
    if state.call_waiting_key == Some(conversation_id) {
        set_status(ui, WAITING_KEY.into());
        return None;
    }
    state.call_waiting_key = None;
    state.call_pending = Some(conversation_id);
    render(state, ui);
    Some(ClientMessage::JoinCall { conversation_id })
}

/// Os comandos de uma chamada já começada. Devolve o aviso ao servidor.
fn call_command(
    state: &mut LocalState,
    command: Command,
    media: &Media,
    ui: &slint::Weak<AppWindow>,
) -> Option<ClientMessage> {
    let msg = match command {
        Command::LeaveCall => {
            let call = state.call.take()?;
            call.leave();
            Some(ClientMessage::LeaveCall {
                conversation_id: call.conversation_id,
            })
        }
        Command::ToggleMute => {
            let call = state.call.as_ref()?;
            state.call_muted = !state.call_muted;
            call.set_muted(state.call_muted);
            None
        }
        Command::ToggleCamera => {
            let call = state.call.as_ref()?;
            state.call_camera = !state.call_camera;
            call.set_camera(state.call_camera);
            None
        }
        Command::CameraFailed(reason) => {
            // A faixa de vídeo ainda está publicada: desliga para valer, para
            // o botão "câmera" tentar de novo do zero.
            let call = state.call.as_ref()?;
            if std::mem::take(&mut state.call_camera) {
                call.set_camera(false);
                set_status(ui, reason);
            }
            None
        }
        Command::OpenSharePicker => {
            state.call.as_ref()?;
            state.share_picker_open = true;
            state.share_sources.clear();
            state.share_previews.clear();
            // O "com som" começa como da última vez.
            let prefs = media.devices.get();
            let (audio, pointer) = (prefs.share_audio, prefs.share_pointer);
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_share_audio(audio);
                ui.set_share_pointer(pointer);
            });
            // Listar as janelas demora um pouco; a lista chega por
            // `ShareSources` e as fotos, uma por uma, por `SharePreview`.
            let commands = media.commands.clone();
            let previews = media.commands.clone();
            crate::screen::list_sources(
                move |sources| {
                    let _ = commands.send(Command::ShareSources(sources));
                },
                move |preview| {
                    let _ = previews.send(Command::SharePreview(preview));
                },
            );
            None
        }
        Command::CloseSharePicker => {
            state.share_picker_open = false;
            state.share_sources.clear();
            state.share_previews.clear();
            None
        }
        Command::ShareSources(sources) => {
            if state.share_picker_open {
                state.share_sources = sources;
            }
            None
        }
        Command::SharePreview(preview) => {
            if state.share_picker_open {
                state.share_previews.insert(preview.id.clone(), preview);
                // Chegam várias seguidas: só a lista, sem remontar a conversa.
                render_share(state, ui);
            }
            return None;
        }
        Command::PointAt { person, x, y } => {
            if let Some(call) = &state.call {
                call.point_at(person, x, y);
            }
            return None;
        }
        Command::StartShare {
            source_id,
            audio,
            pointer,
        } => {
            let call = state.call.as_ref()?;
            let name = state
                .share_sources
                .iter()
                .find(|s| s.id == source_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            media.devices.set_share_audio(audio);
            media.devices.set_share_pointer(pointer);
            state.share_picker_open = false;
            state.share_sources.clear();
            state.share_previews.clear();
            state.call_sharing = true;
            call.set_share(Some(ShareRequest {
                source_id,
                name,
                audio,
                pointer,
            }));
            None
        }
        Command::DevicesChanged => {
            if let Some(call) = &state.call {
                call.devices_changed();
            }
            return None;
        }
        Command::CallWarning(reason) => {
            set_status(ui, reason);
            return None;
        }
        Command::StopShare => {
            let call = state.call.as_ref()?;
            state.call_sharing = false;
            call.set_share(None);
            None
        }
        Command::ShareFailed(reason) => {
            let call = state.call.as_ref()?;
            if std::mem::take(&mut state.call_sharing) {
                call.set_share(None);
                set_status(ui, reason);
            }
            None
        }
        Command::CallJoined(id) => {
            if state.call.as_ref().is_some_and(|c| c.conversation_id == id) {
                state.call_connected = true;
            }
            None
        }
        Command::CallPeople {
            conversation_id,
            people,
        } => {
            if state
                .call
                .as_ref()
                .is_some_and(|c| c.conversation_id == conversation_id)
            {
                state.call_people = people;
                // A foto de quem apareceu agora (a sua já está na tela).
                let others = state
                    .call_people
                    .iter()
                    .filter_map(|p| p.id.parse().ok().map(UserId))
                    .filter(|id| Some(*id) != state.me);
                state.photos.want(others, &mut state.outgoing);
                // Chega várias vezes por segundo: só mexe no que mudou, sem
                // remontar a lista de mensagens.
                render_speaking(state, ui);
            }
            return None;
        }
        Command::CallEnded {
            conversation_id,
            reason,
        } => {
            // Já saímos por conta própria (ou trocamos de chamada): nada a fazer.
            if !state
                .call
                .as_ref()
                .is_some_and(|c| c.conversation_id == conversation_id)
            {
                return None;
            }
            state.call = None;
            if let Some(reason) = reason {
                set_status(ui, reason);
            }
            Some(ClientMessage::LeaveCall { conversation_id })
        }
        _ => None,
    };
    if state.call.is_none() {
        state.call_connected = false;
        state.call_muted = false;
        state.call_people.clear();
        state.call_camera = false;
        state.call_sharing = false;
        state.share_picker_open = false;
        state.share_sources.clear();
        state.share_previews.clear();
    }
    render(state, ui);
    msg
}

/// Chegou o token: entra na sala do LiveKit, saindo da chamada anterior.
fn start_call(
    state: &mut LocalState,
    media: &Media,
    ui: &slint::Weak<AppWindow>,
    conversation_id: ConversationId,
    url: String,
    token: String,
) {
    if state.call_pending != Some(conversation_id) {
        // Resposta a um pedido de reconexão, ou a um pedido já desistido.
        return;
    }
    state.call_pending = None;
    // A chave da mídia vem do grupo MLS da conversa (F2-09). Sem ela não
    // entra: a chamada iria sem cifra ou com uma chave que ninguém tem.
    let Some(key) = media_key(state, conversation_id) else {
        // O servidor já nos contou como "na chamada" ao dar o token: avisa
        // que não entramos, senão os outros nos veem lá e não ouvem nada.
        state
            .outgoing
            .push(ClientMessage::LeaveCall { conversation_id });
        crate::diag::report(
            "chamada: não entrou, este aparelho não tem a chave do grupo desta conversa \
             (está fora do grupo cifrado); pedindo para entrar no grupo de novo"
                .to_string(),
        );
        if let Some(e2ee) = &mut state.e2ee {
            e2ee.catch_up(conversation_id, &mut state.outgoing);
        }
        state.call_waiting_key = Some(conversation_id);
        set_status(ui, WAITING_KEY.into());
        render(state, ui);
        return;
    };
    crate::diag::report(format!(
        "chamada: entrando com a chave da época {}",
        key.epoch
    ));
    if let Some(previous) = state.call.take() {
        previous.leave();
    }
    state.call_connected = false;
    state.call_muted = false;
    state.call_people.clear();
    state.call_camera = false;
    state.call_sharing = false;
    state.share_picker_open = false;
    state.share_sources.clear();
    state.share_previews.clear();
    state.call = Some(call::start(
        conversation_id,
        url,
        token,
        key,
        media.commands.clone(),
        ui.clone(),
        media.devices.clone(),
    ));
}

/// A chave de mídia da época atual do grupo MLS da conversa.
fn media_key(state: &mut LocalState, conversation_id: ConversationId) -> Option<sframe::MediaKey> {
    let (epoch, secret) = state.e2ee.as_mut()?.media_key(conversation_id)?;
    Some(sframe::MediaKey { epoch, secret })
}

/// Toda conversa da lista tem chamada: grupos (inclusive a Sala do Grupo)
/// e, desde a V6-03, as conversas diretas (chamada 1:1).
fn can_call(state: &LocalState, id: ConversationId) -> bool {
    state.conversations.iter().any(|c| c.id == id)
}

/// A conversa é direta (uma pessoa só do outro lado).
fn is_direct(state: &LocalState, id: ConversationId) -> bool {
    state
        .conversations
        .iter()
        .any(|c| c.id == id && c.kind == ConversationKind::Direct)
}

/// Estamos na chamada desta conversa, ou entrando nela.
fn in_this_call(state: &LocalState, id: ConversationId) -> bool {
    state.call.as_ref().is_some_and(|c| c.conversation_id == id) || state.call_pending == Some(id)
}

/// Muda a conversa aberta. Devolve o id, se ele for válido.
fn open(state: &mut LocalState, id: &str, ui: &slint::Weak<AppWindow>) -> Option<ConversationId> {
    let conversation_id = parse_id(id)?;
    switch_to(state, Some(conversation_id));
    // Os membros do grupo são as sugestões do "@".
    if is_managed_group(state, conversation_id) {
        state
            .outgoing
            .push(ClientMessage::ListMembers { conversation_id });
    }
    render(state, ui);
    Some(conversation_id)
}

/// Troca a conversa aberta e esquece o que era da anterior.
fn switch_to(state: &mut LocalState, conversation: Option<ConversationId>) {
    if state.open != conversation {
        state.members_open = false;
        state.members.clear();
        state.owner = None;
    }
    state.open = conversation;
    if let Some(id) = conversation {
        state.mentioned.remove(&id);
        state.unread_pending = state.unread.remove(&id).unwrap_or(0);
    } else {
        state.unread_pending = 0;
    }
    state.unread_mark = None;
    state.messages.clear();
    state.has_older = false;
    state.loading_older = false;
}

/// Um grupo criado por alguém: dá para convidar, remover e sair.
fn is_managed_group(state: &LocalState, id: ConversationId) -> bool {
    id != GROUP_ROOM
        && state
            .conversations
            .iter()
            .any(|c| c.id == id && c.kind == ConversationKind::Group)
}

/// Os comandos de grupo, com a conexão aberta. Devolve o pedido ao servidor.
fn group_command(
    state: &mut LocalState,
    command: Command,
    ui: &slint::Weak<AppWindow>,
) -> Option<ClientMessage> {
    let group = state.open.filter(|&id| is_managed_group(state, id));
    let msg =
        match command {
            Command::CreateGroup(name) => Some(ClientMessage::CreateGroup { name }),
            Command::ShowMembers(open) => {
                state.members_open = open && group.is_some();
                render(state, ui);
                // Ao abrir, pede a lista; depois ela chega sozinha a cada mudança.
                return group
                    .filter(|_| open)
                    .map(|conversation_id| ClientMessage::ListMembers { conversation_id });
            }
            Command::Invite(username) => {
                group
                    .filter(|_| !username.trim().is_empty())
                    .map(|conversation_id| ClientMessage::InviteToGroup {
                        conversation_id,
                        username,
                    })
            }
            Command::Remove(id) => group.zip(id.parse().ok()).map(|(conversation_id, id)| {
                ClientMessage::RemoveFromGroup {
                    conversation_id,
                    user_id: UserId(id),
                }
            }),
            Command::Leave => {
                group.map(|conversation_id| ClientMessage::LeaveGroup { conversation_id })
            }
            // Os outros comandos são tratados em `connection`.
            _ => None,
        };
    state.group_pending = msg.is_some();
    msg
}

/// Sobe o áudio gravado para a conversa aberta. Quando o servidor responder
/// com o id, chega um `AudioUploaded` e a mensagem vai pelo WebSocket.
fn upload_audio(state: &LocalState, media: &Media, ogg: Vec<u8>, ui: &slint::Weak<AppWindow>) {
    let (Some(conversation_id), Some(token)) = (state.open, state.token.clone()) else {
        return;
    };
    set_status(ui, "enviando áudio...".into());
    let media = media.clone();
    tokio::spawn(async move {
        let command = match upload(&media, &token, ogg.clone()).await {
            Ok(attachment_id) => Command::AudioUploaded {
                conversation_id,
                attachment_id,
                ogg,
            },
            Err(reason) => Command::AudioFailed(reason),
        };
        let _ = media.commands.send(command);
    });
}

/// Toca o áudio; se ainda não está aqui, baixa antes.
fn play_audio(state: &LocalState, media: &Media, id: &str, ui: &slint::Weak<AppWindow>) {
    if play_cached(state, media, id) {
        return;
    }
    let (Ok(uuid), Some(token)) = (id.parse(), state.token.clone()) else {
        return;
    };
    let attachment_id = AttachmentId(uuid);
    set_status(ui, "baixando áudio...".into());
    let media = media.clone();
    tokio::spawn(async move {
        let command = match download(&media, &token, attachment_id).await {
            Ok(ogg) => Command::AudioDownloaded { attachment_id, ogg },
            Err(reason) => Command::AudioFailed(reason),
        };
        let _ = media.commands.send(command);
    });
}

/// Toca o áudio se ele já foi baixado. `false` se precisa baixar.
fn play_cached(state: &LocalState, media: &Media, id: &str) -> bool {
    let Some(ogg) = id
        .parse()
        .ok()
        .and_then(|uuid| state.audio_cache.get(&AttachmentId(uuid)))
    else {
        return false;
    };
    media.audio.play(id.to_string(), ogg.clone());
    true
}

fn audio_downloaded(
    state: &mut LocalState,
    media: &Media,
    attachment_id: AttachmentId,
    ogg: Vec<u8>,
) {
    media.audio.play(attachment_id.0.to_string(), ogg.clone());
    state.audio_cache.insert(attachment_id, ogg);
}

/// `POST /anexos` com o arquivo. Devolve o id que o servidor deu.
async fn upload(media: &Media, token: &str, ogg: Vec<u8>) -> Result<AttachmentId, String> {
    let response = media
        .http
        .post(format!("{}{ATTACHMENTS_PATH}", media.base))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, AUDIO_CONTENT_TYPE)
        .body(ogg)
        .send()
        .await
        .map_err(|_| "não deu para enviar o áudio".to_string())?;
    let body = read_body(response, "o servidor recusou o áudio").await?;
    serde_json::from_slice::<Uploaded>(&body)
        .map(|uploaded| uploaded.id)
        .map_err(|_| "o servidor mandou algo que não entendi".into())
}

/// Escolhe um arquivo no app, confere o tamanho antes da leitura e o envia.
fn upload_file(
    state: &LocalState,
    media: &Media,
    pending: crate::PendingFile,
    caption: String,
    ui: &slint::Weak<AppWindow>,
) {
    let (Some(conversation_id), Some(token)) = (state.open, state.token.clone()) else {
        return;
    };
    set_status(ui, "enviando arquivo...".into());
    let media = media.clone();
    tokio::spawn(async move {
        let path = &pending.path;
        let result = async {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("Nome de arquivo inválido.")?;
            let metadata = tokio::fs::metadata(&path)
                .await
                .map_err(|_| "Não deu para abrir o arquivo.")?;
            if metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES as u64 {
                return Err("O arquivo deve ter até 10 MB.");
            }
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|_| "Não deu para ler o arquivo.")?;
            upload_file_body(&media, &token, name, file_content_type(path), bytes).await
        }
        .await;
        let command = match result {
            Ok(attachment_id) => Command::FileUploaded {
                conversation_id,
                attachment_id,
                caption,
            },
            Err(reason) => Command::AudioFailed(reason.into()),
        };
        let _ = media.commands.send(command);
    });
}

async fn upload_file_body(
    media: &Media,
    token: &str,
    name: &str,
    content_type: &'static str,
    bytes: Vec<u8>,
) -> Result<AttachmentId, &'static str> {
    let response = media
        .http
        .post(format!("{}{ATTACHMENTS_PATH}", media.base))
        .query(&[("name", name)])
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|_| "Não deu para enviar o arquivo.")?;
    let body = read_body(response, "O servidor recusou o arquivo")
        .await
        .map_err(|_| "O servidor recusou o arquivo.")?;
    serde_json::from_slice::<Uploaded>(&body)
        .map(|uploaded| uploaded.id)
        .map_err(|_| "O servidor mandou uma resposta inválida.")
}

fn file_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn save_file(
    state: &LocalState,
    media: &Media,
    attachment_id: AttachmentId,
    path: PathBuf,
    ui: &slint::Weak<AppWindow>,
) {
    let Some(token) = state.token.clone() else {
        return;
    };
    set_status(ui, "baixando arquivo...".into());
    let media = media.clone();
    tokio::spawn(async move {
        let result = async {
            let bytes = download(&media, &token, attachment_id).await?;
            tokio::fs::write(&path, bytes)
                .await
                .map_err(|_| "Não deu para salvar o arquivo.".to_string())?;
            Ok::<(), String>(())
        }
        .await;
        let status = match result {
            Ok(()) => "arquivo salvo".to_string(),
            Err(reason) => reason,
        };
        let _ = media.commands.send(Command::AudioFailed(status));
    });
}

fn prefetch_images(state: &mut LocalState, media: &Media) {
    let Some(token) = state.token.clone() else {
        return;
    };
    for file in state
        .messages
        .iter()
        .filter_map(|message| message.file.as_ref())
    {
        if !file.content_type.starts_with("image/")
            || state.image_cache.contains_key(&file.attachment_id)
            || !state.pending_images.insert(file.attachment_id)
        {
            continue;
        }
        let media = media.clone();
        let token = token.clone();
        let id = file.attachment_id;
        tokio::spawn(async move {
            let result = async {
                let bytes = download(&media, &token, id).await?;
                tokio::task::spawn_blocking(move || decode_thumbnail(&bytes))
                    .await
                    .map_err(|_| "Imagem inválida".to_string())?
            }
            .await;
            let command = match result {
                Ok(thumb) => Command::ImageReady {
                    attachment_id: id,
                    width: thumb.width,
                    height: thumb.height,
                    pixels: thumb.pixels,
                },
                Err(_) => Command::ImageFailed(id),
            };
            let _ = media.commands.send(command);
        });
    }
}

fn decode_thumbnail(bytes: &[u8]) -> Result<Thumbnail, String> {
    let image = image::load_from_memory(bytes).map_err(|_| "Imagem inválida")?;
    let image = image.thumbnail(320, 260).to_rgba8();
    Ok(Thumbnail {
        width: image.width(),
        height: image.height(),
        pixels: image.into_raw(),
    })
}

/// `GET /anexos/{id}`.
async fn download(media: &Media, token: &str, id: AttachmentId) -> Result<Vec<u8>, String> {
    let response = media
        .http
        .get(format!("{}{ATTACHMENTS_PATH}/{}", media.base, id.0))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "não deu para baixar o áudio".to_string())?;
    read_body(response, "o áudio não veio").await
}

/// O corpo da resposta, ou a frase de erro que o servidor mandou.
async fn read_body(response: reqwest::Response, fallback: &str) -> Result<Vec<u8>, String> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|_| format!("{fallback} (a conexão caiu)"))?;
    if status.is_success() {
        Ok(body.to_vec())
    } else {
        let reason = String::from_utf8_lossy(&body).trim().to_string();
        Err(if reason.is_empty() {
            format!("{fallback} ({status})")
        } else {
            reason
        })
    }
}

/// "ws://host:3000/ws" vira "http://host:3000"; "wss://" vira "https://".
fn http_base(ws_url: &str) -> String {
    let url = ws_url.trim_end_matches('/');
    let url = url.strip_suffix("/ws").unwrap_or(url);
    if let Some(rest) = url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        url.to_string()
    }
}

/// Sai da conta neste computador e volta para a tela de login.
fn forget_session(state: &mut LocalState, saved: &SavedSession, ui: &slint::Weak<AppWindow>) {
    saved.delete();
    *state = LocalState::default();
    render(state, ui);
    set_direct_error(ui, String::new());
    show_login(ui);
}

/// Aplica uma mensagem do servidor ao estado local e redesenha. Pode devolver
/// algo para mandar de volta ao servidor.
fn apply(
    state: &mut LocalState,
    msg: ServerMessage,
    saved: &SavedSession,
    ui: &slint::Weak<AppWindow>,
    media: &Media,
) -> Option<ClientMessage> {
    let mut reply = None;
    match msg {
        ServerMessage::Welcome {
            user_id,
            username,
            token,
            conversations,
            photo_version,
            profiles: profiles_supported,
            listening_activity,
            diagnostics,
            email,
        } => {
            state.listening_supported = listening_activity;
            state.diagnostics_supported = diagnostics;
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_account_email(email.unwrap_or_default().into());
            });
            state.me = Some(user_id);
            state.my_name = username.clone();
            state.profile_editor_filled = false;
            state.profile_saving = false;
            state
                .profiles
                .welcome(user_id, profiles_supported, &mut state.outgoing);
            let my_id = user_id.0.to_string();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_my_id(my_id.into());
                ui.set_profiles_supported(profiles_supported);
            });
            let my_photo = crate::session::profile_file(saved.profile(), "photo.png");
            state
                .photos
                .welcome(photo_version, my_photo.as_deref(), &mut state.outgoing);
            // As chaves são da conta: outra conta no mesmo perfil abre as dela.
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.reconnected();
            }
            if state.e2ee.as_ref().is_none_or(|e| e.user() != user_id) {
                state.e2ee = match E2ee::open(saved.profile(), user_id) {
                    Ok(e2ee) => Some(e2ee),
                    Err(err) => {
                        set_status(ui, format!("não deu para abrir as chaves: {err}"));
                        None
                    }
                };
            }
            let mut conversations = conversations;
            for conversation in &mut conversations {
                reveal_summary(state, conversation);
            }
            state.group_pending = false;
            state.resuming = false;
            state.logged_in = true;
            saved.save(&token);
            state.token = Some(token);
            // A foto de quem está do outro lado de cada conversa direta.
            let partners: Vec<UserId> = conversations.iter().filter_map(|c| c.other_id).collect();
            state.photos.want(partners, &mut state.outgoing);
            state.conversations = conversations;
            state.profile_names.insert(user_id, username.clone());
            // As contas das chamadas chegam logo depois, em `CallPresence`.
            state.in_call.clear();
            state.call_members.clear();
            state.rejoin_call = state.call.is_some();
            // Voltando de uma queda, pede de novo o histórico da conversa que
            // estava aberta: pode ter chegado mensagem enquanto estávamos fora.
            // As mensagens antigas ficam na tela até o `History` chegar.
            state.loading_older = false;
            reply = state
                .open
                .map(|conversation_id| ClientMessage::OpenConversation { conversation_id });
            let initial = initial_of(&username);
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_user_name(username.into());
                ui.set_user_initial(initial.into());
                ui.set_auth_error(SharedString::new());
                ui.set_status("online".into());
                ui.set_connecting(false);
                ui.set_logged_in(true);
            });
        }
        ServerMessage::ResetRequested => {
            let _ = ui.upgrade_in_event_loop(|ui| {
                ui.set_auth_error(SharedString::new());
                ui.set_auth_mode("reset".into());
                ui.set_auth_notice(
                    "Se esse e-mail tiver conta, o código chegou nele. Vale 15 minutos.".into(),
                );
            });
        }
        ServerMessage::PasswordReset => {
            let _ = ui.upgrade_in_event_loop(|ui| {
                ui.set_auth_error(SharedString::new());
                ui.set_auth_mode("login".into());
                ui.set_auth_notice("Senha trocada. Entre com a nova.".into());
            });
        }
        ServerMessage::EmailChanged { email } => {
            state.profile_pending = None;
            let _ = ui.upgrade_in_event_loop(move |ui| ui.set_account_email(email.into()));
            profile_message(ui, ProfileOp::Email, "e-mail salvo.".into(), false);
        }
        ServerMessage::AuthError { reason } => {
            if state.resuming {
                // O token salvo não vale mais: esquece ele e pede a senha.
                saved.delete();
                *state = LocalState::default();
            }
            set_auth_error(ui, reason);
            show_login(ui);
        }
        ServerMessage::History {
            conversation_id,
            mut messages,
            has_more,
        } => {
            reveal_all(state, &mut messages);
            if state.open == Some(conversation_id) {
                let pending = std::mem::take(&mut state.unread_pending) as usize;
                if pending > 0 && !messages.is_empty() {
                    let from = messages.len().saturating_sub(pending);
                    state.unread_mark = Some(messages[from].id);
                }
                state.messages = messages;
                state.has_older = has_more;
                state.loading_older = false;
            }
        }
        ServerMessage::Older {
            conversation_id,
            mut messages,
            has_more,
        } => {
            reveal_all(state, &mut messages);
            if state.open == Some(conversation_id) {
                state.messages.splice(0..0, messages);
                state.has_older = has_more;
                state.loading_older = false;
                state.prepended = true;
            }
        }
        ServerMessage::NewMessage { mut message } => {
            reveal_all(state, std::slice::from_mut(&mut message));
            // Só mensagem de outra pessoa faz barulho; a minha volta do
            // servidor como confirmação e fica em silêncio.
            if state.me.is_some_and(|me| me != message.author_id) {
                sound::play_received_if_on(ui);
                if mentions::mentions(&message.text, &state.my_name) {
                    if state.open != Some(message.conversation_id) {
                        state.mentioned.insert(message.conversation_id);
                    }
                    call_attention(ui);
                }
                if state.open != Some(message.conversation_id) {
                    *state.unread.entry(message.conversation_id).or_default() += 1;
                }
            }
            if state.open == Some(message.conversation_id) {
                state.messages.push(message.clone());
            }
            if let Some(conv) = state
                .conversations
                .iter_mut()
                .find(|c| c.id == message.conversation_id)
            {
                conv.last_message = Some(message);
            }
            sort_conversations(state);
        }
        ServerMessage::MessageEdited { mut message } => {
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.forget_text(message.id);
            }
            reveal_all(state, std::slice::from_mut(&mut message));
            if let Some(shown) = state.messages.iter_mut().find(|m| m.id == message.id) {
                *shown = message.clone();
            }
            if let Some(conv) = state
                .conversations
                .iter_mut()
                .find(|c| c.id == message.conversation_id)
                && conv
                    .last_message
                    .as_ref()
                    .is_some_and(|m| m.id == message.id)
            {
                conv.last_message = Some(message);
            }
        }
        ServerMessage::MessageDeleted {
            conversation_id,
            message_id,
        } => {
            if state.open == Some(conversation_id) {
                state.messages.retain(|m| m.id != message_id);
            }
            // A linha da lista chega em seguida, num `ConversationUpdated`.
        }
        ServerMessage::ConversationUpdated { mut conversation } => {
            reveal_summary(state, &mut conversation);
            add_conversation(state, conversation);
        }
        ServerMessage::DirectOpened { mut conversation } => {
            reveal_summary(state, &mut conversation);
            state.opening_direct = false;
            let id = conversation.id;
            add_conversation(state, conversation);
            set_direct_error(ui, String::new());
            let _ = ui.upgrade_in_event_loop(|ui| ui.set_direct_search(SharedString::new()));
            if state.open != Some(id) {
                switch_to(state, Some(id));
                reply = Some(ClientMessage::OpenConversation {
                    conversation_id: id,
                });
            }
        }
        ServerMessage::ConversationAdded { mut conversation } => {
            reveal_summary(state, &mut conversation);
            add_conversation(state, conversation);
        }
        ServerMessage::GroupCreated { conversation } => {
            state.group_pending = false;
            let id = conversation.id;
            add_conversation(state, conversation);
            // O grupo novo já abre, com a coluna de membros para convidar.
            switch_to(state, Some(id));
            state.members_open = true;
            reply = Some(ClientMessage::ListMembers {
                conversation_id: id,
            });
            let _ = ui.upgrade_in_event_loop(|ui| {
                ui.set_creating_group(false);
                ui.set_group_error(SharedString::new());
            });
        }
        ServerMessage::ConversationRemoved { conversation_id } => {
            let name = state
                .conversations
                .iter()
                .find(|c| c.id == conversation_id)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            state.conversations.retain(|c| c.id != conversation_id);
            state.in_call.remove(&conversation_id);
            state.call_members.remove(&conversation_id);
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.forget(conversation_id);
            }
            // Saiu do grupo: sai da chamada dele também.
            if state
                .call
                .as_ref()
                .is_some_and(|c| c.conversation_id == conversation_id)
            {
                reply = call_command(state, Command::LeaveCall, media, ui);
            }
            if state.open == Some(conversation_id) {
                switch_to(state, None);
            }
            if std::mem::take(&mut state.group_pending) {
                set_status(ui, format!("você saiu de {name}"));
            } else {
                set_status(ui, format!("você foi removido de {name}"));
            }
        }
        ServerMessage::Members {
            conversation_id,
            owner_id,
            members,
        } => {
            if state.open == Some(conversation_id) {
                state.owner = owner_id;
                state.members = members;
                state.group_pending = false;
                set_group_error(ui, String::new());
            }
        }
        ServerMessage::Photo {
            user_id,
            version,
            png,
        } => {
            let pixels =
                state
                    .photos
                    .received(user_id, version, png.as_ref().map(|p| p.0.as_slice()));
            let id = user_id.0.to_string();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                photos::store(&id, pixels);
                refresh_call_photos(&ui);
            });
            // A foto também aparece na lista de conversas e no cabeçalho.
            render(state, ui);
        }
        ServerMessage::PasswordChanged => {
            state.profile_pending = None;
            profile_message(
                ui,
                ProfileOp::Password,
                "senha trocada. Os outros aparelhos saíram da conta.".into(),
                false,
            );
        }
        ServerMessage::UserRenamed { user_id, username } => {
            renamed(state, user_id, &username);
            state.profiles.renamed(user_id, &username);
            state.profile_names.insert(user_id, username.clone());
            push_profile_views(state, ui);
            if Some(user_id) == state.me {
                if state.profile_pending == Some(ProfileOp::Name) {
                    state.profile_pending = None;
                }
                profile_message(ui, ProfileOp::Name, "nome trocado.".into(), false);
                let initial = initial_of(&username);
                let _ = ui.upgrade_in_event_loop(move |ui| {
                    ui.set_user_name(username.into());
                    ui.set_user_initial(initial.into());
                });
            }
        }
        ServerMessage::PhotoChanged { user_id, version } => {
            state.photos.changed(user_id, version, &mut state.outgoing);
        }
        ServerMessage::Profile { profile } => profile_received(state, media, ui, profile),
        ServerMessage::ProfileChanged { user_id, version } => {
            if version >= 0
                && Some(user_id) == state.me
                && std::mem::take(&mut state.profile_saving)
            {
                edit_message(ui, "página salva.".into(), false);
            }
            state
                .profiles
                .changed(user_id, version, &mut state.outgoing);
        }
        ServerMessage::CallToken {
            conversation_id,
            url,
            token,
        } => start_call(state, media, ui, conversation_id, url, token),
        ServerMessage::CallPresence {
            conversation_id,
            count,
            people,
        } => {
            // Chamada 1:1 começando do outro lado: toca, como um telefone.
            let ringing = count > 0
                && !state.in_call.contains_key(&conversation_id)
                && is_direct(state, conversation_id)
                && !in_this_call(state, conversation_id);
            if ringing {
                sound::play_call_if_on(ui, sound::Cue::Ring);
            }
            if count == 0 {
                state.in_call.remove(&conversation_id);
                state.call_members.remove(&conversation_id);
            } else {
                state.in_call.insert(conversation_id, count);
                let ids: Vec<UserId> = people
                    .iter()
                    .map(|m| m.user_id)
                    .filter(|id| Some(*id) != state.me)
                    .collect();
                state.photos.want(ids, &mut state.outgoing);
                state.call_members.insert(conversation_id, people);
            }
            push_profile_views(state, ui);
        }
        ServerMessage::KeyPackageStock { count } => {
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.key_package_stock(count, &mut state.outgoing);
            }
        }
        ServerMessage::GroupState {
            conversation_id,
            epoch,
            members,
            pending_add,
            pending_remove,
            committer,
        } => {
            let info = ServerGroup {
                epoch,
                members,
                pending_add,
                pending_remove,
                committer,
            };
            let changed = state
                .e2ee
                .as_mut()
                .is_some_and(|e| e.group_state(conversation_id, info, &mut state.outgoing));
            after_mls(state, conversation_id, changed, ui);
        }
        ServerMessage::KeyPackages {
            conversation_id,
            packages,
        } => {
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.key_packages(conversation_id, packages, &mut state.outgoing);
            }
        }
        ServerMessage::Commit {
            conversation_id,
            commit,
        } => {
            let changed = state
                .e2ee
                .as_mut()
                .is_some_and(|e| e.commits(conversation_id, &[commit], false, &mut state.outgoing));
            after_mls(state, conversation_id, changed, ui);
        }
        ServerMessage::Commits {
            conversation_id,
            commits,
        } => {
            let changed = state
                .e2ee
                .as_mut()
                .is_some_and(|e| e.commits(conversation_id, &commits, true, &mut state.outgoing));
            after_mls(state, conversation_id, changed, ui);
        }
        ServerMessage::GroupWelcome {
            conversation_id,
            epoch,
            welcome,
        } => {
            let changed = state
                .e2ee
                .as_mut()
                .is_some_and(|e| e.welcome(conversation_id, epoch, &welcome, &mut state.outgoing));
            after_mls(state, conversation_id, changed, ui);
        }
        ServerMessage::CommitAccepted {
            conversation_id, ..
        } => {
            let changed = state
                .e2ee
                .as_mut()
                .is_some_and(|e| e.accepted(conversation_id, &mut state.outgoing));
            after_mls(state, conversation_id, changed, ui);
        }
        ServerMessage::CommitRejected {
            conversation_id,
            epoch,
        } => {
            if let Some(e2ee) = &mut state.e2ee {
                e2ee.rejected(conversation_id, epoch, &mut state.outgoing);
            }
        }
        ServerMessage::Error { reason } => {
            // Se o erro foi a resposta a um `LoadOlder`, deixa pedir de novo.
            state.loading_older = false;
            // Ou a um `JoinCall`: o botão volta a "entrar na chamada".
            state.call_pending = None;
            if let Some(op) = state.profile_pending.take() {
                profile_message(ui, op, reason, true);
            } else if std::mem::take(&mut state.profile_saving) {
                edit_message(ui, reason, true);
            } else if std::mem::take(&mut state.opening_direct) {
                set_direct_error(ui, reason);
            } else if std::mem::take(&mut state.group_pending) {
                set_group_error(ui, reason);
            } else {
                set_status(ui, reason);
            }
        }
    }
    render(state, ui);
    reply
}

/// Comandos do perfil. `online`: há conexão com o servidor agora.
fn profile_command(
    state: &mut LocalState,
    command: Command,
    media: &Media,
    ui: &slint::Weak<AppWindow>,
    online: bool,
) {
    match command {
        Command::ShowProfile {
            user_id,
            name,
            place,
        } => {
            let Some(user) = parse_user(&user_id) else {
                return;
            };
            if !name.is_empty() {
                state.profile_names.insert(user, name);
            }
            let mut out = Vec::new();
            let (downloads, gone) = state.profiles.show(user, place, &mut out);
            if online {
                state.outgoing.extend(out);
                fetch_profile_images(state, media, ui, downloads);
            }
            forget_profile_media(ui, gone);
            push_profile_views(state, ui);
        }
        Command::HideProfile(place) => {
            let gone = state.profiles.hide(place);
            forget_profile_media(ui, gone);
        }
        Command::SaveProfile(text) => {
            if !online {
                edit_message(ui, "sem conexão: tente de novo daqui a pouco".into(), true);
            } else if !state.profiles.supported() {
                edit_message(
                    ui,
                    "este servidor ainda não guarda páginas de perfil".into(),
                    true,
                );
            } else {
                state.profile_saving = true;
                state.outgoing.push(ClientMessage::SaveProfile { text });
            }
        }
        Command::SetProfileImage { kind, bytes } => {
            if !online || !state.profiles.supported() {
                // A foto pequena já foi pelo `SetPhoto`; só o editor reclama.
                if kind != ProfileImage::Avatar {
                    edit_message(
                        ui,
                        "sem conexão com um servidor que guarda páginas".into(),
                        true,
                    );
                }
                return;
            }
            let Some(token) = state.token.clone() else {
                return;
            };
            let media = media.clone();
            tokio::spawn(async move {
                let result = async {
                    let (body, content_type) =
                        tokio::task::spawn_blocking(move || profiles::prepare_upload(kind, &bytes))
                            .await
                            .map_err(|_| "não deu para preparar a imagem".to_string())??;
                    let response = media
                        .http
                        .post(format!(
                            "{}{PROFILE_PATH}/imagens/{}",
                            media.base,
                            kind.slug()
                        ))
                        .bearer_auth(&token)
                        .header(reqwest::header::CONTENT_TYPE, content_type)
                        .body(body)
                        .send()
                        .await
                        .map_err(|_| "não deu para enviar a imagem".to_string())?;
                    read_body(response, "o servidor recusou a imagem").await?;
                    Ok::<_, String>("imagem salva.".to_string())
                }
                .await;
                let _ = media.commands.send(Command::ProfileImageDone(result));
            });
        }
        Command::RemoveProfileImage(kind) => {
            let (true, Some(token)) = (online, state.token.clone()) else {
                edit_message(ui, "sem conexão: tente de novo daqui a pouco".into(), true);
                return;
            };
            let media = media.clone();
            tokio::spawn(async move {
                let result = async {
                    let response = media
                        .http
                        .delete(format!(
                            "{}{PROFILE_PATH}/imagens/{}",
                            media.base,
                            kind.slug()
                        ))
                        .bearer_auth(&token)
                        .send()
                        .await
                        .map_err(|_| "não deu para tirar a imagem".to_string())?;
                    read_body(response, "o servidor recusou").await?;
                    Ok::<_, String>("imagem tirada.".to_string())
                }
                .await;
                let _ = media.commands.send(Command::ProfileImageDone(result));
            });
        }
        Command::ProfileImageDone(result) => match result {
            Ok(message) => edit_message(ui, message, false),
            Err(reason) => edit_message(ui, reason, true),
        },
        _ => {}
    }
}

fn parse_user(id: &str) -> Option<UserId> {
    id.parse().ok().map(UserId)
}

/// Chegou o perfil de alguém: guarda, baixa as imagens novas e mostra.
fn profile_received(
    state: &mut LocalState,
    media: &Media,
    ui: &slint::Weak<AppWindow>,
    profile: protocol::Profile,
) {
    let user = profile.user_id;
    let text = profile.text.clone();
    let (downloads, removed) = state.profiles.received(profile);
    if !removed.is_empty() {
        let id = user.0.to_string();
        let _ = ui.upgrade_in_event_loop(move |_| {
            for kind in removed {
                profiles::forget(&id, Some(kind));
            }
        });
    }
    fetch_profile_images(state, media, ui, downloads);
    // O editor começa com o que está salvo.
    if Some(user) == state.me && !state.profile_editor_filled {
        state.profile_editor_filled = true;
        let _ = ui.upgrade_in_event_loop(move |ui| {
            ui.set_edit_real_name(text.real_name.into());
            ui.set_edit_location(text.location.into());
            ui.set_edit_about(text.about.into());
            ui.set_edit_accent(text.accent.into());
            ui.set_edit_show_art(text.showcases.contains(&protocol::Showcase::Art));
            ui.set_edit_show_about(text.showcases.contains(&protocol::Showcase::About));
        });
    }
    push_profile_views(state, ui);
}

/// Baixa e abre as imagens; os quadros vão direto para a thread da janela.
fn fetch_profile_images(
    state: &LocalState,
    media: &Media,
    ui: &slint::Weak<AppWindow>,
    downloads: Vec<profiles::Download>,
) {
    let Some(token) = state.token.clone() else {
        return;
    };
    for download in downloads {
        let media = media.clone();
        let ui = ui.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let url = format!(
                "{}{PROFILE_PATH}/{}/imagens/{}",
                media.base,
                download.user.0,
                download.kind.slug()
            );
            let Ok(response) = media.http.get(url).bearer_auth(&token).send().await else {
                return;
            };
            let Ok(bytes) = read_body(response, "a imagem não veio").await else {
                return;
            };
            let kind = download.kind;
            let decoded = tokio::task::spawn_blocking(move || profiles::decode(kind, &bytes)).await;
            match decoded {
                Ok(Ok(frames)) => {
                    let id = download.user.0.to_string();
                    let _ = ui.upgrade_in_event_loop(move |_| profiles::store(&id, kind, frames));
                }
                Ok(Err(reason)) => eprintln!("perfil: imagem {} não abriu: {reason}", kind.slug()),
                Err(_) => {}
            }
        });
    }
}

/// Esquece as imagens de quem saiu da tela.
fn forget_profile_media(ui: &slint::Weak<AppWindow>, gone: Vec<UserId>) {
    if gone.is_empty() {
        return;
    }
    let ids: Vec<String> = gone.into_iter().map(|u| u.0.to_string()).collect();
    let _ = ui.upgrade_in_event_loop(move |_| {
        for id in ids {
            profiles::forget(&id, None);
        }
    });
}

/// Nome da conversa em cuja chamada a pessoa está agora (vazio: nenhuma).
fn in_call_name(state: &LocalState, user: UserId) -> String {
    let name_of = |id: ConversationId| {
        state
            .conversations
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    };
    if Some(user) == state.me {
        return state
            .call
            .as_ref()
            .map(|call| name_of(call.conversation_id))
            .unwrap_or_default();
    }
    state
        .call_members
        .iter()
        .find(|(_, members)| members.iter().any(|m| m.user_id == user))
        .map(|(id, _)| name_of(*id))
        .unwrap_or_default()
}

/// Atualiza o cartão e a página com o que já sabemos de quem está neles.
fn push_profile_views(state: &LocalState, ui: &slint::Weak<AppWindow>) {
    let mut views = Vec::new();
    for place in [profiles::Place::Card, profiles::Place::Page] {
        let Some(user) = state.profiles.showing(place) else {
            continue;
        };
        let name = state.profile_names.get(&user).cloned().unwrap_or_default();
        let view = profiles::ViewRaw::new(
            user,
            &name,
            state.profiles.get(user),
            state.me,
            state.profiles.supported(),
            in_call_name(state, user),
        );
        let activity = state.profiles.get(user).and_then(|p| p.listening.clone());
        views.push((place, view, activity));
    }
    if views.is_empty() {
        return;
    }
    let _ = ui.upgrade_in_event_loop(move |ui| {
        for (place, view, activity) in views {
            let mut data = view.to_view();
            if let Some(activity) = activity {
                data.listening_title = activity.title.into();
                data.listening_artist = activity.artist.into();
                data.listening_elapsed = mm_ss(activity.position_ms).into();
                data.listening_duration = mm_ss(activity.duration_ms).into();
                data.listening_progress = if activity.duration_ms > 0 {
                    activity.position_ms as f32 / activity.duration_ms as f32
                } else {
                    0.0
                };
            }
            match place {
                profiles::Place::Card if ui.get_card().id == view.id.as_str() => {
                    ui.set_card(data);
                }
                profiles::Place::Page if ui.get_page().id == view.id.as_str() => {
                    ui.set_page(data);
                }
                _ => {}
            }
        }
    });
}

/// A frase embaixo do editor da página.
fn edit_message(ui: &slint::Weak<AppWindow>, message: String, error: bool) {
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_edit_message(message.into());
        ui.set_edit_error(error);
    });
}

/// A conversa da chamada em que estamos (ou entrando).
fn call_conversation(state: &LocalState) -> Option<ConversationId> {
    state
        .call
        .as_ref()
        .map(|call| call.conversation_id)
        .or(state.call_pending)
}

/// Abre a conversa da chamada, se ainda não é a aberta. Devolve o id quando
/// abriu (para pedir o histórico ao servidor).
fn open_call_chat(state: &mut LocalState, ui: &slint::Weak<AppWindow>) -> Option<ConversationId> {
    let id = call_conversation(state)?;
    if state.open == Some(id) {
        return None;
    }
    switch_to(state, Some(id));
    render(state, ui);
    Some(id)
}

/// Cifra e manda o texto na conversa. Se o grupo ainda está trocando
/// chaves, a mensagem espera e sai sozinha quando ele ficar pronto.
fn send_text(
    state: &mut LocalState,
    conversation: Option<ConversationId>,
    text: String,
    ui: &slint::Weak<AppWindow>,
) {
    let Some(conversation_id) = conversation else {
        return;
    };
    let Some(e2ee) = &mut state.e2ee else {
        set_status(
            ui,
            "sem as chaves deste aparelho: a mensagem não saiu".into(),
        );
        return;
    };
    if let Err(reason) = e2ee.send(conversation_id, text, &mut state.outgoing) {
        set_status(ui, reason);
    } else if e2ee.queued() > 0 {
        set_status(ui, "trocando chaves... a mensagem sai em seguida".into());
    }
}

/// Cifra o texto novo e pede ao servidor para trocar a mensagem. O
/// `MessageEdited` que volta é o que muda a tela.
fn edit_text(
    state: &mut LocalState,
    id: &str,
    text: String,
    ui: &slint::Weak<AppWindow>,
) -> Option<ClientMessage> {
    let conversation_id = state.open?;
    let message_id = parse_message_id(id)?;
    let Some(e2ee) = &mut state.e2ee else {
        set_status(
            ui,
            "sem as chaves deste aparelho: a mensagem não mudou".into(),
        );
        return None;
    };
    match e2ee.seal(conversation_id, &text) {
        Ok(sealed) => Some(ClientMessage::EditMessage {
            conversation_id,
            message_id,
            sealed,
        }),
        Err(reason) => {
            set_status(ui, reason);
            None
        }
    }
}

fn parse_message_id(id: &str) -> Option<MessageId> {
    id.parse().ok().map(MessageId)
}

/// Preenche o texto das mensagens que vieram cifradas.
fn reveal_all(state: &mut LocalState, messages: &mut [ChatMessage]) {
    for message in messages {
        match &mut state.e2ee {
            Some(e2ee) => e2ee.reveal(message),
            None if message.sealed.is_some() => message.text = e2ee::UNREADABLE.into(),
            None => {}
        }
    }
}

fn reveal_summary(state: &mut LocalState, conversation: &mut ConversationSummary) {
    if let Some(message) = &mut conversation.last_message {
        reveal_all(state, std::slice::from_mut(message));
    }
}

/// Depois de uma mensagem da troca de chaves. Com a época nova, as mensagens
/// que esperavam um commit já abrem, e o rodapé sai de "trocando chaves".
fn after_mls(
    state: &mut LocalState,
    conversation_id: ConversationId,
    epoch_changed: bool,
    ui: &slint::Weak<AppWindow>,
) {
    if epoch_changed {
        epoch_changed_in(state, conversation_id);
    }
    if state.e2ee.as_ref().is_some_and(|e| e.queued() == 0)
        && state
            .outgoing
            .iter()
            .any(|m| matches!(m, ClientMessage::SendMessage { .. }))
    {
        set_status(ui, "online".into());
    }
}

/// A época do grupo da conversa mudou (entrou ou saiu alguém, ou acabamos de
/// entrar). Se estamos na chamada desta conversa, ela passa a cifrar com a
/// chave nova; quem saiu do grupo não tem essa chave e para de ouvir.
/// O aviso enquanto a chamada espera a chave do grupo.
const WAITING_KEY: &str = "esperando a chave desta conversa chegar: a chamada entra sozinha quando ela chegar \
     (precisa de alguém da conversa online)";

fn epoch_changed_in(state: &mut LocalState, conversation_id: ConversationId) {
    // Esperando a chave para entrar na chamada: chegou, entra.
    if state.call_waiting_key == Some(conversation_id)
        && state.call.is_none()
        && media_key(state, conversation_id).is_some()
    {
        state.call_waiting_key = None;
        state.call_pending = Some(conversation_id);
        state
            .outgoing
            .push(ClientMessage::JoinCall { conversation_id });
        crate::diag::report("chamada: a chave do grupo chegou, entrando".to_string());
    }
    if state
        .call
        .as_ref()
        .is_some_and(|c| c.conversation_id == conversation_id)
        && let Some(key) = media_key(state, conversation_id)
        && let Some(call) = &state.call
    {
        crate::diag::report(format!("chamada: chave nova, época {}", key.epoch));
        call.set_key(key);
    }
    let Some(e2ee) = &mut state.e2ee else {
        return;
    };
    for message in state.messages.iter_mut().chain(
        state
            .conversations
            .iter_mut()
            .filter_map(|c| c.last_message.as_mut()),
    ) {
        if message.conversation_id == conversation_id && e2ee.is_waiting(message.id) {
            e2ee.reveal(message);
        }
    }
}

/// Põe a conversa na lista, ou atualiza a que já estava lá.
fn add_conversation(state: &mut LocalState, conversation: ConversationSummary) {
    if let Some(other) = conversation.other_id {
        state.photos.want([other], &mut state.outgoing);
    }
    match state
        .conversations
        .iter_mut()
        .find(|c| c.id == conversation.id)
    {
        Some(existing) => *existing = conversation,
        None => state.conversations.push(conversation),
    }
    sort_conversations(state);
}

/// A conversa com mensagem mais nova sobe para o topo.
fn sort_conversations(state: &mut LocalState) {
    state
        .conversations
        .sort_by_key(|c| std::cmp::Reverse(c.last_message.as_ref().map(|m| m.sent_at_ms)));
}

/// O pedido das mensagens anteriores à mais antiga que temos, se fizer sentido.
fn load_older(state: &mut LocalState) -> Option<ClientMessage> {
    if !state.has_older || state.loading_older {
        return None;
    }
    let conversation_id = state.open?;
    let before = state.messages.first()?.id;
    state.loading_older = true;
    Some(ClientMessage::LoadOlder {
        conversation_id,
        before,
    })
}

/// Converte o estado em linhas que o Slint sabe desenhar.
fn render(state: &mut LocalState, ui: &slint::Weak<AppWindow>) {
    let now = mentions::now_ms();
    state.photos.want(
        state.messages.iter().map(|m| m.author_id),
        &mut state.outgoing,
    );
    let conversations: Vec<ConversationRaw> = state
        .conversations
        .iter()
        .map(|c| ConversationRaw {
            photo_of: c.other_id.map(|id| id.0.to_string()),
            in_call_people: state
                .call_members
                .get(&c.id)
                .map(|people| {
                    people
                        .iter()
                        .map(|m| (m.user_id.0.to_string(), m.username.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            id: c.id.0.to_string().into(),
            name: c.name.clone().into(),
            initial: c
                .name
                .chars()
                .next()
                .map(|ch| ch.to_uppercase().to_string())
                .unwrap_or_default()
                .into(),
            preview: c
                .last_message
                .as_ref()
                .map(|m| match (&m.audio, &m.file) {
                    (Some(audio), _) => format!(
                        "{}: [áudio {}]",
                        m.author_name,
                        mm_ss(audio.duration_ms.into())
                    ),
                    (_, Some(file)) if file.content_type.starts_with("image/") => {
                        format!("{}: [imagem]", m.author_name)
                    }
                    (_, Some(file)) => format!("{}: [arquivo {}]", m.author_name, file.name),
                    _ => format!("{}: {}", m.author_name, m.text),
                })
                .unwrap_or_else(|| "nenhuma mensagem ainda".into())
                .into(),
            mentioned: state.mentioned.contains(&c.id),
            unread: state.unread.get(&c.id).copied().unwrap_or(0) as i32,
            time: c
                .last_message
                .as_ref()
                .map(|m| mentions::list_time(m.sent_at_ms, now))
                .unwrap_or_default()
                .into(),
            is_group: c.kind == ConversationKind::Group,
            selected: state.open == Some(c.id),
            in_call: state
                .in_call
                .get(&c.id)
                .map(|n| match c.kind {
                    ConversationKind::Group => format!("▪ {n} na sala"),
                    // Na direta só pode ser a outra pessoa (ou nós).
                    ConversationKind::Direct if in_this_call(state, c.id) => "em chamada".into(),
                    ConversationKind::Direct => "te chamando".into(),
                })
                .unwrap_or_default()
                .into(),
        })
        .collect();
    // Quem está na chamada da conversa aberta, se não estamos nela.
    let open_call: Vec<(String, String)> = state
        .open
        .filter(|id| state.call.as_ref().map(|c| c.conversation_id) != Some(*id))
        .and_then(|id| state.call_members.get(&id))
        .map(|people| {
            people
                .iter()
                .map(|m| (m.user_id.0.to_string(), m.username.clone()))
                .collect()
        })
        .unwrap_or_default();

    let raw_messages = state.messages.clone();
    let unread_mark = state.unread_mark;
    let unread_total: i32 = state.unread.values().sum::<u32>() as i32;
    let me = state.me;
    let flags = continues_flags(&raw_messages);
    let days = mentions::day_separators(raw_messages.iter().map(|m| m.sent_at_ms), now);
    let my_name = state.my_name.clone();
    let mention_names = mention_candidates(state);
    let previews = state.image_cache.clone();

    let title: SharedString = state
        .open
        .and_then(|id| state.conversations.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_default()
        .into();
    let members = member_rows(state);
    let open_is_group = state.open.is_some_and(|id| is_managed_group(state, id));
    let open_is_direct = state.open.is_some_and(|id| is_direct(state, id));
    let members_open = state.members_open && open_is_group;
    let has_older = state.has_older;
    let loading_older = state.loading_older;
    let prepended = std::mem::take(&mut state.prepended);
    let can_call = state.open.is_some_and(|id| can_call(state, id));
    let call_id = state
        .call
        .as_ref()
        .map(|c| c.conversation_id)
        .or(state.call_pending);
    let call_name: SharedString = call_id
        .and_then(|id| state.conversations.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_default()
        .into();
    let call_is_open = call_id.is_some() && call_id == state.open;
    let call_connected = state.call_connected && state.call.is_some();
    let call_muted = state.call_muted;
    let call_people = call_people_rows(state);
    let call_camera = state.call_camera;
    let call_sharing = state.call_sharing;
    let share_picker_open = state.share_picker_open;
    let (share_screens, share_windows) = share_rows(state);

    // `VecModel` não pode cruzar threads, então montamos ele já dentro da
    // thread da interface.
    let _ = ui.upgrade_in_event_loop(move |ui| {
        let messages: Vec<MessageRow> = raw_messages
            .iter()
            .zip(flags)
            .zip(days)
            .map(|((m, continues), day)| MessageRow {
                id: m.id.0.to_string().into(),
                first_unread: unread_mark == Some(m.id),
                mine: me == Some(m.author_id),
                edited: m.edited,
                author_photo: photos::image(&m.author_id.0.to_string()).unwrap_or_default(),
                author_initial: initial_of(&m.author_name).into(),
                author: m.author_name.clone().into(),
                author_id: m.author_id.0.to_string().into(),
                time: clock(m.sent_at_ms).into(),
                text: m.text.clone().into(),
                // Dia novo sempre começa um bloco, com nome e horário.
                continues: continues && day.is_empty() && unread_mark != Some(m.id),
                mentions_me: me != Some(m.author_id) && mentions::mentions(&m.text, &my_name),
                day: day.into(),
                audio_id: m
                    .audio
                    .map(|a| a.attachment_id.0.to_string())
                    .unwrap_or_default()
                    .into(),
                duration: m
                    .audio
                    .map(|a| mm_ss(a.duration_ms.into()))
                    .unwrap_or_default()
                    .into(),
                file_id: m
                    .file
                    .as_ref()
                    .map(|f| f.attachment_id.0.to_string())
                    .unwrap_or_default()
                    .into(),
                file_name: m
                    .file
                    .as_ref()
                    .map(|f| f.name.clone())
                    .unwrap_or_default()
                    .into(),
                file_size: m
                    .file
                    .as_ref()
                    .map(|f| format_size(f.size_bytes))
                    .unwrap_or_default()
                    .into(),
                is_image: m
                    .file
                    .as_ref()
                    .is_some_and(|f| f.content_type.starts_with("image/")),
                preview_ready: m
                    .file
                    .as_ref()
                    .is_some_and(|f| previews.contains_key(&f.attachment_id)),
                preview_image: m
                    .file
                    .as_ref()
                    .and_then(|f| previews.get(&f.attachment_id))
                    .map(|thumb| {
                        slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                            &thumb.pixels,
                            thumb.width,
                            thumb.height,
                        ))
                    })
                    .unwrap_or_default(),
            })
            .collect();
        let conversations: Vec<ConversationRow> =
            conversations.into_iter().map(conversation_row).collect();
        ui.set_conversations(ModelRc::from(Rc::new(VecModel::from(conversations))));
        ui.set_unread_total(unread_total);
        ui.set_open_call_faces(faces(open_call));
        // Antes de trocar as mensagens, para a tela já saber como rolar.
        if prepended {
            ui.set_keep_from_bottom(true);
        }
        ui.set_messages(ModelRc::from(Rc::new(VecModel::from(messages))));
        let mention_names: Vec<SharedString> =
            mention_names.into_iter().map(SharedString::from).collect();
        ui.set_mention_names(ModelRc::from(Rc::new(VecModel::from(mention_names))));
        ui.set_has_older(has_older);
        ui.set_loading_older(loading_older);
        ui.set_open_title(title);
        ui.set_members(ModelRc::from(Rc::new(VecModel::from(members))));
        ui.set_open_is_group(open_is_group);
        ui.set_open_is_direct(open_is_direct);
        ui.set_members_open(members_open);
        ui.set_can_call(can_call);
        ui.set_call_name(call_name);
        ui.set_call_is_open(call_is_open);
        ui.set_call_connected(call_connected);
        ui.set_call_muted(call_muted);
        show_call_people(&ui, call_people);
        ui.set_call_camera(call_camera);
        ui.set_call_sharing(call_sharing);
        ui.set_share_picker_open(share_picker_open);
        ui.set_share_screens(share_model(share_screens));
        ui.set_share_windows(share_model(share_windows));
    });
}

/// Uma opção da lista de compartilhar, ainda sem virar `ShareSourceRow`: a
/// imagem do Slint não pode cruzar threads, a foto crua pode.
struct ShareRaw {
    id: String,
    name: String,
    detail: String,
    preview: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

impl ShareRaw {
    /// Já na thread da interface.
    fn row(self) -> ShareSourceRow {
        ShareSourceRow {
            id: self.id.into(),
            name: self.name.into(),
            detail: self.detail.into(),
            has_preview: self.preview.is_some(),
            preview: self
                .preview
                .map(slint::Image::from_rgba8)
                .unwrap_or_default(),
        }
    }
}

/// As telas e as janelas da lista de compartilhar, cada uma com a foto,
/// se já chegou.
fn share_rows(state: &LocalState) -> (Vec<ShareRaw>, Vec<ShareRaw>) {
    let raw = |s: &ShareSource| {
        let preview = state.share_previews.get(&s.id);
        ShareRaw {
            id: s.id.clone(),
            name: s.name.clone(),
            detail: preview
                .filter(|_| s.is_screen)
                .map(|p| format!("{}×{}", p.width, p.height))
                .unwrap_or_default(),
            preview: preview.map(|p| p.pixels.clone()),
        }
    };
    let screens = state
        .share_sources
        .iter()
        .filter(|s| s.is_screen)
        .map(raw)
        .collect();
    let windows = state
        .share_sources
        .iter()
        .filter(|s| !s.is_screen)
        .map(raw)
        .collect();
    (screens, windows)
}

/// `ShareRaw`s para o modelo da tela.
fn share_model(list: Vec<ShareRaw>) -> ModelRc<ShareSourceRow> {
    let rows: Vec<ShareSourceRow> = list.into_iter().map(ShareRaw::row).collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// Só a lista de compartilhar, quando chega uma foto.
fn render_share(state: &LocalState, ui: &slint::Weak<AppWindow>) {
    let (screens, windows) = share_rows(state);
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_share_screens(share_model(screens));
        ui.set_share_windows(share_model(windows));
    });
}

/// Só a parte da tela que diz quem fala: a faixa da chamada e os membros.
fn render_speaking(state: &LocalState, ui: &slint::Weak<AppWindow>) {
    let members = member_rows(state);
    let call_people = call_people_rows(state);
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_members(ModelRc::from(Rc::new(VecModel::from(members))));
        show_call_people(&ui, call_people);
    });
}

/// Membros do grupo aberto. Quem está falando na chamada dele fica marcado.
fn member_rows(state: &LocalState) -> Vec<MemberRow> {
    let i_own = state.me.is_some() && state.owner == state.me;
    let call_is_open = state.call.as_ref().map(|c| c.conversation_id) == state.open;
    state
        .members
        .iter()
        .map(|m| {
            let id = m.user_id.0.to_string();
            let speaking =
                call_is_open && state.call_people.iter().any(|p| p.speaking && p.id == id);
            MemberRow {
                id: id.into(),
                name: m.username.clone().into(),
                is_owner: Some(m.user_id) == state.owner,
                can_remove: i_own && Some(m.user_id) != state.me,
                speaking,
            }
        })
        .collect()
}

/// Uma linha da lista de conversas sem as fotos (elas só existem na thread
/// da janela); `conversation_row` monta a linha de verdade lá.
struct ConversationRaw {
    id: SharedString,
    name: SharedString,
    initial: SharedString,
    preview: SharedString,
    time: SharedString,
    is_group: bool,
    selected: bool,
    in_call: SharedString,
    mentioned: bool,
    unread: i32,
    /// Conversa direta: de quem é a foto que aparece na linha.
    photo_of: Option<String>,
    /// Quem está na chamada dela: (id, nome).
    in_call_people: Vec<(String, String)>,
}

/// Na thread da janela: a linha com as fotos do cache (`photos.rs`).
fn conversation_row(raw: ConversationRaw) -> ConversationRow {
    let photo = raw.photo_of.as_deref().and_then(photos::image);
    ConversationRow {
        id: raw.id,
        name: raw.name,
        initial: raw.initial,
        preview: raw.preview,
        time: raw.time,
        is_group: raw.is_group,
        selected: raw.selected,
        in_call: raw.in_call,
        mentioned: raw.mentioned,
        unread: raw.unread,
        has_photo: photo.is_some(),
        photo: photo.unwrap_or_default(),
        call_faces: faces(raw.in_call_people),
    }
}

/// Rostinhos de quem está numa chamada: a foto (do cache) ou a inicial.
fn faces(people: Vec<(String, String)>) -> ModelRc<CallFace> {
    let faces: Vec<CallFace> = people
        .into_iter()
        .map(|(id, name)| {
            let photo = photos::image(&id);
            CallFace {
                initial: name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default()
                    .into(),
                has_photo: photo.is_some(),
                photo: photo.unwrap_or_default(),
                name: name.into(),
                id: id.into(),
            }
        })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(faces)))
}

/// Uma linha da chamada sem a foto: a `slint::Image` só existe na thread da
/// janela, então a linha de verdade (`CallPersonRow`) é montada lá.
struct CallRow {
    id: String,
    name: String,
    speaking: bool,
    muted: bool,
    quality: u8,
    ping_ms: u32,
    is_me: bool,
    voice: i32,
    screen_audio: bool,
    screen: i32,
}

fn call_people_rows(state: &LocalState) -> Vec<CallRow> {
    let me = state.me.map(|id| id.0.to_string());
    let volumes = volume::shared();
    state
        .call_people
        .iter()
        .map(|p| CallRow {
            id: p.id.clone(),
            name: p.name.clone(),
            speaking: p.speaking,
            muted: p.muted,
            quality: p.quality,
            ping_ms: p.ping_ms,
            is_me: me.as_deref() == Some(p.id.as_str()),
            voice: volumes.get(&p.id, volume::Kind::Voice),
            screen_audio: p.screen_audio,
            screen: volumes.get(&p.id, volume::Kind::Screen),
        })
        .collect()
}

/// Na thread da janela: a linha com a foto do cache (`photos.rs`).
fn call_person_row(row: CallRow) -> CallPersonRow {
    let photo = photos::image(&row.id);
    CallPersonRow {
        initial: row
            .name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
            .into(),
        has_photo: photo.is_some(),
        photo: photo.unwrap_or_default(),
        level: 0.0,
        id: row.id.into(),
        name: row.name.into(),
        speaking: row.speaking,
        muted: row.muted,
        quality: i32::from(row.quality),
        ping: row.ping_ms as i32,
        is_me: row.is_me,
        voice: row.voice,
        screen_audio: row.screen_audio,
        screen: row.screen,
    }
}

/// Troca a lista de quem está na chamada. Com as mesmas pessoas na mesma
/// ordem, muda só as linhas, sem recriar a lista: quem está arrastando um
/// volume no mixer não perde o arraste quando alguém começa a falar.
/// A foto vem do cache da janela (`photos.rs`) e o nível da voz fica o que
/// já estava na tela (quem mexe nele é o relógio do `main.rs`).
fn show_call_people(ui: &AppWindow, rows: Vec<CallRow>) {
    let ping = rows.iter().find(|r| r.is_me).map_or(0, |r| r.ping_ms);
    ui.set_call_ping(if ping > 0 {
        format!("ping {ping} ms").into()
    } else {
        SharedString::new()
    });
    let model = ui.get_call_people();
    let rows: Vec<CallPersonRow> = rows.into_iter().map(call_person_row).collect();
    if let Some(current) = model.as_any().downcast_ref::<VecModel<CallPersonRow>>()
        && current.row_count() == rows.len()
        && current.iter().zip(&rows).all(|(old, new)| old.id == new.id)
    {
        for (i, mut row) in rows.into_iter().enumerate() {
            let old = current.row_data(i);
            if let Some(old) = &old {
                row.level = old.level;
            }
            if old.as_ref() != Some(&row) {
                current.set_row_data(i, row);
            }
        }
        return;
    }
    ui.set_call_people(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// Chegou a foto de alguém: põe nas linhas da chamada que já estão na tela.
fn refresh_call_photos(ui: &AppWindow) {
    let model = ui.get_call_people();
    let Some(current) = model.as_any().downcast_ref::<VecModel<CallPersonRow>>() else {
        return;
    };
    for i in 0..current.row_count() {
        let Some(mut row) = current.row_data(i) else {
            continue;
        };
        let photo = photos::image(&row.id);
        let changed = match &photo {
            Some(p) => !row.has_photo || &row.photo != p,
            None => row.has_photo,
        };
        if changed {
            row.has_photo = photo.is_some();
            row.photo = photo.unwrap_or_default();
            current.set_row_data(i, row);
        }
    }
}

fn format_size(bytes: u32) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f32 / (1024 * 1024) as f32)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Quem dá para marcar com "@" na conversa aberta: os membros, quem
/// escreveu nas mensagens carregadas, a outra pessoa da direta e, em grupo,
/// "todos". Sem a própria pessoa.
fn mention_candidates(state: &LocalState) -> Vec<String> {
    let Some(open) = state.open else {
        return Vec::new();
    };
    let conv = state.conversations.iter().find(|c| c.id == open);
    let mut names: Vec<String> = Vec::new();
    let mut add = |id: Option<UserId>, name: &str| {
        if (id.is_none() || id != state.me)
            && !name.eq_ignore_ascii_case(&state.my_name)
            && !names.iter().any(|n| n.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    };
    if let Some(c) = conv
        && c.kind == ConversationKind::Direct
    {
        add(c.other_id, &c.name);
    }
    for m in &state.members {
        add(Some(m.user_id), &m.username);
    }
    for m in state.messages.iter().rev() {
        add(Some(m.author_id), &m.author_name);
    }
    if conv.is_some_and(|c| c.kind == ConversationKind::Group) {
        add(None, mentions::EVERYONE);
    }
    names
}

/// Marcaram a gente: pisca a janela na barra de tarefas se ela não está
/// em primeiro plano.
fn call_attention(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        use slint::ComponentHandle;
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::window::UserAttentionType;
        ui.window().with_winit_window(|w| {
            if !w.has_focus() {
                w.request_user_attention(Some(UserAttentionType::Critical));
            }
        });
    });
}

/// Mensagens mais próximas que isto, da mesma pessoa, ficam no mesmo bloco.
const GROUP_GAP_MS: u64 = 5 * 60 * 1000;

/// Para cada mensagem, diz se ela continua o bloco da anterior (mesma pessoa,
/// menos de 5 minutos depois). A primeira de cada bloco mostra nome e horário.
fn continues_flags(messages: &[ChatMessage]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(messages.len());
    let mut prev: Option<&ChatMessage> = None;
    for m in messages {
        flags.push(prev.is_some_and(|p| {
            p.author_id == m.author_id
                && m.sent_at_ms >= p.sent_at_ms
                && m.sent_at_ms - p.sent_at_ms < GROUP_GAP_MS
        }));
        prev = Some(m);
    }
    flags
}

fn set_group_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_group_error(reason.into()));
}

fn set_status(ui: &slint::Weak<AppWindow>, status: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_status(status.into()));
}

fn set_direct_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_direct_error(reason.into()));
}

fn set_auth_error(ui: &slint::Weak<AppWindow>, reason: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_auth_error(reason.into()));
}

/// Volta para a tela de login, já pronta para digitar.
fn show_login(ui: &slint::Weak<AppWindow>) {
    let _ = ui.upgrade_in_event_loop(|ui| {
        ui.set_logged_in(false);
        ui.set_connecting(false);
    });
}

/// Mostra que estamos sem servidor. Com a conversa aberta, só o rodapé muda;
/// na tela de login, os botões ficam desligados até voltar.
fn show_offline(state: &LocalState, ui: &slint::Weak<AppWindow>, retry_in: Duration) {
    if state.logged_in {
        set_status(ui, "reconectando...".into());
    } else {
        let secs = retry_in.as_secs_f32().ceil() as u64;
        set_auth_error(
            ui,
            format!("sem conexão com o servidor; tentando de novo em {secs}s"),
        );
        let _ = ui.upgrade_in_event_loop(|ui| ui.set_connecting(true));
    }
}

fn parse_id(id: &str) -> Option<ConversationId> {
    id.parse().ok().map(ConversationId)
}

/// "21:06" no fuso horário da máquina.
fn clock(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Sink = futures_util::stream::SplitSink<Socket, Message>;

/// O erro do tungstenite é grande; na caixa (`Box`) ele ocupa só um ponteiro.
async fn send(
    sink: &mut Sink,
    msg: &ClientMessage,
) -> Result<(), Box<tokio_tungstenite::tungstenite::Error>> {
    let json = serde_json::to_string(msg).expect("ClientMessage sempre vira JSON");
    sink.send(Message::Text(json.into()))
        .await
        .map_err(Box::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{MessageId, UserId};
    use uuid::Uuid;

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let secs: Vec<u64> = (0..9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30, 30, 30]);
        assert_eq!(backoff(u32::MAX), Duration::from_secs(30));
    }

    fn msg(author: &str, minute: u64, second: u64) -> ChatMessage {
        ChatMessage {
            id: MessageId(0),
            conversation_id: ConversationId(Uuid::nil()),
            author_id: UserId(Uuid::from_u128(if author == "jobiss" { 1 } else { 2 })),
            author_name: author.into(),
            text: String::new(),
            sealed: None,
            sent_at_ms: (minute * 60 + second) * 1000,
            edited: false,
            audio: None,
            file: None,
        }
    }

    #[test]
    fn groups_same_author_within_five_minutes() {
        let messages = [
            msg("jobiss", 0, 0),
            msg("jobiss", 1, 0),
            // 4min59s depois da anterior: ainda junto.
            msg("jobiss", 5, 59),
            // Exatamente 5 minutos: bloco novo.
            msg("jobiss", 10, 59),
            // Outra pessoa: bloco novo, mesmo logo depois.
            msg("nina", 11, 0),
            msg("nina", 11, 30),
            msg("jobiss", 12, 0),
        ];
        assert_eq!(
            continues_flags(&messages),
            [false, true, true, false, false, true, false]
        );
    }

    #[test]
    fn http_address_comes_from_the_websocket_one() {
        assert_eq!(http_base("ws://127.0.0.1:3000/ws"), "http://127.0.0.1:3000");
        assert_eq!(http_base("wss://mothh.app/ws/"), "https://mothh.app");
        assert_eq!(http_base("ws://localhost:3000"), "http://localhost:3000");
    }

    #[test]
    fn empty_list_has_no_flags() {
        assert!(continues_flags(&[]).is_empty());
    }
}
