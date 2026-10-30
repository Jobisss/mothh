//! Grupos criados pelas pessoas: criar, convidar, sair e remover.
//!
//! Quem cria é o dono. Qualquer membro convida; só o dono remove. Quando o
//! dono sai, o membro mais antigo vira o dono; quando o último sai, o grupo
//! some junto com as mensagens. A Sala do Grupo fica de fora de tudo isso.

use protocol::{
    ConversationId, ConversationSummary, MAX_GROUP_NAME_CHARS, Member, ServerMessage, UserId,
};
use uuid::Uuid;

use crate::auth::normalize_username;
use crate::state::{AppState, GROUP_ROOM, add_member, db_error};

const NO_GROUP: &str = "Esse grupo não existe.";

/// Confere o nome do grupo e tira os espaços das pontas.
pub fn normalize_group_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("O grupo precisa de um nome.".into());
    }
    if name.chars().count() > MAX_GROUP_NAME_CHARS {
        return Err(format!(
            "O nome do grupo passou de {MAX_GROUP_NAME_CHARS} caracteres."
        ));
    }
    Ok(name.to_owned())
}

impl AppState {
    /// Cria o grupo só com o dono dentro.
    pub async fn create_group(
        &self,
        owner: UserId,
        name: &str,
    ) -> Result<ConversationSummary, String> {
        let name = normalize_group_name(name)?;
        let mut tx = self.db.begin().await.map_err(db_error)?;
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO conversations (name, kind, owner_id) VALUES ($1, 'group', $2)
             RETURNING id",
        )
        .bind(&name)
        .bind(owner.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let id = ConversationId(id);
        add_member(&mut tx, id, owner).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.announce_group(id).await?;

        self.summary(owner, id)
            .await?
            .ok_or_else(|| "Erro no servidor, tente de novo.".into())
    }

    /// Coloca `username` no grupo. O grupo aparece na lista dele na hora.
    pub async fn invite(
        &self,
        by: UserId,
        id: ConversationId,
        username: &str,
    ) -> Result<(), String> {
        self.managed_group(by, id).await?;
        let username = normalize_username(username)
            .map_err(|_| format!("Ninguém com o usuário \"{}\".", username.trim()))?;
        let found: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM users WHERE lower(username) = $1")
                .bind(&username)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        let Some((guest,)) = found else {
            return Err(format!("Ninguém com o usuário \"{username}\"."));
        };
        let guest = UserId(guest);

        let inserted = sqlx::query(
            "INSERT INTO conversation_members (conversation_id, user_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(id.0)
        .bind(guest.0)
        .execute(&self.db)
        .await
        .map_err(db_error)?;
        if inserted.rows_affected() == 0 {
            return Err(format!("{username} já está no grupo."));
        }

        if let Some(conversation) = self.summary(guest, id).await? {
            self.emit(
                vec![guest],
                ServerMessage::ConversationAdded { conversation },
            );
        }
        self.announce_members(id).await
    }

    /// Tira a pessoa do grupo. Ela para de receber as mensagens na hora,
    /// porque cada mensagem nova procura os membros no banco.
    pub async fn leave(&self, user: UserId, id: ConversationId) -> Result<(), String> {
        self.managed_group(user, id).await?;
        self.drop_member(id, user).await?;
        self.emit(
            vec![user],
            ServerMessage::ConversationRemoved {
                conversation_id: id,
            },
        );
        self.announce_members(id).await
    }

    /// O dono tira alguém do grupo.
    pub async fn remove(
        &self,
        by: UserId,
        id: ConversationId,
        target: UserId,
    ) -> Result<(), String> {
        let owner = self.managed_group(by, id).await?;
        if owner != Some(by) {
            return Err("Só o dono do grupo pode remover pessoas.".into());
        }
        if target == by {
            return Err("Para sair do seu grupo, use \"sair do grupo\".".into());
        }
        if !self.is_member(target, id).await? {
            return Err("Essa pessoa não está no grupo.".into());
        }
        self.leave(target, id).await
    }

    /// A lista de membros, se `user` participa da conversa.
    pub async fn members(
        &self,
        user: UserId,
        id: ConversationId,
    ) -> Result<Option<ServerMessage>, String> {
        if !self.is_member(user, id).await? {
            return Ok(None);
        }
        self.members_message(id).await.map(Some)
    }

    /// Confere que é um grupo criado por alguém e que `user` está nele.
    /// Devolve o dono.
    async fn managed_group(
        &self,
        user: UserId,
        id: ConversationId,
    ) -> Result<Option<UserId>, String> {
        if id == GROUP_ROOM {
            return Err("A Sala do Grupo é de todo mundo: ninguém entra nem sai dela.".into());
        }
        let found: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT c.owner_id FROM conversations c
             JOIN conversation_members cm ON cm.conversation_id = c.id AND cm.user_id = $2
             WHERE c.id = $1 AND c.kind = 'group'",
        )
        .bind(id.0)
        .bind(user.0)
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        let (owner,) = found.ok_or(NO_GROUP)?;
        Ok(owner.map(UserId))
    }

    /// Apaga o membro e passa a posse adiante, se preciso.
    async fn drop_member(&self, id: ConversationId, user: UserId) -> Result<(), String> {
        let mut tx = self.db.begin().await.map_err(db_error)?;
        // Trava a linha do grupo: dois membros saindo juntos não podem os dois
        // achar que o outro ficou.
        let (owner,): (Option<Uuid>,) =
            sqlx::query_as("SELECT owner_id FROM conversations WHERE id = $1 FOR UPDATE")
                .bind(id.0)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_error)?;
        sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND user_id = $2")
            .bind(id.0)
            .bind(user.0)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;

        let next: Option<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM conversation_members WHERE conversation_id = $1
             ORDER BY joined_at, user_id LIMIT 1",
        )
        .bind(id.0)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        match next {
            // Ninguém ficou: o grupo e as mensagens somem.
            None => {
                sqlx::query("DELETE FROM conversations WHERE id = $1")
                    .bind(id.0)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
            }
            Some((heir,)) if owner == Some(user.0) => {
                sqlx::query("UPDATE conversations SET owner_id = $2 WHERE id = $1")
                    .bind(id.0)
                    .bind(heir)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
            }
            Some(_) => {}
        }
        tx.commit().await.map_err(db_error)?;
        Ok(())
    }

    /// Manda a lista de membros atualizada para quem está no grupo.
    /// Também manda o `GroupState`: quem entrou precisa ser posto no grupo
    /// MLS, e quem saiu, tirado.
    async fn announce_members(&self, id: ConversationId) -> Result<(), String> {
        let recipients = self.member_ids(id).await?;
        if recipients.is_empty() {
            return Ok(());
        }
        let message = self.members_message(id).await?;
        self.emit(recipients, message);
        self.announce_group(id).await
    }

    async fn members_message(&self, id: ConversationId) -> Result<ServerMessage, String> {
        // O dono vem primeiro; depois, por ordem de entrada.
        let rows: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
            "SELECT u.id, u.username, c.owner_id
             FROM conversation_members cm
             JOIN users u ON u.id = cm.user_id
             JOIN conversations c ON c.id = cm.conversation_id
             WHERE cm.conversation_id = $1
             ORDER BY u.id = c.owner_id DESC NULLS LAST, cm.joined_at, u.username",
        )
        .bind(id.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        let owner_id = rows.first().and_then(|(_, _, owner)| owner.map(UserId));
        Ok(ServerMessage::Members {
            conversation_id: id,
            owner_id,
            members: rows
                .into_iter()
                .map(|(user_id, username, _)| Member {
                    user_id: UserId(user_id),
                    username,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_name_is_trimmed_and_checked() {
        assert_eq!(normalize_group_name("  turma  ").unwrap(), "turma");
        assert!(normalize_group_name("   ").is_err());
        assert!(normalize_group_name(&"a".repeat(MAX_GROUP_NAME_CHARS)).is_ok());
        assert!(normalize_group_name(&"a".repeat(MAX_GROUP_NAME_CHARS + 1)).is_err());
    }
}
