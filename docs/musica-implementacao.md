# Música por pedidos e fila compartilhada

Data: 02/11/2026. Status: backend e [painel desktop com toca-discos 3D](musica-frontend.md) iniciais implementados; validação ponta a ponta pendente. Substitui o Spotify como direção principal da F3-01; ver [decisão 0013](decisoes/0013-musica-por-pedidos.md). Configuração e contrato real: [operar o backend](musica-backend.md).

## Experiência

A pessoa informa um link de vídeo ou o nome de uma música/artista no painel da sala. O mothh resolve o pedido, informa o título encontrado e adiciona à fila. Um processo de reprodução transmite a música para os ouvintes pelo LiveKit, como um bot de música em uma chamada. Ao terminar, toca a próxima.

Não haverá catálogo hospedado, uploads ou tela de navegação musical na primeira versão. Busca por nome é uma etapa interna de resolução. A fila mostra os próximos pedidos. Não exige Spotify ou conta Premium dos ouvintes.

## Arquitetura

```text
Cliente Slint: nome/link e comandos
                |
Servidor mothh: autorização, fila e estado
                |
Worker de música: resolvedor -> yt-dlp -> FFmpeg -> publicador Rust
                                                       |
                                             faixa musical LiveKit
                                                       |
                                                ouvintes da sala
```

O servidor mantém uma fila oficial por conversa. Um serviço separado executa resolução e reprodução, isolando dependências externas e falhas do processo principal do chat. Uma instância pode gerenciar vários players, com um player por sala e limite global de concorrência.

O worker recebe identidade técnica e token LiveKit de curta duração, vinculado à sala, sem permissão de assinar mídia dos participantes. Não usa a conta mothh de uma pessoa.

| Componente | Função |
| --- | --- |
| Resolvedor | Distinguir link de texto, encontrar faixa e obter metadados |
| yt-dlp | Resolver fonte e obter o fluxo disponível |
| FFmpeg | Decodificar a mídia para PCM no formato do publicador |
| Publicador Rust | Alimentar fonte de áudio em tempo real e publicar no LiveKit |
| Servidor mothh | Autorizar comandos, ordenar fila e distribuir estado |
| Cliente mothh | Exibir fila, enviar comandos e controlar volume recebido |

Lavalink com `youtube-source` é uma referência desse ecossistema. Não é a escolha inicial porque sua integração de voz é voltada ao Discord; o mothh usa LiveKit. O caminho proposto é extração em subprocessos e publicação Rust.

Referências: [Lavalink REST](https://lavalink.dev/api/rest), [youtube-source](https://github.com/lavalink-devs/youtube-source), [yt-dlp](https://github.com/yt-dlp/yt-dlp).

## Resolução do pedido

1. Cliente envia texto e `request_id` único.
2. Servidor verifica autenticação, associação à conversa, módulo habilitado e limites de pedidos.
3. Reserva posição na fila como `resolving`; resolução acontece fora do bloqueio da fila.
4. Link aceito é normalizado para ID da fonte. Texto é pesquisado pelo resolvedor; inicialmente selecionamos um resultado e informamos claramente o título encontrado.
5. Resolvedor devolve título, duração, canal/autor, link canônico e ID da fonte. Canal não equivale automaticamente a artista verificado.
6. Item passa a `ready` ou `failed`, com motivo visível. A ordem dos pedidos é preservada mesmo se resoluções terminarem em ordem diferente; itens falhos são ignorados na reprodução.

Primeira versão aceita vídeos individuais. Playlists, lives, conteúdo privado, com autenticação ou indisponível retornam erro; não há expansão automática ou contorno de bloqueios. Definir limites configuráveis de duração, tamanho do pedido, fila e frequência por pessoa antes da entrega.

Guardar ID e link canônico, não a URL temporária do áudio. Resolver o fluxo perto da reprodução porque o endereço pode expirar; permitir uma nova resolução limitada se esse for o motivo da falha.

## Fila e protocolo proposto

```text
MusicSession: conversation_id, session_id, generation, revision,
              playback_state, current_item_id, position_ms, updated_at
QueueItem: item_id, request_id, source, source_id, canonical_url,
           title, duration_ms?, requested_by, status, error_code?
```

Comandos: `MusicEnqueue`, `MusicRemove`, `MusicPause`, `MusicResume`, `MusicSkip`, `MusicClear`, `MusicStop`, `MusicGetSnapshot`. Eventos: `MusicSnapshot`, `MusicQueueChanged`, `MusicPlaybackChanged`, `MusicRequestFailed`. São tipos a implementar, não APIs existentes.

Cada mutação incrementa `revision`. Cada execução recebe uma geração: eventos atrasados de uma faixa não podem encerrar a seguinte. Mesmo `request_id` não duplica pedido. Clientes que perdem revisões pedem snapshot completo.

Permissões iniciais propostas: membro adiciona e remove seus pedidos pendentes; administrador pausa, retoma, pula, remove qualquer pedido, limpa e para. Validar no servidor em toda operação. Votação e reordenação ficam para depois.

Fila e estado são transitórios no MVP: reinício do servidor encerra sessões. Persistência e retomada ficam para etapa posterior.

## Reprodução

Com item pronto e ouvintes, o coordenador entrega a faixa ao worker. Ele resolve a mídia e inicia FFmpeg consumindo o fluxo com buffer limitado. Não precisa baixar a música inteira nem armazenar MP3 permanentemente.

Formato inicial proposto: PCM signed 16-bit, 48 kHz, estéreo, quadros de 20 ms; validar com a versão do SDK instalada. O publicador alimenta `NativeAudioSource` respeitando relógio e contrapressão. O SDK faz codificação e transporte. Não enviar todo o PCM de uma vez.

Publicar uma faixa identificada como música, com configuração estéreo e sem processamento de voz. O projeto já usa `NativeAudioSource` e `MUSIC_STEREO` em `crates/client/src/call.rs`, como referência para o novo serviço.

O cliente ajusta o volume dessa faixa independentemente de microfone e demais participantes. Entrada tardia recebe o fluxo corrente; não inicia reprodução local do começo. Latência varia com rede e buffer: não prometer sincronismo em nível de amostra.

| Ação/situação | Comportamento |
| --- | --- |
| Pausar | Preservar posição; suspender consumo/produção com buffer limitado. Se a origem não sobreviver, reabrir na posição confirmada ao retomar; validar no protótipo |
| Pular | Cancelar execução, encerrar subprocessos e iniciar próxima exatamente uma vez |
| Fim natural | Avançar somente após confirmar a geração atual |
| Limpar | Remover pendentes; manter a faixa atual |
| Parar | Encerrar reprodução e limpar fila |
| Sala vazia | Após tolerância configurável, parar e limpar; novos ouvintes começam sem reprodução automática |
| Worker caiu | Estado `interrupted`, limpeza pelo supervisor; não repetir comandos sem conferir geração |
| Solicitante saiu | Reprodução e fila continuam |
| Cliente reconectou | Receber snapshot, sem criar outro worker |

## Criptografia e privacidade

O mothh cifra mídia com chave derivada do MLS do grupo, em `crates/client/src/sframe.rs`. **Não entregar essa chave ao worker**, pois isso ampliaria seu acesso à chamada privada.

Proposta: faixa musical de serviço sem cifra de grupo coexistindo com voz/vídeo cifrados. Essa coexistência precisa ser comprovada no SDK atual antes de confirmar o desenho. Não desabilitar a cifra da chamada para acomodar o bot. Música de serviço não será anunciada como E2EE entre pessoas.

Se o SDK não isolar corretamente a faixa, avaliar uma sala LiveKit separada para música, com tokens próprios e assinatura somente da faixa musical. Isso adiciona uma conexão, mas mantém o worker fora da sala privada. Tornar o worker membro MLS exige nova decisão de segurança.

Fila e pedidos são visíveis ao servidor nesta proposta. Não criar histórico permanente de escuta. Logs não devem conter tokens, cookies ou URLs temporárias assinadas.

## Isolamento e operação

- Executar subprocessos com argumentos separados; nunca interpolar nome/link em shell.
- Permitir apenas fontes previstas e IDs normalizados. Bloquear protocolos arbitrários, caminhos locais e destinos internos, inclusive em redirecionamentos.
- Restringir rede do extrator/decoder; validação inicial da URL sozinha não basta.
- Limitar CPU, memória, processos, duração, buffer e tempo de resolução. Cancelamento encerra a árvore de subprocessos.
- Fonte bloqueada retorna erro; não usar cookies pessoais, credenciais compartilhadas, rotação de proxies ou contorno de bloqueios.
- Fixar versões de yt-dlp/FFmpeg, registrar versões para diagnóstico e atualizar de forma controlada.
- Medir tempo de início, falhas e consumo por sala. Software gratuito não elimina custo de CPU e tráfego da VPS/LiveKit.

## Limitações da fonte

Extração e retransmissão de YouTube não são integração oficial. As políticas restringem separação de áudio/vídeo e acesso não autorizado. Link público e ausência de armazenamento permanente não concedem direitos de retransmissão. Não apresentar o fluxo como automaticamente autorizado. Uso e distribuição dependem das condições da fonte e permissões do conteúdo. [Políticas do YouTube](https://developers.google.com/youtube/terms/developer-policies).

Manter o adaptador de fonte substituível. Remoção, bloqueio regional, resultado incorreto e indisponibilidade são erros esperados, sem garantia de que toda música será encontrada ou tocada.

## Integração no repositório

| Local | Mudança planejada |
| --- | --- |
| `crates/protocol/src/lib.rs` | Comandos e eventos |
| `crates/server/src/music.rs` (novo) | Fila, autorização e coordenação |
| `crates/server/src/state.rs`, `ws.rs` | Estado e despacho autenticado |
| `crates/music-worker/` (novo) | Serviço Rust e subprocessos de mídia |
| `crates/client/src/music.rs` (novo) | Estado da UI e assinatura/volume musical |
| `crates/client/src/net.rs`, `main.rs` | Integração com comandos/eventos existentes |
| `crates/client/src/modules.rs` | Interruptor de música |
| `crates/client/ui/app.slint` | Entrada nome/link, fila e controles |

Servidor-worker usam contrato interno autenticado, associado à geração da sessão. Tokens LiveKit são emitidos pelo servidor; segredos de infraestrutura nunca chegam ao formulário de pedido.

## Etapas e validação

1. **Prova de mídia:** worker Rust + FFmpeg publica conteúdo autorizado de teste. Dois clientes recebem estéreo e ajustam volume independentemente; voz/vídeo continuam cifrados. Validar coexistência ou sala musical separada.
2. **Prova de fonte:** resolver link permitido, iniciar fluxo e testar cancelamento/indisponibilidade.
3. **Fila:** pedidos por link, revisões, permissões, avanço automático e idempotência.
4. **Nome:** pesquisa interna, identificação do resultado e tratamento de busca vazia.
5. **Cliente:** módulo opcional, painel mínimo, próximos pedidos e estados de erro.
6. **Operação:** limites, isolamento, quedas, reconexão e atualização de dependências.

Testar pedidos concorrentes/repetidos, skip durante resolução, término atrasado, comando sem permissão, subprocesso travado, fonte indisponível, sala vazia e reinício. Com dois clientes reais, testar entrada tardia, continuidade após saída do solicitante e preservação da cifra da chamada.

Fora da primeira versão: catálogo, uploads, letras, login de streaming, playlists em lote, lives, equalizador e visualizador de frequências. Interface final após comprovar reprodução e estados reais.

O código inicial adota sala musical separada, subprocesso de reprodução, pausa por reinício na posição confirmada e os limites descritos no [guia operacional](musica-backend.md). Partes acima continuam sendo critérios de evolução; a cadeia completa ainda precisa de teste com serviços reais.
