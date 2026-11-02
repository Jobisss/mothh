# Música

Música é um módulo opcional com experiência de bot na sala: a pessoa informa um link ou nome de música/artista, o mothh resolve o pedido e adiciona à fila compartilhada. Não há catálogo próprio ou tela de navegação musical na primeira versão.

Um worker transmite uma faixa de música pelo LiveKit. Quem entra recebe o fluxo atual; o som continua se quem pediu sair. Cada pessoa controla seu volume, sem conectar Spotify ou assinar Premium.

O painel mostra faixa atual, quem pediu, próximas músicas e comandos permitidos. Membros adicionam pedidos e removem seus pendentes; administradores controlam reprodução e fila. O layout segue a referência visual, com informações, visualizador de áudio e controles abaixo.

O [painel inicial](../musica-frontend.md) está implementado seguindo a referência visual, com o toca-discos de `disco.glb` renderizado em 3D: câmera por arraste e vinil girando durante a reprodução. Teste completo de reprodução com o backend configurado continua necessário.

O caminho proposto usa yt-dlp, FFmpeg e publicador Rust. Acesso não oficial ao YouTube tem restrições, pode falhar e não concede direitos de retransmissão. O worker não recebe a chave MLS da conversa nem deve comprometer sua cifra.

Ver [guia de implementação](../musica-implementacao.md) e [decisão 0013](../decisoes/0013-musica-por-pedidos.md). O [estudo de Spotify](../spotify-implementacao.md) permanece como referência anterior.
