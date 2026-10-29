# Visão do produto

O mothh é um app de conversa para grupos de amigos, nativo e rápido, onde cada pessoa tem um personagem 3D que ela veste com itens colecionáveis. A conversa é o centro; o personagem e os itens são a personalidade.

## Para quem

Grupos pequenos e médios de amigos que hoje se dividem entre WhatsApp (conversa), Discord (chamadas) e Steam (itens e perfil). O app também é um projeto de aprendizado de Rust, então decisões que ensinam a linguagem têm peso.

## Princípios

1. **Simples primeiro.** Uma lista única de conversas, pessoas e grupos juntos. Grupo não tem subcanais: tem um chat e uma sala.
2. **Nativo de verdade.** Interface desenhada direto na GPU, sem HTML, React ou webview por trás.
3. **Privado por padrão.** Mensagens, áudio, vídeo e tela com criptografia ponta a ponta. O servidor repassa dados que não consegue ler.
4. **O personagem é o protagonista.** A interface é preto e branco; a cor vem dos personagens e dos itens.
5. **Modular.** Cada recurso além da conversa é um módulo que a pessoa desliga. Sem nenhum módulo extra, o app continua sendo um chat completo.
6. **Feedback rico e opcional.** Muitos efeitos sonoros e visuais, cada categoria com seu próprio interruptor.

## O que o mothh não é

- Não é uma plataforma de servidores enormes com dezenas de canais.
- Não tem bots.
- Não é um navegador disfarçado: nada de Electron ou Tauri.

## Experiências principais

| Momento | Como é |
| --- | --- |
| Abrir o app | Lista de conversas com placas customizadas; grupos mostram "▪ 3 na sala" |
| Entrar na sala | Os personagens aparecem sentados; quem fala ganha destaque; o fundo é dither animado |
| Botão direito num personagem | Menu com "Inspecionar item", "Ver no mercado", "Ver perfil" |
| Inspecionar | Item girando em 3D, com raridade, float, padrão e dono |
| Mercado | Gráfico de mediana e volume, ofertas, pedidos de compra |
| Perfil | Cartão com mural, moldura e personagem; página completa com wallpaper e vitrines |
| Música | Vinil girando no canto da sala, ouvindo junto |
