//! Anexos: o app envia e baixa arquivos por HTTP, fora do WebSocket.
//!
//! O arquivo fica numa pasta local, com o id como nome; o banco guarda quem
//! enviou, o tipo, o tamanho e a duração ou nome. Mais
//! tarde a pasta vira um armazenamento tipo S3, sem mudar o protocolo.
//!
//! Enviar um arquivo não manda mensagem nenhuma: o app sobe o áudio, recebe o
//! id e depois manda `ClientMessage::SendAudio` pelo WebSocket.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opus_pure::OggOpusReader;
use protocol::{
    AUDIO_CONTENT_TYPE, AttachmentId, MAX_AUDIO_BYTES, MAX_AUDIO_MS, MAX_FILE_BYTES,
    MAX_FILE_NAME_CHARS, Uploaded, UserId,
};
use std::collections::HashMap;
use std::io::Cursor;
use uuid::Uuid;

use crate::auth::Session;
use crate::state::{AppState, db_error};

/// Uma resposta de erro: código HTTP e uma frase para a pessoa.
type Failure = (StatusCode, String);

/// `POST /anexos`: guarda o áudio e responde `{"id": "..."}`.
pub async fn upload(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(failure) => return failure.into_response(),
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let result = if content_type == Some(AUDIO_CONTENT_TYPE) && !params.contains_key("name") {
        state.store_audio(session.user_id, &body).await
    } else if let (Some(name), Some(content_type)) = (params.get("name"), content_type) {
        state
            .store_file(session.user_id, name, content_type, &body)
            .await
    } else {
        Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Arquivo sem nome ou tipo.".into(),
        ))
    };
    match result {
        Ok(id) => Json(Uploaded { id }).into_response(),
        Err(failure) => failure.into_response(),
    }
}

/// `GET /anexos/{id}`: devolve o arquivo para quem enviou ou para quem está
/// numa conversa onde ele apareceu.
pub async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let session = match authorize(&state, &headers).await {
        Ok(session) => session,
        Err(failure) => return failure.into_response(),
    };
    match state
        .read_attachment(session.user_id, AttachmentId(id))
        .await
    {
        Ok((content_type, bytes)) => {
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(failure) => failure.into_response(),
    }
}

/// Confere o `Authorization: Bearer <token>`, o mesmo token do `Resume`.
async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<Session, Failure> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Entre na sua conta primeiro.".into(),
        ))?;
    state
        .resume(token.trim())
        .await
        .map_err(|reason| (StatusCode::UNAUTHORIZED, reason))
}

/// Duração de um Ogg Opus em milissegundos, lida do próprio arquivo: a
/// posição final (em amostras a 48 kHz) menos o atraso do codificador.
/// `None` quando o arquivo não é um Opus válido ou não tem áudio.
pub fn audio_duration_ms(bytes: &[u8]) -> Option<u32> {
    let mut reader = OggOpusReader::new(bytes).ok()?;
    let pre_skip = i64::from(reader.head().pre_skip);
    let mut last = -1;
    for packet in reader.packets() {
        let packet = packet.ok()?;
        if packet.page_granule >= 0 {
            last = packet.page_granule;
        }
    }
    let samples = last - pre_skip;
    if samples <= 0 {
        return None;
    }
    u32::try_from(samples * 1000 / 48_000).ok()
}

/// Falha do servidor, sem detalhe para a pessoa (o detalhe vai para o log).
fn internal(err: impl std::fmt::Display) -> Failure {
    tracing::error!(%err, "erro com anexo");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Erro no servidor, tente de novo.".into(),
    )
}

impl AppState {
    /// Confere o áudio, salva na pasta de anexos e registra no banco.
    pub async fn store_audio(
        &self,
        uploader: UserId,
        bytes: &[u8],
    ) -> Result<AttachmentId, Failure> {
        if bytes.len() > MAX_AUDIO_BYTES {
            return Err((StatusCode::PAYLOAD_TOO_LARGE, "Áudio grande demais.".into()));
        }
        let duration_ms = audio_duration_ms(bytes).ok_or((
            StatusCode::BAD_REQUEST,
            "O arquivo não é um áudio Opus válido.".to_string(),
        ))?;
        if duration_ms > MAX_AUDIO_MS {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("O áudio passou de {} minutos.", MAX_AUDIO_MS / 60_000),
            ));
        }

        let id = AttachmentId(Uuid::new_v4());
        let dir = self.attachments_dir.as_path();
        tokio::fs::create_dir_all(dir).await.map_err(internal)?;
        let path = dir.join(id.0.to_string());
        tokio::fs::write(&path, bytes).await.map_err(internal)?;

        let inserted = sqlx::query(
            "INSERT INTO attachments (id, uploader_id, content_type, size_bytes, duration_ms)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id.0)
        .bind(uploader.0)
        .bind(AUDIO_CONTENT_TYPE)
        .bind(bytes.len() as i32)
        .bind(duration_ms as i32)
        .execute(&self.db)
        .await;
        if let Err(err) = inserted {
            // Sem a linha no banco, o arquivo ficaria perdido na pasta.
            let _ = tokio::fs::remove_file(&path).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, db_error(err)));
        }
        Ok(id)
    }

    /// Guarda imagem ou arquivo comum. O nome nunca vira caminho no disco.
    pub async fn store_file(
        &self,
        uploader: UserId,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<AttachmentId, Failure> {
        let name = name.trim();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.chars().count() > MAX_FILE_NAME_CHARS
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return Err((StatusCode::BAD_REQUEST, "Nome de arquivo inválido.".into()));
        }
        if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Arquivo grande demais.".into(),
            ));
        }
        if content_type.starts_with("image/") {
            validate_image(bytes, content_type)?;
        } else if content_type != "application/pdf" && content_type != "application/octet-stream" {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Tipo de arquivo inválido.".into(),
            ));
        }

        let id = AttachmentId(Uuid::new_v4());
        let dir = self.attachments_dir.as_path();
        tokio::fs::create_dir_all(dir).await.map_err(internal)?;
        let path = dir.join(id.0.to_string());
        tokio::fs::write(&path, bytes).await.map_err(internal)?;
        let inserted = sqlx::query(
            "INSERT INTO attachments (id, uploader_id, content_type, size_bytes, file_name)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id.0)
        .bind(uploader.0)
        .bind(content_type)
        .bind(bytes.len() as i32)
        .bind(name)
        .execute(&self.db)
        .await;
        if let Err(err) = inserted {
            let _ = tokio::fs::remove_file(&path).await;
            return Err((StatusCode::INTERNAL_SERVER_ERROR, db_error(err)));
        }
        Ok(id)
    }

    /// O tipo e o conteúdo do anexo, se `user` pode vê-lo. Quem não pode
    /// recebe o mesmo "não existe" de um id inventado.
    pub async fn read_attachment(
        &self,
        user: UserId,
        id: AttachmentId,
    ) -> Result<(String, Vec<u8>), Failure> {
        let found: Option<(String,)> = sqlx::query_as(
            "SELECT a.content_type FROM attachments a
             WHERE a.id = $1 AND (
                 a.uploader_id = $2
                 OR EXISTS (
                     SELECT 1 FROM messages m
                     JOIN conversation_members cm ON cm.conversation_id = m.conversation_id
                     WHERE m.attachment_id = a.id AND cm.user_id = $2
                 )
             )",
        )
        .bind(id.0)
        .bind(user.0)
        .fetch_optional(&self.db)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, db_error(err)))?;
        let (content_type,) =
            found.ok_or((StatusCode::NOT_FOUND, "Esse anexo não existe.".to_string()))?;
        let bytes = tokio::fs::read(self.attachments_dir.join(id.0.to_string()))
            .await
            .map_err(internal)?;
        Ok((content_type, bytes))
    }
}

/// Só formatos raster conhecidos viram miniatura no cliente.
fn validate_image(bytes: &[u8], content_type: &str) -> Result<(), Failure> {
    use image::ImageFormat;
    let expected = match content_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Imagem não suportada.".into(),
            ));
        }
    };
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| (StatusCode::BAD_REQUEST, "Imagem inválida.".into()))?;
    if reader.format() != Some(expected) {
        return Err((StatusCode::BAD_REQUEST, "Imagem inválida.".into()));
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| (StatusCode::BAD_REQUEST, "Imagem inválida.".into()))?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 20_000_000 {
        return Err((StatusCode::BAD_REQUEST, "Imagem grande demais.".into()));
    }
    Ok(())
}
