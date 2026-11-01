# Personagem modular: rig, peças e rosto

Como fazer o boneco do mothh ser modular de verdade: gerar um cabelo novo no Tripo/Meshy, passar por um script no Blender e ele encaixar sozinho em qualquer boneco no app (Bevy). Complementa [personagem.md](personagem.md) e a tarefa F3-02.

Referência visual aprovada (31/out): estilo "Dude", PS2/pixel, pose A, rosto pintado em textura, cabeças trocáveis (humano, gato, cão, coelho, robô).

Como vai existir mercado, isto é um **contrato**: uma peça comprada hoje tem que continuar funcionando depois. Por isso tudo tem versão (seção 3) e o script recusa peça que não passa nos testes (seção 6).

## A ideia em uma frase

Existe **um corpo base** com **um esqueleto** que nunca muda dentro de uma versão do rig. Toda peça é modelada **no lugar, em cima desse corpo**, e sai do Blender em um de dois jeitos:

| Tipo | Quando | Como entra no Bevy |
| --- | --- | --- |
| **Rígida** | não dobra: cabeça, cabelo, chapéu, fone, óculos, item na mão, mochila | o app pendura a peça no osso do slot (`head`, `sock_mao_r`...) |
| **Skinada** | dobra com o corpo: camiseta, moletom, calça, conjunto, meia, tênis, cinto | a peça traz pesos copiados do corpo; o app liga as juntas dela nos ossos do boneco pelo nome |

## 1. Corpo base e esqueleto

O arquivo mestre é `base.blend` (em `assets/avatar/`, com Git LFS se crescer). Ele só é congelado depois da validação da seção 8.

**Medidas e eixos (Blender)**

- Unidade: metro. Altura total **1,0 m**: pernas 0 a 0,46, tronco 0,46 a 0,71, cabeça 0,71 a 1,0 (cabeça ≈ 30%, umas 3 cabeças de altura; o corte fica na parte mais fina do pescoço). Corpo gerado em 01/11 e escolhido pelo Jobiss no lugar dos 40% da folha de referência; as juntas são as do rig automático do Tripo (`assets/models/modelo-base-with-rig.glb`), simetrizadas.
- Z para cima, o boneco olha para **-Y** (vista frontal, tecla 1), a esquerda dele é **+X**.
- Origem no chão, entre os pés. Escala e rotação aplicadas (Ctrl+A) em tudo.
- Pose A: braços a 45° para baixo. Mãos tipo luva, sem dedos.
- O exportador glTF converte para Y para cima sozinho; não girar nada à mão.

**Esqueleto** `mothh-humanoide-v1` (nomes VRM; a lista oficial, com as posições, está em `tools/avatar/blender/comum.py`):

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

17 ossos que deformam + 4 **sockets** (`sock_*`, "Deform" desligado) para acessórios rígidos. Sem dedos, sem osso de olho ou boca: o rosto é textura.

**Vindo do Mixamo.** O rig automático do Tripo segue o padrão Mixamo (`mixamorig:Hips`, 65 ossos). O `preparar_base.py` reaproveita as juntas e os pesos dele e junta o que o mothh não tem: dedos na mão, clavícula e `Spine1`/`Spine2` no `chest`, dedão no pé. A tabela de/para está em `comum.MIXAMO`.

**Âncoras.** Além dos ossos, o corpo tem pontos de referência (empties presos nos ossos): `anc_pescoco`, `anc_ombro_l/r`, `anc_cintura`, `anc_tornozelo_l/r`. Cada cabeça tem as suas: `anc_topo`, `anc_testa`, `anc_orelha_l/r`, `anc_nuca`, `anc_olhos`. O encaixe usa esses pontos (seção 5).

## 2. Regiões do corpo e máscaras

Contra o "clipping", o corpo pelado é separado em **10 regiões** (objetos no `base.blend`), e cada peça diz quais esconde:

`corpo_pescoco`, `corpo_peito`, `corpo_barriga`, `corpo_quadril`, `corpo_braco_sup`, `corpo_antebraco`, `corpo_maos`, `corpo_coxas`, `corpo_canelas`, `corpo_pes`

Exemplos: camiseta esconde peito e barriga (o braço aparece abaixo da manga); cropped esconde só o peito; regata esconde peito e barriga, mas não os braços; moletom esconde peito, barriga, braço e antebraço.

Quando nem a região mais fina serve (decote, rasgo, manga 3/4), a peça leva uma **máscara**: um PNG preto e branco no UV do corpo; onde é preto a pele não é desenhada. O manifesto já tem o campo `mascara`; o shader toon (F3-03) é que vai ler. Até lá, região inteira.

## 3. Manifesto v1 (o contrato da peça)

Cada peça é um `.glb` + um `.json` gerado pelo `encaixar_peca.py`. O app lê o manifesto, não o nome do arquivo.

```json
{
  "schema_version": 1,
  "id": "moletom_largo",
  "slot": "superior_meio",
  "arquivo": "moletom_largo.glb",
  "tipo": "skinado",
  "osso": null,
  "rig_version": "mothh-humanoide-v1",
  "corpo_perfil": "chibi-v1",
  "perfil_rig": "larga",
  "esconde": ["corpo_peito", "corpo_barriga", "corpo_braco_sup", "corpo_antebraco"],
  "mascara": null,
  "esconde_slots": ["superior_base"],
  "conflitos": [],
  "cabecas": ["*"],
  "variantes": {},
  "encaixe": "no_lugar",
  "ajuste": null,
  "textura": 256,
  "tris": 960,
  "validacao": { "bracos_cima": 1.25, "bracos_frente": 1.32, "sentado": 1.03, "andando": 1.01 }
}
```

- `schema_version`: formato do JSON. `rig_version` e `corpo_perfil`: para qual esqueleto e corpo a peça foi feita. O app **recusa** peça de outra versão (o protótipo já faz isso). Se um dia o rig mudar, vira `mothh-humanoide-v2`, as peças antigas são reencaixadas pelo script (o `ajuste` gravado permite refazer igual) e as duas versões convivem enquanto a migração não termina.
- `cabecas` e `variantes`: compatibilidade com cabeças (seção 4).
- `encaixe` e `ajuste`: como a peça foi posicionada, para dar para refazer.
- `validacao`: o resultado das poses de teste (seção 6).

Raridade, preço e float ficam no servidor, fora daqui.

## 4. Slots, camadas e conflitos

| Slot | Tipo | Osso | Família / camada | Alvo / máx. tris |
| --- | --- | --- | --- | --- |
| `cabeca` | rígida | head | cabeça | 900 / 1400 |
| `cabelo` | rígida | head | cabelo | 600 / 1000 |
| `chapeu` | rígida | head | acessório de cabeça, 2 | 300 / 600 |
| `fone` | rígida | head | acessório de cabeça, 1 | 300 / 500 |
| `oculos` | rígida | head | rosto | 150 / 300 |
| `superior_base` (camiseta, regata) | skinada | | superior, 0 | 600 / 1000 |
| `superior_meio` (moletom, suéter) | skinada | | superior, 1 | 700 / 1200 |
| `superior_fora` (jaqueta, casaco) | skinada | | superior, 2 | 700 / 1200 |
| `inferior` | skinada | | inferior | 500 / 900 |
| `conjunto` (terno, macacão) | skinada | | ocupa superior + inferior | 1200 / 2000 |
| `meias` | skinada | | meias | 150 / 300 |
| `calcados` | skinada | | calçados | 300 / 600 |
| `cintura` (bolsa transversal, cinto) | skinada | | por cima de tudo | 250 / 500 |
| `mao` | rígida | sock_mao_r | mão | 250 / 500 |
| `costas` (mochila) | rígida | sock_costas | costas | 300 / 600 |

**Regras (nesta ordem):**

1. **Um item por slot.** Equipar outro no mesmo slot troca.
2. **Camadas convivem.** Camiseta (0), moletom (1) e jaqueta (2) podem estar juntos. A peça de fora diz em `esconde_slots` quais de dentro somem (o moletom esconde a camiseta; a jaqueta aberta não esconde nada).
3. **Conflitos travam.** `conflitos` lista slots que não podem estar junto. `conjunto` conflita com `superior_*` e `inferior`; equipar um conjunto tira os outros, e vice-versa. O editor mostra o que vai sair antes de trocar.
4. **Cabeça manda no cabelo.** Chapéu pode esconder o cabelo (`esconde_slots: ["cabelo"]`) ou usar a variante "com chapéu" do cabelo, quando o cabelo tiver uma.
5. **Visibilidade final:** uma peça aparece se está equipada e nenhuma peça visível a esconde; uma região do corpo some se alguma peça visível a lista em `esconde`. O protótipo já roda essa regra (teclas C e M).

**Patches** (adesivos tipo CS) ficam para depois: cada roupa vai ter empties `patch_1..3` onde o adesivo é colado como decal.

## 5. Encaixe: âncoras, perfis e ajuste

**Roupa vinda do Tripo vestida no molde** (`--molde`, o caminho recomendado para peças skinadas; passo a passo em [pecas-tripo.md](pecas-tripo.md)): a figura inteira é posta na altura do corpo base, um ajuste fino cola a pele à mostra no corpo, e o script apaga o que é corpo (perto da pele visível ou da cabeça). Sobra a roupa já no lugar e no tamanho certos.

Para peça sozinha, o `encaixar_peca.py --alinhar` tenta, nesta ordem:

1. **Âncoras da peça.** Se a peça trouxer empties com os mesmos nomes das âncoras (ex.: você marcou `anc_orelha_l/r` e `anc_topo` no fone), o script calcula escala, rotação e posição que levam essas âncoras às do corpo/cabeça. Precisa de **3 ou mais âncoras fora de uma linha**; com menos, cai para o perfil do slot. É o encaixe mais confiável e vale a pena para chapéu, fone e óculos.
2. **Perfil de encaixe do slot.** Sem âncoras na peça, usa uma regra por slot medida nas âncoras do alvo: cabelo pela distância entre as orelhas e pelo topo da cabeça; chapéu pelas orelhas e pela testa; óculos pelos olhos; calçados pela caixa dos pés (sola no chão); roupa de cima do pescoço à cintura, calça da cintura ao tornozelo, conjunto do pescoço ao tornozelo. Para roupa é aproximação grossa (escala pela altura da peça, que muda de modelo para modelo); prefira o `--molde`.
3. **Manual.** Se nada servir, a peça fica onde está: posicione no Blender por cima do corpo e rode sem `--alinhar`.

Depois disso, `--ajuste dx,dy,dz,giro_z,escala` corrige o que faltar, e o valor fica gravado no manifesto (`ajuste`), então rodar de novo dá o mesmo resultado.

**Cabeças diferentes.** Todas as cabeças têm o pescoço no mesmo ponto e o mesmo conjunto de âncoras, mas cada uma no seu lugar (as orelhas do gato ficam no topo). Cada cabeça tem um `perfil_cabeca`:

| Perfil | Cabeças | O que muda |
| --- | --- | --- |
| `humano` | humano, robô | orelhas dos lados |
| `orelhas_altas` | gato, coelho | orelhas no topo: chapéu e fone precisam de furo ou variante |
| `focinho` | cão | focinho à frente: óculos e máscara precisam de variante |

Um acessório diz em `cabecas` com quais perfis funciona e pode ter `variantes` (`{"orelhas_altas": "fone_gato.glb"}`). O app usa a variante quando existe, usa a peça normal quando é compatível, e esconde quando não é (o editor avisa). Para gerar a variante: rode o script com `--cabeca cabeca_gato.glb`, que encaixa nas âncoras daquela cabeça.

## 6. Pesos: perfis de rigging e validação

Copiar os pesos da pele mais próxima funciona para roupa justa e falha para roupa larga: a manga grande "pega" peso do osso errado. Por isso o script tem perfis (`--perfil`):

| Perfil | Para | Como |
| --- | --- | --- |
| `justa` (padrão) | regata, calça colada, meia, calçados | copia da face mais próxima |
| `larga` | camiseta com manga, moletom, jaqueta, calça larga | copia e suaviza os pesos pela malha (6 passadas); na axila estica bem menos |
| `saia` | saia, vestido, casaco longo | só quadril, coluna e coxas, bem suavizado |
| `rigida_osso` | sapato duro, cinto rígido | 100% num osso (`--osso`) |

**Validação (sempre roda; `--sem-validar` só para rascunho).** O script testa a pose de descanso e mais quatro (braços para cima, braços à frente, **sentado**, andando) e mede duas coisas: quanto as arestas esticam (percentil 98; acima de 2,2x reprova) e quantos vértices da roupa ficam **dentro de uma parte visível do corpo** (acima de 4% reprova, dizendo qual região). "Dentro" é medido contra a superfície do corpo mais próxima entre todas as regiões, e só conta se essa região estiver visível (cada região é um retalho aberto, e a borda de um retalho vizinho dava falso positivo). Peça reprovada não deixa arquivo nenhum na saída: o `.glb` e o `.json` são escritos numa pasta temporária e só publicados no fim. `--forcar` passa por cima e marca `"forcado": true` no manifesto (a loja recusa). Ainda vale olhar no Blender: o teste pega os casos grosseiros.

## 7. Orçamento

Cada slot tem **alvo** e **máximo** (tabela da seção 4). Acima do alvo o script avisa; acima do máximo recusa (`--reduzir` decima até o alvo). O boneco inteiro tem alvo de **4.000** e teto de **6.500** triângulos; o app soma só o que está visível e não deixa equipar o que passar do teto (já no protótipo; a tecla F mostra triângulos, malhas e malhas skinadas de cada boneco).

Triângulo não é o único custo. Os outros limites:

- **Materiais:** no máximo 2 por peça e ~8 por boneco (cada material é um draw call). Texturas de 128 ou 256 px, filtro "Closest".
- **Ossos:** 17 por boneco; peças skinadas reaproveitam os mesmos, então o custo de animação não cresce com as roupas.
- **Malhas skinadas:** cada peça skinada é uma malha a mais para deformar; o teto prático é ~6 por boneco.
- **Medição real:** a meta é 60 fps com 10 bonecos animados num notebook com vídeo integrado (Intel UHD ou equivalente). Essa medição faz parte da validação da F3-02; os números de cima podem mudar com o resultado.

## 8. Rosto por textura

O rosto é um objeto separado dentro da cabeça, chamado **`rosto`**, com UV fixo ocupando o quadrado 0..1 inteiro (U para a direita de quem olha de frente, V para cima). Toda cabeça tem o seu `rosto` com o mesmo UV, então qualquer textura de olhos serve em qualquer cabeça. O script confere que a cabeça tem o `rosto` e as 6 âncoras.

**Atlas de expressões.** Uma textura de olhos/boca é uma folha de células de 32×32 px (ou 64×64), sempre na mesma ordem, 4 colunas × 2 linhas, fundo transparente:

| Célula | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Expressão | neutro | feliz | surpreso | bravo | piscando | rindo | tonto | triste |

No Bevy, o material do `rosto` recebe o atlas e o app escolhe a célula mudando o `uv_transform` do material. Trocar expressão, piscar e lip sync é mudar um número. Cada boneco tem o próprio material do rosto.

Para combinar olhos de um item com boca de outro, a evolução (com o shader toon, F3-03) é um material próprio (`ExtendedMaterial`) com camadas: pele/bochecha, olhos (atlas do item), boca (atlas de bocas, que também serve para o lip sync). O formato do arquivo não muda.

## 9. No Bevy (como o app monta o boneco)

Já roda no protótipo `tools/avatar-lab` (Bevy 0.20, `cargo run --release` dentro da pasta; precisa do Rust atualizado com `rustup update`):

1. Lê os manifestos `.json` de `assets/avatar` e recusa os de outra versão do contrato.
2. Carrega `base.glb`. Quando fica pronto (`WorldInstanceReady`), guarda `nome do osso → entidade` e `nome da região → entidade`.
3. **Peça rígida:** spawna como filha do osso, com `Transform` igual ao inverso da pose de descanso do osso. Ela fica onde foi modelada e segue o osso.
4. **Peça skinada:** troca cada entidade em `SkinnedMesh.joints` pela entidade do corpo com o mesmo `Name`. Funciona porque o esqueleto da peça é o mesmo do corpo (mesma `rig_version`).
5. Aplica a regra de visibilidade da seção 4 todo frame (barato: só muda o que mudou).
6. Rosto: acha o objeto `rosto`, põe um material sem luz com o atlas e troca o `uv_transform` para mudar a expressão.

7. **Animação:** lê `animacoes.json` e carrega os clipes do `animacoes.glb` num `AnimationGraph` só. Como o `base.glb` não traz animação, o app marca cada osso com o `AnimationTargetId` do caminho de nomes desde o nó `rig` (`rig/hips/spine/...`, o mesmo que o clipe usa) e põe o `AnimationPlayer` no `rig`. Peças skinadas usam esses ossos e as rígidas estão penduradas neles, então tudo anima junto. Clipe com `velocidade` (andar) move o boneco nessa velocidade.

Teclas: 1 a 6 expressão, H cabelo, C camiseta, M moletom (o moletom esconde a camiseta e os braços), W anda/para, N próximo clipe. São 10 bonecos, cada um começando o clipe num ponto diferente, com as peças carregadas uma vez só. Sem `animacoes.json` na pasta, roda a animação de teste feita em código.

## 10. Scripts do Blender

Em `tools/avatar/blender/`:

- `comum.py`: fonte da verdade (versões, ossos, sockets, âncoras, regiões, slots, orçamento, perfis).
- `criar_base_placeholder.py`: boneco de caixas com o esqueleto e as âncoras oficiais, mais cabeça, cabelo, camiseta e moletom de mentira para testar.
- `preparar_base.py`: transforma o corpo gerado (ex.: `assets/models/modelo-base-with-rig.glb`) no `base.blend` (reduz triângulos só em mãos, pés e cabeça, rig de 17 ossos, pesos do rig Mixamo ou automáticos, cabeça cortada num plano e rosto separados, 10 regiões pelo osso que mais pesa em cada face, âncoras). Grava `juntas.json` e avisa se as juntas fugirem do `comum.py`.
- `importar_animacao.py`: passa animações do Mixamo para o nosso rig e gera `animacoes.glb` + `animacoes.json` (seção 13).
- `encaixar_peca.py`: importa, limpa, encaixa (`--molde`, âncoras, perfil ou manual + ajuste), confere orçamento, reduz e deixa as texturas em "Closest" (`--textura 256`), pesa pelo perfil, valida as poses e escreve `.glb` + `.json`.
- `gerar_molde.py`: imagens do corpo base na escala certa (frente, lado, costas, linhas de altura e mapa das regiões) em `docs/produto/molde/`, a base para gerar peças no Tripo.

Corpo base (refaz `base.blend`, `base.glb` e a cabeça; depois a cabeça passa pelo `encaixar_peca.py --slot cabeca --perfil-cabeca humano` para ganhar o manifesto):

```
blender --background --python tools\avatar\blender\preparar_base.py -- ^
  assets\models\modelo-base-with-rig.glb assets\avatar
```

Peça, no cmd (`^` quebra a linha):

```
blender --background --python tools\avatar\blender\encaixar_peca.py -- ^
  --base assets\avatar\base.blend --peca C:\Downloads\moletom_tripo.glb ^
  --slot superior_meio --nome moletom_largo --saida assets\avatar\pecas ^
  --alinhar --perfil larga ^
  --esconde corpo_peito,corpo_barriga,corpo_braco_sup,corpo_antebraco --esconde-slots superior_base
```

Cabelo, com a cabeça de referência para as âncoras:

```
blender --background --python tools\avatar\blender\encaixar_peca.py -- ^
  --base assets\avatar\base.blend --cabeca assets\avatar\cabeca_humano.glb ^
  --peca C:\Downloads\cabelo_tripo.glb --slot cabelo --nome cabelo_franja ^
  --saida assets\avatar\pecas --alinhar --cabecas humano
```

## 11. Validação da F3-02 (antes de gerar itens em quantidade)

A ordem para não desperdiçar geração no Tripo:

1. **Congelar o contrato.** Proporções, eixos, origem, posição do pescoço, escala, nomes de ossos e âncoras, `mothh-humanoide-v1`. É o que está neste doc e no `comum.py`; mudar depois custa reencaixar tudo.
2. **Gerar o corpo base no Tripo.** Referência neutra: sem roupa, sem cabelo, **sem pixelização** (o pixel vem do render), pose A, simétrico. Prompt: *"chibi character base mesh, 2.5 heads tall, big head, A-pose, arms 45 degrees down, mitten hands, no clothes, simple underwear, no hair, symmetrical, low poly, game asset, flat colors"*.
3. **Rig e corte.** Feito em 01/11: o Tripo gerou o rig (padrão Mixamo) e o `preparar_base.py` converteu para o nosso (juntas, pesos, cabeça, 10 regiões, âncoras). Falta olhar as poses de teste no Blender e corrigir ombro e quadril no Weight Paint, se precisar.
4. **Peças de prova**, cada uma testando um caso: um **cabelo** (rígido, encaixe na cabeça), uma **camiseta**, uma **calça** e um **tênis**. Gerar seguindo [pecas-tripo.md](pecas-tripo.md) (roupa vestida no molde; tênis e itens de cabeça sozinhos), rodar o script, olhar no Blender. Essas primeiras peças calibram os padrões do script.
5. **Testar no protótipo.** Copiar para `tools/avatar-lab/assets/avatar`, rodar com 10 bonecos, trocar as peças com o app aberto, conferir sentado/em pé (animação de sentar entra aqui) e medir fps no notebook mais fraco que tiver.
6. **Congelar a v1.** Só depois disso o `base.blend` e o contrato viram referência para todos os itens futuros. A partir daí, gerar dezenas de cosméticos.

## 12. O que fica para depois

- Shader toon e contorno (F3-03), com as camadas do rosto e a máscara do corpo.
- Render da sala em baixa resolução com pixels grandes (o "PS2/pixel" da referência) e dither.
- Sentar no sofá (o Mixamo tem "Sitting Idle", mas a altura do assento tem que bater com a sala) e animações que não existem no Mixamo (falar, reações) feitas à mão no `base.blend`.
- Cores trocáveis por peça (tingimento no shader), para gerar menos modelos.
- Patches (decals nos pontos `patch_1..3`).
- Sala e sofá (F3-06).

## 13. Animações (Mixamo)

As animações vêm prontas do [Mixamo](https://www.mixamo.com) e são passadas (retarget) para o nosso rig de 17 ossos. Um arquivo só (`animacoes.glb`) serve para todo boneco, com qualquer roupa ou cabeça, porque todos têm o mesmo esqueleto.

**Baixar do Mixamo.** Dá para usar os bonecos deles (Y Bot, em T-pose) ou subir o nosso (`modelo-base-with-rig.glb`, A-pose): o script alinha as poses de descanso sozinho. Opções: *FBX Binary*, *30 fps*, *Without Skin* (só o esqueleto, arquivo menor), *Keyframe Reduction: none*. Para andar e correr, tanto faz *In Place*: o script tira o deslocamento de qualquer jeito e guarda a velocidade.

**Onde guardar.** Em `assets/animacoes/mixamo/`, um arquivo por clipe, com o nome que o app vai usar: `parado.fbx`, `andar.fbx`, `acenar.fbx`... O app procura `parado` (ou `idle`) para o boneco parado e `andar` (ou `walking`) para a tecla W.

**Gerar os clipes:**

```
blender --background --python tools\avatar\blender\importar_animacao.py -- ^
  --base assets\avatar\base.blend --saida assets\avatar ^
  --pasta assets\animacoes\mixamo --sem-loop acenar
```

Sai `animacoes.glb` (só o rig e os clipes) e `animacoes.json`:

```json
{
  "schema_version": 1,
  "rig_version": "mothh-humanoide-v1",
  "arquivo": "animacoes.glb",
  "fps": 30.0,
  "clipes": [
    { "nome": "andar", "fonte": "andar.fbx", "quadros": 31, "duracao": 1.0,
      "loop": true, "raiz": "no_lugar", "velocidade": 0.49 }
  ]
}
```

**Como o retarget funciona.** Para cada quadro e cada osso do mothh, o script pega a rotação no mundo do osso Mixamo correspondente (`comum.MIXAMO`), em relação à pose de descanso dele, e aplica no nosso osso já alinhado à direção do osso Mixamo. Por isso T-pose x A-pose e braços de tamanhos diferentes não atrapalham. Dedos, clavículas e `Spine1` ficam de fora: a direção do braço e do peito já inclui o efeito deles. O quadril anda na escala do boneco (altura do nosso quadril dividida pela do Mixamo).

**Raiz.** Por padrão o clipe fica no lugar e o app move o boneco: o deslocamento do quadril no chão sai do clipe e vira `velocidade` (m/s), para os pés não escorregarem. `--raiz-livre nome` mantém o deslocamento (para um pulo ou uma cambalhota, por exemplo). `--sem-loop nome` marca clipes que tocam uma vez só.

**Limites.** O boneco tem pernas curtas e cabeça grande: animação com mão no rosto ou braços cruzados pode atravessar a cabeça ou o corpo. Antes de aceitar um clipe, olhar no Blender (abrir o `base.blend` e importar o `animacoes.glb`) ou no protótipo (`AVATAR_PASTA=avatar_real`, tecla N).
