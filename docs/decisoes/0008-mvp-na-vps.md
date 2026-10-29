# 0008. MVP numa VPS KVM própria

**Decisão:** tudo do MVP roda numa VPS KVM já existente (1 vCPU, 4 GB de RAM, 50 GB NVMe, 4 TB de tráfego por mês) com Docker Compose.

**Por quê:** custo zero extra. Os 4 TB dão cerca de 280 horas por mês de chamada com 5 pessoas em 720p; a CPU única é o primeiro limite.

**Plano B:** mídia num PC de casa (atenção ao CGNAT) ou num plano gratuito de nuvem.
