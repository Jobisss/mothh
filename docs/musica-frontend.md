# Painel de música

Implementação inicial em Slint/Rust, baseada na referência visual enviada: equipamento à esquerda, faixa e informações no centro, visualizador à direita, progresso e controles na parte inferior, fila e link externo no canto superior.

## Toca-discos 3D

O painel usa o modelo original `assets/models/disco.glb`, embutido no executável, com 19 peças separadas. `disco-2.glb` é uma alternativa de malha única preservada para comparação; não é carregada no cliente normal. Ambos vieram sem texturas. A apresentação aplica materiais em preto, cinza e prata, incluindo tampa translúcida.

O botão de música e o painel usam a própria cena 3D, sem a nota musical 2D. Arrastar o aparelho no painel orbita a câmera; clique duplo restaura o enquadramento. Durante a reprodução, somente o vinil e seu rótulo giram, aproximadamente a 33⅓ rpm. Ao pausar ou ficar sem faixa, o disco para e o braço sobe em uma transição de cerca de 300 ms; ao tocar, o braço desce. Manipular o modelo não envia comandos de música.

`turntable_mesh.rs` lê o GLB com `gltf` e rasteriza geometria, profundidade, normais e transparência. `turntable.rs` executa essa renderização em uma thread separada e entrega quadros RGBA de 96×90 ao Slint, ampliados sem suavização. O filtro Bayer usa 55% de intensidade, misturado à cor original para manter os pontos discretos. Não há webview, fotografia substituta ou sequência de quadros pré-renderizada. A animação é limitada a 10 quadros por segundo enquanto o aparelho ou seu ícone está visível; sem visibilidade ou com a faixa pausada e o braço estabilizado, só renderiza quando câmera/estado muda.

A miniatura do vídeo atual é buscada no CDN público do YouTube, com ID validado, timeout de oito segundos, limite de 512 KiB e limite de dimensões/decodificação. A busca roda fora da interface e é cancelada quando a faixa muda ou o app fecha. A imagem é aplicada ao rótulo por UV na própria geometria, acompanhando sua rotação; não é um adesivo fixo sobre o painel. Miniatura indisponível deixa o rótulo neutro e não bloqueia o áudio. O preview pode usar imagem local explícita por `MOTHH_TURNTABLE_PREVIEW_COVER` e não faz acesso de rede.

O mapeamento é específico deste asset: malhas 4 e 14 são disco/rótulo; malha 17 define o eixo; malha 0 é a tampa e malha 2 é o braço. Ao reexportar o modelo, revisar esses índices. Para comparar os dois arquivos sem abrir o chat: `cargo run -p client --example turntable_preview`, que grava PNGs em `target/turntable-preview/`.

## Uso

- Em um grupo, o botão de nota musical abre/fecha o painel sem interromper a música.
- Digitar nome ou link e apertar Enter ou `+` envia um pedido e entra explicitamente na escuta daquela sala. O campo é limpo somente quando o servidor confirma o pedido.
- A fila no canto superior mostra próximos pedidos, resolução e falhas; autor/dono podem remover conforme as permissões do servidor.
- Play/pause controla a sala para o condutor que já está ouvindo. Para os demais, controla apenas a entrada/saída da escuta. Iniciar a escuta com a sala pausada também pede retomada quando quem clica é o condutor.
- Próxima, limpar fila e parar tudo dependem da permissão de controle. O servidor continua validando cada operação.
- Volume é individual, entre 0 e 100%, persistido no perfil. Não altera voz ou compartilhamento de tela. Clique duplo volta a 100%; rolagem ajusta em passos de 5%.
- Link externo abre o vídeo atual no YouTube, reconstruindo a URL a partir do ID validado.
- Ao trocar de conversa, a escuta continua. Uma linha no rodapé lateral permite voltar ao grupo da música ou parar de ouvir.
- Desligar o módulo nas configurações ou sair da conta encerra a conexão musical. Remoção do grupo também encerra a escuta correspondente.

## Dados e transporte

`crates/client/src/music.rs` mantém snapshots por grupo, revisões, sessões encerradas, pedidos aguardando confirmação e uma conexão musical por cliente. `net.rs` faz o despacho pelo WebSocket existente. O listener assina uma sala LiveKit separada da chamada privada e não usa a chave MLS.

Eventos de uma tentativa de conexão antiga são ignorados. Troca de sessão pede credenciais novas apenas se a pessoa já escolheu ouvir aquela sala. Perda do WebSocket suspende a conexão musical; após login/reconexão, a associação aos grupos é conferida antes de retomar. Mutações pendentes não são reenviadas automaticamente.

O áudio é reproduzido pelo WebRTC. A leitura adicional de PCM alimenta 32 bandas logarítmicas calculadas por Goertzel, com janela limitada a 1024 amostras e atualização aproximada de 10 Hz. As barras em funcionamento não são aleatórias. Sem áudio ou fora da sala ouvida, ficam zeradas.

Metadados disponíveis são título e canal de origem. Não apresentar canal como artista verificado ou inventar álbum. A posição interpola a última confirmação recebida; não é um controle de seek. Letras, anterior, shuffle e repetição não são oferecidos porque o backend atual não implementa essas funções.

## Validação visual de desenvolvimento

O executável de debug tem um modo explícito de preview, sem login ou conexão ao servidor. Os dados desse modo são demonstrativos e não fazem parte do fluxo normal. Ele salva a janela renderizada e encerra:

```powershell
cargo build -j 2 -p client
$env:SLINT_BACKEND = 'winit-software'
$env:MOTHH_MUSIC_PREVIEW = "$PWD/target/music-ui-wide.png"
./target/debug/mothh.exe
```

Variantes: `MOTHH_MUSIC_PREVIEW_SIZE=compact`; `MOTHH_MUSIC_PREVIEW_STATE=queue` ou `error`. Remover essas variáveis antes de abrir o aplicativo normalmente. O atalho de preview não existe em release.

Testes de estado e análise de áudio: `cargo test -j 2 -p client music::tests`. Reprodução real depende do [backend configurado](musica-backend.md), LiveKit, PostgreSQL, yt-dlp e FFmpeg; previews e testes locais não substituem essa validação ponta a ponta.
