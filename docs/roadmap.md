# Roadmap

Cada fase termina com algo que dá para usar com amigos. Cada fase também ensina uma parte de Rust.

| Fase | Entrega | Pronto quando | O que ensina de Rust |
| --- | --- | --- | --- |
| 0. Documentação | Visão, arquitetura, design e roadmap neste repositório | A documentação está na `main` | Planejar crates e módulos |
| 1. Conversa de texto | Login, lista de conversas (pessoas e grupos), mensagens ao vivo, mensagens de áudio | Duas pessoas conversam pelo app, cada uma no seu computador | async com Tokio, serde, axum, sqlx |
| 2. Voz, vídeo e E2EE | Chamadas via LiveKit, câmera, tela, MLS nas conversas, SFrame na mídia | Uma chamada cifrada com 5 pessoas | FFI, concorrência, criptografia com crates prontas |
| 3. Extras | Música ouvindo junto, efeitos sonoros completos | Cada extra liga e desliga sozinho nas configurações | Arquitetura de módulos |
| 4. Inventário comemorativo (última) | Itens comemorativos em 3D no inventário de cada pessoa, vistos girando e exibidos no perfil (sem compra, venda nem mercado) | Alguém ganha um item, vê ele em 3D e mostra no perfil | Renderização na GPU, modelo de dados no servidor |

Personagens 3D, sala 3D e mercado de itens estavam nos planos e foram abandonados (01/11/2026).

## Progresso da fase 1

A lista completa, com todas as fases, fica no [TODO](../TODO.md).

- [x] Workspace com `protocol`, `server` e `client`.
- [x] Servidor com WebSocket, uma sala de grupo e mensagens ao vivo (em memória).
- [x] App Slint com lista de conversas e mensagens na fonte VT323.
- [x] Guardar conversas e mensagens no PostgreSQL (sqlx), com histórico das últimas 100.
- [x] Login com conta de verdade (argon2 na senha, sessão salva no app).
- [ ] Conversas diretas e criação de grupos.
- [ ] Reconectar sozinho e rolar até a última mensagem.
- [ ] Mensagens de áudio.

## Desde a fase 1

- Registro de módulos e interruptores nas configurações, mesmo com poucos módulos.
- Design system aplicado (preto e branco, fonte VT323, dither).
- Docker Compose rodando na VPS.

## Em aberto

- Domínio definitivo.
- Licença do repositório.
- Logo definitiva (a mariposa atual é temporária).
