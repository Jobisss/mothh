//! Atividade Spotify transitória, compartilhada com os mesmos leitores do perfil.
use crate::state::AppState;
use protocol::{ListeningActivity, ServerMessage, UserId};
use std::time::{Duration, Instant};

impl AppState {
    pub(crate) fn listening_activity(&self, user: UserId) -> Option<ListeningActivity> {
        self.listening
            .lock()
            .unwrap()
            .get(&user)
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(45))
            .map(|(at, activity)| {
                let mut activity = activity.clone();
                activity.position_ms = (activity.position_ms + at.elapsed().as_millis() as u64)
                    .min(activity.duration_ms);
                activity
            })
    }

    pub async fn set_listening_activity(
        &self,
        user: UserId,
        activity: Option<ListeningActivity>,
    ) -> Result<(), String> {
        if activity.as_ref().is_some_and(|a| {
            a.title.trim().is_empty()
                || a.title.len() > 512
                || a.artist.len() > 512
                || a.title
                    .chars()
                    .chain(a.artist.chars())
                    .any(char::is_control)
                || a.duration_ms > 86_400_000
                || a.position_ms > a.duration_ms
        }) {
            return Err("Atividade musical inválida.".into());
        }
        if activity.is_none() && !self.listening.lock().unwrap().contains_key(&user) {
            return Ok(());
        }
        let recipients = self.people_sharing_with(user).await?;
        {
            let mut entries = self.listening.lock().unwrap();
            entries.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(45));
            match activity {
                Some(activity) => {
                    entries.insert(user, (Instant::now(), activity));
                }
                None => {
                    entries.remove(&user);
                }
            }
        }
        // -1 invalida somente o cache; não altera a versão persistida do perfil.
        self.emit(
            recipients,
            ServerMessage::ProfileChanged {
                user_id: user,
                version: -1,
            },
        );
        Ok(())
    }
}
