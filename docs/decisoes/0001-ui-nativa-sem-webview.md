# 0001. Interface nativa em Rust, sem webview, com Slint

**Decisão:** a interface é escrita em Rust com [Slint](https://slint.dev) e desenhada direto na GPU. Nada de Tauri, Electron ou React.

**Por quê:** o objetivo é um app nativo de verdade e aprender Rust. Slint dá o visual mais polido com menos esforço, tem linguagem declarativa própria com hot reload e roda em desktop e mobile. Rust já compila para código de máquina; não há necessidade de transpilar para C.

**Alternativas:** GPUI (visual do Zed, pouca documentação), iced, egui, Dioxus/Blitz, Makepad.

**Cuidado:** conferir a licença do Slint (GPLv3, royalty-free ou comercial) antes de lançar.
