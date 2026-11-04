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

- [ ] F3-01 Música por nome/link, fila compartilhada e reprodução pelo LiveKit — suspensa na 0.5.1
    - [x] Desativar UI e worker de produção; provedor PO Token passa a ser opcional
    - [x] Atividade Spotify pelo Windows no perfil, com toca-discos 3D e atalho de busca
    - [x] Fotos nos cabeçalhos do chat e remoção do nome do arquivo nas prévias de imagem
    - [ ] Validar atividade real do Spotify e visibilidade entre duas contas na VPS
  - [x] Backend: pedidos, fila por grupo, permissões e supervisão do worker
  - [x] Cliente: painel, fila, escuta separada, controles e volume individual
  - [x] Toca-discos 3D: ícone, câmera por arraste, vinil/rótulo girando e braço móvel
  - [x] Miniatura no centro do disco e pixelização com dither suave (55%)
    - [x] Integração experimental de PO Tokens: bgutil interno e cliente mweb no extrator
    - [ ] Validar geração de tokens e reprodução no IP da VPS após exigência de verificação do YouTube
  - [ ] Configurar o worker, yt-dlp e FFmpeg no servidor de produção
  - [ ] Validar reprodução completa com duas contas, troca de faixa e reconexão
  - [ ] Validar miniaturas reais do YouTube (CDN retornou 404 nos testes locais)
- [ ] F3-02 Efeitos sonoros completos

## Fase P: página de perfil

Página de perfil como a da Steam, com os enfeites do Discord por cima. A proposta (com provador) está no artifact "Provador do mothh". Um clique em alguém abre o mini cartão; "ver perfil" abre a página completa no lugar da conversa.

- [x] P1-01 Servidor: textos e vitrines do perfil, imagens (foto grande, fundo, arte, arte lateral) por HTTP, com GIF
- [x] P1-02 Mini cartão ao clicar em alguém (nome no chat, membros, foto na chamada, rostinhos no topo, sua linha no rodapé)
- [x] P1-03 Página completa: fundo, cabeçalho, arte em destaque, sobre mim
- [x] P1-04 Editor: nome real, de onde, sobre mim, cor de destaque, imagens e vitrines
- [x] P1-05 GIF animado na foto, no fundo e nas artes (só enquanto está na tela)
- [ ] P2-01 Números no perfil: horas em chamada, mensagens, conversas em comum (com opção de esconder)
- [ ] P2-02 Insígnias por evento e conquista, e a vitrine de insígnias
- [ ] P2-03 Nível e XP (por insígnia ganha)
- [ ] P3-01 Catálogo de enfeites: pasta `cosmeticos` com `item.toml`, conferidor
- [ ] P3-02 Moldura, estilo do nome e placa
- [ ] P3-03 Fundos prontos e primeiros itens grátis
- [ ] P4-01 Efeitos da página
- [ ] P4-02 Catálogo vindo do servidor (item novo sem release)
- [ ] P4-03 Foto em GIF animando também na lista e na chamada (com o mouse em cima)

## Fase 4: inventário de itens comemorativos

A última fase. Itens em 3D que marcam datas e momentos, guardados no inventário de cada pessoa e exibidos no perfil. Os detalhes (quais itens, como se ganha) ainda vão ser definidos.

- [ ] F4-01 Definir os itens comemorativos e como a pessoa ganha cada um
- [ ] F4-02 Inventário no servidor
- [ ] F4-03 Ver o item em 3D, girando
- [ ] F4-04 Exibir itens no perfil (vitrine "itens em destaque" da página)

## Versão 0.6: conversa, conta e aparência

Pedidos de 04/11/2026: a conversa ganha o básico que faltava, a conta ganha e-mail e a aparência fica configurável.

- [x] V6-01 Selecionar e copiar o texto das mensagens (Ctrl+C)
- [x] V6-02 Editar e apagar mensagens próprias: lápis e X ao passar o mouse, "editada" na mensagem, apagar some para todo mundo
- [x] V6-03 Chamada 1:1 na conversa direta, com aviso e toque para quem recebe
- [x] V6-04 Cadastro com e-mail (contas antigas continuam), "esqueci a senha" por código e login por usuário ou e-mail
- [x] V6-05 Fontes alternativas nas configurações, para os títulos e para o texto das mensagens
- [x] V6-06 Polimento visual: escala de espaçamento, estados de hover/pressionado/foco, transições curtas e telas vazias
- [ ] V6-07 Tema claro (tokens invertidos do design system)

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
- [x] X-12 Reações na tela cheia: botão no canto de cima mostra a câmera (ou a foto, com a borda da voz) de cada amigo
- [x] X-13 Modo overlay: minimiza o app e deixa uma janelinha por cima de tudo com o nome da conversa, quem está na chamada, o microfone e uma mensagem rápida
- [x] X-14 Módulo "desenhar pela placa de vídeo": desligado, o app abre com o renderizador por software (num PC o OpenGL desenhava a tela deslocada para baixo)
- [x] X-15 Troca de chaves travada: o servidor passa a vez a outro membro online após 20 s sem ação, o app cutuca enquanto espera, e conexão calada por 90 s cai
