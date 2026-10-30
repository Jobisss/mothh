# Infraestrutura do mothh

## Publicar o chat na VPS

O endereço escolhido para o chat é `mothh.joaoschianti.com`. Crie um registro **A** desse subdomínio apontando para o IP público da VPS. Se a VPS também tiver IPv6, só crie o registro **AAAA** quando ele estiver acessível. Aguarde o DNS resolver antes de iniciar o Caddy.

O arquivo `docker-compose.prod.yml` sobe Caddy, servidor e PostgreSQL. Ele publica as portas TCP 80 e 443 da VPS; **confira antes se outro serviço já usa essas portas**. Se já houver Caddy ou outro proxy atendendo os demais subdomínios, integre a rota `mothh.joaoschianti.com → server:3000` nesse proxy em vez de iniciar um segundo proxy nas mesmas portas. O banco e o servidor não publicam portas diretamente na internet.

Na VPS com Docker e o plugin Compose instalados:

```sh
git clone https://github.com/Jobisss/mothh.git
cd mothh
cp .env.prod.example .env.prod
chmod 600 .env.prod
# edite .env.prod e troque a senha nos dois campos
docker compose -f docker-compose.prod.yml up -d --build
docker compose -f docker-compose.prod.yml ps
curl -fsS https://mothh.joaoschianti.com/health
```

Se a senha tiver caracteres especiais, codifique-os na parte da senha de `DATABASE_URL` como URL. `.env.prod` contém credenciais e é ignorado pelo Git; guarde um backup seguro fora do repositório. O Caddy salva certificados em `caddy-data`, o PostgreSQL em `mothh-pg-prod` e os anexos em `mothh-anexos`. Não use `docker compose down -v` numa instalação que já tenha dados.

Para atualizar após um push na `main`:

```sh
git pull --ff-only
docker compose -f docker-compose.prod.yml up -d --build
docker compose -f docker-compose.prod.yml logs --tail=100 server caddy
```

O instalador Windows gerado na CI já usa `wss://mothh.joaoschianti.com/ws`; os anexos usam `https://mothh.joaoschianti.com`. Para testar a entrega completa, abra dois apps em redes diferentes, entre com duas contas, envie texto e imagem e confirme o recebimento.

## Capacidade planejada

O MVP foi dimensionado inicialmente para uma VPS KVM 1 com 1 vCPU, 4 GB RAM, 50 GB NVMe e 4 TB de tráfego por mês. A Fase 1 usa apenas Caddy, servidor e PostgreSQL. Redis e LiveKit entram em tarefas futuras.

Quando chamadas de voz e vídeo entrarem, o SFU enviará uma cópia de cada fluxo para cada participante. Os números abaixo são estimativas para planejamento, não medições do servidor atual.

| Cenário | Taxa por fluxo | Upload estimado | Tráfego por hora |
| --- | --- | --- | --- |
| 5 pessoas só voz | ~40 kbps (Opus) | ~1 Mbps | ~0,4 GB |
| 10 pessoas só voz | ~40 kbps | ~4 Mbps | ~1,6 GB |
| 5 pessoas com câmera em 360p | ~0,5 Mbps | ~11 Mbps | ~5 GB |
| 5 pessoas com câmera em 720p | ~1,5 Mbps | ~31 Mbps | ~14 GB |

## Desenvolvimento local

Use `docker compose up -d` para iniciar apenas o PostgreSQL em `127.0.0.1:5440`; depois execute `cargo run -p server` e `cargo run -p client`. O app compilado localmente usa `ws://127.0.0.1:3000/ws` por padrão. Veja o [README](../README.md) para perfis e variáveis de ambiente.
