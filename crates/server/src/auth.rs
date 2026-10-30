//! Contas e sessões: cadastro, login, `Resume` com token e logout.
//!
//! A senha vira um hash Argon2, que é lento de propósito (para dificultar
//! adivinhar senhas vazadas). Por ser lento, ele roda em `spawn_blocking`, numa
//! thread separada, para não travar as outras conexões do Tokio.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use protocol::{PASSWORD_CHARS, USERNAME_CHARS, UserId};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::state::{AppState, DB_ERROR, SALA_DO_GRUPO, join_conversation, log_db_error};

/// Quem está logado numa conexão.
#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: UserId,
    pub username: String,
    /// O token em texto, que só o aparelho da pessoa e esta conexão conhecem.
    pub token: String,
}

const WRONG_LOGIN: &str = "Usuário ou senha errados.";

impl AppState {
    pub async fn register(&self, username: &str, password: &str) -> Result<Session, String> {
        let username = normalize_username(username)?;
        check_password(password)?;
        let hash = hash_password(password.to_owned()).await?;

        // Uma transação: ou a conta nasce completa (no grupo e com sessão),
        // ou nada fica gravado.
        let mut tx = self.db.begin().await.map_err(log_db_error)?;
        // `DO NOTHING` + `RETURNING` não devolve linha quando o nome já existe.
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO users (id, username, password_hash) VALUES ($1, $2, $3)
             ON CONFLICT (username) DO NOTHING
             RETURNING id",
        )
        .bind(Uuid::new_v4())
        .bind(&username)
        .bind(&hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(log_db_error)?;
        let Some(id) = id else {
            return Err("Esse nome de usuário já existe.".into());
        };
        let user_id = UserId(id);
        join_conversation(&mut tx, SALA_DO_GRUPO, user_id)
            .await
            .map_err(log_db_error)?;
        let token = create_session(&mut tx, user_id).await?;
        tx.commit().await.map_err(log_db_error)?;

        tracing::info!(%username, "conta criada");
        Ok(Session {
            user_id,
            username,
            token,
        })
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Session, String> {
        // Nome fora das regras não tem como existir: mesma resposta de senha
        // errada, para não ajudar a descobrir quais contas existem.
        let username = normalize_username(username).map_err(|_| WRONG_LOGIN.to_string())?;
        let row: Option<(Uuid, String)> =
            sqlx::query_as("SELECT id, password_hash FROM users WHERE username = $1")
                .bind(&username)
                .fetch_optional(&self.db)
                .await
                .map_err(log_db_error)?;
        let Some((id, hash)) = row else {
            return Err(WRONG_LOGIN.into());
        };
        if !verify_password(password.to_owned(), hash).await? {
            return Err(WRONG_LOGIN.into());
        }

        let user_id = UserId(id);
        let mut tx = self.db.begin().await.map_err(log_db_error)?;
        let token = create_session(&mut tx, user_id).await?;
        tx.commit().await.map_err(log_db_error)?;
        Ok(Session {
            user_id,
            username,
            token,
        })
    }

    /// Entra com o token de uma sessão anterior.
    pub async fn resume(&self, token: &str) -> Result<Session, String> {
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "UPDATE sessions s SET last_used_at = now()
             FROM users u
             WHERE s.token_hash = $1 AND u.id = s.user_id
             RETURNING u.id, u.username",
        )
        .bind(token_hash(token))
        .fetch_optional(&self.db)
        .await
        .map_err(log_db_error)?;
        let Some((id, username)) = row else {
            return Err("A sessão expirou. Entre de novo.".into());
        };
        Ok(Session {
            user_id: UserId(id),
            username,
            token: token.to_owned(),
        })
    }

    pub async fn logout(&self, session: &Session) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash(&session.token))
            .execute(&self.db)
            .await
            .map_err(log_db_error)?;
        Ok(())
    }
}

/// Deixa o nome em minúsculas e confere as regras.
fn normalize_username(username: &str) -> Result<String, String> {
    let username = username.trim().to_lowercase();
    let len = username.chars().count();
    if !USERNAME_CHARS.contains(&len) {
        return Err(format!(
            "O nome de usuário precisa ter de {} a {} caracteres.",
            USERNAME_CHARS.start(),
            USERNAME_CHARS.end()
        ));
    }
    let allowed = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.';
    if !username.chars().all(allowed) {
        return Err("Use só letras sem acento, números, _ e . no nome de usuário.".into());
    }
    Ok(username)
}

fn check_password(password: &str) -> Result<(), String> {
    let len = password.chars().count();
    if !PASSWORD_CHARS.contains(&len) {
        return Err(format!(
            "A senha precisa ter de {} a {} caracteres.",
            PASSWORD_CHARS.start(),
            PASSWORD_CHARS.end()
        ));
    }
    Ok(())
}

async fn hash_password(password: String) -> Result<String, String> {
    let hashed = tokio::task::spawn_blocking(move || {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt)?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
    })
    .await;
    // Dois jeitos de falhar: a thread em si, ou o Argon2 dentro dela.
    let hashed = hashed
        .map_err(|e| e.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()));
    hashed.map_err(|e| {
        tracing::error!(error = %e, "falha ao gerar hash de senha");
        DB_ERROR.to_string()
    })
}

async fn verify_password(password: String, hash: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        let Ok(hash) = PasswordHash::new(&hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "falha ao conferir senha");
        DB_ERROR.to_string()
    })
}

/// Cria uma sessão e devolve o token em texto (64 caracteres hexadecimais).
async fn create_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: UserId,
) -> Result<String, String> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    sqlx::query("INSERT INTO sessions (token_hash, user_id) VALUES ($1, $2)")
        .bind(token_hash(&token))
        .bind(user_id.0)
        .execute(&mut **tx)
        .await
        .map_err(log_db_error)?;
    Ok(token)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_is_lowercased_and_checked() {
        assert_eq!(normalize_username(" Nina.B_2 ").unwrap(), "nina.b_2");
        assert!(normalize_username("ab").is_err());
        assert!(normalize_username("joão").is_err());
        assert!(normalize_username("com espaço").is_err());
        assert!(normalize_username(&"a".repeat(21)).is_err());
    }

    #[test]
    fn password_length_is_checked() {
        assert!(check_password("1234567").is_err());
        assert!(check_password("12345678").is_ok());
        assert!(check_password(&"x".repeat(129)).is_err());
    }

    #[tokio::test]
    async fn hashed_password_verifies() {
        let hash = hash_password("senha boa demais".into()).await.unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(
            verify_password("senha boa demais".into(), hash.clone())
                .await
                .unwrap()
        );
        assert!(!verify_password("outra senha".into(), hash).await.unwrap());
    }
}
