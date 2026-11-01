# Módulos e efeitos

O app é modular por padrão. Cada recurso além da conversa é um módulo que a pessoa liga e desliga em Configurações › Módulos. Desligar um módulo remove ele da interface inteira, sem deixar botão morto.

| Módulo | O que desliga |
| --- | --- |
| Inventário | Itens comemorativos no perfil e no inventário (fase 3) |
| Música | Vinil e sincronização |
| Dither de fundo | Fundo animado; vira cor sólida |
| Efeitos sonoros | Por categoria, cada uma com volume |
| Efeitos visuais | Animações longas; "reduzir movimento" do sistema também desliga |

## No código

- Cada módulo é uma crate e uma `feature` do Cargo, para compilar sem ele.
- Um registro de módulos no cliente liga e desliga em tempo de execução.
- O núcleo (conversas, grupos, chamadas) não depende de nenhum módulo.

## Sons e efeitos

Toda ação importante tem som e efeito visual: mensagem enviada, alguém entrou na chamada, menção, item comemorativo recebido. Sons curtos (até 400ms) com cara de chip/8-bit, combinando com o pixel.
