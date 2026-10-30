//! Chamada de voz pelo LiveKit.
//!
//! O servidor do mothh só entrega o token (`CallToken`); a voz vai direto do
//! app para o LiveKit. O microfone e o alto-falante ficam com o próprio
//! WebRTC (`PlatformAudio`), que já faz cancelamento de eco, redução de
//! ruído e ganho automático, e toca sozinho o áudio de quem está na sala.
//!
//! A chamada roda numa tarefa Tokio própria. A rede manda ordens por um
//! `CallHandle` e fica sabendo do resultado pelo canal de comandos
//! (`Command::CallJoined` e `Command::CallEnded`).

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use protocol::ConversationId;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::Command;

/// O que a rede pede para a chamada em andamento.
enum Control {
    Mute(bool),
    Leave,
}

/// Controle de uma chamada. Largar o `CallHandle` também desliga ela.
pub struct CallHandle {
    pub conversation_id: ConversationId,
    control: UnboundedSender<Control>,
}

impl CallHandle {
    pub fn set_muted(&self, muted: bool) {
        let _ = self.control.send(Control::Mute(muted));
    }

    pub fn leave(&self) {
        let _ = self.control.send(Control::Leave);
    }
}

/// Entra na sala do LiveKit com o token que o servidor mandou.
pub fn start(
    conversation_id: ConversationId,
    url: String,
    token: String,
    net: UnboundedSender<Command>,
) -> CallHandle {
    let (control, control_rx) = unbounded_channel();
    tokio::spawn(async move {
        let reason = run(conversation_id, &url, &token, control_rx, &net)
            .await
            .err();
        let _ = net.send(Command::CallEnded {
            conversation_id,
            reason,
        });
    });
    CallHandle {
        conversation_id,
        control,
    }
}

/// A chamada do começo ao fim. `Ok` quando a pessoa saiu; `Err` com uma
/// frase para a tela quando não deu para entrar ou a chamada caiu.
async fn run(
    conversation_id: ConversationId,
    url: &str,
    token: &str,
    mut control: UnboundedReceiver<Control>,
    net: &UnboundedSender<Command>,
) -> Result<(), String> {
    // Precisa viver a chamada inteira: largar desliga microfone e som.
    let audio = PlatformAudio::new().map_err(|err| format!("o som da chamada não abriu: {err}"))?;
    let (room, mut events) = Room::connect(url, token, RoomOptions::default())
        .await
        .map_err(|err| format!("não deu para entrar na chamada: {err}"))?;

    let mic = LocalAudioTrack::create_audio_track("microfone", audio.rtc_source());
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(mic.clone()),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await
        .map_err(|err| format!("o microfone não entrou na chamada: {err}"))?;
    let _ = net.send(Command::CallJoined(conversation_id));

    loop {
        tokio::select! {
            order = control.recv() => match order {
                Some(Control::Mute(true)) => mic.mute(),
                Some(Control::Mute(false)) => mic.unmute(),
                // `None`: a rede largou o controle, então sai também.
                Some(Control::Leave) | None => {
                    let _ = room.close().await;
                    return Ok(());
                }
            },
            event = events.recv() => match event {
                Some(RoomEvent::Disconnected { reason }) => {
                    return Err(format!("a chamada caiu ({reason:?})"));
                }
                None => return Err("a chamada caiu".into()),
                // Quem entra e sai é ouvido sozinho; o resto não interessa aqui.
                Some(_) => {}
            },
        }
    }
}
