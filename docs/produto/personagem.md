# Personagem

Cada pessoa tem um personagem 3D no estilo art toy (designer vinyl): cabeça grande, corpo curto, rosto pintado, superfície fosca. Ele faz parte do perfil e aparece na sala do grupo.

## Estilo

| Elemento | Decisão |
| --- | --- |
| Proporção | Cerca de 2,5 a 3 cabeças de altura; a cabeça ocupa uns 40% do boneco |
| Cabeça | Formato base trocável (humano, gato, outros bichos), com blend shapes para largura, bochecha e queixo |
| Rosto | Olhos, sobrancelhas, nariz e boca são texturas pintadas; expressões trocam a textura; o lip sync alterna bocas desenhadas |
| Mãos | Luva simples ou dedos grossos, poucas poses |
| Shader | Um shader toon próprio: sombra em 2 ou 3 faixas, brilho fosco de vinil, contorno opcional |

Referências de estilo: um gato de terno de vinil, uma menina roxa de moletom de vinil e um personagem low poly cel-shaded com contorno. A identidade deve ser própria: desenhar 2 ou 3 bonecos-base originais antes de modelar itens.

## Base técnica

- **Formato**: glTF 2.0 (.glb), seguindo o padrão [VRM](https://vrm.dev) para nomes de ossos e expressões.
- **Esqueleto único**: todo corpo, roupa e acessório é feito para o mesmo rig humanoide. As animações (sentar, andar, acenar, falar) são feitas uma vez e servem para todos.
- **Definição do avatar**: um JSON pequeno com os parâmetros do corpo e os ids dos itens equipados. O servidor guarda isso; o cliente monta o boneco com os modelos em cache.
- **Limites por item**: teto de polígonos e de textura, e versões mais leves (LOD) para avatares distantes.

## Slots

Corte de cabelo, chapéu ou boné, olhos, camiseta, moletom ou jaqueta (por cima da camiseta), calça, meia, tênis, acessório de mão, costas e efeito.

- **Olhos** são texturas do rosto; olhos especiais (brilhantes, com desenho, animados) são só outra textura.
- **Patches** são adesivos aplicados por cima de uma roupa, como os adesivos de arma do CS. Cada peça tem até 3 posições; o patch é um item próprio, com raridade e desgaste.
- **Gratuitos e pagos**: várias peças grátis para todo mundo montar o personagem; itens raros vêm de caixas e do mercado.
- Itens devem ter desenho original. Um olho igual ao Sharingan, por exemplo, é propriedade de terceiros; um olho próprio inspirado nele pode.

## Criação dos modelos

Modelos podem ser gerados com ferramentas de IA (Tripo, Meshy e parecidas) para ganhar tempo, mas cada um passa pelo Blender antes de entrar no app: encaixe no esqueleto base, malha simplificada, texturas ajustadas ao shader toon. Conferir se o plano da ferramenta permite uso comercial antes de vender um item feito nela.
