# Segurança e criptografia

Mensagens, anexos, áudio, vídeo e tela são cifrados ponta a ponta. O servidor e o LiveKit repassam bytes que não conseguem ler.

| Camada | Protocolo | Por quê |
| --- | --- | --- |
| Mensagens de grupo e 1:1 | MLS (RFC 9420) via OpenMLS | Feito para grupos; troca de chave cresce em log(n). Uma conversa 1:1 é um grupo de dois, então há um protocolo só |
| Mídia ao vivo | SFrame (RFC 9605), com chave derivada do grupo MLS | Cifra cada quadro antes de chegar ao SFU; suportado pelo LiveKit |
| Transporte | TLS 1.3 e DTLS-SRTP | Protege contra quem está na rede |
| Anexos e mensagens de áudio | Chave aleatória por arquivo, enviada dentro da mensagem MLS | O armazenamento só vê bytes cifrados |

## Pontos de atenção

- **Verificação de identidade**: código de segurança ou QR para conferir chaves; sem isso o servidor poderia trocar chaves no meio.
- **Vários dispositivos**: cada aparelho é um membro do grupo MLS, com chave no chaveiro do sistema operacional.
- **Moderação**: com E2EE o servidor não vê conteúdo; denúncias exigem que quem denuncia envie a mensagem decifrada.
- **Imagens públicas** (wallpaper e vitrines do perfil) não são E2EE, porque são públicas, e passam por moderação automática.
- **Economia**: o servidor vê transações de itens e saldo, porque precisa garantir posse. Sorteios usam gerador criptográfico e ficam registrados para auditoria.
- **Contas**: 2FA obrigatório para trocar e vender itens; trava de troca após compra.
