# Como entregar cada tarefa

Este documento diz o que cada tarefa do [TODO](../TODO.md) precisa entregar e como saber que ela está pronta. Primeiro vêm as regras que valem para todas; depois, uma seção por tarefa.

## Regras para qualquer tarefa

Uma tarefa só está entregue quando tudo isto é verdade:

1. **Funciona no app de verdade.** Não basta compilar: abra o servidor e dois apps e faça o que a tarefa promete. Guarde um print ou um GIF curto quando a tarefa muda algo na tela.
2. **Tem teste.** Regra nova no servidor ganha teste em `crates/server/tests/` (com `#[sqlx::test]` quando usa o banco). Mudança no protocolo ganha teste de ida e volta em JSON em `crates/protocol`. Tela nova não precisa de teste automático, mas precisa do print.
3. **Passa nas três verificações**, nesta ordem:
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets   # sem nenhum aviso
   cargo test --workspace                   # com o PostgreSQL ligado
   ```
4. **Segue o design system**: preto e branco, VT323 nos títulos, nomes e listas; Geist no texto das mensagens; cor só nas fotos, nos itens comemorativos e nos erros. Ver [design-system.md](design-system.md).
5. **Pode ser desligada**, se for um módulo (inventário, música, efeitos, dither). Ver [produto/modulos.md](produto/modulos.md).
6. **Documentação em dia**: marque `[x]` no TODO e, se mudar como rodar, atualize o README. Decisão nova e difícil de desfazer vira um arquivo em [decisoes/](decisoes/).
7. **Um commit por tarefa**, direto na `main`, com a mensagem começando pelo código:
   ```
   F1-06: reconectar sozinho quando o servidor cai

   O que mudou, em duas ou três linhas.
   ```
   Se a tarefa for grande, pode ter mais de um commit, todos com o mesmo código no começo.

Comentários no código ficam em português, curtos, explicando o porquê. Variáveis de ambiente novas começam com `MOTHH_`.

---

## Fase 1: conversa de texto

### F1-06 Reconectar sozinho quando o servidor cai
- **Entrega:** em `crates/client/src/net.rs`, quando a conexão cai, o app tenta de novo com espera crescente (1s, 2s, 4s… até 30s) e entra com o token salvo (`Resume`). O rodapé mostra "reconectando…" e depois "online".
- **Pronto quando:** você fecha o servidor com as duas janelas abertas, liga de novo, e as duas voltam sozinhas sem perder a conversa aberta.

### F1-07 Rolar até a última mensagem e carregar antigas
- **Entrega:** a lista de mensagens desce sozinha quando chega mensagem nova, só se você já estava no fim. Subir até o topo pede as 50 anteriores (`ClientMessage::LoadOlder { conversation_id, before }`).
- **Pronto quando:** numa conversa com mais de 100 mensagens dá para ler tudo rolando para cima, e mensagem nova não arranca você do meio da leitura.

### F1-08 Limite de tentativas de login
- **Entrega:** no servidor, 5 tentativas erradas por usuário em 15 minutos bloqueiam aquele usuário por 15 minutos. Pode ser em memória por enquanto (Redis entra depois). A resposta é um `AuthError` com o tempo que falta.
- **Pronto quando:** existe teste que erra a senha 5 vezes e confirma o bloqueio, e que a senha certa volta a funcionar depois do prazo.

### F1-09 Conversas diretas
- **Entrega:** buscar uma pessoa pelo usuário e abrir uma conversa só entre as duas (`kind = direct`). Só existe uma conversa direta por par de pessoas. O nome mostrado na lista é o da outra pessoa.
- **Pronto quando:** Jobiss abre conversa com Nina, as mensagens aparecem só para os dois, e um terceiro usuário não consegue abrir nem ler.

### F1-10 Criar grupos e convidar pessoas
- **Entrega:** botão para criar grupo com nome; dentro do grupo, convidar pelo usuário e sair do grupo. Quem cria é dono e pode remover pessoas.
- **Pronto quando:** um grupo novo aparece na lista de todos os convidados ao vivo, e quem saiu para de receber mensagens.

### F1-11 Agrupar mensagens seguidas
- **Entrega:** mensagens da mesma pessoa com menos de 5 minutos de diferença aparecem juntas, com nome e horário só na primeira.
- **Pronto quando:** o print mostra um bloco agrupado e um bloco novo depois de 5 minutos ou de outra pessoa.

### F1-12 Fundo em dither e sons básicos
- **Entrega:** fundo em dither (Bayer) animado devagar, atrás da lista de conversas. Som escolhido pelo Jobiss para mensagem recebida de outra pessoa; sem som ao enviar nem ao digitar. O som tem interruptor na F1-13.
- **Pronto quando:** dá para ver o dither, ouvir o som de recebida, e desligar cada um sem reiniciar o app.

### F1-13 Configurações e registro de módulos
- **Entrega:** tela de configurações com a lista de módulos e um interruptor para cada. Um registro no cliente guarda o que está ligado (arquivo na pasta de configuração, junto da sessão). Módulo desligado some da interface, sem botão morto.
- **Pronto quando:** desligar o dither e os sons funciona na hora e continua desligado depois de fechar e abrir o app.

### F1-14 Mensagens de áudio
- **Entrega:** segurar um botão grava áudio em Opus; soltar envia. O arquivo vai para o servidor como anexo (pasta local no começo, depois armazenamento tipo S3). Na conversa aparece um player com a duração.
- **Pronto quando:** Jobiss grava 10 segundos e Nina ouve no outro app.

### F1-15 Imagens e arquivos
- **Entrega:** arrastar ou escolher um arquivo para enviar. Imagem aparece em miniatura; outros arquivos aparecem com nome e tamanho e um botão para baixar. Limite de tamanho definido no servidor.
- **Pronto quando:** uma imagem e um PDF enviados de um app abrem no outro.

### F1-16 CI no GitHub Actions
- **Entrega:** `.github/workflows/ci.yml` roda fmt, clippy e testes (com um serviço PostgreSQL) em cada push na `main`. Compila o app no Windows e no Linux.
- **Pronto quando:** o selo verde aparece no README e um push com erro de clippy fica vermelho.

### F1-17 Servidor na VPS com HTTPS
- **Entrega:** `Dockerfile` do servidor e um `docker-compose.prod.yml` com Caddy (HTTPS automático), servidor e PostgreSQL. O app passa a usar `wss://`. Senhas e chaves ficam num `.env` fora do Git. Passo a passo em [infra.md](infra.md).
- **Pronto quando:** você e um amigo, cada um na sua casa, conversam pelo servidor da VPS. Depende do domínio (X-02).

### F1-18 Instalador para Windows
- **Entrega:** um `.msi` ou `.exe` de instalação gerado pelo CI, com ícone da mariposa e atalho no menu Iniciar, apontando para o servidor da VPS.
- **Pronto quando:** um amigo instala sem ter Rust e entra na conversa.

---

## Fase 2: voz, vídeo e E2EE

### F2-01 LiveKit no Docker Compose
- **Entrega:** serviço LiveKit (com TURN embutido) no compose de desenvolvimento e no de produção, com as portas UDP documentadas em [infra.md](infra.md).
- **Pronto quando:** o LiveKit sobe junto com o resto e responde no painel de saúde.

### F2-02 Token de chamada
- **Entrega:** o servidor gera o token do LiveKit só para quem é membro do grupo (`ClientMessage::JoinCall`).
- **Pronto quando:** teste confirma que quem não é membro recebe erro e quem é membro recebe um token válido.

### F2-03 Chamada de voz
- **Entrega:** botão "entrar na chamada" no grupo, usando o SDK do LiveKit em Rust. Mutar microfone e sair. A lista de conversas mostra "▪ 3 na sala".
- **Pronto quando:** três pessoas falam e se ouvem com atraso baixo.

### F2-04 Quem está falando
- **Entrega:** destaque visual no nome de quem está falando, vindo do nível de áudio do LiveKit.
- **Pronto quando:** o destaque acompanha a fala sem piscar.

### F2-05 Câmera
- **Entrega:** ligar e desligar câmera; vídeo aparece em tiles na chamada (360p por padrão, para caber na VPS).
- **Pronto quando:** cinco pessoas com câmera ligada e a CPU da VPS abaixo de 70%.

### F2-06 Compartilhar tela
- **Entrega:** escolher tela ou janela e compartilhar; quem assiste pode ampliar.
- **Pronto quando:** dá para ler um código compartilhado em outra máquina.

### F2-07 Crate `crypto`
- **Entrega:** crate nova que embrulha o OpenMLS: criar grupo, adicionar e remover membro, cifrar e decifrar. Chaves guardadas no cofre do sistema (Windows Credential Manager / keyring). Nenhuma criptografia feita à mão.
- **Pronto quando:** testes cobrem adicionar e remover membro e mostram que quem saiu não decifra mensagens novas.

### F2-08 Mensagens cifradas
- **Entrega:** o texto sai do app já cifrado; o servidor guarda e repassa só bytes cifrados. Decisão registrada em [decisoes/](decisoes/).
- **Pronto quando:** olhando a tabela `messages` no banco não dá para ler nada, e os apps continuam mostrando o texto.

### F2-09 Mídia com SFrame
- **Entrega:** áudio, vídeo e tela cifrados com SFrame, com a chave derivada do grupo MLS.
- **Pronto quando:** a chamada funciona e o LiveKit só vê pacotes cifrados.

---

## Fase 3: extras

### F3-01 Música ouvindo junto
- **Entrega:** vinil girando no topo da conversa do grupo; Spotify "ouvir junto" (limite de 5 pessoas no modo de desenvolvimento) e depois áudio próprio pelo LiveKit. Ver [produto/musica.md](produto/musica.md).
- **Pronto quando:** duas pessoas ouvem a mesma música no mesmo ponto.

### F3-02 Efeitos sonoros completos
- **Entrega:** sons para menção, alguém entrou na chamada e item recebido, com volume por categoria.
- **Pronto quando:** cada categoria liga, desliga e muda de volume separada.

---

## Fase 4: inventário de itens comemorativos

A última fase. Itens em 3D que marcam datas e momentos (por exemplo, o aniversário do grupo ou uma data especial), guardados no inventário de cada pessoa e exibidos no perfil. Não há compra, venda, caixa nem mercado. Os detalhes de cada tarefa ainda vão ser definidos; abaixo está só o combinado até agora.

### F4-01 Itens comemorativos
- **Entrega:** a lista dos primeiros itens (qual data ou momento cada um marca) e a regra de como a pessoa ganha.
- **Pronto quando:** a lista está nesta página e a regra está decidida.

### F4-02 Inventário
- **Entrega:** o servidor guarda quais itens cada pessoa tem; o app mostra a lista.
- **Pronto quando:** um item dado a alguém aparece no inventário dela, também depois de fechar e abrir o app.

### F4-03 Item em 3D
- **Entrega:** o item aparece em 3D, girando, dentro do app.
- **Pronto quando:** abrir um item do inventário mostra o modelo girando sem travar a conversa.

### F4-04 Itens no perfil
- **Entrega:** a pessoa escolhe quais itens aparecem no perfil, e quem abre o perfil vê esses itens.
- **Pronto quando:** outra pessoa abre o seu perfil e vê os itens que você escolheu.
