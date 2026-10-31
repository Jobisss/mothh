//! Entrega do MLS (F2-08): pacotes de chaves, commits e welcomes.
//!
//! O servidor não entende nada do que passa por aqui: guarda e repassa bytes
//! cifrados. O que ele faz é pôr ordem. Cada conversa tem uma época; um
//! commit só é aceito se foi feito na época atual, e aí a época sobe. Dois
//! membros mudando o grupo ao mesmo tempo: o primeiro ganha, o segundo recebe
//! `CommitRejected` e tenta de novo depois de aplicar o do primeiro.
//!
//! Quem deve estar no grupo continua sendo decidido aqui (`conversation_members`).
//! A diferença entre isso e o grupo MLS (`mls_members`) vai no `GroupState`, e
//! um membro online (o `committer`) faz a mudança que falta.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use protocol::{
    ConversationId, EpochCommit, KeyPackageOf, MAX_HANDSHAKE_BYTES, MAX_KEY_PACKAGE_BYTES,
    MAX_KEY_PACKAGES, Sealed, ServerMessage, UserId,
};
use uuid::Uuid;

use crate::state::{AppState, db_error};

/// Quantos commits um `FetchCommits` devolve de uma vez. Quem ficou mais
/// atrás pede de novo a partir do último.
const COMMITS_PAGE: i64 = 500;

/// Quantas conexões cada pessoa tem abertas agora. Serve para escolher o
/// `committer` entre quem está online.
#[derive(Debug, Default)]
pub struct Online {
    connections: Mutex<HashMap<UserId, usize>>,
}

impl Online {
    pub(crate) fn enter(&self, user: UserId) {
        *self.connections.lock().unwrap().entry(user).or_default() += 1;
    }

    pub(crate) fn leave(&self, user: UserId) {
        let mut connections = self.connections.lock().unwrap();
        if let Some(count) = connections.get_mut(&user) {
            *count -= 1;
            if *count == 0 {
                connections.remove(&user);
            }
        }
    }

    fn contains(&self, user: UserId) -> bool {
        self.connections.lock().unwrap().contains_key(&user)
    }
}

/// Resposta a um `Commit`.
fn rejected(conversation_id: ConversationId, epoch: i64) -> ServerMessage {
    ServerMessage::CommitRejected {
        conversation_id,
        epoch: epoch.max(0) as u64,
    }
}

impl AppState {
    /// Quantos pacotes de chaves do usuário ainda estão guardados.
    pub async fn key_package_stock(&self, user: UserId) -> Result<ServerMessage, String> {
        Ok(ServerMessage::KeyPackageStock {
            count: self.count_key_packages(user).await?,
        })
    }

    async fn count_key_packages(&self, user: UserId) -> Result<u32, String> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM key_packages WHERE user_id = $1")
                .bind(user.0)
                .fetch_one(&self.db)
                .await
                .map_err(db_error)?;
        Ok(count.clamp(0, u32::MAX as i64) as u32)
    }

    /// Guarda pacotes novos. Quem estava esperando um pacote para entrar num
    /// grupo agora pode ser posto: os grupos dele são anunciados de novo.
    pub async fn upload_key_packages(
        &self,
        user: UserId,
        packages: &[Sealed],
    ) -> Result<ServerMessage, String> {
        if packages
            .iter()
            .any(|p| p.0.is_empty() || p.0.len() > MAX_KEY_PACKAGE_BYTES)
        {
            return Err("Pacote de chaves inválido.".into());
        }
        let count = self.count_key_packages(user).await?;
        if count as usize + packages.len() > MAX_KEY_PACKAGES as usize {
            return Err("Pacotes de chaves demais.".into());
        }
        for package in packages {
            sqlx::query("INSERT INTO key_packages (user_id, data) VALUES ($1, $2)")
                .bind(user.0)
                .bind(&package.0)
                .execute(&self.db)
                .await
                .map_err(db_error)?;
        }
        self.announce_groups_of(user).await?;
        self.key_package_stock(user).await
    }

    /// Entrega um pacote de chaves de cada pessoa pedida, apagando-o. Só um
    /// membro do grupo MLS pode pedir, e só de quem está na conversa.
    pub async fn claim_key_packages(
        &self,
        by: UserId,
        conversation_id: ConversationId,
        users: &[UserId],
    ) -> Result<ServerMessage, String> {
        if !self.is_mls_member(by, conversation_id).await? {
            return Err("Você ainda não está no grupo cifrado desta conversa.".into());
        }
        let mut packages = Vec::new();
        for &user in users.iter().collect::<HashSet<_>>() {
            if !self.is_member(user, conversation_id).await? {
                continue;
            }
            let claimed: Option<(Vec<u8>,)> = sqlx::query_as(
                "DELETE FROM key_packages WHERE id = (
                     SELECT id FROM key_packages WHERE user_id = $1
                     ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED
                 ) RETURNING data",
            )
            .bind(user.0)
            .fetch_optional(&self.db)
            .await
            .map_err(db_error)?;
            if let Some((data,)) = claimed {
                packages.push(KeyPackageOf {
                    user_id: user,
                    package: Sealed(data),
                });
                // O dono do pacote fica sabendo, para repor.
                let stock = self.key_package_stock(user).await?;
                self.emit(vec![user], stock);
            }
        }
        Ok(ServerMessage::KeyPackages {
            conversation_id,
            packages,
        })
    }

    /// Aceita ou recusa uma mudança no grupo MLS. Ver `ClientMessage::Commit`.
    #[allow(clippy::too_many_arguments)]
    pub async fn commit(
        &self,
        by: UserId,
        conversation_id: ConversationId,
        epoch: u64,
        commit: Option<Sealed>,
        welcome: Option<Sealed>,
        added: Vec<UserId>,
        removed: Vec<UserId>,
    ) -> Result<ServerMessage, String> {
        let too_big =
            |s: &Option<Sealed>| s.as_ref().is_some_and(|s| s.0.len() > MAX_HANDSHAKE_BYTES);
        if too_big(&commit) || too_big(&welcome) {
            return Err("Mudança no grupo grande demais.".into());
        }
        if commit.is_none() && (!added.is_empty() || !removed.is_empty() || welcome.is_some()) {
            return Err("Mudança no grupo sem commit.".into());
        }
        if !added.is_empty() && welcome.is_none() {
            return Err("Faltou o convite de quem entrou.".into());
        }
        let Ok(epoch) = i64::try_from(epoch) else {
            return Err("Época inválida.".into());
        };

        let mut tx = self.db.begin().await.map_err(db_error)?;
        // Trava a conversa: dois commits ao mesmo tempo passam um de cada vez.
        let current: Option<(i64,)> =
            sqlx::query_as("SELECT mls_epoch FROM conversations WHERE id = $1 FOR UPDATE")
                .bind(conversation_id.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
        let Some((current,)) = current else {
            return Err("Essa conversa não existe.".into());
        };
        let (member,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (
                 SELECT 1 FROM conversation_members WHERE conversation_id = $1 AND user_id = $2
             )",
        )
        .bind(conversation_id.0)
        .bind(by.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if !member {
            return Err("Essa conversa não existe.".into());
        }
        let mls: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM mls_members WHERE conversation_id = $1")
                .bind(conversation_id.0)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
        let mls: HashSet<UserId> = mls.into_iter().map(|(id,)| UserId(id)).collect();

        let Some(commit) = commit else {
            // Só registra quem criou o grupo, se ninguém criou ainda.
            if !mls.is_empty() || epoch != current {
                return Ok(rejected(conversation_id, current));
            }
            sqlx::query("INSERT INTO mls_members (conversation_id, user_id) VALUES ($1, $2)")
                .bind(conversation_id.0)
                .bind(by.0)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
            self.announce_group(conversation_id).await?;
            return Ok(ServerMessage::CommitAccepted {
                conversation_id,
                epoch: current as u64,
            });
        };

        if !mls.contains(&by) || epoch != current {
            return Ok(rejected(conversation_id, current));
        }
        for user in &added {
            let (member,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (
                     SELECT 1 FROM conversation_members WHERE conversation_id = $1 AND user_id = $2
                 )",
            )
            .bind(conversation_id.0)
            .bind(user.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;
            if !member {
                return Err("Só dá para pôr no grupo quem está na conversa.".into());
            }
        }

        sqlx::query("INSERT INTO mls_commits (conversation_id, epoch, data) VALUES ($1, $2, $3)")
            .bind(conversation_id.0)
            .bind(epoch)
            .bind(&commit.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        let next = epoch + 1;
        sqlx::query("UPDATE conversations SET mls_epoch = $2 WHERE id = $1")
            .bind(conversation_id.0)
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        for user in &removed {
            for table in ["mls_members", "mls_welcomes"] {
                sqlx::query(&format!(
                    "DELETE FROM {table} WHERE conversation_id = $1 AND user_id = $2"
                ))
                .bind(conversation_id.0)
                .bind(user.0)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            }
        }
        for user in &added {
            sqlx::query(
                "INSERT INTO mls_members (conversation_id, user_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(conversation_id.0)
            .bind(user.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            sqlx::query(
                "INSERT INTO mls_welcomes (conversation_id, user_id, epoch, data)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (conversation_id, user_id)
                 DO UPDATE SET epoch = EXCLUDED.epoch, data = EXCLUDED.data",
            )
            .bind(conversation_id.0)
            .bind(user.0)
            .bind(next)
            .bind(&welcome.as_ref().expect("conferido acima").0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;

        // Quem já estava aplica o commit; quem entrou usa o welcome.
        let others: Vec<UserId> = mls
            .into_iter()
            .filter(|u| *u != by && !removed.contains(u) && !added.contains(u))
            .collect();
        self.emit(
            others,
            ServerMessage::Commit {
                conversation_id,
                commit: EpochCommit {
                    epoch: epoch as u64,
                    commit,
                },
            },
        );
        if let Some(welcome) = welcome {
            self.emit(
                added,
                ServerMessage::GroupWelcome {
                    conversation_id,
                    epoch: next as u64,
                    welcome,
                },
            );
        }
        self.announce_group(conversation_id).await?;
        Ok(ServerMessage::CommitAccepted {
            conversation_id,
            epoch: next as u64,
        })
    }

    /// Os commits a partir da época `from`, para quem ficou para trás.
    pub async fn fetch_commits(
        &self,
        user: UserId,
        conversation_id: ConversationId,
        from: u64,
    ) -> Result<ServerMessage, String> {
        if !self.is_mls_member(user, conversation_id).await? {
            return Err("Você ainda não está no grupo cifrado desta conversa.".into());
        }
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT epoch, data FROM mls_commits
             WHERE conversation_id = $1 AND epoch >= $2
             ORDER BY epoch LIMIT $3",
        )
        .bind(conversation_id.0)
        .bind(i64::try_from(from).unwrap_or(i64::MAX))
        .bind(COMMITS_PAGE)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        Ok(ServerMessage::Commits {
            conversation_id,
            commits: rows
                .into_iter()
                .map(|(epoch, data)| EpochCommit {
                    epoch: epoch as u64,
                    commit: Sealed(data),
                })
                .collect(),
        })
    }

    /// O aparelho perdeu o grupo: sai da lista do MLS, e o `committer` tira a
    /// folha velha e põe a nova numa mudança só.
    pub async fn rejoin_group(
        &self,
        user: UserId,
        conversation_id: ConversationId,
    ) -> Result<(), String> {
        if !self.is_member(user, conversation_id).await? {
            return Err("Essa conversa não existe.".into());
        }
        for table in ["mls_members", "mls_welcomes"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE conversation_id = $1 AND user_id = $2"
            ))
            .bind(conversation_id.0)
            .bind(user.0)
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        }
        self.announce_group(conversation_id).await
    }

    /// Os welcomes guardados para o usuário, para mandar ao entrar.
    pub async fn pending_welcomes(&self, user: UserId) -> Result<Vec<ServerMessage>, String> {
        let rows: Vec<(Uuid, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT w.conversation_id, w.epoch, w.data FROM mls_welcomes w
             JOIN mls_members m ON m.conversation_id = w.conversation_id AND m.user_id = w.user_id
             WHERE w.user_id = $1",
        )
        .bind(user.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .map(|(id, epoch, data)| ServerMessage::GroupWelcome {
                conversation_id: ConversationId(id),
                epoch: epoch.max(0) as u64,
                welcome: Sealed(data),
            })
            .collect())
    }

    /// Manda o `GroupState` de cada conversa do usuário para todos os membros
    /// dela. Chamado quando ele entra, sai ou publica pacotes: o `committer`
    /// e o que dá para fazer podem ter mudado.
    pub(crate) async fn announce_groups_of(&self, user: UserId) -> Result<(), String> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT conversation_id FROM conversation_members WHERE user_id = $1")
                .bind(user.0)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        for (id,) in rows {
            self.announce_group(ConversationId(id)).await?;
        }
        Ok(())
    }

    /// Manda o `GroupState` da conversa para quem está nela.
    pub(crate) async fn announce_group(&self, id: ConversationId) -> Result<(), String> {
        let recipients = self.member_ids(id).await?;
        if recipients.is_empty() {
            return Ok(());
        }
        let state = self.group_state(id, &recipients).await?;
        self.emit(recipients, state);
        Ok(())
    }

    async fn group_state(
        &self,
        id: ConversationId,
        members: &[UserId],
    ) -> Result<ServerMessage, String> {
        let mut mls = self.mls_members(id).await?;
        // Ninguém do grupo MLS continua na conversa (todos saíram antes de
        // pôr os novos): ninguém mais consegue mudar aquele grupo. Recomeça.
        if !mls.is_empty() && !mls.iter().any(|u| members.contains(u)) {
            self.reset_group(id).await?;
            mls.clear();
        }
        let (epoch,): (i64,) = sqlx::query_as("SELECT mls_epoch FROM conversations WHERE id = $1")
            .bind(id.0)
            .fetch_one(&self.db)
            .await
            .map_err(db_error)?;

        // Só entra na lista quem tem pacote: sem pacote não dá para pôr.
        let with_packages: Vec<(Uuid,)> =
            sqlx::query_as("SELECT DISTINCT user_id FROM key_packages WHERE user_id = ANY($1)")
                .bind(members.iter().map(|u| u.0).collect::<Vec<_>>())
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        let with_packages: HashSet<UserId> =
            with_packages.into_iter().map(|(id,)| UserId(id)).collect();
        let mut pending_add: Vec<UserId> = members
            .iter()
            .filter(|u| !mls.contains(u) && with_packages.contains(u))
            .copied()
            .collect();
        let mut pending_remove: Vec<UserId> = mls
            .iter()
            .filter(|u| !members.contains(u))
            .copied()
            .collect();
        pending_add.sort_by_key(|u| u.0);
        pending_remove.sort_by_key(|u| u.0);

        // Quem pode mudar o grupo: quem já está nele, ou, se ainda não
        // existe, quem da conversa tem pacote de chaves (um app sem MLS, de
        // versão velha, nunca criaria o grupo e travaria a conversa). Entre
        // esses, o online de menor id, para todos escolherem o mesmo.
        let candidates: Vec<UserId> = if mls.is_empty() {
            members
                .iter()
                .filter(|u| with_packages.contains(u))
                .copied()
                .collect()
        } else {
            mls.iter()
                .filter(|u| members.contains(u))
                .copied()
                .collect()
        };
        let committer = candidates
            .into_iter()
            .filter(|u| self.online.contains(*u))
            .min_by_key(|u| u.0);

        mls.sort_by_key(|u| u.0);
        Ok(ServerMessage::GroupState {
            conversation_id: id,
            epoch: epoch.max(0) as u64,
            members: mls,
            pending_add,
            pending_remove,
            committer,
        })
    }

    async fn reset_group(&self, id: ConversationId) -> Result<(), String> {
        let mut tx = self.db.begin().await.map_err(db_error)?;
        for table in ["mls_members", "mls_commits", "mls_welcomes"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE conversation_id = $1"))
                .bind(id.0)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        sqlx::query("UPDATE conversations SET mls_epoch = 0 WHERE id = $1")
            .bind(id.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    async fn mls_members(&self, id: ConversationId) -> Result<Vec<UserId>, String> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT user_id FROM mls_members WHERE conversation_id = $1")
                .bind(id.0)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        Ok(rows.into_iter().map(|(id,)| UserId(id)).collect())
    }

    async fn is_mls_member(&self, user: UserId, id: ConversationId) -> Result<bool, String> {
        Ok(self.mls_members(id).await?.contains(&user))
    }
}
