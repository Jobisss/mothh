//! Redução de ruído do microfone na chamada (teclado, ventilador, barulho
//! da rua).
//!
//! O WebRTC já tira um pouco de chiado constante, mas deixa passar barulho
//! que vai e volta. Aqui o som do microfone passa antes pelo RNNoise (uma
//! rede neural pequena, `nnnoiseless`, em Rust puro) e só depois segue para
//! o WebRTC, que faz o cancelamento de eco, o ganho e o Opus. O desvio
//! (`cpp/noise.cpp`) é posto ao entrar na chamada, logo depois de conectar
//! (antes disso o WebRTC ainda não ligou o microfone a nada) e antes de o
//! microfone começar a gravar. A opção nas configurações só liga e desliga
//! o filtro, na hora, sem sair da chamada.
//!
//! O desvio vale enquanto o WebRTC (`LkRuntime`) for o mesmo: `install`
//! guarda uma referência a ele, para ele não ser recriado sem o desvio.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cxx::SharedPtr;
use livekit::rtc_engine::lk_runtime::LkRuntime;
use livekit::webrtc::peer_connection_factory::PeerConnectionFactory;
use nnnoiseless::DenoiseState;
use webrtc_sys::peer_connection_factory::ffi::PeerConnectionFactory as SysFactory;

/// O RNNoise trabalha com pedaços de 10 ms a 48 kHz.
const FRAME: usize = DenoiseState::FRAME_SIZE;
const RATE: u32 = 48_000;

static ENABLED: AtomicBool = AtomicBool::new(true);
static RUNTIME: OnceLock<Arc<LkRuntime>> = OnceLock::new();
static FILTER: Mutex<Option<Filter>> = Mutex::new(None);
/// Pedaços de microfone que passaram pelo desvio (para o teste).
static SEEN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn mothh_noise_install(
        controller: *const c_void,
        denoise: extern "C" fn(*mut i16, usize, usize, u32),
    ) -> i32;
}

// O `PeerConnectionFactory` do libwebrtc é só um invólucro de um campo em
// volta do `SharedPtr` do webrtc-sys, mas não deixa ninguém de fora pegar
// nele (como a faixa em `volume.rs`). Versões presas no Cargo.toml.
const _: () = assert!(size_of::<PeerConnectionFactory>() == size_of::<SharedPtr<SysFactory>>());

/// Põe o filtro no caminho do microfone (se ainda não está). Chamar depois
/// de conectar na sala e antes de publicar o microfone. Devolve se deu certo.
pub fn install() -> bool {
    let runtime = RUNTIME.get_or_init(LkRuntime::instance);
    let factory: &PeerConnectionFactory = runtime.pc_factory();
    // SAFETY: mesmo tamanho (conferido acima) e o único campo é o
    // `SharedPtr`; só lemos através dele enquanto `runtime` vive.
    let sys: &SharedPtr<SysFactory> = unsafe { &*std::ptr::from_ref(factory).cast() };
    let Some(sys) = sys.as_ref() else {
        return false;
    };
    let controller = sys.audio_device();
    let Some(controller) = controller.as_ref() else {
        return false;
    };
    // SAFETY: o C++ só lê o `AdmProxy` de dentro do controlador, que vive
    // tanto quanto o `runtime` guardado acima.
    let result = unsafe { mothh_noise_install(std::ptr::from_ref(controller).cast(), denoise) };
    if result < 0 {
        eprintln!("mothh: a redução de ruído não entrou no microfone");
    }
    result >= 0
}

/// Quantos pedaços de microfone já passaram pelo desvio.
#[cfg(test)]
pub fn seen() -> usize {
    SEEN.load(Ordering::Relaxed)
}

/// Liga ou desliga o filtro (vale na hora).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Chamado pelo WebRTC a cada 10 ms de microfone, na thread de áudio.
/// `samples` tem `frames * channels` amostras intercaladas.
extern "C" fn denoise(samples: *mut i16, frames: usize, channels: usize, rate: u32) {
    SEEN.fetch_add(1, Ordering::Relaxed);
    if !ENABLED.load(Ordering::Relaxed) || samples.is_null() || frames == 0 || channels == 0 {
        return;
    }
    // SAFETY: o C++ entrega um buffer próprio com esse tamanho.
    let samples = unsafe { std::slice::from_raw_parts_mut(samples, frames * channels) };
    let Ok(mut filter) = FILTER.lock() else {
        return;
    };
    filter
        .get_or_insert_with(Filter::new)
        .process(samples, channels, rate);
}

struct Filter {
    state: Box<DenoiseState<'static>>,
    mono: Vec<f32>,
    wide: Vec<f32>,
    clean: Vec<f32>,
    /// O primeiro pedaço sai com estalo (o RNNoise ainda não aprendeu).
    warm: bool,
}

impl Filter {
    fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            mono: Vec::new(),
            wide: Vec::new(),
            clean: Vec::new(),
            warm: false,
        }
    }

    fn process(&mut self, samples: &mut [i16], channels: usize, rate: u32) {
        let frames = samples.len() / channels;
        // Mistura os canais num só (a voz é mono).
        self.mono.clear();
        self.mono.extend(
            samples
                .chunks_exact(channels)
                .map(|frame| frame.iter().map(|&s| f32::from(s)).sum::<f32>() / channels as f32),
        );
        // Para 48 kHz, se o microfone grava em outra taxa. Pedaços de 10 ms
        // sempre dão um múltiplo de 480.
        let wide_len = (frames as u64 * u64::from(RATE) / u64::from(rate.max(1))) as usize;
        if wide_len == 0 || !wide_len.is_multiple_of(FRAME) {
            return;
        }
        resample(&self.mono, wide_len, &mut self.wide);
        self.clean.resize(wide_len, 0.0);
        for (out, input) in self
            .clean
            .as_chunks_mut::<FRAME>()
            .0
            .iter_mut()
            .zip(self.wide.as_chunks::<FRAME>().0)
        {
            self.state.process_frame(out, input);
        }
        if !self.warm {
            self.warm = true;
            return;
        }
        resample(&self.clean, frames, &mut self.mono);
        for (frame, &value) in samples.chunks_exact_mut(channels).zip(&self.mono) {
            let value = value.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
            frame.fill(value);
        }
    }
}

/// Troca a quantidade de amostras por interpolação linear (só quando o
/// microfone não grava a 48 kHz; senão é uma cópia).
fn resample(input: &[f32], len: usize, out: &mut Vec<f32>) {
    out.clear();
    if input.len() == len {
        out.extend_from_slice(input);
        return;
    }
    let step = input.len() as f64 / len as f64;
    out.extend((0..len).map(|i| {
        let pos = i as f64 * step;
        let at = pos as usize;
        let next = (at + 1).min(input.len() - 1);
        let t = (pos - at as f64) as f32;
        input[at] * (1.0 - t) + input[next] * t
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chiado de ventilador (ruído grave, sem voz): o filtro baixa bem.
    #[test]
    fn baixa_o_ruido() {
        let mut filter = Filter::new();
        let mut seed = 1u32;
        let mut low = 0.0f32;
        let mut fan = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = f32::from((seed >> 16) as u16 as i16);
            low = 0.95 * low + 0.05 * white;
            (low * 0.5) as i16
        };
        let rms = |s: &[i16]| {
            (s.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for _ in 0..300 {
            let mut chunk: Vec<i16> = (0..480).map(|_| fan()).collect();
            before.extend_from_slice(&chunk);
            filter.process(&mut chunk, 1, 48_000);
            after.extend_from_slice(&chunk);
        }
        let (before, after) = (rms(&before[48_000..]), rms(&after[48_000..]));
        assert!(after < before / 4.0, "ruído {before:.0} virou {after:.0}");
    }

    /// Estéreo a 44,1 kHz (441 amostras por canal) também passa.
    #[test]
    fn aceita_estereo_e_outra_taxa() {
        let mut filter = Filter::new();
        for _ in 0..3 {
            let mut chunk = vec![1000i16; 441 * 2];
            filter.process(&mut chunk, 2, 44_100);
            assert!(chunk.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]));
        }
    }
}
