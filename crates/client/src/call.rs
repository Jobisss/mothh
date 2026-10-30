//! Chamada de voz pelo LiveKit.
//!
//! O servidor do mothh só entrega o token (`CallToken`); a voz vai direto do
//! app para o LiveKit. O microfone e o alto-falante ficam com o próprio
//! WebRTC (`PlatformAudio`), que já faz cancelamento de eco, redução de
//! ruído e ganho automático, e toca sozinho o áudio de quem está na sala.
//!
//! A chamada roda numa tarefa Tokio própria. A rede manda ordens por um
//! `CallHandle` e fica sabendo do resultado pelo canal de comandos
//! (`Command::CallJoined`, `Command::CallPeople` e `Command::CallEnded`).
//!
//! Quem está falando vem do LiveKit: o servidor dele mede o nível de áudio
//! de cada um e avisa quem passou do limiar ou voltou para baixo dele
//! (`ActiveSpeakersChanged`). Para
//! o destaque não piscar nas pausas entre palavras, quem para de falar fica
//! destacado mais um pouco (`HOLD`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use protocol::ConversationId;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::{Instant, MissedTickBehavior, interval};

use crate::Command;

/// Quanto tempo o destaque fica depois que o LiveKit diz que a pessoa parou
/// de falar. Cobre a pausa entre uma palavra e outra.
const HOLD: Duration = Duration::from_millis(500);

/// De quanto em quanto tempo confere se algum destaque venceu.
const TICK: Duration = Duration::from_millis(100);

/// Uma pessoa na chamada, como a tela mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPerson {
    /// Identidade no LiveKit: o id do usuário no mothh.
    pub id: String,
    pub name: String,
    pub speaking: bool,
}

/// Quem está falando, com o tempo extra do `HOLD` para não piscar.
#[derive(Debug, Default)]
struct Speaking {
    /// Quem o LiveKit diz que fala agora.
    active: HashSet<String>,
    /// Quem parou há pouco, e até quando continua destacado.
    until: HashMap<String, Instant>,
}

impl Speaking {
    /// Chegou a lista nova do LiveKit.
    fn update(&mut self, active: HashSet<String>, now: Instant) {
        for id in self.active.difference(&active) {
            self.until.insert(id.clone(), now + HOLD);
        }
        for id in &active {
            self.until.remove(id);
        }
        self.active = active;
    }

    /// Tira o destaque na hora (saiu da sala ou mutou).
    fn forget(&mut self, id: &str) {
        self.active.remove(id);
        self.until.remove(id);
    }

    fn is_speaking(&self, id: &str, now: Instant) -> bool {
        self.active.contains(id) || self.until.get(id).is_some_and(|&end| now < end)
    }
}

/// Quem está na sala e quem fala. Eu primeiro, os outros por nome, para
/// ninguém trocar de lugar quando começa a falar.
fn people(
    me: &str,
    names: &HashMap<String, String>,
    speaking: &Speaking,
    now: Instant,
) -> Vec<CallPerson> {
    let mut people: Vec<CallPerson> = names
        .iter()
        .map(|(id, name)| CallPerson {
            id: id.clone(),
            name: name.clone(),
            speaking: speaking.is_speaking(id, now),
        })
        .collect();
    people.sort_by_cached_key(|p| (p.id != me, p.name.to_lowercase(), p.id.clone()));
    people
}

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

    let me = room.local_participant();
    let me_id = me.identity().to_string();
    let mut names: HashMap<String, String> = room
        .remote_participants()
        .into_values()
        .map(|p| (p.identity().to_string(), p.name()))
        .collect();
    names.insert(me_id.clone(), me.name());
    let mut speaking = Speaking::default();
    // O que a tela já tem; só manda de novo quando muda.
    let mut shown = Vec::new();
    let mut tick = interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            order = control.recv() => match order {
                Some(Control::Mute(true)) => {
                    mic.mute();
                    speaking.forget(&me_id);
                }
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
                Some(RoomEvent::ParticipantConnected(p)) => {
                    names.insert(p.identity().to_string(), p.name());
                }
                Some(RoomEvent::ParticipantDisconnected(p)) => {
                    let id = p.identity().to_string();
                    names.remove(&id);
                    speaking.forget(&id);
                }
                // O evento traz só quem mudou desde o último aviso; quem
                // continua falando não vem nele. O estado de cada pessoa fica
                // guardado no próprio participante (`is_speaking`).
                Some(RoomEvent::ActiveSpeakersChanged { .. }) => {
                    let mut active: HashSet<String> = room
                        .remote_participants()
                        .into_values()
                        .filter(|p| p.is_speaking())
                        .map(|p| p.identity().to_string())
                        .collect();
                    if me.is_speaking() {
                        active.insert(me_id.clone());
                    }
                    speaking.update(active, Instant::now());
                }
                // O áudio de quem entra é tocado sozinho; o resto não interessa aqui.
                Some(_) => {}
            },
            // Também confere aqui quem saiu ou mudou acima: o envio fica num
            // lugar só, no máximo dez vezes por segundo.
            _ = tick.tick() => {}
        }
        let now_people = people(&me_id, &names, &speaking, Instant::now());
        if now_people != shown {
            shown = now_people;
            let _ = net.send(Command::CallPeople {
                conversation_id,
                people: shown.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn pausa_curta_nao_apaga_o_destaque() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["ana"]), t0);
        assert!(speaking.is_speaking("ana", t0));
        // O LiveKit diz que ela parou (pausa entre palavras)...
        speaking.update(set(&[]), t0 + Duration::from_millis(400));
        assert!(speaking.is_speaking("ana", t0 + Duration::from_millis(600)));
        // ...e que voltou antes do HOLD vencer: o destaque nunca apagou.
        speaking.update(set(&["ana"]), t0 + Duration::from_millis(800));
        assert!(speaking.is_speaking("ana", t0 + Duration::from_secs(5)));
    }

    #[test]
    fn destaque_some_depois_do_hold() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["ana", "bia"]), t0);
        speaking.update(set(&["bia"]), t0 + Duration::from_secs(3));
        let later = t0 + Duration::from_secs(3) + HOLD;
        assert!(!speaking.is_speaking("ana", later));
        assert!(speaking.is_speaking("bia", later));
    }

    #[test]
    fn mutar_tira_o_destaque_na_hora() {
        let t0 = Instant::now();
        let mut speaking = Speaking::default();
        speaking.update(set(&["eu"]), t0);
        speaking.forget("eu");
        assert!(!speaking.is_speaking("eu", t0));
    }

    #[test]
    fn eu_primeiro_e_ordem_nao_muda_ao_falar() {
        let t0 = Instant::now();
        let names: HashMap<String, String> = [("1", "zeca"), ("2", "Bia"), ("3", "ana")]
            .into_iter()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect();
        let mut speaking = Speaking::default();
        let order = |s: &Speaking| -> Vec<String> {
            people("1", &names, s, t0)
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        speaking.update(set(&["2"]), t0);
        assert_eq!(order(&speaking), ["zeca", "ana", "Bia"]);
        assert!(people("1", &names, &speaking, t0)[2].speaking);
    }
}
