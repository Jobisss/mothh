# 0010. Música sincronizada pelo Spotify, com plano B

> Substituída pela [decisão 0013](0013-musica-por-pedidos.md), em 02/11/2026. Conteúdo preservado como histórico.

> Revisão de 02/11/2026: a afirmação de conformidade abaixo ainda precisa ser validada para o cenário concreto de chamadas do mothh. O [guia de implementação](../spotify-implementacao.md) registra as restrições atuais, a arquitetura proposta e os ajustes visuais necessários. Transmissão de arquivo próprio também depende dos direitos de uso; grupo privado, isoladamente, não garante autorização.

**Decisão:** ouvir junto pelo Spotify, sincronizando a faixa e a posição entre os clientes via Web API. Como o Spotify limita apps não aprovados a 5 usuários, o plano B é o dono da sala transmitir um arquivo de áudio próprio pelo LiveKit.

**Por quê:** é a forma permitida de ouvir junto com o Spotify, e o vinil girando na sala é parte da identidade. Ver música (`docs/produto/musica.md`, removido com a decisão 0014).
