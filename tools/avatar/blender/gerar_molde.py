"""Gera o molde do corpo base: imagens de frente, lado e costas na escala certa.

    blender --background --python tools/avatar/blender/gerar_molde.py -- ^
        --base assets/avatar/base.blend --cabeca assets/avatar/cabeca_humano.glb ^
        --saida docs/produto/molde

Sai em --saida (1 px = 1 mm, câmera ortográfica, 1100 x 1100 px por vista):
  frente.png, lado.png, costas.png   corpo sozinho, fundo transparente: base para
                                     desenhar/gerar a roupa por cima (img2img)
  molde.png                          as três vistas lado a lado, com as linhas de
                                     altura do contrato (pescoço, ombro, cintura...)
  regioes.png                        frente e costas com as 10 regiões do corpo em
                                     cores (as do --esconde; legenda na doc)

As linhas vêm do comum.py: mudou o contrato, gere o molde de novo.
"""
import argparse
import math
import os
import sys

import bpy
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import ANCORAS_CORPO, OSSOS  # noqa: E402

PX = 1100          # lado da imagem (px)
LADO = 1.1         # lado da área filmada (m): 1 px = 1 mm
CENTRO_Z = 0.5

J = {n: (h, t) for n, _, h, t in OSSOS}
LINHAS = [  # (texto, altura em m); "pulso" fica do outro lado, colado na cintura
    ("topo", 1.0),
    ("pescoço", J["head"][0][2]),  # corte da cabeça
    ("ombro", J["leftUpperArm"][0][2]),
    ("cotovelo", J["leftLowerArm"][0][2]),
    ("cintura", ANCORAS_CORPO["anc_cintura"][1][2]),
    ("pulso", J["leftHand"][0][2]),
    ("virilha", J["leftUpperLeg"][0][2]),
    ("joelho", J["leftLowerLeg"][0][2]),
    ("tornozelo", J["leftFoot"][0][2]),
    ("chão", 0.0),
]
# Cores das regiões em regioes.png (a legenda em docs/produto/pecas-tripo.md usa as mesmas).
CORES_REGIAO = {
    "corpo_pescoco": (0.75, 0.55, 0.85), "corpo_peito": (0.25, 0.5, 0.9),
    "corpo_barriga": (0.95, 0.6, 0.2), "corpo_quadril": (0.95, 0.4, 0.7),
    "corpo_braco_sup": (0.95, 0.85, 0.25), "corpo_antebraco": (0.9, 0.25, 0.25),
    "corpo_maos": (0.3, 0.8, 0.85), "corpo_coxas": (0.3, 0.7, 0.35),
    "corpo_canelas": (0.65, 0.85, 0.4), "corpo_pes": (0.5, 0.4, 0.8),
}
VISTAS = {  # nome: (posição da câmera, rotação)
    "frente": ((0, -5, CENTRO_Z), (math.pi / 2, 0, 0)),
    "lado": ((5, 0, CENTRO_Z), (math.pi / 2, 0, math.pi / 2)),
    "costas": ((0, 5, CENTRO_Z), (math.pi / 2, 0, math.pi)),
}


def args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--base", required=True)
    p.add_argument("--cabeca", required=True)
    p.add_argument("--saida", required=True)
    return p.parse_args(argv)


def guias(nome_vista):
    """Linhas finas atrás do boneco e o texto de cada uma, viradas para a câmera da vista."""
    objs = []
    cor_linha, cor_texto = (0.85, 0.2, 0.2, 1), (0.1, 0.1, 0.1, 1)
    _, (rx, ry, rz) = VISTAS[nome_vista]
    for texto, z in LINHAS:
        x_texto = 0.33 if texto == "pulso" else -0.54
        bpy.ops.mesh.primitive_cube_add(size=1)
        linha = bpy.context.active_object
        linha.scale = (0.9, 0.0005, 0.0012)
        linha.location = (0, 0.4, z)  # atrás do boneco (na vista de frente)
        bpy.ops.object.text_add()
        t = bpy.context.active_object
        t.data.body = f"{texto}  {z:.3f} m".replace(".", ",")
        t.data.size = 0.022
        t.location = (x_texto, 0.39, z + 0.004)
        for o, cor in ((linha, cor_linha), (t, cor_texto)):
            o.color = cor
            # gira em volta do eixo Z do mundo para ficar de frente para a câmera
            o.location = rot_z(o.location, rz)
            o.rotation_euler = (rx, ry, rz)
            objs.append(o)
        linha.rotation_euler = (0, 0, rz)
    return objs


def rot_z(v, ang):
    c, s = math.cos(ang), math.sin(ang)
    return (v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2])


def renderizar(caminho, transparente):
    cena = bpy.context.scene
    cena.render.film_transparent = transparente
    cena.render.filepath = caminho
    bpy.ops.render.render(write_still=True)


def main():
    a = args()
    os.makedirs(a.saida, exist_ok=True)
    bpy.ops.wm.open_mainfile(filepath=os.path.abspath(a.base))
    antes = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=os.path.abspath(a.cabeca))
    for o in list(bpy.data.objects):
        if o.type == "EMPTY" or (o not in antes and o.type != "MESH"):
            bpy.data.objects.remove(o)
        elif o.type == "MESH":
            o.color = (0.72, 0.72, 0.72, 1)
            if o.name.startswith("rosto"):
                o.color = (0.6, 0.6, 0.6, 1)

    cena = bpy.context.scene
    cena.render.engine = "BLENDER_WORKBENCH"
    sh = cena.display.shading
    sh.light = "STUDIO"
    sh.color_type = "OBJECT"
    sh.show_object_outline = False  # o contorno desenharia as emendas entre as regiões
    sh.background_type = "VIEWPORT"
    sh.background_color = (1, 1, 1)
    cena.view_settings.view_transform = "Standard"
    cena.render.resolution_x = cena.render.resolution_y = PX
    cena.render.image_settings.file_format = "PNG"
    cena.render.image_settings.color_mode = "RGBA"
    cena.render.image_settings.compression = 100
    cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam"))
    cena.collection.objects.link(cam)
    cam.data.type = "ORTHO"
    cam.data.ortho_scale = LADO
    cena.camera = cam

    com_guias = []
    for nome, (loc, rot) in VISTAS.items():
        cam.location, cam.rotation_euler = loc, rot
        renderizar(os.path.join(a.saida, f"{nome}.png"), True)
        objs = guias(nome)
        tmp = os.path.join(a.saida, f"_{nome}_guias.png")
        renderizar(tmp, False)
        com_guias.append(tmp)
        for o in objs:
            bpy.data.objects.remove(o)

    # junta as três vistas com guias numa folha só
    partes = []
    for caminho in com_guias:
        img = bpy.data.images.load(caminho)
        w, h = img.size
        partes.append(np.array(img.pixels[:], dtype=np.float32).reshape(h, w, 4))
        bpy.data.images.remove(img)
        os.remove(caminho)
    folha = np.concatenate(partes, axis=1)

    # regiões coloridas, frente e costas, sem cabeça (ela não é região)
    escondidos = []
    for o in bpy.data.objects:
        if o.type == "MESH":
            if o.name in CORES_REGIAO:
                o.color = (*CORES_REGIAO[o.name], 1)
            else:
                o.hide_render = True
                escondidos.append(o)
    regioes = []
    for nome in ("frente", "costas"):
        cam.location, cam.rotation_euler = VISTAS[nome]
        tmp = os.path.join(a.saida, f"_{nome}_reg.png")
        renderizar(tmp, False)
        img = bpy.data.images.load(tmp)
        regioes.append(np.array(img.pixels[:], dtype=np.float32).reshape(img.size[1], img.size[0], 4))
        bpy.data.images.remove(img)
        os.remove(tmp)
    reg = np.concatenate(regioes, axis=1)
    out_r = bpy.data.images.new("regioes", reg.shape[1], reg.shape[0], alpha=True)
    out_r.pixels = reg.ravel()
    cena.render.image_settings.color_mode = "RGB"
    out_r.save_render(os.path.join(a.saida, "regioes.png"), scene=cena)
    out = bpy.data.images.new("molde", folha.shape[1], folha.shape[0], alpha=True)
    out.pixels = folha.ravel()
    out.save_render(os.path.join(a.saida, "molde.png"), scene=cena)
    print("ok:", a.saida)


main()
