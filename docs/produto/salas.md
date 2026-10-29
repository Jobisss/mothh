# Salas

Cada grupo tem uma sala 3D. Quando alguém entra na chamada, o personagem aparece sentado num sofá ou mesa.

- **Cena**: cada sala é uma cena glTF com assentos marcados por nós nomeados (`seat_01`, `seat_02`). Salas compráveis são cenas diferentes.
- **Câmera**: fixa no início (isométrica ou orbital limitada).
- **Fundo**: dither animado em tons de cinza.
- **Quem fala**: o evento de "active speaker" do LiveKit acende um anel branco no personagem, inverte o chip do nome e mexe a boca.
- **Andar (fase 5)**: clicar no chão para andar até lá, caminho por navmesh. O serviço de presença repassa as posições cerca de 10 vezes por segundo e os clientes interpolam. Só andar, sem outras ações.
- **Áudio espacial (opcional)**: volume cai com a distância entre os personagens.
- **Presença**: quem está onde e o que está vestindo fica num serviço separado das mensagens (Redis), porque muda rápido e não precisa ser guardado.
- **Música**: o vinil da música da sala gira no canto (ver [música](musica.md)).
