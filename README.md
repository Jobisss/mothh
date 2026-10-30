<p align="center">
  <img src="assets/logo-temporaria.png" alt="Mariposa em pixel, logo temporária do mothh" width="160">
</p>

# mothh

App de conversa nativo em Rust, sem webview: chat, voz, vídeo e tela com criptografia ponta a ponta, personagens 3D customizáveis em salas e um mercado de itens cosméticos com raridade e float. Visual preto e branco com dither.

> **Status:** fase 1 começou. Já existe um chat de texto funcionando (servidor + app desktop) com contas de verdade (usuário e senha) e as mensagens guardadas no PostgreSQL.

## O que é

O mothh junta três coisas:

1. **Conversa simples**, como no WhatsApp: uma lista única de pessoas e grupos, sem servidores com dezenas de canais. Mensagens de texto e de áudio, chamadas de voz, câmera e compartilhamento de tela.
2. **Personagens 3D** no estilo art toy. Cada grupo tem uma sala onde os personagens aparecem sentados durante a chamada, com destaque em quem está falando.
3. **Itens e mercado**: roupas e cosméticos com raridade, float e padrão, caixas com chaves e um mercado entre usuários com preço livre, no estilo do mercado da Steam.

Tudo é modular: cada recurso além da conversa (sala 3D, mercado, caixas, música, efeitos) pode ser desligado pela pessoa.

## Documentação

| Documento | Conteúdo |
| --- | --- |
| [Visão do produto](docs/visao.md) | Para quem é, princípios e o que o app não é |
| [Arquitetura](docs/arquitetura.md) | Stack, serviços, organização do código |
| [Segurança e criptografia](docs/seguranca.md) | E2EE com MLS e SFrame, modelo de ameaças |
| [Infraestrutura e custo](docs/infra.md) | Como rodar o MVP numa VPS pequena |
| [Roadmap](docs/roadmap.md) | Fases, entregas e critério de pronto |
| [TODO](TODO.md) | Lista de tarefas, na ordem |
| [Como entregar cada tarefa](docs/tarefas.md) | O que cada tarefa entrega e quando está pronta |
| [Design system](docs/design-system.md) | Cores, fontes, dither, padrões de tela |
| [Produto](docs/produto/) | Personagem, salas, itens e mercado, perfil, música, módulos |
| [Pontos legais](docs/legal.md) | Loot boxes, pagamentos, dados pessoais (tema adiado) |
| [Decisões](docs/decisoes/) | Registro das decisões já tomadas e o porquê |

## Como rodar

Precisa de Rust 1.92+ (edition 2024) e Docker (para o PostgreSQL). No Linux, o app usa as bibliotecas de janela do sistema; no Debian/Ubuntu: `sudo apt install libxkbcommon-x11-0 libfontconfig1-dev`.

```sh
# uma vez: sobe o PostgreSQL 16 em 127.0.0.1:5440
docker compose up -d

# terminal 1: servidor (escuta em 127.0.0.1:3000, mude com MOTHH_ENDERECO)
cargo run -p server

# terminal 2 e 3: um app para cada pessoa
MOTHH_PERFIL=jobiss cargo run -p client
MOTHH_PERFIL=nina   cargo run -p client
```

No PowerShell (Windows), defina a variável antes: `$env:MOTHH_PERFIL="nina"; cargo run -p client`.

Na primeira vez, clique em **criar conta**. Usuário: 3 a 20 caracteres, só `a-z`, `0-9`, `_` e `.` (maiúsculas viram minúsculas). Senha: 8 a 128 caracteres. O app guarda a sessão e entra sozinho nas próximas vezes; **sair**, no rodapé, encerra a sessão.

Para conversar só com uma pessoa, digite o usuário dela no campo acima da lista de conversas e aperte Enter. A conversa direta aparece na lista dos dois com o nome da outra pessoa, e só existe uma por par.

Para mandar um áudio, segure o botão **mic** ao lado do campo de mensagem enquanto fala e solte para enviar (arrastar para fora antes de soltar cancela). O app usa o microfone e o alto-falante padrão do sistema; o arquivo (Opus) sobe para o servidor por HTTP em `/anexos` e a conversa mostra um player com a duração. O servidor guarda os arquivos na pasta `anexos/`, a partir de onde ele foi aberto; mude com `MOTHH_ANEXOS=caminho`. No Linux, compilar o app precisa do pacote `libasound2-dev` (ALSA).

Para enviar uma imagem, PDF ou outro arquivo, abra uma conversa e clique em **+** ao lado do campo de mensagem. Imagens PNG, JPEG, GIF e WebP aparecem em miniatura; clique no arquivo ou na miniatura para escolher onde salvar. O limite é **10 MB por arquivo**, aplicado no servidor. Os anexos ficam na mesma pasta dos áudios.

`MOTHH_PERFIL` (padrão: `padrao`) escolhe onde a sessão fica salva, assim dá para ter duas contas abertas na mesma máquina. Os arquivos ficam na pasta de configuração do sistema: `%APPDATA%\mothh\` no Windows, `~/.config/mothh/` no Linux.

Para apontar o app para outro servidor: `MOTHH_SERVIDOR=ws://ip:3000/ws`. Se o servidor cair, o app tenta de novo sozinho (espera 1s, 2s, 4s… até 30s entre as tentativas) e volta para a mesma conversa; o rodapé mostra "reconectando..." enquanto isso. Testes: `cargo test --workspace` (com o PostgreSQL ligado; cada teste ganha um banco próprio e descartável).

O endereço do banco vem de `DATABASE_URL`. Localmente ele já está em `.cargo/config.toml`; uma variável definida no terminal tem prioridade. O servidor aplica as migrações de `crates/server/migrations/` sozinho ao iniciar.

O código está em três crates:

| Crate | O que faz |
| --- | --- |
| `crates/protocol` | Mensagens trocadas entre app e servidor (JSON pelo WebSocket) |
| `crates/server` | Servidor axum: guarda conversas e mensagens no PostgreSQL (sqlx) e repassa as novas ao vivo |
| `crates/client` | App desktop em Slint (`ui/app.slint`) com a rede numa thread Tokio |

## Stack em uma linha

Slint (interface) · Bevy sobre wgpu (sala 3D) · Tokio + axum (servidor) · PostgreSQL + Redis · LiveKit (mídia) · OpenMLS + SFrame (criptografia).

## Material visual

- Telas de exemplo (conversas, inspecionar item, mercado, perfil, página de perfil): <https://claude.ai/artifact/57Bj2NAo87xDgFrjPqpkeF>
- Design system (tokens, fontes, referências, logo): <https://claude.ai/artifact/24nGVRtoUCj7o2WRBry5s8>
- Plano de arquitetura original: <https://claude.ai/artifact/K9x2dVe4gEe42WFDhg1jfW>

Esses links são privados por enquanto; quem não for o dono precisa receber acesso.

## Licença

Ainda não definida.
