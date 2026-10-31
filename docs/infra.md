# Infraestrutura do mothh

## Publicar o chat na VPS

O endereço escolhido para o chat é `mothh.joaoschianti.com`. Crie um registro **A** desse subdomínio apontando para o IP público da VPS, e outro registro **A** para `livekit.mothh.joaoschianti.com` (chamadas) apontando para o mesmo IP. Se a VPS também tiver IPv6, só crie o registro **AAAA** quando ele estiver acessível. Aguarde o DNS resolver antes de iniciar o Caddy.

O arquivo `docker-compose.prod.yml` sobe servidor, PostgreSQL e LiveKit. Na VPS atual o nginx do sistema atende 80/443 e repassa `mothh.joaoschianti.com` para `127.0.0.1:3006` (servidor) e `livekit.mothh.joaoschianti.com` para `127.0.0.1:3007` (LiveKit); essas duas portas só escutam em `127.0.0.1`. Numa VPS sem proxy, suba também o Caddy com `--profile caddy`: ele publica as portas TCP 80 e 443 da VPS, além das portas de mídia do LiveKit (veja [Chamadas com LiveKit](#chamadas-com-livekit)); **confira antes se outro serviço já usa essas portas**. Se já houver Caddy ou outro proxy atendendo os demais subdomínios, integre a rota `mothh.joaoschianti.com → server:3000` nesse proxy em vez de iniciar um segundo proxy nas mesmas portas. O banco e o servidor não publicam portas diretamente na internet.

Na VPS com Docker e o plugin Compose instalados:

```sh
git clone https://github.com/Jobisss/mothh.git
cd mothh
cp .env.prod.example .env.prod
chmod 600 .env.prod
# edite .env.prod: troque a senha nos dois campos e gere o par do LiveKit
docker compose -f docker-compose.prod.yml up -d --build
docker compose -f docker-compose.prod.yml ps
curl -fsS https://mothh.joaoschianti.com/health
curl -fsS https://livekit.mothh.joaoschianti.com   # responde OK
```

Se a senha tiver caracteres especiais, codifique-os na parte da senha de `DATABASE_URL` como URL. `.env.prod` contém credenciais e é ignorado pelo Git; guarde um backup seguro fora do repositório. O Caddy salva certificados em `caddy-data`, o PostgreSQL em `mothh-pg-prod` e os anexos em `mothh-anexos`. Não use `docker compose down -v` numa instalação que já tenha dados.

Para atualizar após um push na `main`:

```sh
git pull --ff-only
docker compose -f docker-compose.prod.yml up -d --build
docker compose -f docker-compose.prod.yml logs --tail=100 server caddy livekit
```

O instalador Windows gerado na CI já usa `wss://mothh.joaoschianti.com/ws`; os anexos usam `https://mothh.joaoschianti.com`. Para testar a entrega completa, abra dois apps em redes diferentes, entre com duas contas, envie texto e imagem e confirme o recebimento.

## Chamadas com LiveKit

O LiveKit é o servidor das chamadas de voz, vídeo e tela, com TURN embutido para redes que bloqueiam a mídia direta. A configuração fica em `infra/livekit/` (`livekit.dev.yaml` e `livekit.prod.yaml`). A versão da imagem é fixa nos dois compose; para atualizar, troque a tag nos dois arquivos.

| Porta | Protocolo | Para quê | Aberta na internet (VPS) |
| --- | --- | --- | --- |
| 7880 | TCP | Sinalização (WebSocket) e saúde: `GET /` responde `OK` | Não: o Caddy atende `wss://livekit.mothh.joaoschianti.com` e repassa |
| 7881 | TCP | Mídia por TCP, quando o UDP está bloqueado | Sim |
| 7882 | UDP | Toda a mídia WebRTC, multiplexada numa porta só | Sim |
| 3478 | UDP | TURN embutido | Sim |

Usar uma única porta UDP (`rtc.udp_port`) evita mapear uma faixa de milhares de portas no Docker, que deixa o contêiner lento para subir. Se a VPS tiver firewall, libere as portas acima, por exemplo com `ufw`:

```sh
sudo ufw allow 7881/tcp
sudo ufw allow 7882/udp
sudo ufw allow 3478/udp
```

Confira também o firewall do painel do provedor, se houver. Na VPS, o LiveKit descobre o IP público por STUN (`use_external_ip: true`) e o anuncia aos apps; se ele não subir e o log disser `could not resolve external IP`, a VPS está sem DNS ou sem UDP de saída.

O par de chaves fica no `.env.prod` (`MOTHH_LIVEKIT_KEY` e `MOTHH_LIVEKIT_SECRET`): o LiveKit usa o par para validar tokens e o servidor usa o mesmo par para gerá-los. Gere um par novo com `docker run --rm livekit/livekit-server:v1.13.11 generate-keys`; o segredo precisa ter pelo menos 32 caracteres. `MOTHH_LIVEKIT_URL` é o endereço que os apps usam para entrar na chamada.

TURN sobre TLS (porta 443/5349, para redes que só deixam passar HTTPS) ainda não está ligado: o Caddy já ocupa a 443 e exigiria um proxy de camada 4. Fica para quando alguém precisar.

## Capacidade planejada

O MVP foi dimensionado inicialmente para uma VPS KVM 1 com 1 vCPU, 4 GB RAM, 50 GB NVMe e 4 TB de tráfego por mês. A Fase 1 usa Caddy, servidor e PostgreSQL; a Fase 2 adiciona o LiveKit. Redis entra numa tarefa futura.

Quando chamadas de voz e vídeo entrarem, o SFU enviará uma cópia de cada fluxo para cada participante. Os números abaixo são estimativas para planejamento, não medições do servidor atual.

| Cenário | Taxa por fluxo | Upload estimado | Tráfego por hora |
| --- | --- | --- | --- |
| 5 pessoas só voz | ~40 kbps (Opus) | ~1 Mbps | ~0,4 GB |
| 10 pessoas só voz | ~40 kbps | ~4 Mbps | ~1,6 GB |
| 5 pessoas com câmera em 360p | ~0,5 Mbps | ~11 Mbps | ~5 GB |
| 5 pessoas com câmera em 720p | ~1,5 Mbps | ~31 Mbps | ~14 GB |
| 1 tela compartilhada em 1080p para 4 pessoas | ~1,5 Mbps | ~6 Mbps | ~2,7 GB |

## Desenvolvimento local

Use `docker compose up -d` para iniciar o PostgreSQL em `127.0.0.1:5440` e o LiveKit em `ws://127.0.0.1:7880` (`curl http://127.0.0.1:7880` responde `OK`; par de desenvolvimento: `devkey` / `mothh-livekit-dev-secret-nao-usar-em-prod`); depois execute `cargo run -p server` e `cargo run -p client`. O app compilado localmente usa `ws://127.0.0.1:3000/ws` por padrão. Veja o [README](../README.md) para perfis e variáveis de ambiente.

Por padrão o LiveKit de desenvolvimento anuncia `127.0.0.1`, então a chamada só funciona entre apps na mesma máquina. Para testar com outro computador da rede, suba com o IP da máquina: `MOTHH_LIVEKIT_IP=192.168.0.10 docker compose up -d` (no PowerShell: `$env:MOTHH_LIVEKIT_IP="192.168.0.10"; docker compose up -d`) e libere as portas 7880 e 7881/TCP, 7882/UDP e 3478/UDP no firewall do sistema. Nesse modo o LiveKit escuta só nesse IP, então o servidor também precisa de `MOTHH_LIVEKIT_URL=ws://192.168.0.10:7880`.
