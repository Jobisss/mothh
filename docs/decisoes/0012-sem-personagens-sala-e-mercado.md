# 0012. Sem personagens, sala 3D e mercado; inventário de itens comemorativos

**Decisão:** o mothh desiste dos personagens 3D, da sala 3D e de toda a economia (itens com raridade, caixas, saldo e mercado). No lugar, entra um inventário de itens comemorativos em 3D que a pessoa exibe no perfil, sem compra, venda nem troca, como a última fase do roadmap. Substitui a 0002 (personagens e salas com Bevy) e a 0009 (saldo sem saque), que foram apagadas junto com o código e os modelos.

**Por quê:** a primeira roupa gerada no Tripo e passada pelo pipeline (rig do Tripo, retarget do Mixamo, encaixe por molde) ainda saía com defeitos visíveis, e o caminho até um boneco bom no app (Bevy dentro do Slint, editor, shader, dezenas de peças) era longo demais para o que traz. Sem os bonecos, a sala 3D e o mercado de roupas perdem o sentido.

**O que muda nas outras notas:** a 0005 fala de "um chat e uma sala" (agora é um chat e uma chamada); a 0006 diz que a cor vem das raridades dos itens (agora vem das fotos e dos itens comemorativos); a 0010 põe o vinil "na sala" (agora fica no topo da conversa do grupo).

**O que fica de fora:** quais itens existem, como se ganha cada um e como o 3D é desenhado no app ainda vão ser decididos (ver [inventário](../produto/inventario.md)). O código apagado continua no histórico do Git, até o commit `7ccac57`.
