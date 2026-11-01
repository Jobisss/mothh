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

Há tema claro com os mesmos tokens invertidos (ver o design system completo).

## Tipografia

| Família | Fonte | Uso |
| --- | --- | --- |
| `pixel` | [VT323](https://fonts.google.com/specimen/VT323) | Títulos, botões, lista de conversas, nomes. Nunca abaixo de 20px |
| `sans` | [Geist](https://vercel.com/font) | Texto das mensagens |
| `mono` | Geist Mono | Metadados, tamanhos, horários |

Todas são OFL e vão embutidas no binário.

## Layout

- Duas colunas: lista de conversas (320px) e conversa aberta. Sem trilho de servidores e sem painel lateral fixo.
- Conversa de grupo: a faixa da chamada (fotos de quem está nela) no topo; o chat embaixo.

## Ícones e logo

- Ícones: [Pixelarticons](https://pixelarticons.com) (MIT).
- Logo temporária: a mariposa em dither em [`assets/logo-temporaria.png`](../assets/logo-temporaria.png). Branca sobre preto, nunca recolorida, mínimo de 48px.
