# Arquitetura

Tudo em Rust, menos o servidor de mídia (LiveKit, em Go). O cliente desenha a interface direto na GPU; o servidor só vê dados cifrados, exceto o perfil (nome, foto e, na fase 3, os itens comemorativos de cada pessoa).

## Visão geral

```
┌──────────── Cliente (desktop) ────────────┐        ┌────────── Servidor (Rust) ──────────┐      ┌──── Dados ────┐
│ UI em Slint                                │        │ Gateway axum: login, mensagens      │      │ PostgreSQL     │
│ Chamada: voz, câmera e tela (LiveKit)      │◀──WS──▶│ Presença: quem está em cada chamada │◀────▶│ Redis          │
│ Criptografia local: OpenMLS, chaves no SO  │  TLS   │ Perfil: nome, foto, inventário      │      │ Arquivos (S3)  │
└──────────────────┬─────────────────────────┘  +MLS  └──────────────────┬──────────────────┘      └────────────────┘
                   │ WebRTC + SFrame                                      │ token de sala
                   ▼                                                      ▼
            ┌──────────────────────── LiveKit SFU: repassa áudio, vídeo e tela sem decifrar ────────────────────────┐
```

Como uma chamada funciona: o app faz login e troca mensagens com o servidor Rust. Ao entrar na chamada, o servidor entrega um token, e o app conecta direto no LiveKit para áudio, vídeo e tela. O LiveKit não recodifica nada; só repassa pacotes.

## Escolhas

| Camada | Escolha | Alternativa considerada |
| --- | --- | --- |
| Interface | [Slint](https://slint.dev) | GPUI (visual do Zed, menos documentação), iced, egui, Dioxus/Blitz, Makepad |
| Item em 3D (fase 3) | A definir: algo leve que desenhe um modelo glTF girando dentro do Slint | |
| Async e servidor | Tokio + [axum](https://github.com/tokio-rs/axum) | |
| Banco | PostgreSQL via [sqlx](https://github.com/launchbadge/sqlx) | |
| Presença e filas | Redis | |
| Mídia | [LiveKit](https://livekit.io) com o SDK oficial em Rust | mediasoup, str0m, webrtc-rs |
| Criptografia | [OpenMLS](https://github.com/openmls/openmls) + SFrame | Signal Protocol (libsignal, AGPL) |
| Formato 3D | glTF 2.0 (.glb) | |

Slint é distribuído sob GPLv3, licença royalty-free ou comercial; conferir qual se aplica antes de lançar.

## Organização do código (planejada)

Workspace Cargo. Cada módulo opcional é uma crate e uma `feature` do Cargo, além de um interruptor em tempo de execução no cliente.

```
mothh/
  Cargo.toml              # workspace
  crates/
    client/               # app Slint: conversas, chamadas, perfil
    protocol/             # tipos compartilhados (serde), mensagens do WebSocket
    crypto/               # wrapper de OpenMLS e SFrame
    server/               # API axum + WebSocket + PostgreSQL
    music/                # sincronização de música (módulo)
  docker-compose.yml      # Caddy, Postgres, Redis, LiveKit
  docs/
```

## Princípios técnicos

- **Servidor é a fonte da verdade** do perfil e dos itens de cada pessoa. O cliente só pede e mostra.
- **Tipos compartilhados** entre cliente e servidor numa crate própria, para evitar erros de protocolo.
- **Núcleo independente**: conversas, grupos e chamadas funcionam com todos os módulos desligados.
- **Nada de criptografia própria**: só crates auditadas.
