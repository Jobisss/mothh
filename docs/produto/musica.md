# Música

Música é um módulo opcional. Na sala, um vinil gira no canto com a faixa tocando, quantas pessoas estão ouvindo e um botão de pausa. A capa do álbum vira o rótulo do disco.

## Ouvir junto pelo Spotify

O áudio do Spotify não pode passar pela chamada (viola os termos). Cada pessoa toca a música no próprio Spotify e o app só sincroniza: o dono da sala escolhe a faixa, o servidor avisa os outros clientes, e cada um chama a Web API do Spotify (crate [rspotify](https://github.com/ramsayleung/rspotify)) com a mesma faixa e posição.

| Ponto | Situação |
| --- | --- |
| Controlar a reprodução | Exige Spotify Premium de cada ouvinte e o app do Spotify aberto |
| Modo de desenvolvimento | Dono do app precisa de Premium; no máximo 5 usuários de teste (desde fevereiro de 2026) |
| Acesso amplo | Só para empresa registrada com 250 mil usuários ativos por mês (desde março de 2025) |

Fonte: [TechCrunch](https://techcrunch.com/?p=3090291).

## Plano

1. **Teste**: sincronização pelo Spotify para até 5 pessoas com Premium.
2. **Para mais gente**: o dono da sala toca um arquivo de áudio próprio, transmitido como faixa de áudio do LiveKit. Os direitos da música são de quem toca; serve para grupos privados.
3. **Só mostrar**: quem não tem Premium vê o vinil com a faixa do grupo, sem tocar.
