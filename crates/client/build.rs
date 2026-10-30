// Compila a interface (`ui/app.slint`) para código Rust durante o build.
fn main() {
    slint_build::compile("ui/app.slint").expect("erro ao compilar ui/app.slint");

    // A captura de tela do libwebrtc no Linux (portal do PipeWire) chama o
    // GIO, mas o webrtc-sys só usa os cabeçalhos dele e deixa o link para o
    // app. Sem isto o `mothh` não linka desde que a F2-06 usou a captura.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for lib in ["gio-2.0", "gobject-2.0", "glib-2.0"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
