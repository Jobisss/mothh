# 0004. Criptografia ponta a ponta com MLS e SFrame

**Decisão:** MLS (RFC 9420) via OpenMLS para todas as conversas, inclusive 1:1 como grupo de dois. SFrame (RFC 9605) para a mídia, com chave derivada do grupo MLS.

**Por quê:** um protocolo só, feito para grupos, padrão IETF. O servidor e o SFU nunca leem o conteúdo.

**Fica de fora:** a economia, que precisa ser vista pelo servidor, e as imagens públicas do perfil.
