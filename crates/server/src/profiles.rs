//! Página de perfil: textos, vitrines e imagens (foto grande, fundo, artes).
//!
//! Os textos vão pelo WebSocket (`GetProfile`, `SaveProfile`); as imagens,
//! que podem ser GIFs de alguns MB, vão por HTTP, como os anexos. Quem vê a
//! página de alguém é quem divide uma conversa com a pessoa (ou ela mesma),
//! a mesma regra da foto.

use std::io::Cursor;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use protocol::{
    MAX_ABOUT_CHARS, MAX_PROFILE_LINE_CHARS, PROFILE_IMAGE_TYPES, Profile, ProfileImage,
    ProfileImageInfo, ProfileText, ServerMessage, Showcase, UserId,
};
use uuid::Uuid;

use crate::attachments::{Failure, authorize};
use crate::state::{AppState, db_error};

/// Confere e arruma os textos: tira espaços das pontas, corta nada (recusa o
/// que passa do limite), confere a cor e tira vitrines repetidas.
pub fn clean_text(text: &ProfileText) -> Result<ProfileText, String> {
    let line = |value: &str, what: &str| {
        let value = value.trim();
        if value.chars().count() > MAX_PROFILE_LINE_CHARS {
            Err(format!(
                "{what} pode ter até {MAX_PROFILE_LINE_CHARS} letras."
            ))
        } else if value.chars().any(char::is_control) {
            Err(format!("{what} não pode ter quebra de linha."))
        } else {
            Ok(value.to_string())
        }
    };
    let about = text.about.trim();
    if about.chars().count() > MAX_ABOUT_CHARS {
        return Err(format!(
            "O \"sobre mim\" pode ter até {MAX_ABOUT_CHARS} letras."
        ));
    }
    let accent = text.accent.trim();
    let valid_accent = accent.is_empty()
        || (accent.len() == 7
            && accent.starts_with('#')
            && accent[1..].chars().all(|c| c.is_ascii_hexdigit()));
    if !valid_accent {
        return Err("Cor inválida.".into());
    }
    let mut showcases = Vec::new();
    for showcase in &text.showcases {
        if !showcases.contains(showcase) {
            showcases.push(*showcase);
        }
    }
    Ok(ProfileText {
        real_name: line(&text.real_name, "O nome")?,
        location: line(&text.location, "O \"de onde\"")?,
        about: about.to_string(),
        accent: accent.to_ascii_lowercase(),
        showcases,
    })
}

fn showcase_name(showcase: Showcase) -> &'static str {
    match showcase {
        Showcase::Art => "art",
        Showcase::About => "about",
    }
}

fn showcase_from(name: &str) -> Option<Showcase> {
    match name {
        "art" => Some(Showcase::Art),
        "about" => Some(Showcase::About),
        _ => None,
    }
}

/// Confere uma imagem do perfil: formato aceito e igual ao declarado, medidas
/// e tamanho dentro do limite do tipo.
pub fn check_image(kind: ProfileImage, content_type: &str, bytes: &[u8]) -> Result<(), String> {
    use image::ImageFormat;
    if bytes.is_empty() {
        return Err("Imagem vazia.".into());
    }
    if bytes.len() > kind.max_bytes() {
        return Err(format!(
            "A imagem pode ter até {} MB.",
            kind.max_bytes() / (1024 * 1024)
        ));
    }
    let expected = match content_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        _ => return Err("A imagem precisa ser PNG, JPG ou GIF.".into()),
    };
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Imagem inválida.".to_string())?;
    if reader.format() != Some(expected) {
        return Err("Imagem inválida.".into());
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "Imagem inválida.".to_string())?;
    let (max_w, max_h) = kind.max_size();
    if width == 0 || height == 0 || width > max_w || height > max_h {
        return Err(format!("A imagem pode ter até {max_w}x{max_h} px."));
    }
    Ok(())
}

/// Uma resposta de erro de imagem: 400 com a frase, ou 500 sem detalhe.
fn bad(reason: String) -> Failure {
    (StatusCode::BAD_REQUEST, reason)
}

fn internal(err: impl std::fmt::Display) -> Failure {
    tracing::error!(%err, "erro com imagem do perfil");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Erro no servidor, tente de novo.".into(),
    )
}

/// `POST /perfil/imagens/{tipo}`: troca uma imagem do seu perfil.
pub async fn upload_image(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        let session = authorize(&state, &headers).await?;
        let kind = ProfileImage::from_slug(&slug)
            .ok_or((StatusCode::NOT_FOUND, "Tipo de imagem desconhecido.".into()))?;
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .filter(|value| PROFILE_IMAGE_TYPES.contains(value))
            .ok_or_else(|| bad("A imagem precisa ser PNG, JPG ou GIF.".into()))?;
        check_image(kind, content_type, &body).map_err(bad)?;
        state
            .set_profile_image(session.user_id, kind, content_type, &body)
            .await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => failure.into_response(),
    }
}

/// `DELETE /perfil/imagens/{tipo}`: tira uma imagem do seu perfil.
pub async fn delete_image(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let session = authorize(&state, &headers).await?;
        let kind = ProfileImage::from_slug(&slug)
            .ok_or((StatusCode::NOT_FOUND, "Tipo de imagem desconhecido.".into()))?;
        state.remove_profile_image(session.user_id, kind).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(failure) => failure.into_response(),
    }
}

/// `GET /perfil/{id}/imagens/{tipo}`: a imagem do perfil de alguém.
pub async fn download_image(
    State(state): State<AppState>,
    Path((user, slug)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let session = authorize(&state, &headers).await?;
        let kind = ProfileImage::from_slug(&slug)
            .ok_or((StatusCode::NOT_FOUND, "Tipo de imagem desconhecido.".into()))?;
        state
            .read_profile_image(session.user_id, UserId(user), kind)
            .await
    }
    .await;
    match result {
        Ok((content_type, bytes)) => {
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(failure) => failure.into_response(),
    }
}

impl AppState {
    fn profile_image_path(&self, user: UserId, kind: ProfileImage) -> PathBuf {
        self.attachments_dir
            .join("perfil")
            .join(format!("{}-{}", user.0, kind.slug()))
    }

    /// `user` pode ver o perfil de `target`: é a mesma pessoa ou divide uma
    /// conversa com ela.
    async fn can_see_profile(&self, user: UserId, target: UserId) -> Result<bool, String> {
        Ok(user == target || self.people_sharing_with(user).await?.contains(&target))
    }

    /// A página de perfil de `target`, para `user`.
    pub async fn profile(&self, user: UserId, target: UserId) -> Result<ServerMessage, String> {
        if !self.can_see_profile(user, target).await? {
            return Err("Essa pessoa não está em nenhuma conversa sua.".into());
        }
        type Row = (
            String,
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<Vec<String>>,
            Option<i64>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT u.username,
                    (EXTRACT(EPOCH FROM u.created_at) * 1000)::BIGINT,
                    p.real_name, p.location, p.about, p.accent, p.showcases, p.version
             FROM users u LEFT JOIN profiles p ON p.user_id = u.id
             WHERE u.id = $1",
        )
        .bind(target.0)
        .fetch_optional(&self.db)
        .await
        .map_err(db_error)?;
        let Some((
            username,
            member_since_ms,
            real_name,
            location,
            about,
            accent,
            showcases,
            version,
        )) = row
        else {
            return Err("Essa conta não existe.".into());
        };
        let text = match showcases {
            Some(showcases) => ProfileText {
                real_name: real_name.unwrap_or_default(),
                location: location.unwrap_or_default(),
                about: about.unwrap_or_default(),
                accent: accent.unwrap_or_default(),
                showcases: showcases.iter().filter_map(|s| showcase_from(s)).collect(),
            },
            None => ProfileText::default(),
        };
        let images: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT kind, content_type, version FROM profile_images WHERE user_id = $1",
        )
        .bind(target.0)
        .fetch_all(&self.db)
        .await
        .map_err(db_error)?;
        let mut images: Vec<ProfileImageInfo> = images
            .into_iter()
            .filter_map(|(kind, content_type, version)| {
                Some(ProfileImageInfo {
                    kind: ProfileImage::from_slug(&kind)?,
                    version,
                    content_type,
                })
            })
            .collect();
        images.sort_by_key(|image| ProfileImage::ALL.iter().position(|k| *k == image.kind));
        Ok(ServerMessage::Profile {
            profile: Profile {
                user_id: target,
                username,
                member_since_ms,
                text,
                images,
                version: version.unwrap_or(0),
            },
        })
    }

    /// Salva os textos da sua página e avisa quem pode vê-la.
    pub async fn save_profile(&self, user: UserId, text: &ProfileText) -> Result<(), String> {
        let text = clean_text(text)?;
        let showcases: Vec<&str> = text.showcases.iter().map(|s| showcase_name(*s)).collect();
        let (version,): (i64,) = sqlx::query_as(
            "INSERT INTO profiles (user_id, real_name, location, about, accent, showcases, version)
             VALUES ($1, $2, $3, $4, $5, $6, 1)
             ON CONFLICT (user_id) DO UPDATE SET
                 real_name = EXCLUDED.real_name,
                 location = EXCLUDED.location,
                 about = EXCLUDED.about,
                 accent = EXCLUDED.accent,
                 showcases = EXCLUDED.showcases,
                 version = profiles.version + 1,
                 updated_at = now()
             RETURNING version",
        )
        .bind(user.0)
        .bind(&text.real_name)
        .bind(&text.location)
        .bind(&text.about)
        .bind(&text.accent)
        .bind(&showcases)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        self.profile_changed(user, version).await
    }

    /// Sobe a versão da página (criando a linha, se ainda não tem).
    async fn bump_profile(&self, user: UserId) -> Result<i64, String> {
        let (version,): (i64,) = sqlx::query_as(
            "INSERT INTO profiles (user_id, version) VALUES ($1, 1)
             ON CONFLICT (user_id) DO UPDATE
                 SET version = profiles.version + 1, updated_at = now()
             RETURNING version",
        )
        .bind(user.0)
        .fetch_one(&self.db)
        .await
        .map_err(db_error)?;
        Ok(version)
    }

    async fn profile_changed(&self, user: UserId, version: i64) -> Result<(), String> {
        let mut recipients = self.people_sharing_with(user).await?;
        if !recipients.contains(&user) {
            recipients.push(user);
        }
        self.emit(
            recipients,
            ServerMessage::ProfileChanged {
                user_id: user,
                version,
            },
        );
        Ok(())
    }

    /// Guarda a imagem (já conferida) e avisa quem pode ver a página.
    pub async fn set_profile_image(
        &self,
        user: UserId,
        kind: ProfileImage,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<(), Failure> {
        let path = self.profile_image_path(user, kind);
        let dir = path.parent().expect("a pasta do perfil");
        tokio::fs::create_dir_all(dir).await.map_err(internal)?;
        // Escreve ao lado e troca de nome: quem baixa no meio nunca pega metade.
        let partial = path.with_extension("parcial");
        tokio::fs::write(&partial, bytes).await.map_err(internal)?;
        tokio::fs::rename(&partial, &path).await.map_err(internal)?;
        let version = self
            .bump_profile(user)
            .await
            .map_err(|reason| (StatusCode::INTERNAL_SERVER_ERROR, reason))?;
        sqlx::query(
            "INSERT INTO profile_images (user_id, kind, content_type, version)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id, kind) DO UPDATE
                 SET content_type = EXCLUDED.content_type, version = EXCLUDED.version",
        )
        .bind(user.0)
        .bind(kind.slug())
        .bind(content_type)
        .bind(version)
        .execute(&self.db)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, db_error(err)))?;
        self.profile_changed(user, version)
            .await
            .map_err(|reason| (StatusCode::INTERNAL_SERVER_ERROR, reason))
    }

    /// Tira uma imagem da sua página.
    pub async fn remove_profile_image(
        &self,
        user: UserId,
        kind: ProfileImage,
    ) -> Result<(), Failure> {
        let removed = sqlx::query("DELETE FROM profile_images WHERE user_id = $1 AND kind = $2")
            .bind(user.0)
            .bind(kind.slug())
            .execute(&self.db)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, db_error(err)))?;
        let _ = tokio::fs::remove_file(self.profile_image_path(user, kind)).await;
        if removed.rows_affected() > 0 {
            let version = self
                .bump_profile(user)
                .await
                .map_err(|reason| (StatusCode::INTERNAL_SERVER_ERROR, reason))?;
            self.profile_changed(user, version)
                .await
                .map_err(|reason| (StatusCode::INTERNAL_SERVER_ERROR, reason))?;
        }
        Ok(())
    }

    /// O tipo e os bytes de uma imagem do perfil de `target`, para `user`.
    /// Quem não pode ver recebe o mesmo "não existe" de quem não tem imagem.
    pub async fn read_profile_image(
        &self,
        user: UserId,
        target: UserId,
        kind: ProfileImage,
    ) -> Result<(String, Vec<u8>), Failure> {
        let missing = || (StatusCode::NOT_FOUND, "Essa imagem não existe.".to_string());
        let allowed = self
            .can_see_profile(user, target)
            .await
            .map_err(|reason| (StatusCode::INTERNAL_SERVER_ERROR, reason))?;
        if !allowed {
            return Err(missing());
        }
        let found: Option<(String,)> = sqlx::query_as(
            "SELECT content_type FROM profile_images WHERE user_id = $1 AND kind = $2",
        )
        .bind(target.0)
        .bind(kind.slug())
        .fetch_optional(&self.db)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, db_error(err)))?;
        let (content_type,) = found.ok_or_else(missing)?;
        let bytes = tokio::fs::read(self.profile_image_path(target, kind))
            .await
            .map_err(|_| missing())?;
        Ok((content_type, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_and_checks_the_texts() {
        let text = ProfileText {
            real_name: "  João  ".into(),
            location: "Brasil".into(),
            about: " oi\nmais uma linha ".into(),
            accent: "#7DFF9B".into(),
            showcases: vec![Showcase::About, Showcase::About, Showcase::Art],
        };
        let clean = clean_text(&text).unwrap();
        assert_eq!(clean.real_name, "João");
        assert_eq!(clean.about, "oi\nmais uma linha");
        assert_eq!(clean.accent, "#7dff9b");
        assert_eq!(clean.showcases, [Showcase::About, Showcase::Art]);

        let bad_color = ProfileText {
            accent: "vermelho".into(),
            ..ProfileText::default()
        };
        assert!(clean_text(&bad_color).is_err());
        let long = ProfileText {
            location: "x".repeat(MAX_PROFILE_LINE_CHARS + 1),
            ..ProfileText::default()
        };
        assert!(clean_text(&long).is_err());
        let newline = ProfileText {
            real_name: "a\nb".into(),
            ..ProfileText::default()
        };
        assert!(clean_text(&newline).is_err());
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn checks_the_images() {
        assert!(check_image(ProfileImage::Avatar, "image/png", &png(256, 256)).is_ok());
        assert!(check_image(ProfileImage::Avatar, "image/png", &png(513, 100)).is_err());
        assert!(check_image(ProfileImage::Background, "image/png", &png(1920, 1080)).is_ok());
        // Declarou GIF, mandou PNG.
        assert!(check_image(ProfileImage::Art, "image/gif", &png(10, 10)).is_err());
        assert!(check_image(ProfileImage::Art, "image/webp", &png(10, 10)).is_err());
        assert!(check_image(ProfileImage::Art, "image/png", b"").is_err());
    }
}
