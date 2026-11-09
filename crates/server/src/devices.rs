//! Os aparelhos de cada conta.
//!
//! Cada instalação do app que entra numa conta vira um aparelho (diz qual é
//! no `Hello`, antes de entrar). O primeiro aparelho da conta entra
//! aprovado; os seguintes esperam alguém aprovar num aparelho que já está
//! nela, conferindo o código que aparece nos dois. Sem o outro aparelho
//! (perdeu, formatou), dá para entrar desconectando todos os outros: quem
//! roubou a senha até entra, mas o dono percebe na hora.
//!
//! Enquanto espera, o aparelho não vê conversa nenhuma e não é posto nos
//! grupos MLS. Aprovado, os grupos das conversas da pessoa o põem (o
//! `GroupState` passa a listá-lo em `pending_add`) e ele lê dali para frente.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use protocol::{DeviceId, DeviceInfo, MAX_DEVICE_NAME_CHARS, ServerMessage, UserId};
use uuid::Uuid;

use crate::auth::{SESSION_EXPIRED, Session};
use crate::state::{AppState, db_error};

pub const NOT_APPROVED: &str =
    "Este aparelho ainda espera aprovação. Aprove no outro computador em que você já entrou.";

/// O nome que aparece na lista: sem quebra de linha e com tamanho limitado.
fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "computador".into()
    } else {
        name.into()
    }
}

fn new_code() -> String {
    format!("{:06}", OsRng.next_u32() % 1_000_000)
}

impl AppState {
    /// Liga a sessão ao aparelho que a conexão disse ser (`Hello`), criando
    /// o aparelho na primeira vez. Sem `Hello` (app antigo) o aparelho é o
    /// de instalação vazia, com o id da própria conta: assim o app antigo
    /// continua achando que o membro do grupo é a conta.
    pub async fn attach_device(
        &self,
        session: &mut Session,
        hello: Option<(Uuid, String)>,
    ) -> Result<(), String> {
        let user = session.user_id;
        let (install, name) = match hello {
            Some((install, name)) => (install, clean_name(&name)),
            None => (Uuid::nil(), "app antigo".to_string()),
        };
        let mut found: Option<(Uuid, bool)> =
            sqlx::query_as("SELECT id, approved FROM devices WHERE user_id = $1 AND install = $2")
                .bind(user.0)
                .bind(install)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        // O aparelho de cada sessão, se ela já foi ligada a um.
        let (bound,): (Option<Uuid>,) =
            sqlx::query_as("SELECT device_id FROM sessions WHERE token_hash = $1")
                .bind(crate::auth::token_hash(&session.token))
                .fetch_one(&self.db)
                .await
                .map_err(db_error)?;
        // O app antigo se atualizou: a sessão era do aparelho sem instalação
        // (o "da conta"), e agora ele diz qual é. Continua o mesmo aparelho,
        // com os mesmos grupos, em vez de virar um novo esperando aprovação.
        if found.is_none()
            && !install.is_nil()
            && let Some(bound) = bound
        {
            found = sqlx::query_as(
                "UPDATE devices SET install = $2 WHERE id = $1 AND install = $3
                 RETURNING id, approved",
            )
            .bind(bound)
            .bind(install)
            .bind(Uuid::nil())
            .fetch_optional(&self.db)
            .await
            .map_err(db_error)?;
        }
        let mut created = false;
        let (device, approved) = match found {
            Some(found) => found,
            None => {
                let id = if install.is_nil() {
                    user.0
                } else {
                    Uuid::new_v4()
                };
                // O primeiro aparelho da conta entra direto; os outros esperam.
                let (others,): (i64,) =
                    sqlx::query_as("SELECT count(*) FROM devices WHERE user_id = $1 AND approved")
                        .bind(user.0)
                        .fetch_one(&self.db)
                        .await
                        .map_err(db_error)?;
                let approved = others == 0;
                let code = (!approved).then(new_code);
                let inserted: Option<(Uuid, bool)> = sqlx::query_as(
                    "INSERT INTO devices (id, user_id, install, name, approved, code)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT DO NOTHING
                     RETURNING id, approved",
                )
                .bind(id)
                .bind(user.0)
                .bind(install)
                .bind(&name)
                .bind(approved)
                .bind(&code)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
                match inserted {
                    Some(row) => {
                        created = true;
                        row
                    }
                    // Outra conexão do mesmo aparelho criou ao mesmo tempo.
                    None => sqlx::query_as(
                        "SELECT id, approved FROM devices WHERE user_id = $1 AND install = $2",
                    )
                    .bind(user.0)
                    .bind(install)
                    .fetch_one(&self.db)
                    .await
                    .map_err(db_error)?,
                }
            }
        };
        // Uma sessão é de um aparelho só: o token copiado para outra
        // instalação não serve lá.
        if bound.is_some_and(|bound| bound != device) {
            return Err(SESSION_EXPIRED.into());
        }
        sqlx::query("UPDATE sessions SET device_id = $2 WHERE token_hash = $1")
            .bind(crate::auth::token_hash(&session.token))
            .bind(device)
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        sqlx::query("UPDATE devices SET name = $2, last_seen = now() WHERE id = $1")
            .bind(device)
            .bind(&name)
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        session.device = DeviceId(device);
        session.approved = approved;
        if created {
            tracing::info!(username = %session.username, aparelho = %name, aprovado = approved, "aparelho novo");
            if !approved {
                self.announce_devices(user).await?;
            }
        }
        Ok(())
    }

    /// O código que o aparelho mostra enquanto espera aprovação.
    pub async fn approval_code(&self, device: DeviceId) -> Result<Option<String>, String> {
        let found: Option<(Option<String>,)> =
            sqlx::query_as("SELECT code FROM devices WHERE id = $1 AND NOT approved")
                .bind(device.0)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        Ok(found.and_then(|(code,)| code))
    }

    /// Marca que o aparelho esteve conectado agora (ao sair).
    pub async fn device_seen(&self, device: DeviceId) {
        let _ = sqlx::query("UPDATE devices SET last_seen = now() WHERE id = $1")
            .bind(device.0)
            .execute(&self.db)
            .await;
    }

    /// Os aparelhos da conta, do mais antigo para o mais novo.
    pub async fn devices(&self, user: UserId) -> Result<ServerMessage, String> {
        let rows: Vec<(Uuid, String, bool, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT id, name, approved, code,
                    (extract(epoch FROM created_at) * 1000)::BIGINT,
                    (extract(epoch FROM last_seen) * 1000)::BIGINT
             FROM devices WHERE user_id = $1 ORDER BY created_at, id",
        )
        .bind(user.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        Ok(ServerMessage::Devices {
            devices: rows
                .into_iter()
                .map(
                    |(id, name, approved, code, created_at_ms, last_seen_ms)| DeviceInfo {
                        id: DeviceId(id),
                        name,
                        approved,
                        code,
                        created_at_ms,
                        last_seen_ms,
                        online: self.online.contains_device(DeviceId(id)),
                    },
                )
                .collect(),
        })
    }

    /// Manda a lista de aparelhos de novo a todas as conexões da conta.
    pub(crate) async fn announce_devices(&self, user: UserId) -> Result<(), String> {
        let devices = self.devices(user).await?;
        self.emit(vec![user], devices);
        Ok(())
    }

    /// Um aparelho aprovado aprova outro da mesma conta.
    pub async fn approve_device(&self, session: &Session, target: DeviceId) -> Result<(), String> {
        if !session.approved {
            return Err(NOT_APPROVED.into());
        }
        let updated = sqlx::query(
            "UPDATE devices SET approved = true, code = NULL
             WHERE id = $1 AND user_id = $2 AND NOT approved",
        )
        .bind(target.0)
        .bind(session.user_id.0)
        .execute(&self.db)
        .await
        .map_err(db_error)?;
        if updated.rows_affected() == 0 {
            return Err("Esse aparelho não está esperando aprovação.".into());
        }
        tracing::info!(username = %session.username, "aparelho aprovado");
        self.emit_devices(vec![target], ServerMessage::DeviceApproved);
        self.announce_devices(session.user_id).await?;
        self.announce_groups_of(session.user_id).await
    }

    /// Um aparelho aprovado tira outro da conta: as sessões dele caem e os
    /// grupos o tiram (fica em `pending_remove` até alguém fazer o commit).
    pub async fn remove_device(&self, session: &Session, target: DeviceId) -> Result<(), String> {
        if !session.approved {
            return Err(NOT_APPROVED.into());
        }
        if target == session.device {
            return Err("Para tirar este aparelho, use \"sair da conta\".".into());
        }
        let deleted = sqlx::query("DELETE FROM devices WHERE id = $1 AND user_id = $2")
            .bind(target.0)
            .bind(session.user_id.0)
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        if deleted.rows_affected() == 0 {
            return Err("Esse aparelho não está na sua conta.".into());
        }
        tracing::info!(username = %session.username, "aparelho removido");
        self.emit_devices(vec![target], ServerMessage::DeviceRemoved);
        self.announce_devices(session.user_id).await?;
        self.announce_groups_of(session.user_id).await
    }

    /// O aparelho que espera aprovação entra sozinho: todos os outros
    /// aparelhos da conta saem.
    pub async fn take_over_account(&self, session: &mut Session) -> Result<(), String> {
        if session.approved {
            return Err("Este aparelho já está aprovado.".into());
        }
        let mut tx = self.db.begin().await.map_err(db_error)?;
        let removed: Vec<(Uuid,)> =
            sqlx::query_as("DELETE FROM devices WHERE user_id = $1 AND id <> $2 RETURNING id")
                .bind(session.user_id.0)
                .bind(session.device.0)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
        sqlx::query("UPDATE devices SET approved = true, code = NULL WHERE id = $1")
            .bind(session.device.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        tracing::warn!(
            username = %session.username,
            removidos = removed.len(),
            "entrou sem aprovação: os outros aparelhos saíram"
        );
        self.emit_devices(
            removed.into_iter().map(|(id,)| DeviceId(id)).collect(),
            ServerMessage::DeviceRemoved,
        );
        self.emit_devices(vec![session.device], ServerMessage::DeviceApproved);
        self.announce_groups_of(session.user_id).await
    }
}
