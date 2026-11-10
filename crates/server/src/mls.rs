//! Entrega do MLS (F2-08): pacotes de chaves, commits e welcomes.
//!
//! O servidor não entende nada do que passa por aqui: guarda e repassa bytes
//! cifrados. O que ele faz é pôr ordem. Cada conversa tem uma época; um
//! commit só é aceito se foi feito na época atual, e aí a época sobe. Dois
//! membros mudando o grupo ao mesmo tempo: o primeiro ganha, o segundo recebe
//! `CommitRejected` e tenta de novo depois de aplicar o do primeiro.
//!
//! Quem deve estar no grupo continua sendo decidido aqui: os aparelhos
//! aprovados de quem está na conversa (`conversation_members` e `devices`).
//! Cada aparelho é um membro do MLS, então o que um aparelho manda os outros
//! da mesma conta também leem. A diferença entre isso e o grupo MLS
//! (`mls_members`) vai no `GroupState`, e um aparelho online (o `committer`)
//! faz a mudança que falta.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

use protocol::{
    ConversationId, DeviceId, EpochCommit, KeyPackageOf, MAX_HANDSHAKE_BYTES,
    MAX_KEY_PACKAGE_BYTES, MAX_KEY_PACKAGES, Sealed, ServerMessage, UserId,
};
use uuid::Uuid;

use crate::state::{AppState, db_error};

/// Quantos commits um `FetchCommits` devolve de uma vez. Quem ficou mais
/// atrás pede de novo a partir do último.
const COMMITS_PAGE: i64 = 500;

/// Quem foi escolhido para mudar o grupo de uma conversa, em que época e
/// desde quando. Se não agir dentro do prazo, a vez passa ao próximo online:
/// um app travado (ou uma conexão que ainda não caiu) não segura a conversa.
#[derive(Debug, Clone, Copy)]
pub struct Stall {
    pub epoch: i64,
    pub committer: DeviceId,
    pub since: Instant,
}

/// Quantas conexões cada pessoa e cada aparelho têm abertas agora. Serve
/// para escolher o `committer` entre os aparelhos online.
#[derive(Debug, Default)]
pub struct Online {
    connections: Mutex<(HashMap<UserId, usize>, HashMap<DeviceId, usize>)>,
}

fn decrement<K: std::hash::Hash + Eq>(map: &mut HashMap<K, usize>, key: K) {
    if let Some(count) = map.get_mut(&key) {
        *count -= 1;
        if *count == 0 {
            map.remove(&key);
        }
    }
}

impl Online {
    pub(crate) fn enter(&self, user: UserId, device: DeviceId) {
        let mut connections = self.connections.lock().unwrap();
        *connections.0.entry(user).or_default() += 1;
        *connections.1.entry(device).or_default() += 1;
    }

    pub(crate) fn leave(&self, user: UserId, device: DeviceId) {
        let mut connections = self.connections.lock().unwrap();
        decrement(&mut connections.0, user);
        decrement(&mut connections.1, device);
    }

    pub(crate) fn contains(&self, user: UserId) -> bool {
        self.connections.lock().unwrap().0.contains_key(&user)
    }

    pub(crate) fn contains_device(&self, device: DeviceId) -> bool {
        self.connections.lock().unwrap().1.contains_key(&device)
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
    /// Quantos pacotes de chaves do aparelho ainda estão guardados.
    pub async fn key_package_stock(&self, device: DeviceId) -> Result<ServerMessage, String> {
        Ok(ServerMessage::KeyPackageStock {
            count: self.count_key_packages(device).await?,
        })
    }

    async fn count_key_packages(&self, device: DeviceId) -> Result<u32, String> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM key_packages WHERE device_id = $1")
                .bind(device.0)
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
        device: DeviceId,
        packages: &[Sealed],
        replace: bool,
    ) -> Result<ServerMessage, String> {
        if packages
            .iter()
            .any(|p| p.0.is_empty() || p.0.len() > MAX_KEY_PACKAGE_BYTES)
        {
            return Err("Pacote de chaves inválido.".into());
        }
        if replace {
            sqlx::query("DELETE FROM key_packages WHERE device_id = $1")
                .bind(device.0)
                .execute(&self.db)
                .await
                .map_err(db_error)?;
        }
        let count = self.count_key_packages(device).await?;
        if count as usize + packages.len() > MAX_KEY_PACKAGES as usize {
            return Err("Pacotes de chaves demais.".into());
        }
        for package in packages {
            sqlx::query("INSERT INTO key_packages (device_id, data) VALUES ($1, $2)")
                .bind(device.0)
                .bind(&package.0)
                .execute(&self.db)
                .await
                .map_err(db_error)?;
        }
        self.announce_groups_of(user).await?;
        self.key_package_stock(device).await
    }

    /// Entrega um pacote de chaves de cada aparelho pedido, apagando-o. Só
    /// um membro do grupo MLS pode pedir, e só de aparelhos aprovados de
    /// quem está na conversa.
    pub async fn claim_key_packages(
        &self,
        by: DeviceId,
        conversation_id: ConversationId,
        devices: &[DeviceId],
    ) -> Result<ServerMessage, String> {
        if !self.is_mls_member(by, conversation_id).await? {
            return Err("Você ainda não está no grupo cifrado desta conversa.".into());
        }
        let allowed: HashSet<DeviceId> = self
            .conversation_devices(conversation_id)
            .await?
            .into_iter()
            .collect();
        let mut packages = Vec::new();
        for &device in devices.iter().collect::<HashSet<_>>() {
            if !allowed.contains(&device) {
                continue;
            }
            let claimed: Option<(Vec<u8>,)> = sqlx::query_as(
                "DELETE FROM key_packages WHERE id = (
                     SELECT id FROM key_packages WHERE device_id = $1
                     ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED
                 ) RETURNING data",
            )
            .bind(device.0)
            .fetch_optional(&self.db)
            .await
            .map_err(db_error)?;
            if let Some((data,)) = claimed {
                packages.push(KeyPackageOf {
                    device_id: device,
                    package: Sealed(data),
                });
                // O dono do pacote fica sabendo, para repor.
                let stock = self.key_package_stock(device).await?;
                self.emit_devices(vec![device], stock);
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
        (by_user, by): (UserId, DeviceId),
        conversation_id: ConversationId,
        epoch: u64,
        commit: Option<Sealed>,
        welcome: Option<Sealed>,
        added: Vec<DeviceId>,
        removed: Vec<DeviceId>,
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
        .bind(by_user.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if !member {
            return Err("Essa conversa não existe.".into());
        }
        let mls: Vec<(Uuid,)> =
            sqlx::query_as("SELECT device_id FROM mls_members WHERE conversation_id = $1")
                .bind(conversation_id.0)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_error)?;
        let mls: HashSet<DeviceId> = mls.into_iter().map(|(id,)| DeviceId(id)).collect();

        let Some(commit) = commit else {
            // Só registra quem criou o grupo, se ninguém criou ainda.
            if !mls.is_empty() || epoch != current {
                return Ok(rejected(conversation_id, current));
            }
            sqlx::query("INSERT INTO mls_members (conversation_id, device_id) VALUES ($1, $2)")
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
        for device in &added {
            let (member,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (
                     SELECT 1 FROM devices d
                     JOIN conversation_members m ON m.user_id = d.user_id
                     WHERE m.conversation_id = $1 AND d.id = $2 AND d.approved
                 )",
            )
            .bind(conversation_id.0)
            .bind(device.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_error)?;
            if !member {
                return Err("Só dá para pôr no grupo aparelhos de quem está na conversa.".into());
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
        for device in &removed {
            for table in ["mls_members", "mls_welcomes"] {
                sqlx::query(&format!(
                    "DELETE FROM {table} WHERE conversation_id = $1 AND device_id = $2"
                ))
                .bind(conversation_id.0)
                .bind(device.0)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            }
        }
        for device in &added {
            sqlx::query(
                "INSERT INTO mls_members (conversation_id, device_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(conversation_id.0)
            .bind(device.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            sqlx::query(
                "INSERT INTO mls_welcomes (conversation_id, device_id, epoch, data)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (conversation_id, device_id)
                 DO UPDATE SET epoch = EXCLUDED.epoch, data = EXCLUDED.data",
            )
            .bind(conversation_id.0)
            .bind(device.0)
            .bind(next)
            .bind(&welcome.as_ref().expect("conferido acima").0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)?;

        // Quem já estava aplica o commit; quem entrou usa o welcome.
        let others: Vec<DeviceId> = mls
            .into_iter()
            .filter(|d| *d != by && !removed.contains(d) && !added.contains(d))
            .collect();
        self.emit_devices(
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
            self.emit_devices(
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
        device: DeviceId,
        conversation_id: ConversationId,
        from: u64,
    ) -> Result<ServerMessage, String> {
        if !self.is_mls_member(device, conversation_id).await? {
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
        (user, device): (UserId, DeviceId),
        conversation_id: ConversationId,
    ) -> Result<(), String> {
        if !self.is_member(user, conversation_id).await? {
            return Err("Essa conversa não existe.".into());
        }
        for table in ["mls_members", "mls_welcomes"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE conversation_id = $1 AND device_id = $2"
            ))
            .bind(conversation_id.0)
            .bind(device.0)
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        }
        self.announce_group(conversation_id).await
    }

    /// Os welcomes guardados para o aparelho, para mandar ao entrar.
    pub async fn pending_welcomes(&self, device: DeviceId) -> Result<Vec<ServerMessage>, String> {
        let rows: Vec<(Uuid, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT w.conversation_id, w.epoch, w.data FROM mls_welcomes w
             JOIN mls_members m
               ON m.conversation_id = w.conversation_id AND m.device_id = w.device_id
             WHERE w.device_id = $1",
        )
        .bind(device.0)
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
        let devices = self.devices_of(members).await?;
        // Nenhum aparelho do grupo MLS continua na conversa (todos saíram
        // antes de pôr os novos): ninguém mais consegue mudar aquele grupo.
        // Recomeça.
        if !mls.is_empty() && !mls.iter().any(|d| devices.contains(d)) {
            self.reset_group(id).await?;
            mls.clear();
        }
        let (mut epoch,): (i64,) =
            sqlx::query_as("SELECT mls_epoch FROM conversations WHERE id = $1")
                .bind(id.0)
                .fetch_one(&self.db)
                .await
                .map_err(db_error)?;
        // Grupo sem ninguém, mas numa época depois da criação: o único
        // aparelho saiu (pediu para entrar de novo, foi desconectado). A
        // criação só é aceita na época 0, então ninguém conseguia criar de
        // novo e a conversa ficava em "trocando chaves" para sempre.
        if mls.is_empty() && epoch > 0 {
            tracing::warn!(conversa = %id.0, epoca = epoch, "mls: grupo vazio depois da criação; recomeçando");
            self.reset_group(id).await?;
            epoch = 0;
        }

        // Só entra na lista quem tem pacote: sem pacote não dá para pôr.
        let with_packages: Vec<(Uuid,)> =
            sqlx::query_as("SELECT DISTINCT device_id FROM key_packages WHERE device_id = ANY($1)")
                .bind(devices.iter().map(|d| d.0).collect::<Vec<_>>())
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        let with_packages: HashSet<DeviceId> = with_packages
            .into_iter()
            .map(|(id,)| DeviceId(id))
            .collect();
        let mut pending_add: Vec<DeviceId> = devices
            .iter()
            .filter(|d| !mls.contains(d) && with_packages.contains(d))
            .copied()
            .collect();
        let mut pending_remove: Vec<DeviceId> = mls
            .iter()
            .filter(|d| !devices.contains(d))
            .copied()
            .collect();
        pending_add.sort();
        pending_remove.sort();

        // Quem pode mudar o grupo: quem já está nele, ou, se ainda não
        // existe, quem da conversa tem pacote de chaves (um app sem MLS, de
        // versão velha, nunca criaria o grupo e travaria a conversa). Entre
        // esses, o online de menor id, para todos escolherem o mesmo.
        let candidates: Vec<DeviceId> = if mls.is_empty() {
            devices
                .iter()
                .filter(|d| with_packages.contains(d))
                .copied()
                .collect()
        } else {
            mls.iter()
                .filter(|d| devices.contains(d))
                .copied()
                .collect()
        };
        let mut online: Vec<DeviceId> = candidates
            .into_iter()
            .filter(|d| self.online.contains_device(*d))
            .collect();
        online.sort();
        let pending = mls.is_empty() || !pending_add.is_empty() || !pending_remove.is_empty();
        let committer = self.pick_committer(id, epoch, pending, &online);
        if pending {
            self.log_stuck(
                id,
                epoch,
                &mls,
                &pending_add,
                &pending_remove,
                &online,
                committer,
            )
            .await;
        }

        mls.sort();
        Ok(ServerMessage::GroupState {
            conversation_id: id,
            epoch: epoch.max(0) as u64,
            members: mls,
            pending_add,
            pending_remove,
            committer,
        })
    }

    /// O online de menor id, a não ser que ele já tenha sido escolhido há
    /// mais de `stall_timeout` sem mudar nada: aí a vez passa ao próximo.
    /// Sem nada pendente, o grupo não precisa de ninguém e o prazo zera.
    fn pick_committer(
        &self,
        id: ConversationId,
        epoch: i64,
        pending: bool,
        online: &[DeviceId],
    ) -> Option<DeviceId> {
        let mut stalls = self.stalls.lock().unwrap();
        let Some(&first) = online.first() else {
            stalls.remove(&id);
            return None;
        };
        if !pending {
            stalls.remove(&id);
            return Some(first);
        }
        let now = Instant::now();
        match stalls.get_mut(&id) {
            Some(stall) if stall.epoch == epoch && online.contains(&stall.committer) => {
                if now.duration_since(stall.since) > self.stall_timeout && online.len() > 1 {
                    let at = online
                        .iter()
                        .position(|u| *u == stall.committer)
                        .unwrap_or(0);
                    stall.committer = online[(at + 1) % online.len()];
                    stall.since = now;
                    tracing::info!(conversation = %id.0, "quem mudaria o grupo MLS não agiu: a vez passou a outro");
                }
                Some(stall.committer)
            }
            _ => {
                stalls.insert(
                    id,
                    Stall {
                        epoch,
                        committer: first,
                        since: now,
                    },
                );
                Some(first)
            }
        }
    }

    /// Diz no log por que o grupo cifrado da conversa ainda não está pronto:
    /// quem espera entrar e quem pode pôr (ou ninguém online para isso, o
    /// caso de "trocando chaves" sem fim). No máximo uma vez por minuto.
    #[allow(clippy::too_many_arguments)]
    async fn log_stuck(
        &self,
        id: ConversationId,
        epoch: i64,
        mls: &[DeviceId],
        pending_add: &[DeviceId],
        pending_remove: &[DeviceId],
        online: &[DeviceId],
        committer: Option<DeviceId>,
    ) {
        {
            let mut logged = self.stuck_logged.lock().unwrap();
            let now = Instant::now();
            if logged
                .get(&id)
                .is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_secs(60))
            {
                return;
            }
            logged.insert(id, now);
        }
        let mut ids: Vec<Uuid> = mls
            .iter()
            .chain(pending_add)
            .chain(pending_remove)
            .map(|u| u.0)
            .collect();
        ids.extend(committer.map(|u| u.0));
        let names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT d.id, u.username || '/' || d.name FROM devices d
             JOIN users u ON u.id = d.user_id WHERE d.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
        let conversa: String =
            sqlx::query_as::<_, (String,)>("SELECT name FROM conversations WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&self.db)
                .await
                .ok()
                .flatten()
                .map(|(n,)| n)
                .unwrap_or_default();
        let list = |devices: &[DeviceId]| {
            devices
                .iter()
                .map(|u| names.get(&u.0).cloned().unwrap_or_else(|| u.0.to_string()))
                .collect::<Vec<_>>()
                .join(",")
        };
        let quem_muda = match committer {
            Some(u) => names.get(&u.0).cloned().unwrap_or_else(|| u.0.to_string()),
            None => "ninguém (nenhum membro do grupo cifrado online)".into(),
        };
        tracing::warn!(
            %conversa,
            id = %id.0,
            epoca = epoch,
            grupo = %list(mls),
            online_no_grupo = %list(online),
            esperando_entrar = %list(pending_add),
            esperando_sair = %list(pending_remove),
            %quem_muda,
            "mls: grupo cifrado parado"
        );
    }

    /// Um membro avisa que a troca de chaves parou: manda o `GroupState` de
    /// novo a todos (e, se for o caso, com outro `committer`).
    pub(crate) async fn nudge_group(&self, user: UserId, id: ConversationId) -> Result<(), String> {
        if !self.is_member(user, id).await? {
            return Err("Essa conversa não existe.".into());
        }
        self.announce_group(id).await
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

    async fn mls_members(&self, id: ConversationId) -> Result<Vec<DeviceId>, String> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT device_id FROM mls_members WHERE conversation_id = $1")
                .bind(id.0)
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        Ok(rows.into_iter().map(|(id,)| DeviceId(id)).collect())
    }

    async fn is_mls_member(&self, device: DeviceId, id: ConversationId) -> Result<bool, String> {
        Ok(self.mls_members(id).await?.contains(&device))
    }

    /// Os aparelhos aprovados de quem está na conversa: quem deve estar no
    /// grupo MLS dela.
    async fn conversation_devices(&self, id: ConversationId) -> Result<Vec<DeviceId>, String> {
        let members = self.member_ids(id).await?;
        self.devices_of(&members).await
    }

    async fn devices_of(&self, users: &[UserId]) -> Result<Vec<DeviceId>, String> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ANY($1) AND approved")
                .bind(users.iter().map(|u| u.0).collect::<Vec<_>>())
                .fetch_all(&self.db)
                .await
                .map_err(db_error)?;
        Ok(rows.into_iter().map(|(id,)| DeviceId(id)).collect())
    }
}
