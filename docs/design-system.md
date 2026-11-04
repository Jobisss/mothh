# Design system

Preto e branco, textura pixelada e um sotaque de terminal; a única cor da tela vem dos itens. Versão completa e navegável: <https://claude.ai/artifact/24nGVRtoUCj7o2WRBry5s8>. Telas de exemplo: <https://claude.ai/artifact/57Bj2NAo87xDgFrjPqpkeF>.

## Princípios

- **Preto e branco.** A interface usa só tons de cinza. Cor aparece nas fotos das pessoas, nos itens comemorativos e no vermelho de erro.
- **Terminal, mas meio termo.** Títulos, nomes e lista de conversas na fonte de terminal; mensagens longas em sans legível. Prefixo `>` nos rótulos, colchetes nos botões principais (`[ TROCAR FOTO ]`), cursor piscando no campo vazio.
- **Dither.** O fundo da marca é *ordered dithering* (matriz de Bayer 4×4), pixels de 3 a 6px, 2 ou 3 tons, animado devagar. Feito como shader wgpu. Nunca atrás de texto corrido.
- **Cantos quase retos.** 2 e 4px na interface, 8px em cards. Só a foto na conversa direta é redonda.
- **Modular e com feedback.** Muitos sons e efeitos, todos com interruptor.

## Cores (tema escuro, padrão)

| Token | Valor | Uso |
| --- | --- | --- |
| `surface-0` | `#0a0a0a` | Fundo e lista de conversas |
| `surface-1` | `#111111` | Conversa aberta |
| `surface-2` | `#1a1a1a` | Campos, cards, menus |
| `surface-3` | `#262626` | Hover e seleção |
| `border-strong` | `#6b6b6b` | Borda de controles (3:1) |
| `ink` | `#f2f2f2` | Texto e botão principal |
| `ink-muted` | `#9a9a9a` | Texto secundário |
| `danger` | `#ff5a4f` | Erro, sair da chamada |

Há tema claro com os mesmos tokens invertidos, em **Configurações → aparência → tema** (`apply_palette` em `main.rs`): `surface-0` #f4f4f4, `surface-1` #ebebeb, `surface-2` #dedede, `surface-3` #cfcfcf, `border` #d6d6d6, `border-strong` #7a7a7a, `ink` #111111, `ink-muted` #5c5c5c, `danger` #c8372c. O dither do fundo acompanha. A abertura (splash) e os vídeos da chamada continuam sobre preto nos dois temas.

## Tipografia

| Família | Fonte | Uso |
| --- | --- | --- |
| `pixel` | [VT323](https://fonts.google.com/specimen/VT323) | Títulos, botões, lista de conversas, nomes. Nunca abaixo de 20px |
| `sans` | [Geist](https://vercel.com/font) | Texto das mensagens |
| `mono` | Geist Mono | Metadados, tamanhos, horários |

Todas são OFL e vão embutidas no binário. Em **Configurações → fontes** a pessoa troca a `pixel` (Share Tech Mono, Silkscreen, DotGothic16) e a `sans` (Atkinson Hyperlegible, Fira Sans, Lato); VT323 e Geist continuam o padrão e a referência deste documento.

## Espaçamento, estados e transições

- **Escala de espaçamento:** 4, 8, 12, 16, 24 e 32px (`Theme.space-1` a `space-6` no `app.slint`). Recuo de painel 16, vão entre itens 8, entre blocos 12, entre seções 24.
- **Estados de controle**, sempre na mesma ordem: pressionado, com o mouse, normal. Botão principal: `ink` → `ink-soft` (#d9d9d9) → `ink-muted`. Secundário e linhas: transparente → `surface-2` → `surface-3`. Campo: borda `border-strong`, `ink-muted` com o mouse, `ink` com foco. Desabilitado: opacidade 0,5.
- **Transições:** 100ms (`Theme.quick`) para hover e pressionado; 150ms (`Theme.enter`) de fade para painéis entrando (configurações, perfil, cartão, membros, menus). Nada mais longo: o app responde, não encena.
- **Telas vazias** (`EmptyState`): a mariposa apagada, um título no sotaque de terminal (`> escolha uma conversa`) e uma dica em cinza. Nunca uma área preta sem nada.

## Layout

- Duas colunas: lista de conversas (320px) e conversa aberta. Sem trilho de servidores e sem painel lateral fixo.
- Conversa de grupo: a faixa da chamada (fotos de quem está nela) no topo; o chat embaixo.

## Ícones e logo

- Ícones: [Pixelarticons](https://pixelarticons.com) (MIT).
- Logo: a mariposa em dither em [`assets/logo-temporaria.png`](../assets/logo-temporaria.png) (o nome do arquivo ficou; a logo é definitiva desde 04/11/2026). Branca sobre preto, nunca recolorida, mínimo de 48px.
