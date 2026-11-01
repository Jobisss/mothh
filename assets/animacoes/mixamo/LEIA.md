# Animações do Mixamo

Arquivos originais baixados do [Mixamo](https://www.mixamo.com), um por clipe. O nome do arquivo é o nome do clipe no app (`parado.fbx`, `andar.fbx`, `acenar.fbx`...).

Download: *FBX Binary*, *30 fps*, *Without Skin*, *Keyframe Reduction: none*. Pode ser no Y Bot ou no nosso boneco subido lá.

Depois de pôr ou trocar um arquivo aqui, gere os clipes de novo:

```
blender --background --python tools\avatar\blender\importar_animacao.py -- ^
  --base assets\avatar\base.blend --saida assets\avatar --pasta assets\animacoes\mixamo
```

Detalhes em `docs/produto/personagem-modular.md`, seção 13.
