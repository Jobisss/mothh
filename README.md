<p align="center">
  <img src="assets/logo-temporaria.png" alt="Mariposa em pixel, logo temporária do mothh" width="160">
</p>

# mothh

App de conversa nativo em Rust, sem webview: chat, voz, vídeo e tela com criptografia ponta a ponta, personagens 3D customizáveis em salas e um mercado de itens cosméticos com raridade e float. Visual preto e branco com dither.

> **Status:** fase 1 começou. Já existe um chat de texto funcionando (servidor + app desktop), ainda sem banco de dados e sem login.

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
| [Design system](docs/design-system.md) | Cores, fontes, dither, padrões de tela |
| [Produto](docs/produto/) | Personagem, salas, itens e mercado, perfil, música, módulos |
| [Pontos legais](docs/legal.md) | Loot boxes, pagamentos, dados pessoais (tema adiado) |
| [Decisões](docs/decisoes/) | Registro das decisões já tomadas e o porquê |

## Como rodar

Precisa de Rust 1.85+ (edition 2024). No Linux, o app usa as bibliotecas de janela do sistema; no Debian/Ubuntu: `sudo apt install libxkbcommon-x11-0 libfontconfig1-dev`.

```sh
# terminal 1: servidor (escuta em 127.0.0.1:3000, mude com MOTHH_ENDERECO)
cargo run -p server

# terminal 2 e 3: um app para cada pessoa
MOTHH_NOME=Jobiss cargo run -p client
MOTHH_NOME=Nina   cargo run -p client
```

Para apontar o app para outro servidor: `MOTHH_SERVIDOR=ws://ip:3000/ws`. Testes: `cargo test --workspace`.

O código está em três crates:

| Crate | O que faz |
| --- | --- |
| `crates/protocol` | Mensagens trocadas entre app e servidor (JSON pelo WebSocket) |
| `crates/server` | Servidor axum: guarda as conversas em memória e repassa mensagens |
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
