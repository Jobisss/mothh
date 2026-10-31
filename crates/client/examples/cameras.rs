//! Diagnóstico das câmeras: lista o que o nokhwa vê, os formatos de cada
//! uma e tenta pegar alguns quadros. `cargo run -p client --example cameras`
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{ApiBackend, RequestedFormat, RequestedFormatType};

fn main() {
    let cams = match nokhwa::query(ApiBackend::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("query falhou: {e}");
            return;
        }
    };
    println!("{} camera(s)", cams.len());
    for info in &cams {
        println!(
            "\n== {:?} | {} | misc={} | {}",
            info.index(),
            info.human_name(),
            info.misc(),
            info.description()
        );
        let fmt = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut cam = match Camera::new(info.index().clone(), fmt) {
            Ok(c) => c,
            Err(e) => {
                println!("  Camera::new falhou: {e}");
                continue;
            }
        };
        match cam.compatible_camera_formats() {
            Ok(list) => {
                println!("  {} formatos:", list.len());
                for f in list.iter().take(40) {
                    println!("    {f}");
                }
            }
            Err(e) => println!("  compatible_camera_formats falhou: {e}"),
        }
        println!("  formato escolhido: {}", cam.camera_format());
        if let Err(e) = cam.open_stream() {
            println!("  open_stream falhou: {e}");
            continue;
        }
        for i in 0..10 {
            match cam.frame() {
                Ok(buf) => {
                    let r = buf.resolution();
                    let dec = buf
                        .decode_image::<RgbAFormat>()
                        .map(|im| (im.width(), im.height()));
                    println!(
                        "  quadro {i}: {:?} {}x{} {} bytes, esperado nv12={} yuyv={} -> decode {:?}",
                        buf.source_frame_format(),
                        r.width(),
                        r.height(),
                        buf.buffer().len(),
                        r.width() * r.height() * 3 / 2,
                        r.width() * r.height() * 2,
                        dec.map_err(|e| e.to_string())
                    );
                }
                Err(e) => println!("  quadro {i}: erro {e}"),
            }
        }
        let _ = cam.stop_stream();
    }
}
