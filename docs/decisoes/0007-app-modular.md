# 0007. App modular, cada recurso desligável

**Decisão:** cada recurso além da conversa é um módulo: crate própria, `feature` do Cargo e interruptor em tempo de execução. Sons e efeitos têm interruptor por categoria.

**Por quê:** o app deve ser muito focado em UX, cheio de efeitos e personalização, mas cada pessoa decide o que quer ver.
