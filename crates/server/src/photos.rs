//! Fotos de perfil: cada pessoa guarda um PNG pequeno, e quem divide uma
//! conversa com ela pode pedir.
//!
//! O servidor não decodifica a imagem: confere a assinatura do PNG, o tamanho
//! do arquivo e as medidas do cabeçalho (IHDR), e guarda os bytes como vieram.
//! Quem abre a imagem é o app.

use protocol::{PHOTO_MAX_BYTES, PHOTO_SIZE, Photo, ServerMessage, UserId};
use uuid::Uuid;

use crate::state::{AppState, db_error};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Confere que é um PNG de no máximo `PHOTO_MAX_BYTES` e `PHOTO_SIZE` px de lado.
pub fn check_png(png: &[u8]) -> Result<(), String> {
    if png.len() > PHOTO_MAX_BYTES {
        return Err("Foto grande demais.".into());
    }
    // Assinatura (8 bytes) e o começo do IHDR: tamanho (4), "IHDR" (4),
    // largura (4) e altura (4).
    if png.len() < 24 || !png.starts_with(PNG_SIGNATURE) || &png[12..16] != b"IHDR" {
        return Err("A foto precisa ser um PNG.".into());
    }
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if width == 0 || height == 0 || width > PHOTO_SIZE || height > PHOTO_SIZE {
        return Err(format!(
            "A foto precisa ter até {PHOTO_SIZE}x{PHOTO_SIZE} px."
        ));
    }
    Ok(())
}

impl AppState {
    /// Versão da foto de `user` (0: sem foto).
    pub async fn photo_version(&self, user: UserId) -> Result<i64, String> {
        let found: Option<(i64,)> =
            sqlx::query_as("SELECT version FROM user_photos WHERE user_id = $1")
                .bind(user.0)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        Ok(found.map_or(0, |(v,)| v))
    }

    /// Troca a foto e avisa quem divide alguma conversa com a pessoa (e ela mesma).
    pub async fn set_photo(&self, user: UserId, png: &Photo) -> Result<(), String> {
        check_png(&png.0)?;
        let (version,): (i64,) = sqlx::query_as(
            "INSERT INTO user_photos (user_id, png, version) VALUES ($1, $2, 1)
             ON CONFLICT (user_id) DO UPDATE
                 SET png = EXCLUDED.png, version = user_photos.version + 1, updated_at = now()
             RETURNING version",
        )
        .bind(user.0)
        .bind(&png.0)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        let mut recipients = self.people_sharing_with(user).await?;
        if !recipients.contains(&user) {
            recipients.push(user);
        }
        self.emit(
            recipients,
            ServerMessage::PhotoChanged {
                user_id: user,
                version,
            },
        );
        Ok(())
    }

    /// A foto de `target`, se `user` divide uma conversa com essa pessoa (ou é ela).
    pub async fn photo(&self, user: UserId, target: UserId) -> Result<ServerMessage, String> {
        if user != target && !self.people_sharing_with(user).await?.contains(&target) {
            return Err("Essa pessoa não está em nenhuma conversa sua.".into());
        }
        let found: Option<(i64, Vec<u8>)> =
            sqlx::query_as("SELECT version, png FROM user_photos WHERE user_id = $1")
                .bind(target.0)
                .fetch_optional(&self.db)
                .await
                .map_err(db_error)?;
        Ok(match found {
            Some((version, png)) => ServerMessage::Photo {
                user_id: target,
                version,
                png: Some(Photo(png)),
            },
            None => ServerMessage::Photo {
                user_id: target,
                version: 0,
                png: None,
            },
        })
    }

    /// Todo mundo que está em pelo menos uma conversa com `user`.
    pub(crate) async fn people_sharing_with(&self, user: UserId) -> Result<Vec<UserId>, String> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT other.user_id FROM conversation_members mine
             JOIN conversation_members other ON other.conversation_id = mine.conversation_id
             WHERE mine.user_id = $1",
        )
        .bind(user.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        Ok(rows.into_iter().map(|(id,)| UserId(id)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn accepts_a_small_png() {
        assert!(check_png(&png(128, 128)).is_ok());
        assert!(check_png(&png(64, 100)).is_ok());
    }

    #[test]
    fn refuses_what_is_not_a_small_png() {
        assert!(check_png(b"GIF89a.................").is_err());
        assert!(check_png(&png(129, 128)).is_err());
        assert!(check_png(&png(0, 10)).is_err());
        let mut big = png(128, 128);
        big.resize(PHOTO_MAX_BYTES + 1, 0);
        assert!(check_png(&big).is_err());
    }
}
