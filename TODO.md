# TODO

Lista de tarefas do mothh, na ordem em que devem ser feitas. Cada tarefa tem um código (ex.: `F1-03`). O que entregar e quando ela está pronta fica em [docs/tarefas.md](docs/tarefas.md), na seção com o mesmo código.

Marque `[x]` no mesmo commit que entrega a tarefa.

## Fase 1: conversa de texto

- [x] F1-01 Workspace com `protocol`, `server` e `client`
- [x] F1-02 Chat ao vivo pelo WebSocket (em memória)
- [x] F1-03 App Slint com lista de conversas e mensagens
- [x] F1-04 Conversas e mensagens no PostgreSQL
- [x] F1-05 Login com conta de verdade
- [x] F1-06 Reconectar sozinho quando o servidor cai
- [x] F1-07 Rolar até a última mensagem e carregar mensagens antigas
- [x] F1-08 Limite de tentativas de login
- [x] F1-09 Conversas diretas (pessoa com pessoa)
- [x] F1-10 Criar grupos e convidar pessoas
- [x] F1-11 Agrupar mensagens seguidas da mesma pessoa
- [x] F1-12 Fundo em dither e sons básicos
- [x] F1-13 Tela de configurações com registro de módulos
- [x] F1-14 Mensagens de áudio
- [x] F1-15 Imagens e arquivos
- [x] F1-16 CI no GitHub Actions
- [x] F1-17 Servidor na VPS com HTTPS (Caddy + wss)
- [x] F1-18 Instalador do app para Windows

## Fase 2: voz, vídeo e E2EE

- [x] F2-01 LiveKit no Docker Compose
- [x] F2-02 Token de chamada emitido pelo servidor
- [x] F2-03 Chamada de voz no grupo
- [x] F2-04 Indicador de quem está falando
- [x] F2-05 Câmera
- [x] F2-06 Compartilhar tela
- [x] F2-07 Crate `crypto` com OpenMLS
- [x] F2-08 Mensagens de texto cifradas com MLS
- [x] F2-09 Mídia cifrada com SFrame

## Fase 3: personagens e salas (MVP)

- [ ] F3-01 Crate `client-room` com Bevy desenhando dentro do Slint
- [ ] F3-02 Esqueleto base e sistema de slots
- [ ] F3-03 Shader toon com contorno
- [ ] F3-04 Editor de personagem
- [ ] F3-05 Itens grátis iniciais
- [ ] F3-06 Sala 3D com assentos
- [ ] F3-07 Destaque de quem fala na sala
- [ ] F3-08 Cartão de perfil (placa, moldura, mural)

## Fase 4: economia

- [ ] F4-01 Revisão jurídica e confirmação de 18+
- [ ] F4-02 Inventário e itens com raridade, float e padrão
- [ ] F4-03 Inspecionar item em 3D
- [ ] F4-04 Caixas e chaves
- [ ] F4-05 Saldo interno sem saque
- [ ] F4-06 Mercado: ofertas e pedidos de compra
- [ ] F4-07 Gráfico de preço do mercado

## Fase 5: extras

- [ ] F5-01 Andar pela sala
- [ ] F5-02 Música ouvindo junto (vinil)
- [ ] F5-03 Página de perfil com vitrines
- [ ] F5-04 Áudio espacial
- [ ] F5-05 Efeitos sonoros completos

## Pendências fora das fases

- [ ] X-01 Definir a licença do repositório
- [x] X-02 Definir o domínio
- [x] X-03 Adicionar o `OFL.txt` das fontes em `crates/client/ui/fonts/`
- [ ] X-04 Logo definitiva
- [ ] X-05 Diminuir o atraso da câmera (a Iriun Webcam funciona, mas a imagem chega com um pouco de delay)
