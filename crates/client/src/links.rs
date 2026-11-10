//! Links no chat: achar no texto, deixar clicável e a prévia do site.
//!
//! A prévia (título, descrição e uma miniatura pequena) é buscada só por
//! quem envia, antes de cifrar, e vai junto do texto dentro da mensagem
//! cifrada. Quem recebe não acessa o site: nem o IP de quem recebe nem o
//! fato de a mensagem ter sido lida vazam para fora. GIF e imagem por link
//! sobem como anexo e saem logo depois do texto, como imagem enviada: quem
//! recebe baixa do servidor do mothh, não do site.
//!
//! O Slint não tem navegador embutido: vídeo do YouTube vira a miniatura
//! com ▶, que abre no navegador.

use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

/// Separa o texto da prévia no que é cifrado. Versão antiga do app mostra
/// o que vem depois como texto (todo mundo se atualiza sozinho).
const MARK: &str = "\u{1e}mothh-previa:";

/// Tamanho máximo do texto mais a prévia, antes de cifrar. O servidor
/// aceita até `MAX_SEALED_BYTES` cifrados; sobra espaço para o MLS.
pub const MAX_PLAIN_BYTES: usize = protocol::MAX_SEALED_BYTES - 1500;

/// Miniatura da prévia: cabe na mensagem (JPEG pequeno).
pub const THUMB_MAX_BYTES: usize = 9000;

/// O que o card mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Página comum: card com imagem, site, título e descrição.
    #[default]
    Link,
    /// Vídeo (YouTube): miniatura com ▶.
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Preview {
    pub url: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub site: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// JPEG pequeno, em base64 no JSON.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "b64")]
    pub thumb: Option<Vec<u8>>,
}

mod b64 {
    use super::BASE64;
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => s.serialize_str(&BASE64.encode(bytes)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let text = Option::<String>::deserialize(d)?;
        Ok(text.and_then(|t| BASE64.decode(t).ok()))
    }
}

/// Junta o texto e a prévia no que vai ser cifrado. Se não couber, tira a
/// miniatura, depois a descrição; se nem assim, vai só o texto.
pub fn attach(text: &str, preview: &Preview) -> String {
    let mut preview = preview.clone();
    for step in 0..3 {
        match step {
            1 => preview.thumb = None,
            2 => preview.description.clear(),
            _ => {}
        }
        if let Ok(json) = serde_json::to_string(&preview) {
            let full = format!("{text}{MARK}{json}");
            if full.len() <= MAX_PLAIN_BYTES {
                return full;
            }
        }
    }
    text.to_string()
}

/// O contrário de [`attach`]: o texto que aparece e a prévia, se tiver.
pub fn split(plain: &str) -> (&str, Option<Preview>) {
    match plain.split_once(MARK) {
        Some((text, json)) => (text, serde_json::from_str(json).ok()),
        None => (plain, None),
    }
}

/// Os links do texto, em posições de byte. "https://", "http://" e
/// "www.", sem a pontuação que costuma vir colada no fim da frase.
pub fn find(text: &str) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let next = ["https://", "http://", "www."]
            .iter()
            .filter_map(|p| lower[from..].find(p).map(|i| (from + i, p.len())))
            .min();
        let Some((start, prefix)) = next else { break };
        let starts_word = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '/' && c != '.' && c != '@');
        let end = start
            + text[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
                .unwrap_or(text.len() - start);
        let end = start + trim_end(&text[start..end]);
        if starts_word
            && end > start + prefix
            && text[start + prefix..end].contains(|c: char| c.is_alphanumeric())
        {
            found.push((start, end));
            from = end;
        } else {
            from = start + prefix;
        }
    }
    found
}

/// Tira do fim o ponto final, a vírgula e o parêntese que fecha sem ter
/// aberto dentro do link: "(veja https://x.com/a)." fica "https://x.com/a".
fn trim_end(link: &str) -> usize {
    let mut end = link.len();
    loop {
        let Some(last) = link[..end].chars().next_back() else {
            return end;
        };
        let unbalanced = |open: char, close: char| {
            last == close && link[..end].matches(open).count() < link[..end].matches(close).count()
        };
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '…')
            || unbalanced('(', ')')
            || unbalanced('[', ']')
            || unbalanced('{', '}')
        {
            end -= last.len_utf8();
        } else {
            return end;
        }
    }
}

/// O endereço para abrir: "www.x.com" vira "https://www.x.com".
pub fn href(link: &str) -> String {
    let lower = link.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        link.to_string()
    } else {
        format!("https://{link}")
    }
}

/// O link na posição `offset` (em bytes) do texto, se o clique caiu num.
pub fn at(text: &str, offset: usize) -> Option<String> {
    find(text)
        .into_iter()
        .find(|&(start, end)| (start..=end).contains(&offset))
        .map(|(start, end)| href(&text[start..end]))
}

/// O primeiro link do texto, para a prévia.
pub fn first(text: &str) -> Option<String> {
    find(text).first().map(|&(s, e)| href(&text[s..e]))
}

/// Só endereços da web abrem no navegador (nada de `file:` ou programas).
pub fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Abre o link no navegador padrão.
/// Trava contra rajada: no máximo um link por segundo e meio. Um clique de
/// verdade nunca é mais rápido que isso; um bug que mexa no cursor de várias
/// mensagens seguidas não enche o navegador de abas.
fn take_open_slot(now: std::time::Instant) -> bool {
    use std::sync::Mutex;
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(1500)) {
        return false;
    }
    *last = Some(now);
    true
}

pub fn open_in_browser(url: &str) {
    if !openable(url) || !take_open_slot(std::time::Instant::now()) {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sem janela de console piscando.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// O id do vídeo num link do YouTube.
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let host = host.trim_start_matches("www.").trim_start_matches("m.");
    let id = match host {
        "youtu.be" => path.split(['?', '#', '/']).next(),
        "youtube.com" | "music.youtube.com" => {
            if let Some(query) = path.strip_prefix("watch?").or_else(|| {
                path.split_once('?')
                    .filter(|(p, _)| *p == "watch")
                    .map(|(_, q)| q)
            }) {
                query
                    .split(['&', '#'])
                    .find_map(|pair| pair.strip_prefix("v="))
            } else {
                ["shorts/", "embed/", "live/"]
                    .iter()
                    .find_map(|p| path.strip_prefix(p))
                    .and_then(|r| r.split(['?', '#', '/']).next())
            }
        }
        _ => None,
    }?;
    (id.len() >= 6
        && id.len() <= 20
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then(|| id.to_string())
}

/// Sites de GIF: a imagem da página é o próprio GIF animado.
pub fn is_gif_site(url: &str) -> bool {
    let host = host(url);
    ["tenor.com", "giphy.com", "gph.is", "imgur.com"]
        .iter()
        .any(|site| host == *site || host.ends_with(&format!(".{site}")))
}

/// "www.youtube.com" de "https://www.youtube.com/watch?v=...".
pub fn host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

/// O que interessa do HTML de uma página.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub title: String,
    pub description: String,
    pub site: String,
    pub image: String,
}

/// Lê as tags `og:` (e `twitter:`, e o `<title>`) do começo da página.
pub fn parse_html(html: &str) -> Meta {
    let lower = html.to_ascii_lowercase();
    let mut meta = Meta::default();
    let mut fallback_title = String::new();
    let mut fallback_description = String::new();
    let mut fallback_image = String::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start + 5..start + len];
        from = start + len;
        let attrs = attributes(tag);
        let get = |key: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        let Some(content) = get("content") else {
            continue;
        };
        let content = decode_entities(content.trim());
        let key = get("property")
            .or_else(|| get("name"))
            .unwrap_or("")
            .to_ascii_lowercase();
        let slot = match key.as_str() {
            "og:title" => &mut meta.title,
            "twitter:title" => &mut fallback_title,
            "og:description" => &mut meta.description,
            "twitter:description" | "description" => &mut fallback_description,
            "og:site_name" => &mut meta.site,
            "og:image" | "og:image:url" | "og:image:secure_url" => &mut meta.image,
            "twitter:image" | "twitter:image:src" => &mut fallback_image,
            _ => continue,
        };
        if slot.is_empty() {
            *slot = content;
        }
    }
    if meta.title.is_empty() {
        meta.title = fallback_title;
    }
    if meta.title.is_empty()
        && let Some(start) = lower.find("<title")
        && let Some(open) = lower[start..].find('>')
        && let Some(close) = lower[start + open..].find("</title")
    {
        meta.title = decode_entities(html[start + open + 1..start + open + close].trim());
    }
    if meta.description.is_empty() {
        meta.description = fallback_description;
    }
    if meta.image.is_empty() {
        meta.image = fallback_image;
    }
    meta.title = clip(&squash(&meta.title), 150);
    meta.description = clip(&squash(&meta.description), 280);
    meta.site = clip(&squash(&meta.site), 60);
    meta
}

/// `name="valor"`, `name='valor'` e `name=valor` de dentro de uma tag.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_ascii_lowercase();
        if key.contains(char::is_whitespace) {
            // Atributo sem valor antes ("async name=..."): pula a palavra.
            let skip = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = &rest[skip..];
            continue;
        }
        let value_part = rest[eq + 1..].trim_start();
        let (value, after) = match value_part.chars().next() {
            Some(q @ ('"' | '\'')) => match value_part[1..].find(q) {
                Some(close) => (&value_part[1..1 + close], &value_part[close + 2..]),
                None => (&value_part[1..], ""),
            },
            _ => {
                let end = value_part
                    .find(char::is_whitespace)
                    .unwrap_or(value_part.len());
                (&value_part[..end], &value_part[end..])
            }
        };
        out.push((key, value.to_string()));
        rest = after;
    }
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(10)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "middot" => Some('·'),
            "bull" => Some('•'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "trade" => Some('™'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Reduz a imagem da prévia até caber em [`THUMB_MAX_BYTES`].
pub fn encode_thumb(image: &image::DynamicImage) -> Option<Vec<u8>> {
    for (width, quality) in [(360, 72), (320, 60), (240, 55), (180, 50)] {
        let small = image.thumbnail(width, width * 9 / 16).to_rgb8();
        let mut bytes = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality);
        if small.write_with_encoder(encoder).is_ok() && bytes.len() <= THUMB_MAX_BYTES {
            return Some(bytes);
        }
    }
    None
}

/// GIF ou imagem por link sobe como anexo: até este tamanho.
const MEDIA_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Do HTML só interessa o começo, onde ficam as tags `og:`.
const HTML_MAX_BYTES: usize = 768 * 1024;
/// A imagem da prévia antes de reduzir.
const IMAGE_MAX_BYTES: usize = 6 * 1024 * 1024;
/// Quanto quem envia espera pela prévia antes de mandar sem ela.
pub const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// O cliente HTTP para os sites dos links (separado do servidor do mothh).
pub fn web_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; mothh-preview/1.0)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(FETCH_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// O que mandar junto do link: a prévia, ou (link de GIF ou imagem) o
/// arquivo para subir como anexo (bytes, tipo e nome).
pub enum Fetched {
    Preview(Preview),
    Media(Vec<u8>, &'static str, String),
}

/// Busca a prévia do link. `None` quando não tem nada para mostrar.
pub async fn fetch(web: &reqwest::Client, url: &str) -> Option<Fetched> {
    tokio::time::timeout(FETCH_TIMEOUT, fetch_inner(web, url))
        .await
        .ok()
        .flatten()
}

async fn fetch_inner(web: &reqwest::Client, url: &str) -> Option<Fetched> {
    if let Some(id) = youtube_id(url) {
        return youtube(web, url, &id).await;
    }
    let response = web
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,image/*;q=0.9,*/*;q=0.5",
        )
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let base = response.url().clone();
    let content_type = content_type(&response);
    if content_type.starts_with("image/") {
        let bytes = read_capped(response, MEDIA_MAX_BYTES).await?;
        return media(bytes);
    }
    if !content_type.contains("html") {
        return None;
    }
    let html = read_capped(response, HTML_MAX_BYTES).await?;
    let meta = parse_html(&String::from_utf8_lossy(&html));
    let image_url = (!meta.image.is_empty())
        .then(|| base.join(&meta.image).ok())
        .flatten()
        .filter(|u| matches!(u.scheme(), "http" | "https"));
    let image = match &image_url {
        Some(image_url) => get_image(web, image_url.as_str()).await,
        None => None,
    };
    // Tenor, Giphy...: a imagem da página é o GIF. Vai como anexo, animado.
    if is_gif_site(url)
        && let Some(bytes) = &image
        && image::guess_format(bytes).ok() == Some(image::ImageFormat::Gif)
        && let Some(fetched) = media(bytes.clone())
    {
        return Some(fetched);
    }
    let thumb = match image {
        Some(bytes) => tokio::task::spawn_blocking(move || {
            image::load_from_memory(&bytes)
                .ok()
                .and_then(|image| encode_thumb(&image))
        })
        .await
        .ok()
        .flatten(),
        None => None,
    };
    if meta.title.is_empty() && meta.description.is_empty() && thumb.is_none() {
        return None;
    }
    let site = if meta.site.is_empty() {
        host(base.as_str()).trim_start_matches("www.").to_string()
    } else {
        meta.site
    };
    Some(Fetched::Preview(Preview {
        url: url.to_string(),
        kind: Kind::Link,
        site,
        title: meta.title,
        description: meta.description,
        thumb,
    }))
}

/// Vídeo do YouTube: título e canal pelo oEmbed, e a miniatura 16:9.
async fn youtube(web: &reqwest::Client, url: &str, id: &str) -> Option<Fetched> {
    let mut preview = Preview {
        url: url.to_string(),
        kind: Kind::Video,
        site: "YouTube".into(),
        ..Default::default()
    };
    let oembed = format!(
        "https://www.youtube.com/oembed?format=json&url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3D{id}"
    );
    if let Ok(response) = web.get(oembed).send().await
        && response.status().is_success()
        && let Some(body) = read_capped(response, 64 * 1024).await
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body)
    {
        let text = |key: &str| {
            json.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        preview.title = clip(&squash(&text("title")), 150);
        preview.description = clip(&squash(&text("author_name")), 80);
    }
    if let Some(bytes) = get_image(web, &format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")).await
    {
        preview.thumb = tokio::task::spawn_blocking(move || {
            image::load_from_memory(&bytes)
                .ok()
                .and_then(|image| encode_thumb(&image))
        })
        .await
        .ok()
        .flatten();
    }
    // Sem título nem miniatura (YouTube fora do ar), o card ficaria vazio.
    (!preview.title.is_empty() || preview.thumb.is_some()).then_some(Fetched::Preview(preview))
}

/// O link é a própria imagem: confere que abre e prepara para subir.
fn media(bytes: Vec<u8>) -> Option<Fetched> {
    let (content_type, ext) = match image::guess_format(&bytes).ok()? {
        image::ImageFormat::Gif => ("image/gif", "gif"),
        image::ImageFormat::Png => ("image/png", "png"),
        image::ImageFormat::Jpeg => ("image/jpeg", "jpg"),
        image::ImageFormat::WebP => ("image/webp", "webp"),
        _ => return None,
    };
    let name = format!("{}.{ext}", if ext == "gif" { "gif" } else { "imagem" });
    Some(Fetched::Media(bytes, content_type, name))
}

async fn get_image(web: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let response = web
        .get(url)
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() || !content_type(&response).starts_with("image/") {
        return None;
    }
    read_capped(response, IMAGE_MAX_BYTES).await
}

fn content_type(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Lê o corpo até `max` bytes. HTML maior é cortado (o começo basta);
/// imagem maior não serve.
async fn read_capped(mut response: reqwest::Response, max: usize) -> Option<Vec<u8>> {
    let is_html = content_type(&response).contains("html");
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > max {
            if is_html {
                body.truncate(max);
                break;
            }
            return None;
        }
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<&str> {
        find(text).into_iter().map(|(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn acha_links() {
        assert_eq!(
            links("olha https://asciilands.io/play/ e www.site.com."),
            ["https://asciilands.io/play/", "www.site.com"]
        );
        assert_eq!(
            links("(veja https://x.com/a_(b))."),
            ["https://x.com/a_(b)"]
        );
        assert_eq!(links("(veja https://x.com/a)."), ["https://x.com/a"]);
        assert_eq!(
            links("https://asciilands.io/play/\\"),
            ["https://asciilands.io/play/\\"]
        );
        assert!(links("email@www.site.com e https:// sozinho").is_empty());
        assert_eq!(links("HTTPS://X.COM?q=1&b=2,"), ["HTTPS://X.COM?q=1&b=2"]);
        assert_eq!(
            at("ver www.x.com agora", 6).as_deref(),
            Some("https://www.x.com")
        );
        assert_eq!(at("ver www.x.com agora", 16), None);
    }

    #[test]
    fn previa_vai_e_volta() {
        let preview = Preview {
            url: "https://x.com".into(),
            kind: Kind::Video,
            title: "Título".into(),
            thumb: Some(vec![1, 2, 3]),
            ..Default::default()
        };
        let full = attach("olha https://x.com", &preview);
        let (text, back) = split(&full);
        assert_eq!(text, "olha https://x.com");
        assert_eq!(back, Some(preview));
        assert_eq!(split("só texto"), ("só texto", None));
    }

    #[test]
    fn previa_grande_demais_perde_a_miniatura() {
        let preview = Preview {
            url: "https://x.com".into(),
            thumb: Some(vec![7; MAX_PLAIN_BYTES]),
            description: "d".into(),
            ..Default::default()
        };
        let (_, back) = split(&attach("oi", &preview));
        let back = back.unwrap();
        assert!(back.thumb.is_none());
        assert_eq!(back.description, "d");
    }

    #[test]
    fn youtube() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?feature=x&v=dQw4w9WgXcQ#t=1",
            "https://youtu.be/dQw4w9WgXcQ?si=abc",
            "https://m.youtube.com/shorts/dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
        ] {
            assert_eq!(youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
        }
        assert_eq!(youtube_id("https://youtube.com/"), None);
        assert_eq!(
            youtube_id("https://notyoutube.com/watch?v=dQw4w9WgXcQ"),
            None
        );
    }

    #[test]
    fn le_meta_tags() {
        let html = r#"<html><head><title>Fallback</title>
            <meta name="description" content="desc &amp; mais">
            <meta property='og:title' content='Ascii &quot;Lands&quot; &mdash; jogo' />
            <meta content="https://img/x.png" property="og:image">
            <meta property=og:site_name content=Itch>
            </head>"#;
        let meta = parse_html(html);
        assert_eq!(meta.title, "Ascii \"Lands\" — jogo");
        assert_eq!(meta.description, "desc & mais");
        assert_eq!(meta.image, "https://img/x.png");
        assert_eq!(meta.site, "Itch");
        assert_eq!(
            parse_html("<title> Só\n título </title>").title,
            "Só título"
        );
    }

    #[test]
    fn abre_so_web() {
        assert!(openable("https://x.com/a?b=1&c=2"));
        assert!(!openable("file:///c:/windows"));
        assert!(!openable("https://x.com/a b"));
        assert!(is_gif_site("https://media1.tenor.com/x"));
        assert_eq!(host("https://user@Www.X.com:8080/a"), "www.x.com");
    }
}
