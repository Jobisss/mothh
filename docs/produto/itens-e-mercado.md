# Itens e mercado

O servidor é a única fonte da verdade sobre itens e saldo. Cada item é uma instância única gerada no servidor, com sorteio feito por gerador criptográfico e registrado para auditoria.

## Modelo de item

| Campo | Exemplo | Uso |
| --- | --- | --- |
| `item_def_id` | Moletom Home Love | Definição: arte, slot, coleção |
| `rarity` | comum, incomum, raro, épico, lendário | Probabilidade na caixa e cor do item |
| `float` | 0.071234 (0 a 1) | Desgaste visual; muda a textura e o preço |
| `pattern_seed` | 0 a 999 | Variação do padrão |
| `serial` | 00042 / 5000 | Itens numerados e limitados |
| `owner_id`, `tradable_after` | 7 dias | Trava de troca contra fraude |

Faixas de desgaste: Nova de fábrica (até 0.07), Pouco usada (até 0.15), Testada em campo (até 0.38), Bem desgastada (até 0.45), Veterana de guerra (até 1). No 3D, `float` e `pattern_seed` entram no shader do item como máscara de desgaste e deslocamento do padrão.

## Tipos de cosmético

| Tipo | Onde aparece |
| --- | --- |
| Roupas, acessórios e patches | Personagem na sala, inspetor, perfil |
| Placa | A linha da pessoa na lista de conversas |
| Moldura | Em volta da foto |
| Mural | Fundo do cartão de perfil |
| Wallpaper | Fundo da página de perfil |
| Sala | Cenário da sala do grupo |

## Caixas e chaves

A caixa cai de graça usando o app; a chave é vendida. Abrir consome caixa e chave e sorteia raridade, depois float e padrão. As chances de cada raridade ficam visíveis na tela antes de abrir. Animação de roleta de 4 a 6 segundos, com som por raridade.

## Mercado

- **Livro de ofertas** por definição de item: ordens de compra e venda, casadas pelo servidor numa transação atômica do PostgreSQL.
- **Preço livre**: quem vende escolhe o preço. A página do item mostra a mediana diária e o volume em 30 dias, as ofertas (vendedor, float, desgaste, preço) e os pedidos de compra.
- **Taxa** sobre cada venda.
- **Saldo interno**, sem saque em dinheiro.

## Inspecionar

Botão direito num personagem ou item → "Inspecionar item": tela cheia sobre dither, o item gira em 3D (arrastar para girar, rolar para zoom). Ao lado: raridade, desgaste, barra de float, padrão, série, dono e mediana no mercado, com "Ver no mercado" e "Fazer oferta".
