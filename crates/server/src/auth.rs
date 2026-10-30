//! Contas e sessões: criar conta, entrar, continuar entrado e sair.
//!
//! A senha vira um hash argon2 (feito para ser lento, o que atrapalha quem
//! tenta adivinhar). O token de sessão é aleatório e o banco guarda só o
//! sha256 dele: quem ler o banco não consegue entrar como ninguém.

use std::ops::RangeInclusive;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use protocol::UserId;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::state::{AppState, GROUP_ROOM, add_member, db_error};

pub const USERNAME_CHARS: RangeInclusive<usize> = 3..=20;
pub const PASSWORD_CHARS: RangeInclusive<usize> = 8..=128;

const WRONG_LOGIN: &str = "Usuário ou senha incorretos.";

/// Quem está entrado numa conexão.
#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: UserId,
    pub username: String,
    /// O token em texto, que só o app e esta conexão conhecem.
    pub token: String,
}

/// Confere o nome de usuário e devolve ele em minúsculas.
pub fn normalize_username(username: &str) -> Result<String, String> {
    let username = username.trim().to_lowercase();
    if !USERNAME_CHARS.contains(&username.chars().count()) {
        return Err("O usuário precisa ter de 3 a 20 caracteres.".into());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
    {
        return Err("Use só letras sem acento, números, _ e . no usuário.".into());
    }
    Ok(username)
}

pub fn check_password(password: &str) -> Result<(), String> {
    if PASSWORD_CHARS.contains(&password.chars().count()) {
        Ok(())
    } else {
        Err("A senha precisa ter de 8 a 128 caracteres.".into())
    }
}

/// Calcula o hash numa thread de bloqueio: o argon2 ocupa a CPU por um tempo
/// e travaria as outras conexões se rodasse direto no runtime async.
async fn hash_password(password: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
    })
    .await
    .ok()
    .and_then(Result::ok)
    .ok_or_else(|| "Erro no servidor, tente de novo.".into())
}

async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}

/// 32 bytes aleatórios em hexadecimal (64 caracteres).
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

impl AppState {
    /// Cria a conta, coloca ela na Sala do Grupo e já abre uma sessão.
    pub async fn register(&self, username: &str, password: &str) -> Result<Session, String> {
        let username = normalize_username(username)?;
        check_password(password)?;
        let hash = hash_password(password.to_owned()).await?;

        let mut tx = self.db.begin().await.map_err(db_error)?;
        let inserted: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
            "INSERT INTO users (username, password_hash) VALUES ($1, $2) RETURNING id",
        )
        .bind(&username)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await;
        let user_id = match inserted {
            Ok((id,)) => UserId(id),
            // O índice único do banco é quem garante que o nome não se repete.
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err("Esse usuário já existe.".into());
            }
            Err(err) => return Err(db_error(err)),
        };
        add_member(&mut tx, GROUP_ROOM, user_id)
            .await
            .map_err(db_error)?;
        let token = create_session(&mut tx, user_id).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;

        tracing::info!(%username, "conta criada");
        Ok(Session {
            user_id,
            username,
            token,
        })
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<Session, String> {
        // Nome fora do formato não pode existir: mesma resposta de senha errada,
        // para não contar a ninguém quais usuários existem.
        let username = normalize_username(username).map_err(|_| WRONG_LOGIN.to_string())?;
        let found: Option<(Uuid, String)> =
            sqlx::query_as("SELECT id, password_hash FROM users WHERE lower(username) = $1")
                .bind(&username)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        let Some((id, hash)) = found else {
            return Err(WRONG_LOGIN.into());
        };
        if !verify_password(password.to_owned(), hash).await {
            return Err(WRONG_LOGIN.into());
        }

        let user_id = UserId(id);
        let mut conn = self.db.acquire().await.map_err(db_error)?;
        let token = create_session(&mut conn, user_id).await.map_err(db_error)?;
        Ok(Session {
            user_id,
            username,
            token,
        })
    }

    /// Continua uma sessão salva pelo app, sem pedir a senha de novo.
    pub async fn resume(&self, token: &str) -> Result<Session, String> {
        let found: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT u.id, u.username FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1",
        )
        .bind(token_hash(token))
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        let (id, username) = found.ok_or("Sua sessão expirou, entre de novo.")?;
        Ok(Session {
            user_id: UserId(id),
            username,
            token: token.to_owned(),
        })
    }

    /// Apaga a sessão: o token salvo no app deixa de valer.
    pub async fn logout(&self, token: &str) -> Result<(), String> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash(token))
            .execute(&self.db)
            .await
            .map_err(db_error)?;
        Ok(())
    }
}

async fn create_session(
    conn: &mut sqlx::PgConnection,
    user_id: UserId,
) -> Result<String, sqlx::Error> {
    let token = new_token();
    sqlx::query("INSERT INTO sessions (token_hash, user_id) VALUES ($1, $2)")
        .bind(token_hash(&token))
        .bind(user_id.0)
        .execute(conn)
        .await?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_is_lowercased_and_checked() {
        assert_eq!(normalize_username("  Jobiss ").unwrap(), "jobiss");
        assert_eq!(normalize_username("nina.r_2").unwrap(), "nina.r_2");
        assert!(normalize_username("jo").is_err());
        assert!(normalize_username(&"a".repeat(21)).is_err());
        assert!(normalize_username("joão").is_err());
        assert!(normalize_username("com espaço").is_err());
    }

    #[test]
    fn password_length_is_checked() {
        assert!(check_password("1234567").is_err());
        assert!(check_password("12345678").is_ok());
        assert!(check_password(&"x".repeat(129)).is_err());
    }

    #[test]
    fn token_is_64_hex_chars_and_hash_is_stable() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(token, new_token());
        assert_eq!(token_hash(&token), token_hash(&token));
        assert_eq!(token_hash(&token).len(), 32);
    }
}
