# 0013. Música por nome/link e fila compartilhada

> Encerrada pela [decisão 0014](0014-sem-musica-por-pedidos.md), em 04/11/2026: o YouTube bloqueia a VPS e o backend saiu do repositório. Conteúdo preservado como histórico.

Data: 2026-11-02. Substitui a [decisão 0010](0010-musica-via-spotify.md).

**Decisão:** música como bot de sala: pessoa informa nome/link, servidor mantém fila e worker separado resolve fonte, decodifica e publica faixa musical no LiveKit. Caminho inicial: yt-dlp, FFmpeg e publicador Rust. Backend e painel desktop com toca-discos 3D iniciais implementados; validação ponta a ponta pendente.

**Por quê:** a experiência desejada é pedir músicas e ouvir a mesma fila, sem catálogo próprio ou Premium obrigatório. Spotify deixa de ser o caminho principal.

**Condições:** acesso não oficial ao YouTube tem restrições e pode exigir manutenção. Não implica autorização de retransmissão. O worker não recebe chave MLS; a implementação usa sala LiveKit separada da chamada privada.

O guia de implementação (`docs/musica-implementacao.md`) saiu do repositório com a decisão 0014; está no histórico do git até a v0.6.2.
