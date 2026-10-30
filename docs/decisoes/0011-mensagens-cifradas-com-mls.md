# 0011. Mensagens de texto cifradas com MLS, servidor só entrega

**Decisão:** o texto sai do app já cifrado com o grupo MLS da conversa (F2-08). O servidor guarda só os bytes cifrados (`messages.sealed`; a coluna `text` deixou de existir) e funciona como "serviço de entrega" do MLS: guarda pacotes de chaves, ordena os commits por época e entrega os welcomes. Quem está numa conversa continua sendo decidido pelo servidor; a diferença entre isso e o grupo MLS vai no `GroupState`, e um membro online escolhido pelo servidor (`committer`) faz a mudança.

**Como funciona:**
- Cada aparelho publica 20 pacotes de chaves e repõe quando ficam menos de 10.
- Cada conversa é um grupo MLS com o id da conversa. Se ainda não existe, o `committer` cria e põe os outros. Gente nova, gente que saiu e quem perdeu o estado (app reinstalado, `RejoinGroup`) entram na mesma regra.
- O servidor só aceita um commit feito na época atual dele. Dois ao mesmo tempo: o segundo recebe `CommitRejected`, descarta o seu e aplica o do primeiro. Por isso o app só troca as chaves depois do `CommitAccepted`.
- Quem ficou offline pede os commits que perdeu (`FetchCommits`). Mensagens de uma época futura esperam o commit e abrem depois.
- O MLS não decifra a mesma mensagem duas vezes nem a própria. O app guarda o texto já lido num arquivo cifrado com a chave do aparelho (`<perfil>.<usuário>.textos`), e o histórico do servidor é preenchido a partir dele.
- A chave de assinatura fica no cofre do sistema. Sem cofre (Linux sem Secret Service), cai num arquivo `<perfil>.cofre.json` e avisa no log.
- A chamada (F2-09) pega a chave do SFrame da época atual com `E2ee::media_key`, que exporta `"mothh sframe"`, 32 bytes.

**O que muda para quem usa:**
- As mensagens de texto antigas (em claro) foram apagadas pela migração `0007_mls.sql`: o servidor não tinha como cifrá-las. Áudios e arquivos ficaram.
- Quem entra numa conversa não lê o que foi dito antes de entrar. Uma instalação nova do app não lê as mensagens da instalação antiga. Essas aparecem como "[cifrada: este aparelho não tem a chave desta mensagem]".
- Mensagem escrita enquanto o grupo troca chaves espera e sai sozinha ("trocando chaves..." no rodapé).

**Fica de fora (por enquanto):** áudios e arquivos continuam sem cifra ponta a ponta (chave por arquivo dentro da mensagem MLS, próximo passo); conferir a identidade do outro (código de segurança); vários aparelhos na mesma conta (hoje cada conta tem um aparelho só; abrir a mesma conta em outro computador ao mesmo tempo deixa um dos dois sem conseguir ler); o servidor ainda vê quem fala com quem e quando.
