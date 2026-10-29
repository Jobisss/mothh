# Arquitetura

Tudo em Rust, menos o servidor de mídia (LiveKit, em Go). O cliente desenha a interface direto na GPU; o servidor só vê dados cifrados, exceto na economia, onde ele precisa garantir saldo e posse dos itens.

## Visão geral

```
┌──────────── Cliente (desktop) ────────────┐        ┌────────── Servidor (Rust) ──────────┐      ┌──── Dados ────┐
│ UI em Slint                                │        │ Gateway axum: login, mensagens      │      │ PostgreSQL     │
│ Sala 3D em Bevy (renderiza numa textura)   │◀──WS──▶│ Presença: quem está em cada sala    │◀────▶│ Redis          │
│ Criptografia local: OpenMLS, chaves no SO  │  TLS   │ Economia: caixas, mercado, saldo    │      │ Arquivos (S3)  │
└──────────────────┬─────────────────────────┘  +MLS  └──────────────────┬──────────────────┘      └────────────────┘
                   │ WebRTC + SFrame                                      │ token de sala
                   ▼                                                      ▼
            ┌──────────────────────── LiveKit SFU: repassa áudio, vídeo e tela sem decifrar ────────────────────────┐
```

Como uma chamada funciona: o app faz login e troca mensagens com o servidor Rust. Ao entrar na sala, o servidor entrega um token, e o app conecta direto no LiveKit para áudio, vídeo e tela. O LiveKit não recodifica nada; só repassa pacotes.

## Escolhas

| Camada | Escolha | Alternativa considerada |
| --- | --- | --- |
| Interface | [Slint](https://slint.dev) | GPUI (visual do Zed, menos documentação), iced, egui, Dioxus/Blitz, Makepad |
| Sala 3D | [Bevy](https://bevyengine.org) sobre wgpu, renderizado numa textura dentro do Slint | Fyrox |
| Async e servidor | Tokio + [axum](https://github.com/tokio-rs/axum) | |
| Banco | PostgreSQL via [sqlx](https://github.com/launchbadge/sqlx) | |
| Presença e filas | Redis | |
| Mídia | [LiveKit](https://livekit.io) com o SDK oficial em Rust | mediasoup, str0m, webrtc-rs |
| Criptografia | [OpenMLS](https://github.com/openmls/openmls) + SFrame | Signal Protocol (libsignal, AGPL) |
| Formato 3D | glTF 2.0 (.glb), referência VRM para ossos | |

Slint é distribuído sob GPLv3, licença royalty-free ou comercial; conferir qual se aplica antes de lançar. Bevy é pré-1.0: fixar a versão no `Cargo.toml`.

## Organização do código (planejada)

Workspace Cargo. Cada módulo opcional é uma crate e uma `feature` do Cargo, além de um interruptor em tempo de execução no cliente.

```
mothh/
  Cargo.toml              # workspace
  crates/
    client/               # app Slint: conversas, chamadas, perfil
    client-room/          # sala 3D em Bevy (módulo)
    protocol/             # tipos compartilhados (serde), mensagens do WebSocket
    crypto/               # wrapper de OpenMLS e SFrame
    server/               # API axum + WebSocket + PostgreSQL
    economy/              # itens, caixas, mercado (só no servidor; módulo)
    music/                # sincronização de música (módulo)
  docker-compose.yml      # Caddy, Postgres, Redis, LiveKit
  docs/
```

## Princípios técnicos

- **Servidor é a fonte da verdade** de itens, saldo e posse. O cliente só pede e mostra.
- **Tipos compartilhados** entre cliente e servidor numa crate própria, para evitar erros de protocolo.
- **Núcleo independente**: conversas, grupos e chamadas funcionam com todos os módulos desligados.
- **Nada de criptografia própria**: só crates auditadas.
