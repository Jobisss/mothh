# 0002. Personagens e salas em 3D com Bevy

**Decisão:** personagens e salas são 3D (2D foi descartado), renderizados com [Bevy](https://bevyengine.org) numa textura exibida dentro do Slint. Modelos em glTF, esqueleto humanoide único, shader toon próprio.

**Por quê:** o estilo art toy pede 3D, e um esqueleto único deixa qualquer combinação de itens funcionar sem modelar cada uma. O foco é a base da criação do boneco, não os modelos.

**Alternativa:** Fyrox. **Cuidado:** Bevy é pré-1.0; fixar a versão.
