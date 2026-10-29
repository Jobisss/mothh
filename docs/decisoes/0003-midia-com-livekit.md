# 0003. Voz, vídeo e tela com LiveKit

**Decisão:** WebRTC com o SFU [LiveKit](https://livekit.io) auto-hospedado e o SDK oficial em Rust.

**Por quê:** escrever um SFU do zero é um projeto à parte. LiveKit é open source, tem SDK em Rust, simulcast, TURN embutido, detecção de quem fala e suporte a SFrame.

**Alternativas:** mediasoup, str0m, webrtc-rs (bons para estudar depois).
