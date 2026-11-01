# Peças no Tripo: especificação

Como gerar roupas e acessórios no Tripo para o boneco do mothh e passar pelo `encaixar_peca.py` sem retrabalho. O contrato (slots, regiões, orçamento) está em [personagem-modular.md](personagem-modular.md); aqui fica o passo a passo da geração.

O problema que esta especificação resolve: o Tripo faz roupa para gente de proporção normal. O nosso boneco tem tronco curto e largo, cabeça de 30% da altura e pés enormes. Uma camiseta "normal" escalada para ele fica estreita ou comprida demais, e a validação reprova. Por isso a geração parte sempre do **molde** do nosso corpo.

## 1. O molde

Em [`molde/`](molde/), gerado por `tools/avatar/blender/gerar_molde.py` a partir do `base.blend`:

| Arquivo | Para quê |
| --- | --- |
| `frente.png`, `lado.png`, `costas.png` | O corpo sozinho, fundo transparente, 1 px = 1 mm. É a base para desenhar ou gerar a roupa por cima. |
| `molde.png` | As três vistas com as linhas de altura: pescoço 0,71, ombro 0,66, cotovelo 0,58, cintura 0,51, pulso 0,50, virilha 0,46, joelho 0,29, tornozelo 0,09. |
| `regioes.png` | As 10 regiões do corpo em cores: são os nomes do `--esconde`. |

Cores do `regioes.png`:

| Cor | Região | Cor | Região |
| --- | --- | --- | --- |
| lilás | `corpo_pescoco` | amarelo | `corpo_braco_sup` (ombro até o cotovelo) |
| azul | `corpo_peito` (até a base do pescoço) | vermelho | `corpo_antebraco` |
| laranja | `corpo_barriga` (faixa fina, mais nas costas) | ciano | `corpo_maos` |
| rosa | `corpo_quadril` (baixo-ventre, virilha, centro do bumbum) | verde | `corpo_coxas` (sobe até a cintura nos lados) |
| verde-claro | `corpo_canelas` | roxo | `corpo_pes` |

Mudou o corpo ou o contrato? Gere o molde de novo (seção 7).

## 2. Duas rotas

| Rota | Para | Como sai do Tripo | Flag do script |
| --- | --- | --- | --- |
| **A. Vestido no molde** | tudo que dobra com o corpo: camiseta, moletom, jaqueta, calça, short, conjunto, meia, cinto. Serve também para cabelo. | o boneco inteiro vestido (corpo + roupa numa malha só) | `--molde` |
| **B. Peça sozinha** | o que é rígido ou fica fora do corpo: chapéu, boné, fone, óculos, item de mão, mochila, calçados | só a peça | `--alinhar` |

**Por que a rota A.** O Tripo gera um personagem inteiro muito melhor que uma roupa solta. A figura vestida sai com a mesma altura do nosso corpo, então a roupa já vem no lugar e no tamanho certos, seja ela cropped, comprida ou com capuz. O script põe a figura na altura do corpo, cola a pele que ficou à mostra no corpo base e apaga tudo que é corpo (a pele visível e a cabeça). Sobra só a roupa.

## 3. Rota A, passo a passo

1. **Imagem de referência.** Abra o `frente.png` num gerador de imagem (img2img, inpainting) ou num programa de desenho e vista o boneco por cima. Se o Tripo da sua conta aceitar várias vistas (multiview), faça também `lado.png` e `costas.png` com a mesma roupa.
2. **Regras da imagem** (o script depende delas):
   - **Não mexa no corpo.** Mesma pose A, mesmas proporções, mesmos pés, mão do mesmo tamanho. A imagem precisa continuar encaixando no molde.
   - **Só a peça.** Nada de cabelo, chapéu, óculos ou sapato junto: tudo que não é pele vira "roupa" e fica na peça. A cabeça fica careca e lisa, sem rosto desenhado.
   - **Roupa solta.** A roupa precisa ficar a pelo menos uns 1,5 cm da pele (no boneco de 1 m). Roupa pintada colada no corpo é apagada junto com a pele. Manga colada no braço some.
   - **Bainhas longe das juntas.** Manga curta termina pelo menos 2 cm acima da linha do cotovelo; short, 2 cm acima ou abaixo do joelho; manga longa termina no pulso. Bainha em cima da junta atravessa a pele quando o boneco dobra.
   - **Cores chapadas, sem detalhe miúdo.** A textura final tem 256 px e o render é pixelado: estampa pequena, costura e texto viram ruído. Blocos grandes de cor.
   - **Sem marca de terceiros.** Nada de logo, personagem ou estampa que não seja nossa.
3. **Tripo.** Image to 3D (ou multiview) com a imagem. Textura ligada, PBR desligado. Se houver limite de faces, uns 8 a 10 mil para a figura inteira: a roupa é uma parte disso e o script ainda reduz para o orçamento do slot. Baixe o `.glb`.
4. **Script**, na raiz do repositório (PowerShell, uma linha; troque o caminho do Blender se for outro):

```
& "D:\SteamLibrary\steamapps\common\Blender\blender.exe" --background --python tools\avatar\blender\encaixar_peca.py -- --base assets\avatar\base.blend --cabeca assets\avatar\cabeca_humano.glb --peca C:\Downloads\camiseta_tripo.glb --molde --slot superior_base --nome camiseta_azul --saida assets\avatar\pecas --perfil larga --esconde corpo_peito,corpo_barriga --reduzir --textura 256
```

5. **Conferir** a saída do script (seção 6) e olhar a peça no Blender ou no protótipo.

## 4. Rota B, passo a passo

1. **Imagem ou texto** só da peça, de frente, sem cabeça nem corpo. Para itens de cabeça, desenhe por cima da cabeça do `frente.png` e depois apague a cabeça: a proporção sai certa.
2. **Tripo:** Image to 3D. Limite de faces perto do máximo do slot (tabela abaixo).
3. **Script.** Itens de cabeça precisam do `--cabeca` (é dele que vêm as âncoras de orelha, testa e topo):

```
& "D:\SteamLibrary\steamapps\common\Blender\blender.exe" --background --python tools\avatar\blender\encaixar_peca.py -- --base assets\avatar\base.blend --cabeca assets\avatar\cabeca_humano.glb --peca C:\Downloads\bone_tripo.glb --slot chapeu --nome bone_preto --saida assets\avatar\pecas --alinhar --cabecas humano --reduzir --textura 256
```

Calçados (o par inteiro numa peça só, encaixa na caixa dos pés):

```
& "D:\SteamLibrary\steamapps\common\Blender\blender.exe" --background --python tools\avatar\blender\encaixar_peca.py -- --base assets\avatar\base.blend --peca C:\Downloads\tenis_tripo.glb --slot calcados --nome tenis_branco --saida assets\avatar\pecas --alinhar --perfil justa --esconde corpo_pes --reduzir --textura 256
```

O encaixe automático existe para cabelo, chapéu, fone, óculos (âncoras da cabeça), calçados (caixa dos pés), roupa de cima, calça e conjunto. Para mão e costas o script deixa a peça onde ela estiver: posicione no Blender por cima do corpo, ou use `--ajuste dx,dy,dz,giro_z,escala`.

## 5. Ficha por slot

| Slot | Rota | `--perfil` | `--esconde` (só o que a peça cobre inteiro) | Outras flags | Tris alvo / máx. |
| --- | --- | --- | --- | --- | --- |
| `superior_base` camiseta | A | `larga` | `corpo_peito,corpo_barriga` | | 600 / 1000 |
| `superior_base` manga longa | A | `larga` | `corpo_peito,corpo_barriga,corpo_braco_sup,corpo_antebraco` | | 600 / 1000 |
| `superior_base` regata | A | `justa` | `corpo_peito,corpo_barriga` | | 600 / 1000 |
| `superior_base` cropped | A | `justa` | `corpo_peito` | | 600 / 1000 |
| `superior_meio` moletom | A | `larga` | `corpo_peito,corpo_barriga,corpo_braco_sup,corpo_antebraco` | `--esconde-slots superior_base` | 700 / 1200 |
| `superior_fora` jaqueta aberta | A | `larga` | `corpo_braco_sup,corpo_antebraco` | | 700 / 1200 |
| `superior_fora` casaco fechado | A | `larga` | `corpo_peito,corpo_barriga,corpo_braco_sup,corpo_antebraco` | `--esconde-slots superior_base,superior_meio` | 700 / 1200 |
| `inferior` calça | A | `larga` (justa se for colada) | `corpo_quadril,corpo_coxas,corpo_canelas` | | 500 / 900 |
| `inferior` short | A | `larga` | `corpo_quadril,corpo_coxas` | | 500 / 900 |
| `inferior` saia | A | `saia` | `corpo_quadril` | | 500 / 900 |
| `conjunto` macacão, terno | A | `larga` | as do superior + as do inferior | `--conflitos superior_base,superior_meio,superior_fora,inferior` | 1200 / 2000 |
| `meias` | A | `justa` | `corpo_canelas` (só se a meia cobrir a canela toda) | | 150 / 300 |
| `cintura` cinto, pochete | A | `rigida_osso --osso hips` | nenhuma | | 250 / 500 |
| `calcados` | B | `justa` | `corpo_pes` | | 300 / 600 |
| `cabelo` | A ou B | (rígido) | nenhuma | A: `--molde-dist 0.006`; B: `--alinhar --cabecas humano` | 600 / 1000 |
| `chapeu` boné | B | (rígido) | nenhuma | `--cabecas humano`; se cobrir o cabelo, `--esconde-slots cabelo` | 300 / 600 |
| `fone`, `oculos` | B | (rígido) | nenhuma | `--cabecas humano` | 300 / 500, 150 / 300 |
| `mao` | B | (rígido) | nenhuma | posicionar à mão | 250 / 500 |
| `costas` mochila | B | (rígido) | nenhuma | posicionar à mão | 300 / 600 |

**`--esconde` é a parte mais fácil de errar.** A região escondida some de verdade. Se a peça não cobre a região inteira, aparece um buraco: manga curta escondendo `corpo_braco_sup` deixa o meio do braço vazio. Na dúvida, não esconda; a validação diz se a pele atravessa a roupa.

**Cabelo pela rota A:** gere a figura com o cabelo na cabeça careca e use `--molde-dist 0.006`. A distância menor evita apagar a franja, que fica colada na testa.

## 6. Lendo a saída do script

O script testa a peça em cinco poses (descanso, braços para cima, braços à frente, sentado, andando) e mede duas coisas:

| Medida | Aviso | Erro | O que fazer |
| --- | --- | --- | --- |
| esticada (p98) | 1,6x | 2,2x | `--perfil larga` (suaviza os pesos); na axila e no gancho da calça é normal avisar |
| dentro do corpo | 1% | 4% | a região citada atravessa a roupa: `--folga 0.008`, esconder a região se a peça cobre ela inteira, ou bainha mais longe da junta |

Com `--molde`, olhe também a linha `molde: N faces -> M de roupa (K ilhas...)`. Se sobrou pouca roupa, ela estava colada na pele: diminua `--molde-dist` ou refaça a imagem com a roupa mais solta. Se sobraram pedaços de pele (rosto, coxa), a figura do Tripo saiu diferente do molde: aumente `--molde-dist` (0.015, 0.02) ou refaça a imagem sem mexer no corpo.

Peça reprovada não deixa arquivo. `--forcar` passa por cima e marca `"forcado": true` no manifesto (a loja recusa).

## 7. Manutenção

- **Molde:** depois de mudar o corpo ou o contrato:

```
& "D:\SteamLibrary\steamapps\common\Blender\blender.exe" --background --python tools\avatar\blender\gerar_molde.py -- --base assets\avatar\base.blend --cabeca assets\avatar\cabeca_humano.glb --saida docs\produto\molde
```

- **Calibrar:** os limites (`--molde-dist` 0,012, folga 0,004, poses de teste) foram testados com peças simuladas no formato do Tripo, não com saídas reais. As primeiras peças de verdade (uma camiseta, uma calça, um tênis, um cabelo) servem para acertar os padrões. Anote aqui o que mudou.
- **Licença:** confira se o plano do Tripo permite uso comercial antes de vender um item feito nele.
