# Diagnóstico da chamada

## O que o app escreve no log

`%APPDATA%\mothh\<perfil>.log`, a cada 15 s durante a chamada:

- `som: ping X ms; fulano: K kbps, perda P%, jitter J ms` — a voz que chega de cada pessoa.
- `rede: subida udp/srflx rtt 40 ms 2400 kbps livres; descida ...; vídeo [h] 1920x1080 30 qps alvo 2500 kbps limite Bandwidth`
  — por onde a chamada passa (`udp` direto é o normal; `tcp` ou `relay` é o plano B,
  onde uma perda trava voz e vídeo juntos), quanto a subida aguenta e, para cada vídeo
  que sai, o tamanho, os quadros, a taxa alvo e o que está limitando (`Cpu` ou `Bandwidth`).

## Teste da voz com a tela ligada

`call::voz_com_tela::voz_continua_chegando` (ignorado): quem mostra manda um tom como
microfone e, com `MOTHH_TESTE_TELA=1`, a tela 1080p; quem assiste conta os pedaços de
100 ms sem voz e o log mostra perda, jitter e o estado do envio.

```sh
docker run -d --name lk --network host livekit/livekit-server:v1.13.11 --dev --bind 127.0.0.1 --node-ip 127.0.0.1
# Subida apertada em 2 Mbit/s só para a mídia (porta UDP 7882):
tc qdisc add dev lo root handle 1: htb default 10
tc class add dev lo parent 1: classid 1:10 htb rate 10gbit
tc class add dev lo parent 1: classid 1:20 htb rate 2mbit ceil 2mbit
tc qdisc add dev lo parent 1:20 handle 20: pfifo limit 100
tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip dport 7882 0xffff flowid 1:20
MOTHH_LIVEKIT_URL=ws://127.0.0.1:7880 MOTHH_LIVEKIT_KEY=devkey MOTHH_LIVEKIT_SECRET=secret \
MOTHH_TESTE_TELA=1 MOTHH_TESTE_SEGUNDOS=40 \
cargo test -p client voz_continua_chegando -- --ignored --nocapture
```

Medido em 09/11/2026 (tela de ruído puro, o pior caso): sem limite, voz perfeita com e
sem tela. Com 2 Mbit/s de subida, sem tela a voz segue perfeita; com a tela, a voz ainda
chega, mas com ~5% de pacotes perdidos e 300–400 ms de atraso, porque o vídeo enche a
fila do roteador.

## "Uma pessoa para de ouvir a outra" (09/11/2026)

A chave da mídia é a da época do grupo MLS da conversa (`sframe.rs`). Se um aparelho fica
numa época velha, ele cifra e decifra com a chave velha: os outros não o ouvem e ele não
ouve os outros, sem erro nenhum na tela.

Causa achada no código: o pedido dos commits que faltam (`FetchCommits`) ficava marcado
como "esperando resposta" para sempre se a resposta se perdesse (conexão caiu no meio, ou o
servidor respondeu com erro). Daí em diante o aparelho não pedia mais nada e ficava na
época velha até fechar o app. Correção: o pedido vale por 10 s, é zerado quando a conexão
volta e, durante a chamada, o app confere a época com o servidor a cada 15 s. O log mostra
`chamada: entrando com a chave da época N` e `chamada: chave nova, época N`; duas pessoas
na mesma chamada com épocas diferentes por mais de alguns segundos é este problema.
