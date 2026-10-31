//! No Windows, o app de release abre sem a janela preta do terminal
//! (`windows_subsystem = "windows"` no `main.rs`). Sem terminal, o que o app
//! escreve com `eprintln!`/`println!` (e a mensagem de um panic) iria para o
//! nada; aqui isso passa a ir para um arquivo:
//! `%APPDATA%\mothh\<perfil>.log`. A execução anterior fica em `<perfil>.log.old`.
//!
//! No modo debug (`cargo run`) o terminal continua, e nada muda.

/// Manda stdout e stderr para o arquivo de log do perfil. Só faz algo no
/// Windows em release; nos outros casos não faz nada.
pub fn redirect(profile: &str) {
    #[cfg(all(windows, not(debug_assertions)))]
    windows_only::redirect(profile);
    #[cfg(not(all(windows, not(debug_assertions))))]
    let _ = profile;
}

#[cfg(all(windows, not(debug_assertions)))]
mod windows_only {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};

    pub fn redirect(profile: &str) {
        let Some(path) = crate::session::profile_file(profile, "log") else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
        let Ok(file) = std::fs::File::create(&path) else {
            return;
        };
        let Ok(copy) = file.try_clone() else {
            return;
        };
        // `into_raw_handle` larga o arquivo sem fechar: ele fica aberto até o
        // app sair. O `eprintln!` do Rust pede o handle ao Windows a cada
        // escrita, então trocar aqui já vale para o resto do app.
        unsafe {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, HANDLE(file.into_raw_handle()));
            let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE(copy.into_raw_handle()));
        }
    }
}
