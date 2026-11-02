//! Páginas de perfil: o mini cartão (um clique em alguém) e a página
//! completa ("ver perfil"), a sua e a dos outros.
//!
//! A rede (`net.rs`) guarda os perfis que chegaram e decide o que baixar;
//! as imagens vêm por HTTP e são abertas aqui, numa tarefa à parte. GIF vira
//! uma lista de quadros: o Slint não toca GIF, então um timer na thread da
//! janela troca o quadro (`start_animation`).
//!
//! Para não ocupar memória com GIFs de todo mundo, só ficam abertas as
//! imagens de quem está à vista (cartão ou página) e as suas.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::rc::Rc;
use std::time::{Duration, Instant};

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::{AnimationDecoder, DynamicImage, ImageFormat, RgbaImage};
use protocol::{ClientMessage, Profile, ProfileImage, ProfileText, Showcase, UserId};
use slint::ComponentHandle;

use crate::{AppWindow, ProfileViewData};

/// Onde o perfil aparece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// O mini cartão, aberto com um clique na pessoa.
    Card,
    /// A página completa, no lugar da conversa.
    Page,
}

/// Uma imagem que precisa ser baixada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub user: UserId,
    pub kind: ProfileImage,
    pub version: i64,
}

#[derive(Default)]
pub struct Profiles {
    /// O servidor guarda páginas de perfil (veio `profiles` no `Welcome`).
    supported: bool,
    me: Option<UserId>,
    known: HashMap<UserId, Profile>,
    /// Pedimos e esperamos o `Profile`.
    asked: HashSet<UserId>,
    /// Versão de cada imagem já baixada (ou baixando).
    images: HashMap<(UserId, ProfileImage), i64>,
    card: Option<UserId>,
    page: Option<UserId>,
}

impl Profiles {
    /// Entrou na conta. Quem cai e volta pode ter perdido `ProfileChanged`:
    /// os perfis são pedidos de novo (as imagens só se a versão mudou).
    pub fn welcome(&mut self, me: UserId, supported: bool, out: &mut Vec<ClientMessage>) {
        self.supported = supported;
        self.me = Some(me);
        self.known.clear();
        self.asked.clear();
        if !supported {
            self.images.clear();
            return;
        }
        for user in self.visible() {
            self.ask(user, out);
        }
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn get(&self, user: UserId) -> Option<&Profile> {
        self.known.get(&user)
    }

    pub fn showing(&self, place: Place) -> Option<UserId> {
        match place {
            Place::Card => self.card,
            Place::Page => self.page,
        }
    }

    /// Quem está à vista: o seu perfil sempre, mais o do cartão e o da página.
    fn visible(&self) -> Vec<UserId> {
        let mut users: Vec<UserId> = self.me.into_iter().collect();
        for user in [self.card, self.page].into_iter().flatten() {
            if !users.contains(&user) {
                users.push(user);
            }
        }
        users
    }

    fn ask(&mut self, user: UserId, out: &mut Vec<ClientMessage>) {
        if self.supported && self.asked.insert(user) {
            out.push(ClientMessage::GetProfile { user_id: user });
        }
    }

    /// Mostra o perfil de `user` no cartão ou na página. Pede ao servidor o
    /// que falta; devolve as imagens já conhecidas que faltam baixar e quem
    /// saiu de vista (as imagens dessa pessoa podem ser esquecidas).
    pub fn show(
        &mut self,
        user: UserId,
        place: Place,
        out: &mut Vec<ClientMessage>,
    ) -> (Vec<Download>, Vec<UserId>) {
        let before = self.visible();
        match place {
            Place::Card => self.card = Some(user),
            Place::Page => self.page = Some(user),
        }
        let gone = self.gone(before);
        if !self.known.contains_key(&user) {
            self.ask(user, out);
            return (Vec::new(), gone);
        }
        (self.missing_images(user), gone)
    }

    /// Fechou o cartão ou a página.
    pub fn hide(&mut self, place: Place) -> Vec<UserId> {
        let before = self.visible();
        match place {
            Place::Card => self.card = None,
            Place::Page => self.page = None,
        }
        self.gone(before)
    }

    fn gone(&mut self, before: Vec<UserId>) -> Vec<UserId> {
        let now = self.visible();
        let gone: Vec<UserId> = before.into_iter().filter(|u| !now.contains(u)).collect();
        for user in &gone {
            self.images.retain(|(owner, _), _| owner != user);
        }
        gone
    }

    /// Chegou um perfil. Devolve as imagens a baixar e as que a pessoa tirou.
    pub fn received(&mut self, profile: Profile) -> (Vec<Download>, Vec<ProfileImage>) {
        let user = profile.user_id;
        self.asked.remove(&user);
        let removed: Vec<ProfileImage> = ProfileImage::ALL
            .into_iter()
            .filter(|kind| {
                self.images.contains_key(&(user, *kind))
                    && !profile.images.iter().any(|image| image.kind == *kind)
            })
            .collect();
        for kind in &removed {
            self.images.remove(&(user, *kind));
        }
        self.known.insert(user, profile);
        let downloads = if self.visible().contains(&user) {
            self.missing_images(user)
        } else {
            Vec::new()
        };
        (downloads, removed)
    }

    fn missing_images(&mut self, user: UserId) -> Vec<Download> {
        let Some(profile) = self.known.get(&user) else {
            return Vec::new();
        };
        let mut downloads = Vec::new();
        for image in &profile.images {
            if self.images.get(&(user, image.kind)) != Some(&image.version) {
                downloads.push(Download {
                    user,
                    kind: image.kind,
                    version: image.version,
                });
            }
        }
        for download in &downloads {
            self.images
                .insert((download.user, download.kind), download.version);
        }
        downloads
    }

    /// A pessoa mudou a página. À vista: pede de novo; senão, esquece o
    /// texto guardado (na próxima vez que abrir, vem o novo).
    pub fn changed(&mut self, user: UserId, version: i64, out: &mut Vec<ClientMessage>) {
        if self.known.get(&user).is_some_and(|p| p.version == version) {
            return;
        }
        self.known.remove(&user);
        if self.visible().contains(&user) {
            self.asked.remove(&user);
            self.ask(user, out);
        }
    }

    /// O nome de alguém mudou: o perfil guardado também.
    pub fn renamed(&mut self, user: UserId, username: &str) {
        if let Some(profile) = self.known.get_mut(&user) {
            profile.username = username.to_string();
        }
    }
}

/// O que a janela mostra do perfil de alguém, ainda fora da thread da janela.
#[derive(Debug, Clone, Default)]
pub struct ViewRaw {
    pub id: String,
    pub is_me: bool,
    pub name: String,
    pub sub: String,
    pub about: String,
    /// "#rrggbb" ou vazio.
    pub accent: String,
    pub member_since: String,
    pub in_call: String,
    pub show_art: bool,
    pub show_about: bool,
    pub full: bool,
    pub loading: bool,
}

impl ViewRaw {
    /// Monta a partir do perfil (se já chegou) ou só do nome que a tela já sabe.
    pub fn new(
        user: UserId,
        fallback_name: &str,
        profile: Option<&Profile>,
        me: Option<UserId>,
        supported: bool,
        in_call: String,
    ) -> Self {
        let name = profile.map_or(fallback_name, |p| p.username.as_str());
        let text = profile.map(|p| p.text.clone()).unwrap_or_default();
        let sub = [text.real_name.as_str(), text.location.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        ViewRaw {
            id: user.0.to_string(),
            is_me: Some(user) == me,
            name: name.to_string(),
            sub,
            about: text.about.clone(),
            accent: text.accent.clone(),
            member_since: profile.map_or(String::new(), |p| member_since(p.member_since_ms)),
            in_call,
            show_art: text.showcases.contains(&Showcase::Art),
            show_about: text.showcases.contains(&Showcase::About),
            full: supported && profile.is_some(),
            loading: supported && profile.is_none(),
        }
    }

    /// Na thread da janela.
    pub fn to_view(&self) -> ProfileViewData {
        let accent = parse_color(&self.accent);
        ProfileViewData {
            id: self.id.clone().into(),
            is_me: self.is_me,
            name: self.name.clone().into(),
            initial: crate::net::initial_of(&self.name).into(),
            sub: self.sub.clone().into(),
            about: self.about.clone().into(),
            has_accent: accent.is_some(),
            accent: accent.unwrap_or_default(),
            member_since: self.member_since.clone().into(),
            in_call: self.in_call.clone().into(),
            show_art: self.show_art,
            show_about: self.show_about,
            full: self.full,
            loading: self.loading,
        }
    }
}

/// "#7dff9b" vira cor; o resto, nada.
pub fn parse_color(text: &str) -> Option<slint::Color> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(slint::Color::from_rgb_u8(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

/// "no mothh desde nov de 2026".
fn member_since(ms: i64) -> String {
    const MONTHS: [&str; 12] = [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ];
    if ms <= 0 {
        return String::new();
    }
    // Dias desde 1970 até ano e mês (algoritmo de Howard Hinnant).
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("no mothh desde {} de {year}", MONTHS[(month - 1) as usize])
}

/// Os textos que o editor manda salvar.
pub fn text_from_editor(
    real_name: &str,
    location: &str,
    about: &str,
    accent: &str,
    show_art: bool,
    show_about: bool,
) -> ProfileText {
    let mut showcases = Vec::new();
    if show_art {
        showcases.push(Showcase::Art);
    }
    if show_about {
        showcases.push(Showcase::About);
    }
    ProfileText {
        real_name: real_name.trim().to_string(),
        location: location.trim().to_string(),
        about: about.trim().to_string(),
        accent: accent.trim().to_string(),
        showcases,
    }
}

// ---------- imagens ----------

/// Um quadro aberto, pronto para virar `slint::Image` na thread da janela.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Quanto tempo o quadro fica na tela (0 numa imagem parada).
    pub delay_ms: u32,
}

/// Até que tamanho cada imagem é aberta para mostrar (a página nunca usa
/// maior que isso, e GIF grande com muitos quadros pesa na memória).
fn display_size(kind: ProfileImage) -> (u32, u32) {
    match kind {
        ProfileImage::Avatar => (256, 256),
        ProfileImage::Background => (1280, 720),
        ProfileImage::Art => (900, 900),
        ProfileImage::ArtSide => (450, 900),
    }
}

/// Mais quadros que isso: o GIF pula alguns (cada um fica mais tempo).
const MAX_FRAMES: usize = 60;
/// O GIF é lido até aqui; o resto da animação é cortado.
const READ_FRAMES: usize = 600;

fn fit(image: RgbaImage, (max_w, max_h): (u32, u32)) -> RgbaImage {
    if image.width() <= max_w && image.height() <= max_h {
        return image;
    }
    DynamicImage::ImageRgba8(image)
        .resize(max_w, max_h, image::imageops::FilterType::Triangle)
        .to_rgba8()
}

/// O meio quadrado da imagem (a foto do perfil é quadrada).
fn square(image: RgbaImage) -> RgbaImage {
    let side = image.width().min(image.height());
    if image.width() == image.height() {
        return image;
    }
    image::imageops::crop_imm(
        &image,
        (image.width() - side) / 2,
        (image.height() - side) / 2,
        side,
        side,
    )
    .to_image()
}

/// Navegadores tratam atraso de 0 ou 10 ms como 100 ms; aqui também.
fn frame_delay(delay: image::Delay) -> u32 {
    let (num, den) = delay.numer_denom_ms();
    let ms = num.checked_div(den).unwrap_or(0);
    if ms <= 10 { 100 } else { ms }
}

/// Os quadros de um GIF, já compostos (cada um é a imagem inteira).
fn gif_frames(bytes: &[u8]) -> Result<Vec<(RgbaImage, u32)>, String> {
    let decoder = GifDecoder::new(Cursor::new(bytes)).map_err(|_| "GIF inválido".to_string())?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames().take(READ_FRAMES) {
        let frame = frame.map_err(|_| "GIF inválido".to_string())?;
        let delay = frame_delay(frame.delay());
        frames.push((frame.into_buffer(), delay));
    }
    if frames.is_empty() {
        return Err("GIF vazio".into());
    }
    Ok(frames)
}

/// Fica com no máximo `MAX_FRAMES`, pulando quadros e somando os atrasos.
fn thin(frames: Vec<(RgbaImage, u32)>) -> Vec<(RgbaImage, u32)> {
    if frames.len() <= MAX_FRAMES {
        return frames;
    }
    let step = frames.len().div_ceil(MAX_FRAMES);
    let mut out = Vec::new();
    let mut iter = frames.into_iter().peekable();
    while let Some((image, delay)) = iter.next() {
        let mut total = delay;
        for _ in 1..step {
            match iter.next() {
                Some((_, d)) => total += d,
                None => break,
            }
        }
        out.push((image, total));
    }
    out
}

/// Abre uma imagem do perfil para mostrar: GIF vira quadros, o resto um só.
pub fn decode(kind: ProfileImage, bytes: &[u8]) -> Result<Vec<Frame>, String> {
    let size = display_size(kind);
    let frames = if image::guess_format(bytes).ok() == Some(ImageFormat::Gif) {
        thin(gif_frames(bytes)?)
    } else {
        let image = image::load_from_memory(bytes).map_err(|_| "imagem inválida".to_string())?;
        vec![(image.to_rgba8(), 0)]
    };
    let single = frames.len() == 1;
    Ok(frames
        .into_iter()
        .map(|(image, delay)| {
            let image = if kind == ProfileImage::Avatar {
                square(image)
            } else {
                image
            };
            let image = fit(image, size);
            Frame {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
                delay_ms: if single { 0 } else { delay },
            }
        })
        .collect())
}

/// Prepara uma imagem escolhida para o servidor: medidas e tamanho dentro do
/// limite do tipo. GIF continua GIF (reduzido se precisar); a foto vira
/// quadrada; imagem parada vai em PNG (foto) ou JPG (o resto, mais leve).
pub fn prepare_upload(kind: ProfileImage, bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let max = kind.max_size();
    let limit_mb = kind.max_bytes() / (1024 * 1024);
    let too_big = || format!("a imagem ficou maior que {limit_mb} MB");
    if image::guess_format(bytes).ok() == Some(ImageFormat::Gif) {
        let decoder =
            GifDecoder::new(Cursor::new(bytes)).map_err(|_| "GIF inválido".to_string())?;
        let (w, h) = image::ImageDecoder::dimensions(&decoder);
        let needs_square = kind == ProfileImage::Avatar && w != h;
        if w <= max.0 && h <= max.1 && !needs_square && bytes.len() <= kind.max_bytes() {
            return Ok((bytes.to_vec(), "image/gif"));
        }
        let frames = gif_frames(bytes)?;
        let mut out = Vec::new();
        {
            let mut encoder = GifEncoder::new_with_speed(&mut out, 10);
            encoder
                .set_repeat(Repeat::Infinite)
                .map_err(|_| "não deu para preparar o GIF".to_string())?;
            for (image, delay) in frames {
                let image = if kind == ProfileImage::Avatar {
                    square(image)
                } else {
                    image
                };
                let image = fit(image, max);
                let frame = image::Frame::from_parts(
                    image,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(delay, 1),
                );
                encoder
                    .encode_frame(frame)
                    .map_err(|_| "não deu para preparar o GIF".to_string())?;
            }
        }
        if out.len() > kind.max_bytes() {
            return Err(format!("GIF grande demais: até {limit_mb} MB"));
        }
        return Ok((out, "image/gif"));
    }
    let image = image::load_from_memory(bytes).map_err(|_| "imagem inválida".to_string())?;
    let image = image.to_rgba8();
    let image = if kind == ProfileImage::Avatar {
        square(image)
    } else {
        image
    };
    let image = fit(image, max);
    let mut out = Vec::new();
    let content_type = if kind == ProfileImage::Avatar {
        image
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .map_err(|_| "não deu para preparar a imagem".to_string())?;
        "image/png"
    } else {
        let rgb = DynamicImage::ImageRgba8(image).to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
            .encode_image(&rgb)
            .map_err(|_| "não deu para preparar a imagem".to_string())?;
        "image/jpeg"
    };
    if out.len() > kind.max_bytes() {
        return Err(too_big());
    }
    Ok((out, content_type))
}

// ---------- na thread da janela ----------

struct Animation {
    frames: Vec<(slint::Image, u32)>,
    /// Soma dos atrasos (0: imagem parada).
    total_ms: u64,
}

thread_local! {
    /// Imagens abertas, por (id da pessoa, tipo). Só na thread da janela.
    static MEDIA: RefCell<HashMap<(String, ProfileImage), Animation>> = RefCell::new(HashMap::new());
    /// Quando as animações começaram (todas andam no mesmo relógio).
    static EPOCH: Instant = Instant::now();
}

pub fn store(user: &str, kind: ProfileImage, frames: Vec<Frame>) {
    let frames: Vec<(slint::Image, u32)> = frames
        .into_iter()
        .map(|f| {
            let buffer = slint::SharedPixelBuffer::clone_from_slice(&f.rgba, f.width, f.height);
            (slint::Image::from_rgba8(buffer), f.delay_ms)
        })
        .collect();
    let total_ms = frames.iter().map(|(_, d)| u64::from(*d)).sum();
    MEDIA.with(|media| {
        media
            .borrow_mut()
            .insert((user.to_string(), kind), Animation { frames, total_ms });
    });
}

pub fn forget(user: &str, kind: Option<ProfileImage>) {
    MEDIA.with(|media| {
        media
            .borrow_mut()
            .retain(|(owner, k), _| owner != user || kind.is_some_and(|kind| kind != *k));
    });
}

/// O quadro de agora (e o número dele, para saber se mudou).
fn current(user: &str, kind: ProfileImage) -> Option<(usize, slint::Image)> {
    let now_ms = EPOCH.with(|epoch| epoch.elapsed().as_millis() as u64);
    MEDIA.with(|media| {
        let media = media.borrow();
        let animation = media.get(&(user.to_string(), kind))?;
        if animation.total_ms == 0 {
            return animation
                .frames
                .first()
                .map(|(image, _)| (0, image.clone()));
        }
        let mut t = now_ms % animation.total_ms;
        for (index, (image, delay)) in animation.frames.iter().enumerate() {
            if t < u64::from(*delay) {
                return Some((index, image.clone()));
            }
            t -= u64::from(*delay);
        }
        animation
            .frames
            .last()
            .map(|(image, _)| (animation.frames.len() - 1, image.clone()))
    })
}

/// Um lugar da tela que mostra imagens do perfil (cartão, página, editor).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Slot {
    Card,
    Page,
    Editor,
}

/// O que cada lugar da tela mostra agora: de quem e qual quadro.
type Shown = HashMap<(Slot, ProfileImage), (String, Option<usize>)>;

/// Troca os quadros dos GIFs que estão na tela. Roda na thread da janela.
pub fn start_animation(ui: &AppWindow) -> slint::Timer {
    let shown: Rc<RefCell<Shown>> = Rc::new(RefCell::new(HashMap::new()));
    let weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(30),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut shown = shown.borrow_mut();
            let mut slots = Vec::new();
            if ui.get_card_open() {
                slots.push((Slot::Card, ui.get_card().id.to_string()));
            }
            if ui.get_page_open() {
                slots.push((Slot::Page, ui.get_page().id.to_string()));
            }
            if ui.get_profile_open() {
                slots.push((Slot::Editor, ui.get_my_id().to_string()));
            }
            for (slot, user) in slots {
                for kind in ProfileImage::ALL {
                    let frame = current(&user, kind);
                    let key = (slot, kind);
                    let index = frame.as_ref().map(|(i, _)| *i);
                    if shown.get(&key) == Some(&(user.clone(), index)) {
                        continue;
                    }
                    shown.insert(key, (user.clone(), index));
                    let image = frame.map(|(_, image)| image).unwrap_or_default();
                    set_image(&ui, slot, kind, image);
                }
            }
            // O que fechou mostra de novo do zero quando abrir.
            if !ui.get_card_open() {
                shown.retain(|(slot, _), _| *slot != Slot::Card);
            }
            if !ui.get_page_open() {
                shown.retain(|(slot, _), _| *slot != Slot::Page);
            }
            if !ui.get_profile_open() {
                shown.retain(|(slot, _), _| *slot != Slot::Editor);
            }
        },
    );
    timer
}

fn set_image(ui: &AppWindow, slot: Slot, kind: ProfileImage, image: slint::Image) {
    match (slot, kind) {
        (Slot::Card, ProfileImage::Avatar) => ui.set_card_photo(image),
        (Slot::Card, ProfileImage::Background) => ui.set_card_background(image),
        (Slot::Card, _) => {}
        (Slot::Page, ProfileImage::Avatar) => ui.set_page_photo(image),
        (Slot::Page, ProfileImage::Background) => ui.set_page_background(image),
        (Slot::Page, ProfileImage::Art) => ui.set_page_art(image),
        (Slot::Page, ProfileImage::ArtSide) => ui.set_page_art_side(image),
        (Slot::Editor, ProfileImage::Avatar) => ui.set_edit_avatar(image),
        (Slot::Editor, ProfileImage::Background) => ui.set_edit_background(image),
        (Slot::Editor, ProfileImage::Art) => ui.set_edit_art(image),
        (Slot::Editor, ProfileImage::ArtSide) => ui.set_edit_art_side(image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ProfileImageInfo;

    fn user(n: u128) -> UserId {
        UserId(uuid::Uuid::from_u128(n))
    }

    fn profile(n: u128, images: &[(ProfileImage, i64)], version: i64) -> Profile {
        Profile {
            user_id: user(n),
            username: format!("p{n}"),
            member_since_ms: 0,
            text: ProfileText::default(),
            images: images
                .iter()
                .map(|(kind, version)| ProfileImageInfo {
                    kind: *kind,
                    version: *version,
                    content_type: "image/png".into(),
                })
                .collect(),
            version,
        }
    }

    #[test]
    fn asks_once_and_downloads_only_new_versions() {
        let mut profiles = Profiles::default();
        let mut out = Vec::new();
        profiles.welcome(user(1), true, &mut out);
        assert_eq!(out, [ClientMessage::GetProfile { user_id: user(1) }]);
        out.clear();
        profiles.show(user(2), Place::Card, &mut out);
        profiles.show(user(2), Place::Page, &mut out);
        assert_eq!(out.len(), 1, "um pedido só, enquanto espera");

        let (downloads, _) = profiles.received(profile(2, &[(ProfileImage::Background, 3)], 3));
        assert_eq!(downloads.len(), 1);
        // Mesma versão: nada novo para baixar.
        let (downloads, _) = profiles.received(profile(2, &[(ProfileImage::Background, 3)], 3));
        assert!(downloads.is_empty());
        // Tirou o fundo, pôs uma arte.
        let (downloads, removed) = profiles.received(profile(2, &[(ProfileImage::Art, 5)], 5));
        assert_eq!(removed, [ProfileImage::Background]);
        assert_eq!(downloads[0].kind, ProfileImage::Art);
    }

    #[test]
    fn forgets_images_of_who_left_the_screen() {
        let mut profiles = Profiles::default();
        let mut out = Vec::new();
        profiles.welcome(user(1), true, &mut out);
        profiles.show(user(2), Place::Card, &mut out);
        profiles.received(profile(2, &[(ProfileImage::Avatar, 1)], 1));
        // O cartão passa para outra pessoa: a 2 sai de vista.
        let (_, gone) = profiles.show(user(3), Place::Card, &mut out);
        assert_eq!(gone, [user(2)]);
        // Voltando para a 2, baixa a foto de novo (foi esquecida).
        let (downloads, _) = profiles.show(user(2), Place::Card, &mut out);
        assert_eq!(downloads.len(), 1);
        // A sua página nunca sai de vista.
        assert!(profiles.hide(Place::Card).contains(&user(2)));
        let (_, gone) = profiles.show(user(1), Place::Page, &mut out);
        assert!(gone.is_empty());
    }

    #[test]
    fn old_server_gets_no_profile_messages() {
        let mut profiles = Profiles::default();
        let mut out = Vec::new();
        profiles.welcome(user(1), false, &mut out);
        profiles.show(user(2), Place::Page, &mut out);
        assert!(out.is_empty());
    }

    fn gif(width: u32, height: u32, frames: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for i in 0..frames {
                let shade = (i * 40 % 256) as u8;
                let image = RgbaImage::from_pixel(width, height, image::Rgba([shade, 0, 0, 255]));
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image,
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(50, 1),
                    ))
                    .unwrap();
            }
        }
        bytes
    }

    #[test]
    fn gif_becomes_frames_and_long_ones_get_thinner() {
        let frames = decode(ProfileImage::Art, &gif(8, 6, 3)).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!((frames[0].width, frames[0].height), (8, 6));
        assert_eq!(frames[0].delay_ms, 50);
        let many = decode(ProfileImage::Art, &gif(4, 4, 150)).unwrap();
        assert!(many.len() <= MAX_FRAMES);
        let total: u32 = many.iter().map(|f| f.delay_ms).sum();
        assert_eq!(total, 150 * 50, "a animação dura o mesmo");
        // Foto: sai quadrada.
        let avatar = decode(ProfileImage::Avatar, &gif(10, 6, 2)).unwrap();
        assert_eq!((avatar[0].width, avatar[0].height), (6, 6));
    }

    #[test]
    fn upload_keeps_small_gifs_and_shrinks_big_images() {
        let small = gif(64, 64, 2);
        let (bytes, content_type) = prepare_upload(ProfileImage::Avatar, &small).unwrap();
        assert_eq!(content_type, "image/gif");
        assert_eq!(bytes, small);

        let wide = gif(700, 300, 2);
        let (bytes, _) = prepare_upload(ProfileImage::Avatar, &wide).unwrap();
        let decoded = decode(ProfileImage::Art, &bytes).unwrap();
        assert_eq!((decoded[0].width, decoded[0].height), (300, 300));

        let mut png = Vec::new();
        RgbaImage::from_pixel(3000, 1000, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let (bytes, content_type) = prepare_upload(ProfileImage::Background, &png).unwrap();
        assert_eq!(content_type, "image/jpeg");
        let back = image::load_from_memory(&bytes).unwrap();
        assert!(back.width() <= 1920 && back.height() <= 1080);
    }

    #[test]
    fn member_since_and_colors() {
        // 2 de novembro de 2026, 12h UTC.
        assert_eq!(
            member_since(1_793_620_800_000),
            "no mothh desde nov de 2026"
        );
        assert_eq!(member_since(0), "");
        assert!(parse_color("#7dff9b").is_some());
        assert!(parse_color("verde").is_none());
    }
}
