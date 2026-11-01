# Visão do produto

O mothh é um app de conversa para grupos de amigos, nativo e rápido. A conversa é o centro; o perfil, com foto e itens comemorativos em 3D, é a personalidade.

## Para quem

Grupos pequenos e médios de amigos que hoje se dividem entre WhatsApp (conversa) e Discord (chamadas). O app também é um projeto de aprendizado de Rust, então decisões que ensinam a linguagem têm peso.

## Princípios

1. **Simples primeiro.** Uma lista única de conversas, pessoas e grupos juntos. Grupo não tem subcanais: tem um chat e uma chamada.
2. **Nativo de verdade.** Interface desenhada direto na GPU, sem HTML, React ou webview por trás.
3. **Privado por padrão.** Mensagens, áudio, vídeo e tela com criptografia ponta a ponta. O servidor repassa dados que não consegue ler.
4. **Preto e branco.** A interface é em tons de cinza; a cor vem das fotos das pessoas e dos itens comemorativos.
5. **Modular.** Cada recurso além da conversa é um módulo que a pessoa desliga. Sem nenhum módulo extra, o app continua sendo um chat completo.
6. **Feedback rico e opcional.** Muitos efeitos sonoros e visuais, cada categoria com seu próprio interruptor.

## O que o mothh não é

- Não é uma plataforma de servidores enormes com dezenas de canais.
- Não tem bots.
- Não é um navegador disfarçado: nada de Electron ou Tauri.

## Experiências principais

| Momento | Como é |
| --- | --- |
| Abrir o app | A mariposa procura atualização; depois, a lista de conversas com as fotos de cada um e quem está em chamada |
| Chamada | Fotos de quem está nela, com a borda crescendo no ritmo da voz de quem fala |
| Perfil | Foto, nome e, na fase 4, os itens comemorativos que a pessoa escolheu mostrar |
| Item comemorativo | O item girando em 3D, com a data ou o momento que ele marca |
| Música | Vinil girando no topo da conversa do grupo, ouvindo junto |
