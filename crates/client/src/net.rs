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
    ConversationKind, ConversationSummary, GROUP_ROOM, MAX_FILE_BYTES, Member, ServerMessage,
    Uploaded, UserId,
};
use slint::{ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, interval, sleep_until};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::audio::{AudioHandle, mm_ss};
use crate::call::{self, CallHandle, CallPerson, ShareRequest};
use crate::devices::Devices;
use crate::e2ee::{self, E2ee, ServerGroup};
use crate::screen::{Preview, ShareSource};
use crate::session::SavedSession;
use crate::sframe;
use crate::sound;
use crate::{
    AppWindow, CallPersonRow, Command, ConversationRow, MemberRow, MessageRow, ShareSourceRow,
};

/// De quanto em quanto tempo mandamos um ping para o servidor.
const PING_EVERY: Duration = Duration::from_secs(15);
/// Tempo máximo para enviar ou baixar um áudio.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Sem ouvir nada do servidor por esse tempo, a conexão é dada como morta.
/// Pega o caso em que a rede some e o sistema não avisa que o TCP fechou.
const SILENCE_LIMIT: Duration = Duration::from_secs(45);

#[derive(Default)]
struct LocalState {
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
    /// Acabamos de reconectar no meio de uma chamada: avisa o servidor de
    /// novo, porque ele tirou a gente da conta quando a conexão caiu.
    rejoin_call: bool,
    /// As chaves das conversas (F2-08). Abre no `Welcome`, com o id da conta.
    e2ee: Option<E2ee>,
    /// O que mandar para o servidor depois de tratar uma mensagem ou um
    /// comando, além da resposta direta (a troca de chaves gera várias).
    outgoing: Vec<ClientMessage>,
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
        if let Ok((socket, _)) = connect_async(&url).await {
            attempt = 0;
            match connection(socket, &mut state, &saved, &ui, &mut commands, &media).await {
                Ended::Quit => return,
                Ended::Lost => {}
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
                    Command::Login { username, password } => {
                        Some(ClientMessage::Login { username, password })
                    }
                    Command::Register { username, password } => {
                        Some(ClientMessage::Register { username, password })
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
                        send_text(state, text, ui);
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
                    Command::SendFile(path) => {
                        upload_file(state, media, path, ui);
                        None
                    }
                    Command::FileUploaded { conversation_id, attachment_id } => {
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
                    command @ (Command::LeaveCall
                    | Command::ToggleMute
                    | Command::ToggleCamera
                    | Command::CameraFailed(_)
                    | Command::OpenSharePicker
                    | Command::CloseSharePicker
                    | Command::ShareSources(_)
                    | Command::SharePreview(_)
                    | Command::StartShare { .. }
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
                    // Os botões ficam desligados sem conexão; isto é só garantia.
                    Command::Login { .. } | Command::Register { .. } => {
                        set_auth_error(ui, "sem conexão com o servidor".into());
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
                    Command::Send(_) => {
                        set_status(ui, "sem conexão: a mensagem não foi enviada".into());
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
                    Command::SendFile(_) | Command::FileUploaded { .. } | Command::SaveFile { .. } => {
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
            let audio = media.devices.get().share_audio;
            let _ = ui.upgrade_in_event_loop(move |ui| ui.set_share_audio(audio));
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
        Command::StartShare { source_id, audio } => {
            let call = state.call.as_ref()?;
            let name = state
                .share_sources
                .iter()
                .find(|s| s.id == source_id)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            media.devices.set_share_audio(audio);
            state.share_picker_open = false;
            state.share_sources.clear();
            state.share_previews.clear();
            state.call_sharing = true;
            call.set_share(Some(ShareRequest {
                source_id,
                name,
                audio,
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
        set_status(
            ui,
            "a chamada ainda não tem chave: espere a troca de chaves do grupo".into(),
        );
        return;
    };
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

/// Só grupos (inclusive a Sala do Grupo) têm chamada.
fn can_call(state: &LocalState, id: ConversationId) -> bool {
    state
        .conversations
        .iter()
        .any(|c| c.id == id && c.kind == ConversationKind::Group)
}

/// Muda a conversa aberta. Devolve o id, se ele for válido.
fn open(state: &mut LocalState, id: &str, ui: &slint::Weak<AppWindow>) -> Option<ConversationId> {
    let conversation_id = parse_id(id)?;
    switch_to(state, Some(conversation_id));
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
fn upload_file(state: &LocalState, media: &Media, path: PathBuf, ui: &slint::Weak<AppWindow>) {
    let (Some(conversation_id), Some(token)) = (state.open, state.token.clone()) else {
        return;
    };
    set_status(ui, "enviando arquivo...".into());
    let media = media.clone();
    tokio::spawn(async move {
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
            upload_file_body(&media, &token, name, file_content_type(&path), bytes).await
        }
        .await;
        let command = match result {
            Ok(attachment_id) => Command::FileUploaded {
                conversation_id,
                attachment_id,
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
    let image = image.thumbnail(240, 170).to_rgba8();
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
        } => {
            state.me = Some(user_id);
            // As chaves são da conta: outra conta no mesmo perfil abre as dela.
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
            state.conversations = conversations;
            // As contas das chamadas chegam logo depois, em `CallPresence`.
            state.in_call.clear();
            state.rejoin_call = state.call.is_some();
            // Voltando de uma queda, pede de novo o histórico da conversa que
            // estava aberta: pode ter chegado mensagem enquanto estávamos fora.
            // As mensagens antigas ficam na tela até o `History` chegar.
            state.loading_older = false;
            reply = state
                .open
                .map(|conversation_id| ClientMessage::OpenConversation { conversation_id });
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_user_name(username.into());
                ui.set_auth_error(SharedString::new());
                ui.set_status("online".into());
                ui.set_connecting(false);
                ui.set_logged_in(true);
            });
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
        ServerMessage::CallToken {
            conversation_id,
            url,
            token,
        } => start_call(state, media, ui, conversation_id, url, token),
        ServerMessage::CallPresence {
            conversation_id,
            count,
        } => {
            if count == 0 {
                state.in_call.remove(&conversation_id);
            } else {
                state.in_call.insert(conversation_id, count);
            }
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
            if std::mem::take(&mut state.opening_direct) {
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

/// Cifra e manda o texto na conversa aberta. Se o grupo ainda está trocando
/// chaves, a mensagem espera e sai sozinha quando ele ficar pronto.
fn send_text(state: &mut LocalState, text: String, ui: &slint::Weak<AppWindow>) {
    let Some(conversation_id) = state.open else {
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
fn epoch_changed_in(state: &mut LocalState, conversation_id: ConversationId) {
    if state
        .call
        .as_ref()
        .is_some_and(|c| c.conversation_id == conversation_id)
        && let Some(key) = media_key(state, conversation_id)
        && let Some(call) = &state.call
    {
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
    let conversations: Vec<ConversationRow> = state
        .conversations
        .iter()
        .map(|c| ConversationRow {
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
            time: c
                .last_message
                .as_ref()
                .map(|m| clock(m.sent_at_ms))
                .unwrap_or_default()
                .into(),
            is_group: c.kind == ConversationKind::Group,
            selected: state.open == Some(c.id),
            in_call: state
                .in_call
                .get(&c.id)
                .map(|n| format!("▪ {n} na sala"))
                .unwrap_or_default()
                .into(),
        })
        .collect();

    let raw_messages = state.messages.clone();
    let flags = continues_flags(&raw_messages);
    let previews = state.image_cache.clone();

    let title: SharedString = state
        .open
        .and_then(|id| state.conversations.iter().find(|c| c.id == id))
        .map(|c| c.name.clone())
        .unwrap_or_default()
        .into();
    let members = member_rows(state);
    let open_is_group = state.open.is_some_and(|id| is_managed_group(state, id));
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
            .map(|(m, continues)| MessageRow {
                author: m.author_name.clone().into(),
                time: clock(m.sent_at_ms).into(),
                text: m.text.clone().into(),
                continues,
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
        ui.set_conversations(ModelRc::from(Rc::new(VecModel::from(conversations))));
        // Antes de trocar as mensagens, para a tela já saber como rolar.
        if prepended {
            ui.set_keep_from_bottom(true);
        }
        ui.set_messages(ModelRc::from(Rc::new(VecModel::from(messages))));
        ui.set_has_older(has_older);
        ui.set_loading_older(loading_older);
        ui.set_open_title(title);
        ui.set_members(ModelRc::from(Rc::new(VecModel::from(members))));
        ui.set_open_is_group(open_is_group);
        ui.set_members_open(members_open);
        ui.set_can_call(can_call);
        ui.set_call_name(call_name);
        ui.set_call_is_open(call_is_open);
        ui.set_call_connected(call_connected);
        ui.set_call_muted(call_muted);
        ui.set_call_people(ModelRc::from(Rc::new(VecModel::from(call_people))));
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
        ui.set_call_people(ModelRc::from(Rc::new(VecModel::from(call_people))));
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

fn call_people_rows(state: &LocalState) -> Vec<CallPersonRow> {
    state
        .call_people
        .iter()
        .map(|p| CallPersonRow {
            name: p.name.clone().into(),
            speaking: p.speaking,
        })
        .collect()
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
