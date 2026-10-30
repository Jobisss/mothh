//! Fundo em dither (Bayer 4×4) atrás da lista de conversas.
//!
//! Cada quadro é uma imagem pequena, um pixel por "célula" do dither; o Slint
//! amplia sem suavizar (`image-rendering: pixelated`) e cada célula vira um
//! quadrado de `CELL` px na tela. Gerar na CPU sai barato: a coluna tem
//! 320px, então são uns 80 × 180 pixels por quadro.

use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Tamanho de cada célula na tela, em px lógicos (o design pede 3 a 6).
pub const CELL: f32 = 4.0;
/// Quadros por segundo. É para mexer devagar, não precisa de mais.
pub const FPS: u64 = 8;

/// Os três tons: surface-0, um meio-termo e surface-3. Escuros para o texto
/// por cima continuar legível.
const TONES: [u8; 3] = [0x0a, 0x17, 0x26];

/// Matriz de Bayer 4×4, com os limiares de 0 a 15.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Um quadro de `cols` × `rows` células no instante `t` (em segundos).
pub fn frame(cols: u32, rows: u32, t: f32) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(cols, rows);
    let pixels = buffer.make_mut_slice();
    for y in 0..rows {
        for x in 0..cols {
            let tone = TONES[level(x, y, t)];
            pixels[(y * cols + x) as usize] = Rgba8Pixel {
                r: tone,
                g: tone,
                b: tone,
                a: 255,
            };
        }
    }
    buffer
}

/// Qual dos tons vai na célula (x, y): um campo de ondas suaves que anda com
/// o tempo, quantizado pelo limiar de Bayer.
fn level(x: u32, y: u32, t: f32) -> usize {
    let (fx, fy) = (x as f32, y as f32);
    // Entre 0 e 1.
    let field = 0.5
        + 0.25 * (fx * 0.09 + t * 0.35).sin()
        + 0.25 * (fy * 0.06 - t * 0.25 + (fx * 0.05 + t * 0.15).sin()).sin();
    let value = field.clamp(0.0, 1.0) * (TONES.len() - 1) as f32;
    let base = value.floor() as usize;
    let threshold = (BAYER[(y % 4) as usize][(x % 4) as usize] as f32 + 0.5) / 16.0;
    if value - base as f32 > threshold {
        (base + 1).min(TONES.len() - 1)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(cols: u32, rows: u32, t: f32) -> Vec<u8> {
        frame(cols, rows, t)
            .as_slice()
            .iter()
            .map(|p| p.r)
            .collect()
    }

    #[test]
    fn uses_only_the_three_tones() {
        let used = tones(80, 180, 3.7);
        assert!(used.iter().all(|tone| TONES.contains(tone)));
        // E usa mais de um, senão não seria dither.
        for tone in TONES {
            assert!(used.contains(&tone), "faltou o tom {tone:#x}");
        }
    }

    #[test]
    fn moves_slowly_over_time() {
        let now = tones(80, 180, 10.0);
        let next = tones(80, 180, 10.0 + 1.0 / FPS as f32);
        let later = tones(80, 180, 20.0);
        let changed = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
        // De um quadro para o outro muda pouca coisa; em 10s muda bastante.
        assert!(changed(&now, &next) < now.len() / 10);
        assert!(changed(&now, &later) > now.len() / 10);
    }

    #[test]
    fn same_instant_same_frame() {
        assert_eq!(tones(20, 20, 1.5), tones(20, 20, 1.5));
    }

    #[test]
    fn frame_has_requested_size() {
        let buffer = frame(7, 3, 0.0);
        assert_eq!((buffer.width(), buffer.height()), (7, 3));
    }
}
