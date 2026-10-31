"""Convenções do boneco base do mothh (ver docs/produto/personagem-modular.md).

Este arquivo é a fonte da verdade dos nomes de ossos e regiões do corpo.
O Rust (tools/avatar-lab e depois client-room) usa os mesmos nomes.
"""

# Ossos que deformam a malha (nomes do padrão VRM humanoide).
# Eixos do Blender: Z para cima, o boneco olha para -Y, a esquerda dele é +X.
# Unidade: metro. Altura total 1,0 m. Pés no chão (Z=0), origem entre os pés.
OSSOS = [
    # nome,            pai,              cabeça (x, y, z),      cauda (x, y, z)
    ("hips",           None,            (0.0, 0.0, 0.30),      (0.0, 0.0, 0.37)),
    ("spine",          "hips",          (0.0, 0.0, 0.37),      (0.0, 0.0, 0.46)),
    ("chest",          "spine",         (0.0, 0.0, 0.46),      (0.0, 0.0, 0.56)),
    ("neck",           "chest",         (0.0, 0.0, 0.56),      (0.0, 0.0, 0.61)),
    ("head",           "neck",          (0.0, 0.0, 0.61),      (0.0, 0.0, 0.98)),
    ("leftUpperArm",   "chest",         (0.09, 0.0, 0.545),    (0.165, 0.0, 0.47)),
    ("leftLowerArm",   "leftUpperArm",  (0.165, 0.0, 0.47),    (0.235, 0.0, 0.40)),
    ("leftHand",       "leftLowerArm",  (0.235, 0.0, 0.40),    (0.27, 0.0, 0.365)),
    ("rightUpperArm",  "chest",         (-0.09, 0.0, 0.545),   (-0.165, 0.0, 0.47)),
    ("rightLowerArm",  "rightUpperArm", (-0.165, 0.0, 0.47),   (-0.235, 0.0, 0.40)),
    ("rightHand",      "rightLowerArm", (-0.235, 0.0, 0.40),   (-0.27, 0.0, 0.365)),
    ("leftUpperLeg",   "hips",          (0.06, 0.0, 0.29),     (0.06, 0.0, 0.16)),
    ("leftLowerLeg",   "leftUpperLeg",  (0.06, 0.0, 0.16),     (0.06, 0.0, 0.04)),
    ("leftFoot",       "leftLowerLeg",  (0.06, 0.0, 0.04),     (0.06, -0.07, 0.01)),
    ("rightUpperLeg",  "hips",          (-0.06, 0.0, 0.29),    (-0.06, 0.0, 0.16)),
    ("rightLowerLeg",  "rightUpperLeg", (-0.06, 0.0, 0.16),    (-0.06, 0.0, 0.04)),
    ("rightFoot",      "rightLowerLeg", (-0.06, 0.0, 0.04),    (-0.06, -0.07, 0.01)),
]

# Ossos de encaixe (não deformam). Acessórios rígidos ficam presos neles.
SOCKETS = [
    ("sock_mao_l",   "leftHand",   (0.27, 0.0, 0.365),  (0.27, 0.0, 0.33)),
    ("sock_mao_r",   "rightHand",  (-0.27, 0.0, 0.365), (-0.27, 0.0, 0.33)),
    ("sock_costas",  "chest",      (0.0, 0.09, 0.50),   (0.0, 0.09, 0.56)),
    ("sock_quadril", "hips",       (0.0, -0.08, 0.31),  (0.0, -0.08, 0.36)),
]

NOMES_OSSOS = {o[0] for o in OSSOS}

# Regiões do corpo: objetos separados no base.blend. Uma peça diz quais esconde,
# assim a pele não atravessa a roupa (e não precisa de folga gigante).
REGIOES = [
    "corpo_tronco", "corpo_quadril", "corpo_bracos", "corpo_maos",
    "corpo_coxas", "corpo_canelas", "corpo_pes",
]

# Slots, tipo e osso de encaixe (rígidos) — igual ao manifesto em Rust.
SLOTS = {
    "cabeca":     ("rigido",  "head"),
    "cabelo":     ("rigido",  "head"),
    "cabeca_acc": ("rigido",  "head"),
    "superior":   ("skinado", None),
    "inferior":   ("skinado", None),
    "conjunto":   ("skinado", None),
    "meias":      ("skinado", None),
    "calcados":   ("skinado", None),
    "mao":        ("rigido",  "sock_mao_r"),
    "costas":     ("rigido",  "sock_costas"),
    "cintura":    ("skinado", None),
}

# Orçamento (triângulos) por slot, pensando em ~10 bonecos na tela.
ORCAMENTO_TRIS = {
    "cabeca": 1200, "cabelo": 800, "cabeca_acc": 500, "superior": 900,
    "inferior": 700, "conjunto": 1500, "meias": 200, "calcados": 400,
    "mao": 400, "costas": 500, "cintura": 400,
}
