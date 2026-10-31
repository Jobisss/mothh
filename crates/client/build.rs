// Compila a interface (`ui/app.slint`) para código Rust durante o build.
fn main() {
    slint_build::compile("ui/app.slint").expect("erro ao compilar ui/app.slint");

    icone_do_exe();
    volume_das_faixas();

    // No Linux, o capturador de tela do libwebrtc (F2-06) fala com o portal
    // do sistema (PipeWire) por D-Bus usando a GLib, mas o webrtc-sys não
    // linka ela. Sem isto o link falha com `g_dbus_*` e `g_variant_*`
    // indefinidos. Precisa do pacote `libglib2.0-dev`, que a CI já instala.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        for lib in ["gio-2.0", "gobject-2.0", "glib-2.0"] {
            println!("cargo:rustc-link-lib=dylib={lib}");
        }
    }
}

// O volume de cada pessoa na chamada (`cpp/volume.cpp`, usado em
// `volume.rs`). Compila com os mesmos cabeçalhos e definições do libwebrtc
// pré-compilado que o webrtc-sys usa, senão as classes não batem.
fn volume_das_faixas() {
    println!("cargo:rerun-if-changed=cpp/volume.cpp");
    let webrtc = webrtc_sys_build::webrtc_dir();
    if !webrtc.exists() {
        webrtc_sys_build::download_webrtc().expect("erro ao baixar o libwebrtc");
    }
    let include = webrtc.join("include");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("cpp/volume.cpp")
        .include(&include)
        .include(include.join("third_party/abseil-cpp/"))
        .include(include.join("third_party/libc++/"))
        .define("WEBRTC_APM_DEBUG_DUMP", "0")
        .warnings(false);
    for (key, value) in webrtc_sys_build::webrtc_defines() {
        build.define(&key, value.as_deref());
    }
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            build.flag("/std:c++20").flag("/EHsc");
        }
        Ok("macos") => {
            build.flag("-std=c++20").flag("-stdlib=libc++");
        }
        _ => {
            build
                .flag("-std=c++20")
                .flag_if_supported("-Wno-changes-meaning")
                .flag("-Wno-deprecated-declarations");
        }
    }
    build.compile("mothh_volume");
}

// Ícone do mothh.exe (recurso do Windows): é o que o Explorer, a barra de
// tarefas e os atalhos mostram. O mesmo .ico vai no instalador.
#[cfg(windows)]
fn icone_do_exe() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=../../installer/mothh.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../installer/mothh.ico");
    res.compile().expect("erro ao embutir o ícone no mothh.exe");
}

#[cfg(not(windows))]
fn icone_do_exe() {}
