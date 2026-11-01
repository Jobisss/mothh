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

## Fase 3: extras

- [ ] F3-01 Música ouvindo junto (vinil)
- [ ] F3-02 Efeitos sonoros completos

## Fase 4: inventário de itens comemorativos

A última fase. Itens em 3D que marcam datas e momentos, guardados no inventário de cada pessoa e exibidos no perfil. Os detalhes (quais itens, como se ganha) ainda vão ser definidos.

- [ ] F4-01 Definir os itens comemorativos e como a pessoa ganha cada um
- [ ] F4-02 Inventário no servidor
- [ ] F4-03 Ver o item em 3D, girando
- [ ] F4-04 Exibir itens no perfil

## Pendências fora das fases

- [ ] X-01 Definir a licença do repositório
- [x] X-02 Definir o domínio
- [x] X-03 Adicionar o `OFL.txt` das fontes em `crates/client/ui/fonts/`
- [ ] X-04 Logo definitiva
- [ ] X-05 Diminuir o atraso da câmera (a Iriun Webcam funciona, mas a imagem chega com um pouco de delay)
- [x] X-06 Fones que não tocavam na chamada: usar o dispositivo de comunicação do Windows (fone Bluetooth) e seguir o padrão do sistema quando o fone é ligado ou tirado
- [x] X-07 Redução de ruído com intensidade (slider de 0 a 100%, com portão acima de 50%)
- [x] X-08 Voz das pessoas mais alta acima de 100% no volume
- [x] X-09 Indicadores na chamada: conexão de cada pessoa, seu ping e quem está mutado
- [x] X-10 Diagnóstico do áudio no log (taxa do Opus, perda e jitter de cada pessoa) e voz em até 64 kbps
- [ ] X-11 Voz de quem está longe (EUA) chega com qualidade de ligação: confirmar pelo log se é rede ou microfone (fone Bluetooth); se for rede, LiveKit mais perto ou em várias regiões
