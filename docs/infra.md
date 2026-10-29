# Infraestrutura e custo

O MVP roda inteiro numa VPS pequena que já existe, sem custo extra.

## Máquina do MVP

| Recurso | Valor |
| --- | --- |
| Plano | VPS KVM 1 |
| CPU | 1 vCPU |
| RAM | 4 GB |
| Disco | 50 GB NVMe |
| Tráfego | 4 TB por mês |

Tudo sobe com Docker Compose: Caddy (HTTPS automático), servidor axum, PostgreSQL, Redis e LiveKit com TURN embutido. Por ser KVM, as portas UDP do WebRTC ficam sob controle e não há CGNAT.

## Quanto aguenta

Um SFU envia uma cópia de cada fluxo para cada pessoa, então o upload do servidor ≈ pessoas × (pessoas − 1) × taxa por fluxo.

| Cenário | Taxa por fluxo | Upload do servidor | Tráfego por hora |
| --- | --- | --- | --- |
| 5 pessoas só voz | ~40 kbps (Opus) | ~1 Mbps | ~0,4 GB |
| 10 pessoas só voz | ~40 kbps | ~4 Mbps | ~1,6 GB |
| 5 pessoas com câmera em 360p | ~0,5 Mbps | ~11 Mbps | ~5 GB |
| 5 pessoas com câmera em 720p | ~1,5 Mbps | ~31 Mbps | ~14 GB |

Os 4 TB dão cerca de 280 horas por mês de chamada com 5 pessoas em 720p. O primeiro limite é a CPU única: subir de plano quando o uso passar de 70% durante as chamadas.

## Alternativas se precisar

- Rodar o LiveKit num PC de casa (exige IP público; atenção ao CGNAT das operadoras) e deixar o resto na VPS.
- Oracle Cloud Always Free ou o plano gratuito do LiveKit Cloud só para mídia. Conferir os limites atuais antes.

## Desenvolvimento

Tudo roda em localhost com o mesmo `docker-compose.yml`. Domínio próprio: a definir.
