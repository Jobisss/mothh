fn main() {
    // O webrtc-sys baixa/linka o pacote nativo; no Linux o portal do WebRTC
    // também precisa da GLib, como no build do desktop.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for lib in ["gio-2.0", "gobject-2.0", "glib-2.0"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}
