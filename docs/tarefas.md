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
4. **Segue o design system**: preto e branco, VT323 nos títulos, nomes e listas; Geist no texto das mensagens; cor só em raridades e erros. Ver [design-system.md](design-system.md).
5. **Pode ser desligada**, se for um módulo (sala 3D, mercado, caixas, música, efeitos, dither). Ver [produto/modulos.md](produto/modulos.md).
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
- **Entrega:** fundo em dither (Bayer) animado devagar, atrás da lista de conversas. Um bipe curto e baixo para mensagem recebida de outra pessoa; sem som ao enviar nem ao digitar. O som tem interruptor na F1-13.
- **Pronto quando:** dá para ver o dither, ouvir o bipe de recebida, e desligar cada um sem reiniciar o app.

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

## Fase 3: personagens e salas (MVP)

### F3-01 Bevy dentro do Slint
- **Entrega:** crate `client-room` que roda o Bevy desenhando numa textura mostrada dentro da janela Slint. Feature do Cargo `sala-3d`.
- **Pronto quando:** um cubo girando aparece no topo do chat do grupo, e o app compila e roda com a feature desligada.

### F3-02 Esqueleto base e slots
- **Entrega:** um esqueleto único (referência VRM) e slots: cabelo, boné/chapéu, olhos, camiseta, moletom, calça, meia, tênis e até 3 patches por peça. Peças em glTF (.glb) carregadas por slot. Ver [produto/personagem.md](produto/personagem.md).
- **Pronto quando:** trocar uma peça em código troca o modelo sem quebrar a animação.

### F3-03 Shader toon
- **Entrega:** material fosco com sombra em degraus e contorno opcional.
- **Pronto quando:** o personagem parece art toy, não plástico brilhante; o print vai para a decisão do visual.

### F3-04 Editor de personagem
- **Entrega:** tela com o personagem girando e uma lista por slot para escolher as peças. Salva no servidor.
- **Pronto quando:** você monta um personagem, fecha o app, abre de novo e ele está igual.

### F3-05 Itens grátis iniciais
- **Entrega:** um conjunto inicial de peças grátis para todo mundo, com licença comercial conferida e rig no esqueleto base.
- **Pronto quando:** cada slot tem pelo menos 3 opções.

### F3-06 Sala com assentos
- **Entrega:** cenário da sala com sofá e assentos; quem entra na chamada aparece sentado.
- **Pronto quando:** cinco pessoas na chamada aparecem cada uma no seu lugar, vestidas do seu jeito.

### F3-07 Destaque de quem fala na sala
- **Entrega:** o personagem de quem fala ganha destaque (luz, contorno ou leve animação), usando o sinal da F2-04.
- **Pronto quando:** dá para saber quem fala só olhando a sala.

### F3-08 Cartão de perfil
- **Entrega:** cartão com mural, foto com moldura, bio, personagem e itens equipados; placa na linha da lista de conversas. Ver [produto/perfil.md](produto/perfil.md).
- **Pronto quando:** clicar em alguém abre o cartão igual ao mockup.

---

## Fase 4: economia

Nada desta fase vai ao ar antes da F4-01.

### F4-01 Revisão jurídica e 18+
- **Entrega:** revisão de [legal.md](legal.md) com alguém da área e confirmação de idade 18+ na entrada.
- **Pronto quando:** existe um registro em [decisoes/](decisoes/) com o que foi aprovado.

### F4-02 Inventário e itens
- **Entrega:** crate `economy` no servidor. Cada item tem raridade, float (0 a 1, desgaste) e padrão. O servidor é a fonte da verdade de quem tem o quê. Toda mudança de posse numa transação do banco.
- **Pronto quando:** testes mostram que um item nunca fica com dois donos, nem com requisições ao mesmo tempo.

### F4-03 Inspecionar em 3D
- **Entrega:** botão direito em item ou personagem → "Inspecionar", com o item girando, float e padrão. Ver [produto/itens-e-mercado.md](produto/itens-e-mercado.md).
- **Pronto quando:** o desgaste do float aparece no modelo.

### F4-04 Caixas e chaves
- **Entrega:** abrir caixa com chave; sorteio feito no servidor, com as chances publicadas. Animação e som por raridade.
- **Pronto quando:** teste com muitas aberturas confirma as chances publicadas.

### F4-05 Saldo interno
- **Entrega:** saldo dentro do app, sem saque. Extrato com cada entrada e saída.
- **Pronto quando:** o saldo bate com a soma do extrato em todos os testes.

### F4-06 Mercado
- **Entrega:** vender com preço livre, comprar, e pedidos de compra que fecham sozinhos quando aparece oferta. Taxa por venda.
- **Pronto quando:** um item sai de uma caixa, é inspecionado e vendido para outra pessoa.

### F4-07 Gráfico de preço
- **Entrega:** gráfico de preço médio por dia e lista de ofertas, no estilo do mercado da Steam.
- **Pronto quando:** o gráfico bate com as vendas registradas.

---

## Fase 5: extras

### F5-01 Andar pela sala
- **Entrega:** mover o personagem com teclado ou clique; posição sincronizada com os outros.
- **Pronto quando:** dois personagens andam e se veem andando sem travar.

### F5-02 Música ouvindo junto
- **Entrega:** vinil girando no canto da sala; Spotify "ouvir junto" (limite de 5 pessoas no modo de desenvolvimento) e depois áudio próprio pelo LiveKit. Ver [produto/musica.md](produto/musica.md).
- **Pronto quando:** duas pessoas ouvem a mesma música no mesmo ponto.

### F5-03 Página de perfil
- **Entrega:** página completa com wallpaper e vitrines numa grade, estilo Steam. Imagens enviadas passam por moderação.
- **Pronto quando:** a página fica igual ao artboard "PaginaPerfil" do mockup.

### F5-04 Áudio espacial
- **Entrega:** volume de cada pessoa pela distância na sala.
- **Pronto quando:** quem está longe soa mais baixo, e o interruptor desliga o efeito.

### F5-05 Efeitos sonoros completos
- **Entrega:** sons para menção, alguém entrou na sala, item recebido, venda concluída e caixa por raridade, com volume por categoria.
- **Pronto quando:** cada categoria liga, desliga e muda de volume separada.
