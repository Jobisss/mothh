# Operar e integrar o backend de música

Implementação inicial: 02/11/2026. Servidor e worker implementados; o [painel desktop](musica-frontend.md) envia comandos e assina a sala musical. A imagem Docker de produção inclui e habilita o módulo; fora dela, a configuração é manual.

## Atualizar a VPS com Docker Compose

Na pasta do projeto na VPS, preservando o `.env.prod` existente:

```sh
git pull --ff-only
docker compose -f docker-compose.prod.yml build server
docker compose -f docker-compose.prod.yml up -d --no-deps --force-recreate server
docker compose -f docker-compose.prod.yml logs --tail=100 server
```

O Dockerfile compila `server` e `music-worker`, instala FFmpeg e `yt-dlp[default]` (com EJS) e inclui Deno. Define os três caminhos de música na própria imagem, sem exigir edição do `.env.prod`. As versões de Deno e yt-dlp ficam fixadas no Dockerfile; atualize-as e reconstrua a imagem quando necessário. O LiveKit usa as configurações existentes. Não há novas portas.

Se você adicionou caminhos manualmente ao `.env.prod`, eles prevalecem sobre os padrões da imagem: remova-os ou use `/usr/local/bin/music-worker`, `/opt/yt-dlp/bin/yt-dlp` e `/usr/bin/ffmpeg`, respectivamente. `MOTHH_MUSIC_WORKER=` vazio desabilita o módulo. Depois de alterar ambiente, recrie o contêiner; apenas `restart` não aplica a mudança. Não execute `down -v`.

Confira as dependências dentro do contêiner:

```sh
docker compose -f docker-compose.prod.yml exec server sh -ec '
  test -x "$MOTHH_MUSIC_WORKER"
  "$MOTHH_YT_DLP" --version
  "$MOTHH_FFMPEG" -version | head -n 1
  deno --version
  ldd "$MOTHH_MUSIC_WORKER"
'
```

Não deve haver bibliotecas `not found`. No app, reabra o painel, peça uma faixa curta e ative a escuta em duas contas. A mensagem de módulo desabilitado deve desaparecer; resolver uma faixa e reproduzi-la dependem também do acesso ao provedor e da conectividade LiveKit. O teste de imagem no CI verifica executáveis e dependências, sem acessar YouTube nem transmitir áudio.

## Executáveis e configuração

Compilar com a mesma toolchain e bibliotecas nativas do cliente LiveKit:

```sh
cargo build -p server -p music-worker --release
```

Instalar yt-dlp e FFmpeg de fontes oficiais no host de reprodução; versões devem ser controladas pelo operador. Versões recentes do extrator podem precisar de runtime JavaScript e componentes adicionais conforme seu README. Não são baixados automaticamente pelo servidor. [yt-dlp](https://github.com/yt-dlp/yt-dlp), [FFmpeg](https://ffmpeg.org/download.html).

Configurar o servidor (exemplo PowerShell, a partir da raiz):

```powershell
$env:MOTHH_MUSIC_WORKER = (Resolve-Path target/release/music-worker.exe).Path
$env:MOTHH_YT_DLP = 'C:\caminho\yt-dlp.exe'
$env:MOTHH_FFMPEG = 'C:\caminho\ffmpeg.exe'
cargo run -p server --release
```

No Linux, usar caminhos absolutos equivalentes e `music-worker` sem `.exe`. São necessárias as variáveis LiveKit existentes (`MOTHH_LIVEKIT_URL`, `MOTHH_LIVEKIT_KEY`, `MOTHH_LIVEKIT_SECRET`) e PostgreSQL. Sem `MOTHH_MUSIC_WORKER`, o servidor responde que música está desabilitada. Executáveis ausentes geram falha visível no pedido/reprodução.

O servidor cria subprocessos no mesmo host. Não há API pública do worker ou porta adicional. Configuração e JWT passam por stdin; stdout transporta eventos JSON. Variáveis de banco e chave LiveKit não são herdadas pelo worker. O JWT permite publicar apenas na sala musical, não assinar outras faixas nem publicar dados.

## Contrato WebSocket

Usar o WebSocket existente, depois de autenticar. IDs abaixo são exemplos; substituir por UUIDs reais do grupo e da requisição.

```json
{
  "type": "music",
  "conversation_id": "11111111-1111-4111-8111-111111111111",
  "command": {
    "action": "enqueue",
    "request_id": "22222222-2222-4222-8222-222222222222",
    "query": "nome da música e artista"
  }
}
```

`query` também aceita link de vídeo YouTube; o servidor reconstrói uma URL de host fixo. Pesquisa textual seleciona um resultado. Não há catálogo ou endpoint de navegação.

| `action` | Campos adicionais | Permissão |
| --- | --- | --- |
| `get_snapshot` | Nenhum | Membro |
| `listen` | Nenhum | Membro |
| `enqueue` | `request_id`, `query` | Membro |
| `remove` | `item_id` | Autor do pedido pendente ou dono |
| `pause`, `resume`, `skip`, `clear`, `stop` | Nenhum | Dono do grupo |

Na Sala do Grupo, sem dono, quem faz o primeiro pedido vira condutor da sessão. `controller_id` no snapshot identifica quem controla. A sessão transitória expira após cinco minutos sem comandos e sem worker ativo. Uma sessão nova escolhe novo condutor.

`expected_revision` opcional fica no envelope ao lado de `command`. Quando informado, uma mutação sobre revisão antiga é recusada. Repetir `enqueue` com mesmo usuário/`request_id` devolve o snapshot sem duplicar o pedido, dentro da janela dos últimos 512 pedidos daquela sessão. Solicitações recusadas antes de reservar item podem ser tentadas novamente.

Respostas:

- `music_snapshot`: inclui `snapshot`, com conversa, sessão, revisão, geração, condutor, faixa atual, fila, estado, posição confirmada, horário da atualização e último erro.
- `music_error`: inclui conversa, `request_id` quando aplicável e `reason`.
- `music_token`: inclui conversa, `session_id`, URL LiveKit e token somente de escuta, válido por 60 segundos para conectar.

O servidor também distribui snapshots aos membros após resolução, controle e progresso. A mesma revisão pode chegar como resposta e evento: o cliente deve ignorar duplicatas. Ao trocar de sessão, descartar revisões da anterior. `get_snapshot` recupera o estado após reconexão/perda de eventos.

## Fluxo do cliente

1. Pedir `get_snapshot` ao abrir o painel.
2. Ao optar por ouvir, enviar `listen`, conectar à sala do token e assinar a faixa `mothh-music`. Essa conexão é separada da chamada privada; não usar a chave MLS nela.
3. Enviar `enqueue` com UUID novo por pedido e reutilizá-lo em retries do mesmo pedido.
4. Renderizar `resolving`, `ready` e `failed`. Faixas com erro ficam visíveis na fila até remoção/limpeza, mas não bloqueiam as próximas.
5. Tratar estados `idle`, `starting`, `playing`, `paused` e `interrupted`. Posição é atualizada aproximadamente a cada segundo; interpolação é apenas visual.
6. Ao parar de ouvir, desconectar da sala musical. Quando não houver participantes remotos por 30 segundos durante reprodução, o worker encerra e o servidor limpa a sessão musical.
7. Mudança de membros interrompe a reprodução e encerra a sessão. Membros restantes precisam obter snapshot/token novos; a sala antiga nunca é reutilizada. Isso não expulsa imediatamente conexões antigas, mas interrompe sua fonte de música.

## Limites e comportamento atual

Falhas de reprodução geram um aviso `Falha na reprodução de música` nos logs do servidor, com conversa, geração e motivo. O worker drena stderr do yt-dlp e do FFmpeg em paralelo, mantendo até 8 KiB por processo; apenas categorias conhecidas e o estado de saída são enviados ao servidor/app. O conteúdo bruto não é registrado, para não expor URLs assinadas ou credenciais. Consulte com `docker compose -f docker-compose.prod.yml logs -f --tail=100 server`. A mensagem `Fonte parou de enviar áudio` é um timeout de leitura de 30 segundos, não prova de que o YouTube recusou o pedido; o diagnóstico adicional distingue bloqueio, formato, runtime e entrada inválida quando reconhecidos.

- Somente grupos; comandos em conversas diretas são recusados.
- Até 64 sessões em memória, quatro reproduções simultâneas e duas resoluções simultâneas no servidor.
- Até 50 itens contando a faixa atual; até cinco pedidos pendentes por pessoa e dois segundos entre pedidos aceitos.
- Pedido com até 300 bytes, duração positiva e no máximo 20 minutos; lives e playlists não são aceitas.
- Resolução com timeout de 30 segundos e saída limitada a 256 KiB.
- `clear` remove pendentes; `stop` remove tudo e encerra a faixa; `skip` avança; falha de reprodução marca o item e tenta a próxima.
- Pausa encerra o processo atual e preserva a última posição confirmada. Retomar reabre a fonte e FFmpeg descarta o início até a posição: pode demorar, principalmente em faixas longas. Não é seek instantâneo.
- Sem vaga de reprodução, a faixa fica `interrupted`; `resume` tenta novamente.
- Reinício do servidor perde as filas. Sem armazenamento permanente das músicas.
- Antes de abrir a fonte, o worker aguarda até 30 segundos por um ouvinte; não consome a fila inteira sem pessoas conectadas.

## Isolamento e limites de validação

Música usa uma sala `music-<session_id>` exclusiva. O worker não recebe chave MLS e não participa da sala de voz/vídeo. Tokens de ouvintes não autorizam publicar; token do worker não autoriza assinar mídia. A música é conteúdo conhecido pelo serviço, não E2EE entre pessoas.

O código usa argumentos separados, URLs normalizadas, ambientes reduzidos, buffers limitados, timeouts e cancelamento de processos. Isso **não é uma sandbox de sistema operacional**. Para produção, aplicar usuário dedicado, restrições de arquivos/rede/egress, limites de CPU/memória e contenção de descendentes pelo supervisor do host. Não há rotação de proxies, cookies pessoais ou contorno de bloqueios.

Acesso não oficial ao YouTube permanece sujeito às condições da fonte e às permissões sobre o conteúdo; esta implementação não concede autorização de retransmissão. Ver [arquitetura e limitações](musica-implementacao.md).

Testes automatizados de unidade não exigem serviços. `cargo test -p server --test music` precisa do PostgreSQL configurado e testa autorização, idempotência e troca de sala, sem acessar YouTube/LiveKit. Reprodução completa exige serviços reais e conteúdo autorizado de teste; não considerar a compilação como validação ponta a ponta.
