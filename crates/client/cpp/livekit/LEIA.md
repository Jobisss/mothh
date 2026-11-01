Cópia sem mudanças de `include/livekit/adm_proxy.h` e
`include/livekit/synthetic_audio_device.h` do webrtc-sys 0.3.42 (Apache-2.0,
LiveKit). O `noise.cpp` precisa do desenho exato da `AdmProxy`; o webrtc-sys
está preso em `=0.3.42` no Cargo.toml. Ao subir a versão, copiar de novo.
