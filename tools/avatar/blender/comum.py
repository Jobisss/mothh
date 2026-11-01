"""Convenções do boneco base do mothh (ver docs/produto/personagem-modular.md).

Este arquivo é a fonte da verdade dos nomes de ossos e regiões do corpo.
O Rust (tools/avatar-lab e depois client-room) usa os mesmos nomes.
"""

# Ossos que deformam a malha (nomes do padrão VRM humanoide).
# Eixos do Blender: Z para cima, o boneco olha para -Y, a esquerda dele é +X.
# Unidade: metro. Altura total 1,0 m (cabeça 0,71 a 1,0). Pés no chão (Z=0), origem entre os pés.
OSSOS = [
    # nome,            pai,              cabeça (x, y, z),          cauda (x, y, z)
    # Medidas do rig do Tripo (padrão Mixamo) em assets/models/modelo-base-with-rig.glb,
    # simetrizadas por preparar_base.py (que grava as mesmas em juntas.json).
    ("hips",            None,             (0.0, 0.006, 0.486),       (0.0, 0.002, 0.506)),
    ("spine",           "hips",           (0.0, 0.002, 0.506),       (0.0, 0.002, 0.56)),
    ("chest",           "spine",          (0.0, 0.002, 0.56),        (0.0, 0.018, 0.693)),
    ("neck",            "chest",          (0.0, 0.018, 0.693),       (0.0, 0.015, 0.71)),
    ("head",            "neck",           (0.0, 0.015, 0.71),        (0.0, -0.009, 1.0)),
    ("leftUpperArm",    "chest",          (0.062, 0.014, 0.662),     (0.137, 0.018, 0.584)),
    ("leftLowerArm",    "leftUpperArm",   (0.137, 0.018, 0.584),     (0.192, 0.014, 0.503)),
    ("leftHand",        "leftLowerArm",   (0.192, 0.014, 0.503),     (0.249, 0.009, 0.45)),
    ("rightUpperArm",   "chest",          (-0.062, 0.014, 0.662),    (-0.137, 0.018, 0.584)),
    ("rightLowerArm",   "rightUpperArm",  (-0.137, 0.018, 0.584),    (-0.192, 0.014, 0.503)),
    ("rightHand",       "rightLowerArm",  (-0.192, 0.014, 0.503),    (-0.249, 0.009, 0.45)),
    ("leftUpperLeg",    "hips",           (0.055, 0.006, 0.463),     (0.07, 0.002, 0.287)),
    ("leftLowerLeg",    "leftUpperLeg",   (0.07, 0.002, 0.287),      (0.086, 0.029, 0.092)),
    ("leftFoot",        "leftLowerLeg",   (0.086, 0.029, 0.092),     (0.102, -0.053, 0.033)),
    ("rightUpperLeg",   "hips",           (-0.055, 0.006, 0.463),    (-0.07, 0.002, 0.287)),
    ("rightLowerLeg",   "rightUpperLeg",  (-0.07, 0.002, 0.287),     (-0.086, 0.029, 0.092)),
    ("rightFoot",       "rightLowerLeg",  (-0.086, 0.029, 0.092),    (-0.102, -0.053, 0.033)),
]

# Ossos de encaixe (não deformam). Acessórios rígidos ficam presos neles.
SOCKETS = [
    ("sock_mao_l",   "leftHand",   (0.235, 0.015, 0.43),  (0.235, 0.015, 0.39)),
    ("sock_mao_r",   "rightHand",  (-0.235, 0.015, 0.43), (-0.235, 0.015, 0.39)),
    ("sock_costas",  "chest",      (0.0, 0.07, 0.58),     (0.0, 0.07, 0.64)),
    ("sock_quadril", "hips",       (0.0, -0.07, 0.47),    (0.0, -0.07, 0.52)),
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
    # nome:            (osso,   posição no corpo base)
    "anc_pescoco":     ("neck",  (0.0, 0.015, 0.70)),
    "anc_ombro_l":     ("chest", (0.07, 0.014, 0.67)),
    "anc_ombro_r":     ("chest", (-0.07, 0.014, 0.67)),
    "anc_cintura":     ("hips",  (0.0, 0.0, 0.51)),
    "anc_tornozelo_l": ("leftFoot",  (0.086, 0.029, 0.092)),
    "anc_tornozelo_r": ("rightFoot", (-0.086, 0.029, 0.092)),
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

# ---------------------------------------------------------------------------
# Mixamo (e rigs automáticos no mesmo padrão, como o do Tripo).
# O rig deles tem 65 ossos (dedos, clavículas, Spine1/Spine2, dedão do pé);
# o nosso tem 17. Os nomes vêm com prefixo ("mixamorig:Hips", "mixamorig1:Hips").

# Osso do mothh: (osso Mixamo que dá a rotação, junta onde começa, junta que dá a direção).
# A direção serve para alinhar poses de descanso diferentes (T-pose do Mixamo x A-pose nossa).
MIXAMO = {
    "hips":  ("Hips",   "Hips",   "Spine"),
    "spine": ("Spine",  "Spine",  "Spine1"),
    "chest": ("Spine2", "Spine1", "Neck"),
    "neck":  ("Neck",   "Neck",   "Head"),
    "head":  ("Head",   "Head",   "HeadTop_End"),
}
for _lado, _l in (("left", "Left"), ("right", "Right")):
    MIXAMO.update({
        f"{_lado}UpperArm": (f"{_l}Arm",     f"{_l}Arm",     f"{_l}ForeArm"),
        f"{_lado}LowerArm": (f"{_l}ForeArm", f"{_l}ForeArm", f"{_l}Hand"),
        f"{_lado}Hand":     (f"{_l}Hand",    f"{_l}Hand",    f"{_l}HandMiddle1"),
        f"{_lado}UpperLeg": (f"{_l}UpLeg",   f"{_l}UpLeg",   f"{_l}Leg"),
        f"{_lado}LowerLeg": (f"{_l}Leg",     f"{_l}Leg",     f"{_l}Foot"),
        f"{_lado}Foot":     (f"{_l}Foot",    f"{_l}Foot",    f"{_l}ToeBase"),
    })
assert set(MIXAMO) == NOMES_OSSOS


def nome_mixamo(nome):
    """"mixamorig:LeftArm", "mixamorig1:LeftArm", "mixamorig_LeftArm" -> "LeftArm"."""
    nome = nome.split(":")[-1]
    for pre in ("mixamorig_", "mixamorig"):
        if nome.startswith(pre):
            return nome[len(pre):]
    return nome


def osso_dos_pesos(nome):
    """Osso do mothh que herda os pesos de um osso Mixamo (dedos -> mão, clavícula -> peito...)."""
    n = nome_mixamo(nome)
    if n.endswith("Eye"):
        return "head"
    for l, lado in (("Left", "left"), ("Right", "right")):
        if n.startswith(l):
            resto = n[len(l):]
            if resto == "Shoulder":
                return "chest"
            if resto.startswith("Hand"):
                return lado + "Hand"
            if resto.startswith(("Foot", "Toe")):
                return lado + "Foot"
            return {"Arm": lado + "UpperArm", "ForeArm": lado + "LowerArm",
                    "UpLeg": lado + "UpperLeg", "Leg": lado + "LowerLeg"}.get(resto)
    if n in ("Spine1", "Spine2"):
        return "chest"
    if n.startswith("Neck"):
        return "neck"
    if n.startswith("Head"):
        return "head"
    return {"Hips": "hips", "Spine": "spine"}.get(n)
