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

# Versões do contrato. Mudou o rig ou o corpo de um jeito que quebra peças antigas?
# Sobe a versão; peças com versão diferente são recusadas pelo app.
SCHEMA_VERSION = 1
RIG_VERSION = "mothh-humanoide-v1"
CORPO_PERFIL = "chibi-v1"

# Âncoras: empties (filhos do rig) que marcam pontos do corpo e da cabeça.
# O encaixe usa esses pontos em vez da caixa da peça. Ver ANCORAS_DA_CABECA abaixo.
ANCORAS_CORPO = {
    # nome:            (osso,   posição no corpo placeholder)
    "anc_pescoco":     ("neck",  (0.0, 0.0, 0.59)),
    "anc_ombro_l":     ("chest", (0.10, 0.0, 0.56)),
    "anc_ombro_r":     ("chest", (-0.10, 0.0, 0.56)),
    "anc_cintura":     ("hips",  (0.0, 0.0, 0.36)),
    "anc_tornozelo_l": ("leftFoot",  (0.06, 0.0, 0.05)),
    "anc_tornozelo_r": ("rightFoot", (-0.06, 0.0, 0.05)),
}
# Toda cabeça tem estas âncoras (no próprio .glb da cabeça, como empties).
ANCORAS_CABECA = ["anc_topo", "anc_testa", "anc_orelha_l", "anc_orelha_r", "anc_nuca", "anc_olhos"]

# Perfis de cabeça: acessórios dizem com quais perfis são compatíveis.
PERFIS_CABECA = {
    "humano": "orelhas dos lados, sem focinho (humano, robô)",
    "orelhas_altas": "orelhas no topo (gato, coelho)",
    "focinho": "focinho à frente, orelhas caídas (cão)",
}

# Regiões do corpo: objetos separados no base.blend. Uma peça diz quais esconde,
# assim a pele não atravessa a roupa. Quando a região inteira for grossa demais
# (cropped, regata), a peça leva uma máscara (ver doc, seção 2).
REGIOES = [
    "corpo_pescoco", "corpo_peito", "corpo_barriga", "corpo_quadril",
    "corpo_braco_sup", "corpo_antebraco", "corpo_maos",
    "corpo_coxas", "corpo_canelas", "corpo_pes",
]

# Slots: tipo, osso de encaixe (rígidos) e camada. Slots da mesma "familia" com
# camadas diferentes podem coexistir (camiseta + moletom + jaqueta); a camada de
# fora esconde as regiões e, se pedir, a peça de dentro.
SLOTS = {
    #  slot              tipo       osso           família     camada
    "cabeca":         ("rigido",  "head",        "cabeca",   0),
    "cabelo":         ("rigido",  "head",        "cabelo",   0),
    "chapeu":         ("rigido",  "head",        "cabeca_acc", 2),
    "oculos":         ("rigido",  "head",        "rosto_acc", 1),
    "fone":           ("rigido",  "head",        "cabeca_acc", 1),
    "superior_base":  ("skinado", None,          "superior", 0),
    "superior_meio":  ("skinado", None,          "superior", 1),
    "superior_fora":  ("skinado", None,          "superior", 2),
    "inferior":       ("skinado", None,          "inferior", 0),
    "conjunto":       ("skinado", None,          "conjunto", 0),
    "meias":          ("skinado", None,          "meias",    0),
    "calcados":       ("skinado", None,          "calcados", 1),
    "cintura":        ("skinado", None,          "cintura",  3),
    "mao":            ("rigido",  "sock_mao_r",  "mao",      0),
    "costas":         ("rigido",  "sock_costas", "costas",   3),
}

# Orçamento de triângulos por slot: (alvo, máximo). Acima do alvo o script avisa;
# acima do máximo ele recusa. O boneco inteiro tem alvo e teto próprios.
ORCAMENTO_TRIS = {
    "cabeca": (900, 1400), "cabelo": (600, 1000), "chapeu": (300, 600),
    "oculos": (150, 300), "fone": (300, 500),
    "superior_base": (600, 1000), "superior_meio": (700, 1200), "superior_fora": (700, 1200),
    "inferior": (500, 900), "conjunto": (1200, 2000), "meias": (150, 300),
    "calcados": (300, 600), "cintura": (250, 500), "mao": (250, 500), "costas": (300, 600),
}
ORCAMENTO_BONECO = (4000, 6500)  # corpo visível + peças, alvo e teto

# Perfis de rigging (como os pesos são copiados do corpo).
PERFIS_RIG = {
    "justa": "copia direta da face mais próxima (camiseta, calça justa, meia)",
    "larga": "copia + suaviza bastante (moletom, jaqueta, calça larga)",
    "saia":  "só quadril e coxas, suavizado (saia, vestido, casaco longo)",
    "rigida_osso": "100% em um osso só (sapato duro, cinto rígido)",
}
