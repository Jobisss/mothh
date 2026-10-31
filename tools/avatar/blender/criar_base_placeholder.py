"""Gera um boneco base de MENTIRA (caixas) com o esqueleto oficial, e duas peças
de teste. Serve para testar o pipeline e o protótipo em Bevy antes dos modelos
reais do Tripo/Meshy.

    blender --background --python tools/avatar/blender/criar_base_placeholder.py -- SAIDA_DIR

Gera: base.blend, base.glb, cabeca_humano.glb (com o objeto "rosto"),
cabelo_bob.glb (rígido) e camiseta_crua.glb (sem pesos, para testar o encaixar_peca.py).
"""
import os
import sys

import bpy  # noqa: I001 (no módulo pip, o bpy precisa vir antes do bmesh)
import bmesh
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import OSSOS, SOCKETS  # noqa: E402

saida = sys.argv[sys.argv.index("--") + 1] if "--" in sys.argv else "."
os.makedirs(saida, exist_ok=True)

bpy.ops.wm.read_factory_settings(use_empty=True)
cena = bpy.context.scene


def material(nome, cor):
    m = bpy.data.materials.new(nome)
    m.use_nodes = True
    p = m.node_tree.nodes["Principled BSDF"]
    p.inputs["Base Color"].default_value = (*cor, 1.0)
    p.inputs["Roughness"].default_value = 1.0
    return m


def caixa(nome, centro, tamanho, mat, cortes=3):
    me = bpy.data.meshes.new(nome)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.subdivide_edges(bm, edges=bm.edges[:], cuts=cortes, use_grid_fill=True)
    for v in bm.verts:
        v.co = Vector((v.co.x * tamanho[0], v.co.y * tamanho[1], v.co.z * tamanho[2])) + Vector(centro)
    bm.to_mesh(me)
    bm.free()
    ob = bpy.data.objects.new(nome, me)
    ob.data.materials.append(mat)
    cena.collection.objects.link(ob)
    return ob


def juntar(nome, partes):
    for o in bpy.context.selected_objects:
        o.select_set(False)
    for o in partes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = partes[0]
    bpy.ops.object.join()
    partes[0].name = nome
    partes[0].data.name = nome
    return partes[0]


# --- esqueleto -------------------------------------------------------------
arm_data = bpy.data.armatures.new("rig")
arm = bpy.data.objects.new("rig", arm_data)
cena.collection.objects.link(arm)
bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="EDIT")
for nome, pai, cab, cau in OSSOS + SOCKETS:
    b = arm_data.edit_bones.new(nome)
    b.head, b.tail = Vector(cab), Vector(cau)
    if pai:
        b.parent = arm_data.edit_bones[pai]
        b.use_connect = False
    b.use_deform = not nome.startswith("sock_")
bpy.ops.object.mode_set(mode="OBJECT")


def pesar_por_distancia(ob, ossos):
    """Pesos simples por distância ao segmento do osso (bom o bastante para caixas)."""
    segs = {n: (Vector(h), Vector(t)) for n, _, h, t in OSSOS if n in ossos}
    grupos = {n: ob.vertex_groups.new(name=n) for n in segs}
    for v in ob.data.vertices:
        ds = []
        for n, (a, b) in segs.items():
            ab = b - a
            t = max(0.0, min(1.0, (v.co - a).dot(ab) / ab.length_squared))
            ds.append((max((a + ab * t - v.co).length, 1e-4), n))
        ds.sort()
        ds = ds[:2]
        ws = [(1.0 / d ** 4, n) for d, n in ds]
        s = sum(w for w, _ in ws)
        for w, n in ws:
            grupos[n].add([v.index], w / s, "REPLACE")
    ob.parent = arm
    ob.modifiers.new("Armature", "ARMATURE").object = arm


pele = material("pele", (0.85, 0.55, 0.38))
cueca = material("roupa_baixo", (0.45, 0.45, 0.5))

regioes = {
    "corpo_tronco": ([caixa("t", (0, 0, 0.495), (0.17, 0.12, 0.13), pele)], ["spine", "chest", "neck", "leftUpperArm", "rightUpperArm"]),
    "corpo_quadril": ([caixa("q", (0, 0, 0.36), (0.16, 0.11, 0.12), cueca)], ["hips", "spine", "leftUpperLeg", "rightUpperLeg"]),
    "corpo_bracos": ([caixa("bl", (0.165, 0, 0.47), (0.05, 0.05, 0.19), pele), caixa("br", (-0.165, 0, 0.47), (0.05, 0.05, 0.19), pele)],
                     ["leftUpperArm", "leftLowerArm", "rightUpperArm", "rightLowerArm"]),
    "corpo_maos": ([caixa("ml", (0.26, 0, 0.37), (0.05, 0.045, 0.06), pele, 1), caixa("mr", (-0.26, 0, 0.37), (0.05, 0.045, 0.06), pele, 1)],
                   ["leftHand", "rightHand"]),
    "corpo_coxas": ([caixa("cl", (0.06, 0, 0.22), (0.07, 0.08, 0.13), pele), caixa("cr", (-0.06, 0, 0.22), (0.07, 0.08, 0.13), pele)],
                    ["leftUpperLeg", "rightUpperLeg", "leftLowerLeg", "rightLowerLeg"]),
    "corpo_canelas": ([caixa("sl", (0.06, 0, 0.10), (0.06, 0.07, 0.12), pele), caixa("sr", (-0.06, 0, 0.10), (0.06, 0.07, 0.12), pele)],
                      ["leftUpperLeg", "leftLowerLeg", "rightUpperLeg", "rightLowerLeg"]),
    "corpo_pes": ([caixa("pl", (0.06, -0.02, 0.02), (0.08, 0.13, 0.04), pele, 1), caixa("pr", (-0.06, -0.02, 0.02), (0.08, 0.13, 0.04), pele, 1)],
                  ["leftFoot", "rightFoot", "leftLowerLeg", "rightLowerLeg"]),
}
# As caixas dos braços estão em pé; inclinar para a pose A (45 graus).
for nome, (partes, ossos) in regioes.items():
    ob = juntar(nome, partes) if len(partes) > 1 else partes[0]
    ob.name = ob.data.name = nome
    if nome == "corpo_bracos":
        for v in ob.data.vertices:
            lado = 1 if v.co.x > 0 else -1
            ox, oz = 0.165 * lado, 0.47
            dx, dz = v.co.x - ox, v.co.z - oz
            # gira em torno do eixo Y para o braço seguir o osso
            import math
            a = -math.radians(45) * lado
            v.co.x = ox + dx * math.cos(a) + dz * math.sin(a)
            v.co.z = oz - dx * math.sin(a) + dz * math.cos(a)
    pesar_por_distancia(ob, ossos)


def selecionar(objs):
    for o in bpy.context.view_layer.objects:
        o.select_set(False)
    for o in objs:
        o.select_set(True)


def exportar(caminho, objs):
    selecionar(objs)
    bpy.ops.export_scene.gltf(
        filepath=caminho, export_format="GLB", use_selection=True,
        export_yup=True, export_skins=True, export_animations=False,
        export_morph=False, export_apply=False,
    )


corpo = [o for o in cena.objects if o.name.startswith("corpo_")]
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(saida, "base.blend"))
exportar(os.path.join(saida, "base.glb"), [arm] + corpo)

# --- cabeça humana (rígida, presa no osso "head") ------------------------------
cab = caixa("cabeca_humano", (0, 0, 0.80), (0.36, 0.34, 0.36), pele)
me = bpy.data.meshes.new("rosto")
bm = bmesh.new()
# Ordem anti-horária vista de frente (normal para -Y, para fora do rosto).
vs = [bm.verts.new(p) for p in [(-0.14, -0.173, 0.66), (0.14, -0.173, 0.66), (0.14, -0.173, 0.86), (-0.14, -0.173, 0.86)]]
f = bm.faces.new(vs)
uv = bm.loops.layers.uv.new("UVMap")
# UV fixo do rosto: o quadrado 0..1 inteiro. U cresce para a direita de quem olha
# de frente, V para cima (no Blender). A textura do rosto é desenhada de frente.
for loop, c in zip(f.loops, [(0, 0), (1, 0), (1, 1), (0, 1)]):
    loop[uv].uv = c
bm.to_mesh(me)
bm.free()
rosto = bpy.data.objects.new("rosto", me)
rosto.data.materials.append(material("rosto", (1, 1, 1)))
cena.collection.objects.link(rosto)
exportar(os.path.join(saida, "cabeca_humano.glb"), [cab, rosto])

# --- cabelo (rígido) -------------------------------------------------------------
cab_mat = material("cabelo", (0.30, 0.12, 0.12))
cabelo = juntar("cabelo_bob", [
    caixa("topo", (0, 0.01, 0.985), (0.39, 0.37, 0.06), cab_mat, 1),
    caixa("tras", (0, 0.10, 0.82), (0.39, 0.16, 0.34), cab_mat, 1),
    caixa("lado_l", (0.19, -0.04, 0.84), (0.03, 0.22, 0.28), cab_mat, 1),
    caixa("lado_r", (-0.19, -0.04, 0.84), (0.03, 0.22, 0.28), cab_mat, 1),
    caixa("franja", (0, -0.165, 0.93), (0.36, 0.03, 0.08), cab_mat, 1),
])
exportar(os.path.join(saida, "cabelo_bob.glb"), [cabelo])

# --- camiseta crua (sem pesos: o encaixar_peca.py é que pesa) --------------------
camisa_mat = material("camiseta", (0.75, 0.15, 0.12))
camiseta = juntar("camiseta_crua", [
    caixa("ct", (0, 0, 0.495), (0.185, 0.135, 0.14), camisa_mat),
    caixa("ml", (0.12, 0, 0.52), (0.07, 0.07, 0.07), camisa_mat, 2),
    caixa("mr", (-0.12, 0, 0.52), (0.07, 0.07, 0.07), camisa_mat, 2),
])
camiseta.parent = None
selecionar([camiseta])
bpy.ops.export_scene.gltf(filepath=os.path.join(saida, "camiseta_crua.glb"), export_format="GLB",
                          use_selection=True, export_yup=True)
print("ok:", saida)
