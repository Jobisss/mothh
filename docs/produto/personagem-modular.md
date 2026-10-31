# Personagem modular: rig, peças e rosto

Como fazer o boneco do mothh ser modular de verdade: gerar um cabelo novo no Tripo/Meshy, passar por um script no Blender e ele encaixar sozinho em qualquer boneco no app (Bevy). Complementa [personagem.md](personagem.md) e a tarefa F3-02.

Referência visual aprovada (31/out): estilo "Dude", PS2/pixel, 2,5 cabeças, pose A, rosto pintado em textura, cabeças trocáveis (humano, gato, cão, coelho, robô).

## A ideia em uma frase

Existe **um corpo base** com **um esqueleto** que nunca muda. Toda peça é modelada **no lugar, em cima desse corpo**, e sai do Blender em um de dois jeitos:

| Tipo | Quando | Como entra no Bevy |
| --- | --- | --- |
| **Rígida** | não dobra: cabeça, cabelo, chapéu, fone, óculos, item na mão, mochila | o app pendura a peça no osso do slot (`head`, `sock_mao_r`...) |
| **Skinada** | dobra com o corpo: camiseta, calça, conjunto, meia, tênis, cinto | a peça traz pesos copiados do corpo; o app liga as juntas dela nos ossos do boneco pelo nome |

Como a cabeça do estilo é rígida (um osso só), quase tudo de cabeça é peça rígida: não precisa de pesos e encaixa em qualquer boneco.

## 1. Corpo base e esqueleto

O arquivo mestre é `base.blend` (guardar no repositório em `assets/avatar/`, com Git LFS se crescer). Nada nele muda depois que as primeiras peças existirem; mudar o corpo obriga a reencaixar todas.

**Medidas e eixos (Blender)**

- Unidade: metro. Altura total **1,0 m**: pernas 0 a 0,30, tronco 0,30 a 0,61, cabeça 0,61 a 0,98 (cabeça ≈ 40%).
- Z para cima, o boneco olha para **-Y** (vista frontal do Blender, tecla 1), a esquerda dele é **+X**.
- Origem do objeto no chão, entre os pés. Escala e rotação aplicadas (Ctrl+A) em tudo.
- Pose A: braços a 45° para baixo. Mãos tipo luva, sem dedos.
- O exportador glTF do Blender converte para Y para cima sozinho; não girar nada à mão.

**Esqueleto** (nomes do padrão VRM, a lista oficial está em `tools/avatar/blender/comum.py`):

```
hips
├─ spine ─ chest ─┬─ neck ─ head
│                 ├─ leftUpperArm ─ leftLowerArm ─ leftHand ─ (sock_mao_l)
│                 ├─ rightUpperArm ─ rightLowerArm ─ rightHand ─ (sock_mao_r)
│                 └─ (sock_costas)
├─ leftUpperLeg ─ leftLowerLeg ─ leftFoot
├─ rightUpperLeg ─ rightLowerLeg ─ rightFoot
└─ (sock_quadril)
```

- 17 ossos que deformam + 4 **sockets** (`sock_*`, com "Deform" desligado) para acessórios rígidos.
- Sem dedos, sem olhos/mandíbula em osso: o rosto é textura.
- Nunca renomear, nunca mudar a pose de descanso. Animações (sentar, acenar, falar) são feitas uma vez nesse rig e servem para todos.

**Corpo dividido em regiões.** O corpo pelado é separado em objetos: `corpo_tronco`, `corpo_quadril`, `corpo_bracos`, `corpo_maos`, `corpo_coxas`, `corpo_canelas`, `corpo_pes`. Cada peça diz quais regiões esconde (a camiseta esconde `corpo_tronco`). Assim a pele nunca atravessa a roupa e a roupa não precisa ficar larga. É o truque mais importante contra "clipping".

**Cabeças.** A cabeça não faz parte do corpo: é a peça do slot `cabeca` (humano, gato, cão...). Toda cabeça cabe na **caixa padrão de cabeça** (x de -0,18 a 0,18, y de -0,17 a 0,17, z de 0,61 a 0,98) e tem o pescoço no mesmo lugar. Chapéus e fones são feitos para essa caixa, então servem em todas. Cabelo é só para cabeças que aceitam cabelo (o manifesto da cabeça diz `aceita: ["cabelo"]`).

## 2. Slots

| Slot | Tipo | Osso | Esconde (padrão) | Tris |
| --- | --- | --- | --- | --- |
| `cabeca` | rígida | head | | 1200 |
| `cabelo` | rígida | head | | 800 |
| `cabeca_acc` (chapéu, fone, óculos) | rígida | head | `cabelo` às vezes (chapéu) | 500 |
| `superior` | skinada | | corpo_tronco | 900 |
| `inferior` | skinada | | corpo_quadril, corpo_coxas | 700 |
| `conjunto` (terno, macacão) | skinada | | tronco, quadril, coxas; ocupa superior + inferior | 1500 |
| `meias` | skinada | | corpo_canelas | 200 |
| `calcados` | skinada | | corpo_pes | 400 |
| `cintura` (bolsa transversal, cinto) | skinada | | | 400 |
| `mao` | rígida | sock_mao_r | | 400 |
| `costas` (mochila) | rígida | sock_costas | | 500 |

Com ~10 bonecos na tela, um boneco completo fica entre 3 e 5 mil triângulos: folga grande para o Bevy. Texturas: 128×128 ou 256×256 por peça, filtro "Closest" (pixel nítido), sem mipmap borrado. Um material por peça (no máximo 2).

**Manifesto.** Cada peça é um `.glb` + um `.json` gerado pelo script:

```json
{ "id": "camiseta_vermelha", "slot": "superior", "arquivo": "camiseta_vermelha.glb",
  "tipo": "skinado", "osso": null, "esconde": ["corpo_tronco"], "tris": 408 }
```

O app lê o manifesto, não o nome do arquivo. Raridade, preço e float ficam no servidor, fora daqui.

## 3. Rosto por textura

O rosto é um objeto separado dentro da cabeça, chamado **`rosto`**: um pedaço de malha na frente da cara, com UV fixo ocupando o quadrado 0..1 inteiro (U para a direita de quem olha de frente, V para cima). Toda cabeça (humano, gato, robô) tem o seu `rosto` com o mesmo UV, então qualquer textura de olhos serve em qualquer cabeça.

**Atlas de expressões.** Uma textura de olhos/boca é uma folha com células de 32×32 px (ou 64×64), sempre na mesma ordem:

| Célula | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Expressão | neutro | feliz | surpreso | bravo | piscando | rindo | tonto | triste |

4 colunas × 2 linhas, fundo transparente. No Bevy, o material do `rosto` recebe esse atlas e o app escolhe a célula mudando o `uv_transform` do material (escala ¼ × ½ e deslocamento da célula). Trocar expressão, piscar e lip sync é só mudar um número: não carrega nada, não recria malha. Cada boneco tem o próprio material do rosto, então cada um tem a sua expressão.

**Itens de olhos.** Um olho especial (brilhante, com desenho) é só outro atlas no mesmo formato. Para combinar olhos de um item com boca de outro, a evolução (junto com o shader toon, F3-03) é um material próprio (`ExtendedMaterial`) com 2 ou 3 camadas: pele/bochecha, olhos (atlas do item de olhos), boca (atlas de bocas, que também serve para o lip sync). O shader escolhe a célula de cada camada separada. O formato do arquivo não muda, só quantas camadas o material lê.

**Pintar no Blender:** marcar a área do rosto na cabeça gerada, separar (P) como objeto `rosto`, desdobrar com "Project from View" olhando de frente e escalar o UV para preencher 0..1. Pintar as células no Aseprite/Photopea em cima de um molde de 32×32.

## 4. No Bevy (como o app monta o boneco)

Isso já roda no protótipo `tools/avatar-lab` (Bevy 0.20, `cargo run --release` dentro da pasta; precisa do Rust atualizado com `rustup update`):

1. Carrega `base.glb` como cena. Quando fica pronto (`WorldInstanceReady`), guarda um mapa `nome do osso → entidade` e `nome da região → entidade`.
2. **Peça rígida:** spawna a cena da peça como filha da entidade do osso, com `Transform` igual ao inverso da pose de descanso do osso. Resultado: ela fica exatamente onde foi modelada e segue o osso daí em diante.
3. **Peça skinada:** spawna a cena e, quando pronta, troca cada entidade em `SkinnedMesh.joints` pela entidade do corpo com o mesmo `Name`. Funciona porque o esqueleto da peça é idêntico ao do corpo (mesma pose de descanso, então as "inverse bind matrices" batem). O esqueleto duplicado que veio na peça fica parado e não custa quase nada.
4. Esconde as regiões listadas em `esconde` (`Visibility::Hidden`).
5. Rosto: acha o objeto `rosto`, põe um `StandardMaterial` sem luz (`unlit`) com o atlas e `AlphaMode::Mask`, e troca o `uv_transform` para mudar a expressão.

Teclas do protótipo: 1 a 6 trocam a expressão, H tira/põe o cabelo, C tira/põe a camiseta (e mostra o tronco de volta). São 10 bonecos animados por código, todos com o mesmo corpo e as mesmas peças carregadas uma vez só.

## 5. Script do Blender

Em `tools/avatar/blender/`:

- `comum.py`: nomes dos ossos, sockets, regiões, slots e orçamento. Fonte da verdade.
- `criar_base_placeholder.py`: gera um boneco de caixas com o esqueleto oficial (só para testar o pipeline enquanto o corpo de verdade não existe).
- `encaixar_peca.py`: o encaixe automático. Abre o `base.blend`, importa a peça (`.glb`, `.fbx`, `.obj`), junta tudo num objeto, apaga esqueleto/pesos que vieram do Tripo/Meshy, alinha pela caixa da região (opcional), reduz para o orçamento de triângulos, deixa as texturas em "Closest" e:
  - rígida: exporta no lugar;
  - skinada: empurra a peça 4 mm para fora, **copia os pesos do corpo** (Data Transfer, face mais próxima interpolada), limita a 4 ossos por vértice, normaliza, liga no rig e exporta junto com o esqueleto.
  
  No fim confere tudo (nenhum vértice sem peso, só ossos oficiais) e escreve o `.glb` e o `.json`.

```
blender --background --python tools/avatar/blender/encaixar_peca.py -- ^
  --base assets\avatar\base.blend --peca C:\Downloads\cabelo_tripo.glb ^
  --slot cabelo --nome cabelo_franja --saida assets\avatar\pecas --alinhar
```

(No cmd, `^` quebra a linha.) `--esconde corpo_tronco,corpo_bracos` diz o que a peça cobre; `--tris 600` muda o orçamento.

## 6. Passo a passo prático

**Uma vez só: o corpo base**

1. No Tripo/Meshy, gere a partir da imagem de referência (frente) o **boneco pelado**, pose A, simétrico. Prompt: *"chibi character base mesh, 2.5 heads tall, big head, A-pose, arms 45 degrees down, mitten hands, no clothes, simple underwear, symmetrical, low poly, game asset, flat colors"*. Peça low poly / quad e textura simples.
2. Blender: importe, escale para 1,0 m de altura, pés no chão, olhando para -Y, aplique transformações. Limpe a malha (Decimate ou "Remesh" + retopo leve) até ~2,5 mil triângulos sem a cabeça.
3. Separe a cabeça (vira `cabeca_humano`) e o rosto (vira `rosto`, UV 0..1). Separe o corpo nas 7 regiões com os nomes da seção 1.
4. Rode `criar_base_placeholder.py` para ter o esqueleto oficial pronto, copie o objeto `rig` para o seu arquivo e ajuste só a posição das juntas ao seu corpo (Edit Mode), sem renomear nada.
5. Selecione as regiões e depois o rig, Ctrl+P → "With Automatic Weights". Teste girando os ossos em Pose Mode; corrija ombro e quadril com Weight Paint se precisar.
6. Salve como `assets/avatar/base.blend`. A partir daqui ele é congelado.

**Cada peça nova (ex.: um cabelo)**

1. No Tripo/Meshy use **imagem para 3D** com uma imagem da peça **sozinha**, de frente, fundo neutro: *"short bob haircut, hair piece only, no head, no face, hollow inside, low poly, stylized, game asset, flat colors"*. Para roupas: *"red t-shirt only, no body, A-pose sleeves at 45 degrees, hollow, low poly, game asset"*. Gerar em cima do boneco base (imagem do base vestindo a peça) também funciona: depois é só apagar o resto.
2. Baixe em `.glb` e confira a licença comercial do plano.
3. Rode o `encaixar_peca.py` com `--alinhar`. Abra o `.glb` gerado junto com o `base.blend` e veja se encaixou; se não, mova/escale no Blender por cima do corpo e rode de novo sem `--alinhar`.
4. Roupa: gire os ossos em Pose Mode e veja se dobra sem rasgar. Se a manga atravessar o braço, aumente `--folga` ou acrescente `corpo_bracos` em `--esconde`.
5. Copie `.glb` + `.json` para `assets/avatar/pecas/`. O app enxerga a peça nova sem mexer em código.

**Uma textura de rosto nova**

1. Abra o molde (128×64 px, 4×2 células de 32×32), desenhe as 8 expressões na ordem da tabela, fundo transparente.
2. Salve como PNG em `assets/avatar/rostos/`. O app aplica no `rosto` de qualquer cabeça.

## 7. O que fica para depois

- Shader toon e contorno (F3-03), já com as camadas de rosto.
- Render da sala em baixa resolução com pixels grandes (o "PS2/pixel" da referência) e dither.
- Animações de verdade (sentar no sofá, acenar, falar) feitas no `base.blend` e exportadas como clipes glTF.
- Cores trocáveis por peça (o mesmo cabelo em várias cores) via tingimento no shader, para gerar menos modelos.
- Sala e sofá (F3-06): não fazem parte deste passo.
