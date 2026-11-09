//! Contas e sessões: criar conta, entrar, continuar entrado e sair.
//!
//! A senha vira um hash argon2 (feito para ser lento, o que atrapalha quem
//! tenta adivinhar). O token de sessão é aleatório e o banco guarda só o
//! sha256 dele: quem ler o banco não consegue entrar como ninguém.

use std::ops::RangeInclusive;
use std::time::Instant;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use protocol::{DeviceId, ServerMessage, UserId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::login_limit::locked_message;
use crate::state::{AppState, GROUP_ROOM, add_member, db_error};

pub const USERNAME_CHARS: RangeInclusive<usize> = 3..=20;
pub const PASSWORD_CHARS: RangeInclusive<usize> = 8..=128;

/// Resposta ao `Resume` com um token que não vale mais.
pub(crate) const SESSION_EXPIRED: &str = "Sua sessão expirou, entre de novo.";

const WRONG_LOGIN: &str = "Usuário ou senha incorretos.";
const WRONG_CODE: &str = "Código incorreto ou vencido. Confira o e-mail ou peça outro.";
/// Erros com o mesmo código antes de ele ser descartado.
const RESET_CODE_ATTEMPTS: i32 = 5;

/// Quem está entrado numa conexão.
#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: UserId,
    pub username: String,
    /// O token em texto, que só o app e esta conexão conhecem.
    pub token: String,
    /// O aparelho desta sessão. Até a conexão dizer qual é (`attach_device`),
    /// é o da sessão salva, ou o próprio id da conta.
    pub device: DeviceId,
    /// `false`: aparelho novo esperando aprovação. Só vê os próprios
    /// aparelhos; nada de conversas nem anexos.
    pub approved: bool,
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

/// Confere o e-mail e devolve ele em minúsculas. Não valida de verdade
/// (só o envio confirma): basta ter a cara de um endereço.
pub fn normalize_email(email: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if valid {
        Ok(email)
    } else {
        Err("Esse e-mail não parece válido.".into())
    }
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

pub(crate) fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

impl AppState {
    /// Cria a conta, coloca ela na Sala do Grupo e já abre uma sessão. O
    /// e-mail é obrigatório: é por ele que a senha se recupera.
    pub async fn register(
        &self,
        username: &str,
        password: &str,
        email: Option<&str>,
    ) -> Result<Session, String> {
        let username = normalize_username(username)?;
        let email = normalize_email(
            email.ok_or("Informe um e-mail: é por ele que você recupera a senha.")?,
        )?;
        check_password(password)?;
        let hash = hash_password(password.to_owned()).await?;

        let mut tx = self.db.begin().await.map_err(db_error)?;
        let inserted: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
            "INSERT INTO users (username, password_hash, email) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(&username)
        .bind(&hash)
        .bind(&email)
        .fetch_one(&mut *tx)
        .await;
        let user_id = match inserted {
            Ok((id,)) => UserId(id),
            // Os índices únicos do banco são quem garante que nome e e-mail
            // não se repetem; o nome do índice diz qual dos dois foi.
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err(if err.constraint() == Some("users_email_key") {
                    "Esse e-mail já tem conta. Entre com ele ou peça um código em \"esqueci a senha\"."
                        .into()
                } else {
                    "Esse usuário já existe.".into()
                });
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
            device: DeviceId(user_id.0),
            approved: true,
        })
    }

    /// Entra com o usuário ou com o e-mail da conta (o que tiver `@`).
    pub async fn login(&self, username: &str, password: &str) -> Result<Session, String> {
        // Nome (ou e-mail) fora do formato não pode existir: mesma resposta
        // de senha errada, para não contar a ninguém quais contas existem.
        let by_email = username.contains('@');
        let key = if by_email {
            normalize_email(username)
        } else {
            normalize_username(username)
        }
        .map_err(|_| WRONG_LOGIN.to_string())?;
        // Conta também contas que não existem, para o bloqueio não revelar
        // quais existem. Bloqueado, nem confere a senha.
        self.login_limiter
            .begin(&key, Instant::now())
            .map_err(locked_message)?;
        let found: Option<(Uuid, String, String)> = sqlx::query_as(if by_email {
            "SELECT id, username, password_hash FROM users WHERE lower(email) = $1"
        } else {
            "SELECT id, username, password_hash FROM users WHERE lower(username) = $1"
        })
        .bind(&key)
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        let Some((id, username, hash)) = found else {
            return Err(WRONG_LOGIN.into());
        };
        if !verify_password(password.to_owned(), hash).await {
            return Err(WRONG_LOGIN.into());
        }
        self.login_limiter.succeeded(&key);

        let user_id = UserId(id);
        let mut conn = self.db.acquire().await.map_err(db_error)?;
        let token = create_session(&mut conn, user_id).await.map_err(db_error)?;
        Ok(Session {
            user_id,
            username,
            token,
            device: DeviceId(user_id.0),
            approved: true,
        })
    }

    /// Troca a senha de `session`, conferindo a atual. As outras sessões da
    /// conta caem: quem trocou a senha porque alguém sabia dela tira o
    /// intruso. Tentativas erradas contam no mesmo limite do login.
    pub async fn change_password(
        &self,
        session: &Session,
        current: &str,
        new: &str,
    ) -> Result<(), String> {
        check_password(new)?;
        let username = session.username.to_lowercase();
        self.login_limiter
            .begin(&username, Instant::now())
            .map_err(locked_message)?;
        let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = $1")
            .bind(session.user_id.0)
            .fetch_one(&self.db)
            .await
            .map_err(db_error)?;
        if !verify_password(current.to_owned(), hash).await {
            return Err("A senha atual não confere.".into());
        }
        self.login_limiter.succeeded(&username);
        let hash = hash_password(new.to_owned()).await?;
        let mut tx = self.db.begin().await.map_err(db_error)?;
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(session.user_id.0)
            .bind(&hash)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
            .bind(session.user_id.0)
            .bind(token_hash(&session.token))
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        tracing::info!(username = %session.username, "senha trocada");
        Ok(())
    }

    /// O e-mail da conta, se já tiver um.
    pub async fn email_of(&self, user: UserId) -> Result<Option<String>, String> {
        let (email,): (Option<String>,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
            .bind(user.0)
            .fetch_one(&self.db)
            .await
            .map_err(db_error)?;
        Ok(email)
    }

    /// Define ou troca o e-mail da conta. Devolve ele já no formato guardado.
    pub async fn set_email(&self, user: UserId, email: &str) -> Result<String, String> {
        let email = normalize_email(email)?;
        let updated = sqlx::query("UPDATE users SET email = $2 WHERE id = $1")
            .bind(user.0)
            .bind(&email)
            .execute(&self.db)
            .await;
        match updated {
            Ok(_) => {
                tracing::info!("e-mail da conta definido");
                Ok(email)
            }
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                Err("Esse e-mail já é de outra conta.".into())
            }
            Err(err) => Err(db_error(err)),
        }
    }

    /// Gera um código de seis dígitos, guarda o hash dele por 15 minutos e
    /// manda ao e-mail. Sem conta com esse e-mail não faz nada, e quem pediu
    /// recebe a mesma resposta: não dá para descobrir e-mails por aqui.
    pub async fn request_password_reset(&self, email: &str) -> Result<(), String> {
        let Ok(email) = normalize_email(email) else {
            return Ok(());
        };
        // O mesmo limite do login, por e-mail: evita pedir códigos sem parar.
        self.login_limiter
            .begin(&format!("reset:{email}"), Instant::now())
            .map_err(locked_message)?;
        let found: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE lower(email) = $1")
            .bind(&email)
            .fetch_optional(&self.db)
            .await
            .map_err(db_error)?;
        let Some((user_id,)) = found else {
            tracing::info!("pedido de código para e-mail sem conta");
            return Ok(());
        };
        let code = format!("{:06}", OsRng.next_u32() % 1_000_000);
        sqlx::query(
            "INSERT INTO password_resets (user_id, code_hash, expires_at, attempts)
             VALUES ($1, $2, now() + interval '15 minutes', 0)
             ON CONFLICT (user_id) DO UPDATE
             SET code_hash = EXCLUDED.code_hash, expires_at = EXCLUDED.expires_at, attempts = 0",
        )
        .bind(user_id)
        .bind(token_hash(&code))
        .execute(&self.db)
        .await
        .map_err(db_error)?;
        self.mailer.send_reset_code(&email, &code);
        Ok(())
    }

    /// Troca a senha com o código recebido por e-mail. Todas as sessões da
    /// conta caem: quem recupera a senha quer tirar quem estava dentro.
    pub async fn reset_password(
        &self,
        email: &str,
        code: &str,
        password: &str,
    ) -> Result<(), String> {
        check_password(password)?;
        let email = normalize_email(email).map_err(|_| WRONG_CODE.to_string())?;
        let key = format!("reset:{email}");
        self.login_limiter
            .begin(&key, Instant::now())
            .map_err(locked_message)?;
        let found: Option<(Uuid, Vec<u8>, bool, i32)> = sqlx::query_as(
            "SELECT u.id, r.code_hash, r.expires_at < now() AS expired, r.attempts
             FROM password_resets r JOIN users u ON u.id = r.user_id
             WHERE lower(u.email) = $1",
        )
        .bind(&email)
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        let Some((user_id, code_hash, expired, attempts)) = found else {
            return Err(WRONG_CODE.into());
        };
        if expired || attempts >= RESET_CODE_ATTEMPTS {
            sqlx::query("DELETE FROM password_resets WHERE user_id = $1")
                .bind(user_id)
                .execute(&self.db)
                .await
                .map_err(db_error)?;
            return Err(WRONG_CODE.into());
        }
        if token_hash(code.trim()) != code_hash {
            sqlx::query("UPDATE password_resets SET attempts = attempts + 1 WHERE user_id = $1")
                .bind(user_id)
                .execute(&self.db)
                .await
                .map_err(db_error)?;
            return Err(WRONG_CODE.into());
        }
        let hash = hash_password(password.to_owned()).await?;
        let mut tx = self.db.begin().await.map_err(db_error)?;
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user_id)
            .bind(&hash)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        sqlx::query("DELETE FROM password_resets WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.login_limiter.succeeded(&key);
        tracing::info!("senha recuperada por código");
        Ok(())
    }

    /// Troca o nome de usuário (vale também para entrar). Avisa quem divide
    /// uma conversa com a pessoa, e ela mesma (as outras conexões dela).
    /// Devolve o nome novo, já no formato guardado.
    pub async fn rename(&self, user: UserId, username: &str) -> Result<String, String> {
        let username = normalize_username(username)?;
        let updated = sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
            .bind(user.0)
            .bind(&username)
            .execute(&self.db)
            .await;
        match updated {
            Ok(_) => {}
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err("Esse nome já é de outra pessoa.".into());
            }
            Err(err) => return Err(db_error(err)),
        }
        let mut recipients = self.people_sharing_with(user).await?;
        if !recipients.contains(&user) {
            recipients.push(user);
        }
        self.emit(
            recipients,
            ServerMessage::UserRenamed {
                user_id: user,
                username: username.clone(),
            },
        );
        tracing::info!(%username, "trocou de nome");
        Ok(username)
    }

    /// Continua uma sessão salva pelo app, sem pedir a senha de novo.
    ///
    /// `Ok(None)` quando o token não vale mais; `Err` quando o banco falhou.
    /// A diferença importa: com o token inválido o app esquece a sessão e
    /// pede a senha, mas uma falha passageira do banco não pode deslogar
    /// ninguém.
    pub async fn resume(&self, token: &str) -> Result<Option<Session>, String> {
        let found: Option<(Uuid, String, Option<Uuid>, Option<bool>)> = sqlx::query_as(
            "SELECT u.id, u.username, s.device_id, d.approved
             FROM sessions s JOIN users u ON u.id = s.user_id
             LEFT JOIN devices d ON d.id = s.device_id
             WHERE s.token_hash = $1",
        )
        .bind(token_hash(token))
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        Ok(found.map(|(id, username, device, approved)| Session {
            user_id: UserId(id),
            username,
            token: token.to_owned(),
            device: DeviceId(device.unwrap_or(id)),
            // Sessão de antes dos aparelhos: era o único aparelho da conta.
            approved: approved.unwrap_or(true),
        }))
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
    fn email_is_lowercased_and_checked() {
        assert_eq!(
            normalize_email("  Nina@Exemplo.COM ").unwrap(),
            "nina@exemplo.com"
        );
        for bad in [
            "nina",
            "@exemplo.com",
            "nina@",
            "nina@exemplo",
            "ni na@exemplo.com",
            "a@b@c.com",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
    }

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
