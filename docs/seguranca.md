# Segurança e criptografia

Mensagens, anexos, áudio, vídeo e tela são cifrados ponta a ponta. O servidor e o LiveKit repassam bytes que não conseguem ler.

| Camada | Protocolo | Por quê |
| --- | --- | --- |
| Mensagens de grupo e 1:1 | MLS (RFC 9420) via OpenMLS | Feito para grupos; troca de chave cresce em log(n). Uma conversa 1:1 é um grupo de dois, então há um protocolo só |
| Mídia ao vivo | Cifra por quadro do LiveKit (AES-GCM, da família do SFrame, RFC 9605), com chave derivada do grupo MLS | Cifra cada quadro antes de chegar ao SFU |
| Transporte | TLS 1.3 e DTLS-SRTP | Protege contra quem está na rede |
| Anexos e mensagens de áudio | Chave aleatória por arquivo, enviada dentro da mensagem MLS | O armazenamento só vê bytes cifrados |

## Como está hoje (F2-07)

A crate `crates/crypto` embrulha o OpenMLS 0.9 e é a única que fala com ele. Nada de criptografia feita à mão: o MLS vem do OpenMLS, a cifra do arquivo de estado é ChaCha20-Poly1305 da RustCrypto e o cofre é o do sistema (crate `keyring`).

- **`Identity`** é a pessoa neste aparelho. A chave de assinatura (Ed25519) e a chave que cifra o arquivo de estado ficam no cofre do sistema: Credential Manager no Windows, Keychain no macOS, Secret Service no Linux. O estado dos grupos (árvores, segredos de época, pacotes de chaves ainda não usados) fica num arquivo cifrado na pasta de configuração, salvo por `Identity::save`.
- **`Group`** é um grupo MLS com a suíte obrigatória do RFC 9420 (X25519, AES-128-GCM, SHA-256, Ed25519). `add` recebe pacotes de chaves e devolve um *commit* para quem já está e um *welcome* para quem entra; `remove` troca as chaves e devolve o commit; `encrypt` e `receive` cuidam das mensagens. O servidor só repassa bytes.
- Os testes em `crates/crypto/tests/grupo.rs` cobrem entrar, sair, cifrar, decifrar, reabrir o app com o estado salvo e, principalmente, que quem foi removido não decifra mais nada.

## Mensagens de texto (F2-08)

O texto sai do app cifrado e a tabela `messages` só tem bytes (`sealed`). O servidor entrega pacotes de chaves, commits (em ordem de época) e welcomes, e diz em cada `GroupState` quem falta pôr ou tirar do grupo MLS e quem deve fazer isso. Do lado do app, tudo fica em `crates/client/src/e2ee.rs`. Detalhes e limites na [decisão 0011](decisoes/0011-mensagens-cifradas-com-mls.md). Testes: `crates/server/tests/mls.rs` (servidor com o MLS de verdade) e os do `e2ee.rs`.

## Mídia (F2-09)

Voz, câmera e tela saem cifradas do app (`crates/client/src/sframe.rs`). Quem cifra é o `FrameCryptor` do WebRTC, ligado pelo SDK do LiveKit (`RoomOptions::encryption`): AES-GCM em cada quadro, com o índice da chave no fim do quadro. É o esquema do LiveKit, da mesma ideia do SFrame, mas não o formato byte a byte da RFC 9605; trocar por SFrame "puro" exigiria escrever o transformador de quadros à mão, e o SDK em Rust não abre esse ponto.

- **Chave**: o segredo exportado do grupo MLS da conversa com o rótulo `mothh sframe` (32 bytes), passado por HKDF. Todos do grupo chegam ao mesmo segredo, então a sala usa uma chave só.
- **Troca de chave**: cada época MLS ocupa uma posição de um anel de 16 (`época % 16`). Quando o grupo muda, o app recebe a chave nova (`CallHandle::set_key`) e passa a cifrar com ela; quem recebe ainda guarda as anteriores, para os quadros que já estavam no caminho. Quem saiu do grupo não tem a chave nova e para de ouvir.
- **Sem chave, sem chamada**: a sala só abre com a cifra ligada; não existe chamada sem cifra.
- **Enquanto a F2-08 não entra**, a chave é fixa por conversa (`sframe::test_key`). O LiveKit já só vê quadros cifrados, mas quem tem o código chega à mesma chave: não é segredo até o MLS ligar.
- **Teste**: `livekit_so_ve_quadros_cifrados` (em `sframe.rs`, roda com um LiveKit de verdade) publica um tom de 440 Hz cifrado. Quem tem a chave ouve o tom; quem entra na sala sem cifra recebe os bytes como o LiveKit repassou e não ouve nada; quem tem a chave errada também não; e depois da troca de época quem só conhece a chave nova ouve.

## Pontos de atenção

- **Verificação de identidade**: código de segurança ou QR para conferir chaves; sem isso o servidor poderia trocar chaves no meio.
- **Vários dispositivos**: cada aparelho é um membro do grupo MLS, com chave no chaveiro do sistema operacional.
- **Moderação**: com E2EE o servidor não vê conteúdo; denúncias exigem que quem denuncia envie a mensagem decifrada.
- **Imagens públicas** (wallpaper e vitrines do perfil) não são E2EE, porque são públicas, e passam por moderação automática.
- **Economia**: o servidor vê transações de itens e saldo, porque precisa garantir posse. Sorteios usam gerador criptográfico e ficam registrados para auditoria.
- **Contas**: 2FA obrigatório para trocar e vender itens; trava de troca após compra.
