//! Ctrl+V no campo do chat. Arquivos copiados no Explorer ou uma imagem
//! (print, imagem copiada do navegador) viram anexo da conversa aberta.
//! Texto continua com o colar normal do campo.

use std::path::PathBuf;

/// O que tem na área de transferência e vira anexo.
pub enum Paste {
    Files(Vec<PathBuf>),
    Image(arboard::ImageData<'static>),
}

/// Lê a área de transferência. `None` quando é texto (ou nada), e o campo
/// cola sozinho. Texto vem antes da imagem porque o Word e o Excel põem uma
/// imagem junto do texto copiado, e aí o certo é colar o texto.
pub fn read() -> Option<Paste> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    if let Ok(files) = clipboard.get().file_list() {
        let files: Vec<PathBuf> = files.into_iter().filter(|path| path.is_file()).collect();
        if !files.is_empty() {
            return Some(Paste::Files(files));
        }
    }
    if clipboard.get_text().is_ok_and(|text| !text.is_empty()) {
        return None;
    }
    clipboard.get_image().ok().map(Paste::Image)
}

/// Grava a imagem colada como PNG numa pasta temporária, para seguir o mesmo
/// caminho de envio de um arquivo escolhido.
pub fn save_image(image: arboard::ImageData<'static>) -> Option<PathBuf> {
    let width = u32::try_from(image.width).ok()?;
    let height = u32::try_from(image.height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())?;
    let dir = std::env::temp_dir().join("mothh-colado");
    std::fs::create_dir_all(&dir).ok()?;
    let name = chrono::Local::now()
        .format("imagem-%Y%m%d-%H%M%S.png")
        .to_string();
    let path = dir.join(name);
    rgba.save(&path).ok()?;
    Some(path)
}
