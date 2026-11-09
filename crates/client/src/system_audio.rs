//! O dispositivo de áudio "de comunicação" do sistema.
//!
//! O Windows tem dois padrões de saída (e de entrada): o normal e o de
//! comunicação. Com fone Bluetooth eles costumam ser diferentes: quando o
//! microfone do fone abre numa chamada, o fone passa para o modo "Hands-Free"
//! e o som mandado para a saída estéreo dele ("Fones de ouvido") fica mudo;
//! quem toca é a saída "Headset"/"Hands-Free", que é o padrão de comunicação.
//! Por isso a chamada usa o padrão de comunicação, como o Discord.
//!
//! O id que volta é o do endpoint do Windows, o mesmo que o WebRTC usa como
//! id do dispositivo. Fora do Windows não há essa distinção: `None`, e a
//! chamada fica com o padrão normal (achado pelo nome).

/// Id do dispositivo de comunicação padrão: saída (`render`) ou microfone.
pub fn communication_default(render: bool) -> Option<String> {
    #[cfg(windows)]
    {
        // Numa thread própria: o COM é iniciado e encerrado nela, sem mexer
        // na configuração das threads do Tokio.
        std::thread::spawn(move || windows_only::communication_default(render))
            .join()
            .ok()
            .flatten()
    }
    #[cfg(not(windows))]
    {
        let _ = render;
        None
    }
}

#[cfg(windows)]
mod windows_only {
    use windows::Win32::Media::Audio::{
        DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
        MMDeviceEnumerator, eAll, eCapture, eCommunications, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };
    use windows::core::Interface;

    pub fn communication_default(render: bool) -> Option<String> {
        // SAFETY: COM iniciado nesta thread e encerrado no fim; a string do id
        // é copiada antes de ser devolvida ao COM com `CoTaskMemFree`.
        unsafe {
            let started = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let id = (|| {
                let devices: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let flow = if render { eRender } else { eCapture };
                let device = devices
                    .GetDefaultAudioEndpoint(flow, eCommunications)
                    .ok()?;
                let raw = device.GetId().ok()?;
                let id = raw.to_string().ok();
                CoTaskMemFree(Some(raw.0 as *const _));
                id
            })();
            if started {
                CoUninitialize();
            }
            id
        }
    }

    pub fn set_ducking(allow: bool) {
        // SAFETY: COM iniciado nesta thread e encerrado no fim; as interfaces
        // são soltas (Drop) antes do `CoUninitialize`.
        unsafe {
            let started = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let mut done = 0;
            (|| {
                let devices: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let list = devices.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE).ok()?;
                for i in 0..list.GetCount().ok()? {
                    let Ok(device) = list.Item(i) else { continue };
                    let Ok(sessions) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
                    else {
                        continue;
                    };
                    // `None`: a sessão padrão deste programa no dispositivo.
                    let Ok(session) = sessions.GetAudioSessionControl(None, 0) else {
                        continue;
                    };
                    let Ok(session) = session.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    if session.SetDuckingPreference(!allow).is_ok() {
                        done += 1;
                    }
                }
                Some(())
            })();
            eprintln!(
                "som: abaixar os outros sons {} em {done} dispositivo(s)",
                if allow { "ligado" } else { "desligado" }
            );
            if started {
                CoUninitialize();
            }
        }
    }
}

/// Liga ou desliga o "abaixar os outros sons" do Windows para o mothh.
///
/// Quando um app abre o microfone de comunicação, o Windows abaixa o volume
/// de todo o resto (música, jogo, vídeo) em 80%, a "atenuação" de Som >
/// Comunicações. O Discord desliga isso por padrão; aqui também. Cada
/// dispositivo tem uma sessão de áudio padrão por programa, que é a que o
/// WebRTC usa: nela vai a preferência (`SetDuckingPreference`). Vale para a
/// sessão já aberta e para a próxima, então é chamado ao abrir o app, ao
/// trocar o interruptor e antes de cada chamada abrir o microfone.
pub fn set_ducking(allow: bool) {
    ALLOW_DUCKING.store(allow, std::sync::atomic::Ordering::Relaxed);
    apply_ducking();
}

/// Aplica de novo a última escolha de `set_ducking` (dispositivo novo).
pub fn apply_ducking() {
    #[cfg(windows)]
    {
        let allow = ALLOW_DUCKING.load(std::sync::atomic::Ordering::Relaxed);
        let _ = std::thread::spawn(move || windows_only::set_ducking(allow)).join();
    }
}

/// Começa desligado: sem o módulo ler o arquivo, o padrão é não abaixar.
static ALLOW_DUCKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
