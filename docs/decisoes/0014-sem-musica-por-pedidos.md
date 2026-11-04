# 0014. Sem música por pedidos; fica a atividade do Spotify

Data: 2026-11-04. Encerra a [decisão 0013](0013-musica-por-pedidos.md).

**Decisão:** o mothh não toca música pedida por nome ou link. Sai do repositório o backend de música (worker, yt-dlp, FFmpeg, fila no servidor, provedor de PO Token no Compose) e o painel de música do app. Fica a [atividade do Spotify no perfil](../spotify-atividade.md), com o toca-discos 3D.

**Por quê:** a fonte era o YouTube extraído de uma VPS, e o YouTube bloqueia IPs de datacenter ("Sign in to confirm you're not a bot"). PO Tokens não contornam a restrição por IP; cookies de conta duram pouco e arriscam a conta; o que faz bots como o Jockie funcionarem é proxy residencial pago e manutenção constante. As APIs oficiais (Deezer, Spotify) entregam metadados e prévias de 30 segundos, não a faixa inteira. Não vale o custo nem a fragilidade para um app de amigos.

**Fica em aberto:** se um dia voltar, o caminho honesto é quem pede transmitir do próprio PC (loopback de áudio, como o compartilhar tela já faz), usando o servidor só para a fila. Histórico e desenho do que existiu estão nos commits até a v0.6.2 e nas notas da [v0.5.0](../releases/v0.5.0.md).
