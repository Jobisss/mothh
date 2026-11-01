"""Gera um boneco base de MENTIRA (caixas, nas medidas antigas; o corpo de verdade
agora é assets/avatar/base.blend, feito pelo preparar_base.py) com o esqueleto oficial, e duas peças
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
from comum import ANCORAS_CORPO, OSSOS, SOCKETS  # noqa: E402

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


def caixa(nome, centro, tamanho, mat, cortes=3, tubo=False):
    """Caixa subdividida. tubo=True tira as tampas de cima e de baixo (manga, perna de calça)."""
    me = bpy.data.meshes.new(nome)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.subdivide_edges(bm, edges=bm.edges[:], cuts=cortes, use_grid_fill=True)
    if tubo:
        bm.normal_update()
        bmesh.ops.delete(bm, geom=[f for f in bm.faces if abs(f.normal.z) > 0.9], context="FACES_ONLY")
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

import math  # noqa: E402


def braco(nome, cx, cz, comp, cortes=3, larg=0.05, mat=None, tubo=False):
    """Caixa em pé girada 45 graus para a pose A, centrada no meio do osso."""
    ob = caixa(nome, (cx, 0, cz), (larg, larg, comp), mat or pele, cortes, tubo)
    lado = 1 if cx > 0 else -1
    ang = -math.radians(45) * lado
    for v in ob.data.vertices:
        dx, dz = v.co.x - cx, v.co.z - cz
        v.co.x = cx + dx * math.cos(ang) + dz * math.sin(ang)
        v.co.z = cz - dx * math.sin(ang) + dz * math.cos(ang)
    return ob


PERNAS = ["leftUpperLeg", "leftLowerLeg", "rightUpperLeg", "rightLowerLeg"]
regioes = {
    "corpo_pescoco": ([caixa("pc", (0, 0, 0.59), (0.07, 0.07, 0.05), pele, 1)], ["neck", "chest"]),
    "corpo_peito": ([caixa("pt", (0, 0, 0.52), (0.17, 0.12, 0.08), pele)], ["chest", "spine", "leftUpperArm", "rightUpperArm"]),
    "corpo_barriga": ([caixa("br", (0, 0, 0.44), (0.165, 0.115, 0.08), pele)], ["spine", "chest", "hips"]),
    "corpo_quadril": ([caixa("q", (0, 0, 0.35), (0.16, 0.11, 0.10), cueca)], ["hips", "spine", "leftUpperLeg", "rightUpperLeg"]),
    "corpo_braco_sup": ([braco("bsl", 0.1275, 0.5075, 0.11), braco("bsr", -0.1275, 0.5075, 0.11)],
                        ["leftUpperArm", "leftLowerArm", "rightUpperArm", "rightLowerArm", "chest"]),
    "corpo_antebraco": ([braco("abl", 0.20, 0.435, 0.10), braco("abr", -0.20, 0.435, 0.10)],
                        ["leftUpperArm", "leftLowerArm", "leftHand", "rightUpperArm", "rightLowerArm", "rightHand"]),
    "corpo_maos": ([caixa("ml", (0.26, 0, 0.37), (0.05, 0.045, 0.06), pele, 1), caixa("mr", (-0.26, 0, 0.37), (0.05, 0.045, 0.06), pele, 1)],
                   ["leftHand", "rightHand"]),
    "corpo_coxas": ([caixa("cl", (0.06, 0, 0.22), (0.07, 0.08, 0.13), pele), caixa("cr", (-0.06, 0, 0.22), (0.07, 0.08, 0.13), pele)], PERNAS),
    "corpo_canelas": ([caixa("sl", (0.06, 0, 0.10), (0.06, 0.07, 0.12), pele), caixa("sr", (-0.06, 0, 0.10), (0.06, 0.07, 0.12), pele)], PERNAS),
    "corpo_pes": ([caixa("pl", (0.06, -0.02, 0.02), (0.08, 0.13, 0.04), pele, 1), caixa("pr", (-0.06, -0.02, 0.02), (0.08, 0.13, 0.04), pele, 1)],
                  ["leftFoot", "rightFoot", "leftLowerLeg", "rightLowerLeg"]),
}
for nome, (partes, ossos) in regioes.items():
    ob = juntar(nome, partes) if len(partes) > 1 else partes[0]
    ob.name = ob.data.name = nome
    pesar_por_distancia(ob, ossos)


def ancora(nome, pos, osso=None):
    """Empty que marca um ponto de encaixe. Com osso, fica pendurado nele."""
    e = bpy.data.objects.new(nome, None)
    e.empty_display_type = "SPHERE"
    e.empty_display_size = 0.01
    cena.collection.objects.link(e)
    if osso:
        e.parent = arm
        e.parent_type = "BONE"
        e.parent_bone = osso
    e.matrix_world.translation = Vector(pos)
    return e


bpy.context.view_layer.update()
ancoras_corpo = [ancora(n, pos, osso) for n, (osso, pos) in ANCORAS_CORPO.items()]


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
exportar(os.path.join(saida, "base.glb"), [arm] + corpo + ancoras_corpo)

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
# Âncoras da cabeça (ficam dentro do .glb da cabeça; cada cabeça tem as suas).
anc_cab = [ancora(n, p) for n, p in {
    "anc_topo": (0, 0, 0.98), "anc_testa": (0, -0.17, 0.90), "anc_orelha_l": (0.18, 0, 0.80),
    "anc_orelha_r": (-0.18, 0, 0.80), "anc_nuca": (0, 0.17, 0.75), "anc_olhos": (0, -0.17, 0.78),
}.items()]
exportar(os.path.join(saida, "cabeca_humano.glb"), [cab, rosto] + anc_cab)

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
    braco("ml", 0.12, 0.515, 0.07, 2, 0.068, camisa_mat, tubo=True),
    braco("mr", -0.12, 0.515, 0.07, 2, 0.068, camisa_mat, tubo=True),
])
camiseta.parent = None
selecionar([camiseta])
bpy.ops.export_scene.gltf(filepath=os.path.join(saida, "camiseta_crua.glb"), export_format="GLB",
                          use_selection=True, export_yup=True)
# --- moletom largo cru (testa o perfil "larga") ---------------------------------
mol_mat = material("moletom", (0.35, 0.25, 0.40))
moletom = juntar("moletom_cru", [
    caixa("mt", (0, 0, 0.47), (0.21, 0.16, 0.20), mol_mat),
    braco("mbl", 0.135, 0.50, 0.13, 3, 0.085, mol_mat, True), braco("mbr", -0.135, 0.50, 0.13, 3, 0.085, mol_mat, True),
    braco("mal", 0.205, 0.43, 0.11, 3, 0.10, mol_mat, True), braco("mar", -0.205, 0.43, 0.11, 3, 0.10, mol_mat, True),
])
selecionar([moletom])
bpy.ops.export_scene.gltf(filepath=os.path.join(saida, "moletom_cru.glb"), export_format="GLB",
                          use_selection=True, export_yup=True)
print("ok:", saida)
