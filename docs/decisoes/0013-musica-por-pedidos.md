# 0013. Música por nome/link e fila compartilhada

Data: 2026-11-02. Substitui a [decisão 0010](0010-musica-via-spotify.md).

**Decisão:** música como bot de sala: pessoa informa nome/link, servidor mantém fila e worker separado resolve fonte, decodifica e publica faixa musical no LiveKit. Caminho inicial: yt-dlp, FFmpeg e publicador Rust. Backend e painel desktop com toca-discos 3D iniciais implementados; validação ponta a ponta pendente.

**Por quê:** a experiência desejada é pedir músicas e ouvir a mesma fila, sem catálogo próprio ou Premium obrigatório. Spotify deixa de ser o caminho principal.

**Condições:** acesso não oficial ao YouTube tem restrições e pode exigir manutenção. Não implica autorização de retransmissão. O worker não recebe chave MLS; a implementação usa sala LiveKit separada da chamada privada.

Ver [guia de implementação](../musica-implementacao.md).
