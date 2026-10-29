# Módulos e efeitos

O app é modular por padrão. Cada recurso além da conversa é um módulo que a pessoa liga e desliga em Configurações › Módulos. Desligar um módulo remove ele da interface inteira, sem deixar botão morto.

| Módulo | O que desliga |
| --- | --- |
| Sala 3D | O palco com personagens; a chamada vira só áudio e tiles de vídeo |
| Personagens | Personagens no chat e no perfil |
| Customizações de outros | Placas, molduras e murais de outras pessoas aparecem no padrão |
| Mercado | Mercado e "Ver no mercado" |
| Caixas | Caixas, chaves e a tela de abrir caixa |
| Música | Vinil e sincronização |
| Dither de fundo | Fundo animado; vira cor sólida |
| Efeitos sonoros | Por categoria, cada uma com volume |
| Efeitos visuais | Animações longas; "reduzir movimento" do sistema também desliga |
| Áudio espacial | Volume pela distância na sala |

## No código

- Cada módulo é uma crate e uma `feature` do Cargo, para compilar sem ele.
- Um registro de módulos no cliente liga e desliga em tempo de execução.
- O núcleo (conversas, grupos, chamadas) não depende de nenhum módulo.

## Sons e efeitos

Toda ação importante tem som e efeito visual: mensagem enviada, alguém entrou na sala, menção, item recebido, caixa abrindo (som próprio por raridade), venda concluída. Sons curtos (até 400ms, exceto a caixa) com cara de chip/8-bit, combinando com o pixel.
