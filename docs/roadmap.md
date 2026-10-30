# Roadmap

Cada fase termina com algo que dá para usar com amigos. Dinheiro real só entra na última. Cada fase também ensina uma parte de Rust.

| Fase | Entrega | Pronto quando | O que ensina de Rust |
| --- | --- | --- | --- |
| 0. Documentação | Visão, arquitetura, design e roadmap neste repositório | A documentação está na `main` | Planejar crates e módulos |
| 1. Conversa de texto | Login, lista de conversas (pessoas e grupos), mensagens ao vivo, mensagens de áudio | Duas pessoas conversam pelo app, cada uma no seu computador | async com Tokio, serde, axum, sqlx |
| 2. Voz, vídeo e E2EE | Chamadas via LiveKit, câmera, tela, MLS nas conversas, SFrame na mídia | Uma chamada cifrada com 5 pessoas | FFI, concorrência, criptografia com crates prontas |
| 3. Personagens e salas (MVP) | Editor de personagem, sala 3D com assentos, destaque de quem fala, itens grátis, cartão de perfil (placa, moldura, mural) | A sala enche durante a chamada, com cada um vestido do seu jeito | Renderização na GPU, ECS do Bevy |
| 4. Economia | Caixas e chaves, inventário, inspecionar em 3D, mercado com gráfico, ofertas e pedidos de compra, saldo interno sem saque | Um item sai de uma caixa, é inspecionado e vendido no mercado | Transações, regras de negócio seguras |
| 5. Extras | Andar pela sala, música ouvindo junto, página de perfil com vitrines, áudio espacial, efeitos sonoros completos | Cada extra liga e desliga sozinho nas configurações | Arquitetura de módulos |

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
- Revisão jurídica antes da fase 4.
