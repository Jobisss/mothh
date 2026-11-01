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
        IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };

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
}
