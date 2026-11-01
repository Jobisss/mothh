//! Fotos de perfil dos outros (a sua fica no arquivo do perfil e na tela).
//!
//! A foto sobe para o servidor como um PNG quadrado de `PHOTO_SIZE` px.
//! Quem divide uma conversa com a pessoa pede a foto quando precisa dela
//! (hoje: quando ela aparece numa chamada) e pede de novo quando chega um
//! `PhotoChanged` com versão nova.
//!
//! Servidor antigo (sem `photo_version` no `Welcome`) não guarda fotos: aí o
//! app não manda nem pede nada, e cada um vê só a própria foto.
//!
//! A rede roda numa thread do Tokio e a `slint::Image` só vive na thread da
//! janela: a rede decodifica o PNG e manda os pixels; as imagens prontas
//! ficam em `IMAGES`, do lado da janela.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use image::{DynamicImage, ImageFormat};
use protocol::{ClientMessage, PHOTO_MAX_BYTES, PHOTO_SIZE, Photo, UserId};

#[derive(Default)]
pub struct Photos {
    /// O servidor guarda fotos (veio `photo_version` no `Welcome`).
    supported: bool,
    /// Versão da foto que já temos de cada pessoa (0: ela não tem foto).
    versions: HashMap<UserId, i64>,
    /// Pedimos e esperamos o `Photo`.
    pending: HashSet<UserId>,
    /// A sua foto nova, esperando o servidor (sem conexão ou servidor antigo).
    upload: Option<Vec<u8>>,
}

impl Photos {
    /// Entrou na conta. Quem cai e volta pode ter perdido `PhotoChanged`:
    /// esquece as versões (as imagens continuam na tela até chegar a nova).
    /// Manda a sua foto se o servidor ainda não tem e ela existe aqui.
    pub fn welcome(
        &mut self,
        my_version: Option<i64>,
        local_photo: Option<&Path>,
        out: &mut Vec<ClientMessage>,
    ) {
        self.supported = my_version.is_some();
        self.versions.clear();
        self.pending.clear();
        if !self.supported {
            return;
        }
        if self.upload.is_none()
            && my_version == Some(0)
            && let Some(path) = local_photo
            && let Ok(image) = image::open(path)
        {
            self.upload = encode(&image).ok();
        }
        if let Some(png) = self.upload.take() {
            out.push(ClientMessage::SetPhoto { png: Photo(png) });
        }
    }

    /// Você escolheu uma foto nova. Vai agora se der; senão no próximo `Welcome`.
    pub fn set_mine(&mut self, png: Vec<u8>, online: bool, out: &mut Vec<ClientMessage>) {
        if online && self.supported {
            out.push(ClientMessage::SetPhoto { png: Photo(png) });
        } else {
            self.upload = Some(png);
        }
    }

    /// Pede a foto de quem ainda não temos (e não pedimos).
    pub fn want(&mut self, users: impl IntoIterator<Item = UserId>, out: &mut Vec<ClientMessage>) {
        if !self.supported {
            return;
        }
        for user_id in users {
            if !self.versions.contains_key(&user_id) && self.pending.insert(user_id) {
                out.push(ClientMessage::GetPhoto { user_id });
            }
        }
    }

    /// Chegou a foto (ou a falta dela). Devolve os pixels para a janela:
    /// `None` quando a pessoa não tem foto ou a imagem não abriu.
    pub fn received(
        &mut self,
        user_id: UserId,
        version: i64,
        png: Option<&[u8]>,
    ) -> Option<Pixels> {
        self.pending.remove(&user_id);
        self.versions.insert(user_id, version);
        png.and_then(|png| decode(png).ok())
    }

    /// Alguém trocou a foto: pede de novo se é de alguém que já mostramos.
    pub fn changed(&mut self, user_id: UserId, version: i64, out: &mut Vec<ClientMessage>) {
        if let Some(&known) = self.versions.get(&user_id)
            && known != version
        {
            self.versions.remove(&user_id);
            self.want([user_id], out);
        }
    }
}

/// Imagem decodificada, pronta para virar `slint::Image` na thread da janela.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Recorta o meio quadrado, reduz para `PHOTO_SIZE` e codifica em PNG.
pub fn encode(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let side = image.width().min(image.height());
    let square = image.crop_imm(
        (image.width() - side) / 2,
        (image.height() - side) / 2,
        side,
        side,
    );
    let small = square.resize_exact(
        PHOTO_SIZE,
        PHOTO_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    let mut png = Vec::new();
    small
        .to_rgba8()
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("não deu para preparar a foto: {e}"))?;
    if png.len() > PHOTO_MAX_BYTES {
        return Err("a foto ficou grande demais".into());
    }
    Ok(png)
}

/// Abre o PNG de alguém. Recusa o que passa do tamanho combinado.
fn decode(png: &[u8]) -> Result<Pixels, String> {
    if png.len() > PHOTO_MAX_BYTES {
        return Err("foto grande demais".into());
    }
    let image =
        image::load_from_memory_with_format(png, ImageFormat::Png).map_err(|e| e.to_string())?;
    if image.width() > PHOTO_SIZE || image.height() > PHOTO_SIZE {
        return Err("foto grande demais".into());
    }
    let rgba = image.to_rgba8();
    Ok(Pixels {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

thread_local! {
    /// Fotos prontas, por id da pessoa. Só na thread da janela.
    static IMAGES: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
}

/// Guarda (ou apaga, com `None`) a foto de alguém. Na thread da janela.
pub fn store(id: &str, pixels: Option<Pixels>) {
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        match pixels {
            Some(p) => {
                let buffer = slint::SharedPixelBuffer::clone_from_slice(&p.rgba, p.width, p.height);
                images.insert(id.to_string(), slint::Image::from_rgba8(buffer));
            }
            None => {
                images.remove(id);
            }
        }
    });
}

/// A foto de alguém, se já chegou. Na thread da janela.
pub fn image(id: &str) -> Option<slint::Image> {
    IMAGES.with(|images| images.borrow().get(id).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> UserId {
        UserId(uuid::Uuid::from_u128(1))
    }

    #[test]
    fn encode_makes_a_small_square_png_that_decodes() {
        let wide = DynamicImage::new_rgba8(300, 200);
        let png = encode(&wide).unwrap();
        let back = decode(&png).unwrap();
        assert_eq!((back.width, back.height), (PHOTO_SIZE, PHOTO_SIZE));
        // E passa no que o servidor confere (assinatura e IHDR).
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
    }

    #[test]
    fn old_server_gets_no_photo_messages() {
        let mut photos = Photos::default();
        let mut out = Vec::new();
        photos.welcome(None, None, &mut out);
        photos.want([me()], &mut out);
        photos.set_mine(vec![1], true, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn asks_once_and_again_only_when_the_version_changes() {
        let mut photos = Photos::default();
        let mut out = Vec::new();
        photos.welcome(Some(0), None, &mut out);
        photos.want([me()], &mut out);
        photos.want([me()], &mut out);
        assert_eq!(out.len(), 1, "pedido repetido enquanto espera");
        photos.received(me(), 2, None);
        out.clear();
        photos.want([me()], &mut out);
        photos.changed(me(), 2, &mut out);
        assert!(out.is_empty(), "já temos essa versão");
        photos.changed(me(), 3, &mut out);
        assert_eq!(out, [ClientMessage::GetPhoto { user_id: me() }]);
    }

    #[test]
    fn photo_picked_offline_goes_on_the_next_welcome() {
        let mut photos = Photos::default();
        let mut out = Vec::new();
        photos.set_mine(vec![9], false, &mut out);
        assert!(out.is_empty());
        photos.welcome(Some(5), None, &mut out);
        assert_eq!(
            out,
            [ClientMessage::SetPhoto {
                png: Photo(vec![9])
            }]
        );
    }
}
