"""Transforma o corpo gerado no Tripo/Meshy no boneco base do mothh.

    blender --background --python tools/avatar/blender/preparar_base.py -- ^
        assets/models/modelo-base.glb SAIDA_DIR [--tris 3800] [--juntas juntas.json]

Entra: um .glb do corpo pelado, 1,0 m, pés no chão, olhando para -Y (o Tripo já
exporta assim quando a referência é de frente).
Sai em SAIDA_DIR:
  base.blend         arquivo mestre (rig + 10 regiões + âncoras do corpo)
  base.glb           o mesmo, para o app
  cabeca_humano.glb  cabeça separada, com o objeto "rosto" (UV 0..1) e as 6 âncoras

Passos: reduz triângulos, cria o rig oficial nas juntas medidas do corpo,
pesa (pesos automáticos do Blender; se falhar, por distância), corta a cabeça,
separa o corpo nas 10 regiões pelo osso mais próximo e cria as âncoras.
As posições das juntas (JUNTAS) foram medidas no modelo-base.glb de 01/11; para
outro corpo, passe --juntas com um JSON no mesmo formato.
"""
import json
import os
import sys

import bpy  # noqa: I001
import bmesh
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import ANCORAS_CABECA, OSSOS  # noqa: E402

# nome: (cabeça, cauda), em metros, Blender (Z para cima, frente -Y, esquerda +X).
JUNTAS = {
    "hips": ((0, 0, 0.37), (0, 0, 0.43)),
    "spine": ((0, 0, 0.43), (0, 0, 0.51)),
    "chest": ((0, 0, 0.51), (0, 0, 0.60)),
    "neck": ((0, 0, 0.60), (0, 0, 0.67)),
    "head": ((0, 0, 0.67), (0, 0, 1.0)),
    "leftUpperArm": ((0.085, 0, 0.585), (0.16, 0, 0.51)),
    "leftLowerArm": ((0.16, 0, 0.51), (0.215, 0, 0.45)),
    "leftHand": ((0.215, 0, 0.45), (0.26, 0, 0.385)),
    "leftUpperLeg": ((0.07, 0, 0.36), (0.07, 0, 0.20)),
    "leftLowerLeg": ((0.07, 0, 0.20), (0.07, 0, 0.08)),
    "leftFoot": ((0.07, 0, 0.08), (0.07, -0.12, 0.02)),
}
SOCKETS = {
    "sock_mao_l": ("leftHand", (0.26, 0, 0.385), (0.26, 0, 0.35)),
    "sock_mao_r": ("rightHand", (-0.26, 0, 0.385), (-0.26, 0, 0.35)),
    "sock_costas": ("chest", (0, 0.07, 0.52), (0, 0.07, 0.58)),
    "sock_quadril": ("hips", (0, -0.07, 0.40), (0, -0.07, 0.45)),
}
ANCORAS_CORPO = {
    "anc_pescoco": ("neck", (0, 0, 0.63)),
    "anc_ombro_l": ("chest", (0.09, 0, 0.60)),
    "anc_ombro_r": ("chest", (-0.09, 0, 0.60)),
    "anc_cintura": ("hips", (0, 0, 0.42)),
    "anc_tornozelo_l": ("leftFoot", (0.07, 0, 0.08)),
    "anc_tornozelo_r": ("rightFoot", (-0.07, 0, 0.08)),
}
ANCORAS_CAB = {
    "anc_topo": (0, 0, 1.0), "anc_testa": (0, -0.135, 0.90), "anc_orelha_l": (0.172, 0, 0.80),
    "anc_orelha_r": (-0.172, 0, 0.80), "anc_nuca": (0, 0.145, 0.80), "anc_olhos": (0, -0.14, 0.80),
}
CORTE_CABECA = 0.67      # faces acima disso viram a cabeça (rígida)
ROSTO = (-0.12, 0.12, 0.70, 0.94)  # x0, x1, z0, z1 do quadrado do rosto (vista de frente)

OSSO_REGIAO = {
    "hips": "corpo_quadril", "spine": "corpo_barriga", "chest": "corpo_peito", "neck": "corpo_pescoco",
    "UpperArm": "corpo_braco_sup", "LowerArm": "corpo_antebraco", "Hand": "corpo_maos",
    "UpperLeg": "corpo_coxas", "LowerLeg": "corpo_canelas", "Foot": "corpo_pes",
}


def espelhar(juntas):
    out = dict(juntas)
    for n, (h, t) in juntas.items():
        if n.startswith("left"):
            out["right" + n[4:]] = ((-h[0], h[1], h[2]), (-t[0], t[1], t[2]))
    return out


def regiao_do_osso(osso):
    for chave, reg in OSSO_REGIAO.items():
        if osso == chave or osso.endswith(chave):
            return reg
    return None


def ctx(ob, objs=None):
    objs = objs or [ob]
    return bpy.context.temp_override(object=ob, active_object=ob, selected_objects=objs,
                                     selected_editable_objects=objs)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    entrada, saida = argv[0], argv[1]
    alvo_tris = int(argv[argv.index("--tris") + 1]) if "--tris" in argv else 3800
    juntas = JUNTAS
    if "--juntas" in argv:
        with open(argv[argv.index("--juntas") + 1], encoding="utf-8") as f:
            juntas = {k: tuple(map(tuple, v)) for k, v in json.load(f).items()}
    juntas = espelhar(juntas)
    faltam = {n for n, *_ in OSSOS} - set(juntas)
    assert not faltam, f"faltam juntas: {faltam}"
    os.makedirs(saida, exist_ok=True)

    bpy.ops.wm.read_factory_settings(use_empty=True)
    cena = bpy.context.scene
    bpy.ops.import_scene.gltf(filepath=entrada)
    malhas = [o for o in cena.objects if o.type == "MESH"]
    for o in list(cena.objects):
        if o.type != "MESH":
            bpy.data.objects.remove(o)
    if len(malhas) > 1:
        with ctx(malhas[0], malhas):
            bpy.ops.object.join()
    corpo = malhas[0]
    corpo.name = corpo.data.name = "corpo"
    corpo.parent = None
    with ctx(corpo):
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)

    # 1. confere orientação e escala
    zs = [v.co.z for v in corpo.data.vertices]
    altura = max(zs) - min(zs)
    print(f"altura {altura:.3f} m, pés em z={min(zs):.3f}")
    assert 0.9 < altura < 1.1 and abs(min(zs)) < 0.02, "o corpo precisa ter ~1,0 m e pés em z=0"

    # 2. reduz triângulos e junta vértices soltos
    bm = bmesh.new()
    bm.from_mesh(corpo.data)
    bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=0.0005)
    bm.to_mesh(corpo.data)
    bm.free()
    tris = sum(len(p.vertices) - 2 for p in corpo.data.polygons)
    if tris > alvo_tris:
        d = corpo.modifiers.new("dec", "DECIMATE")
        d.ratio = alvo_tris / tris
        d.use_symmetry = True
        d.symmetry_axis = "X"
        with ctx(corpo):
            bpy.ops.object.modifier_apply(modifier=d.name)
    print(f"triângulos: {tris} -> {sum(len(p.vertices) - 2 for p in corpo.data.polygons)}")

    pele = bpy.data.materials.new("pele")
    pele.use_nodes = True  # noqa (aviso de depreciação do Blender 5)
    pb = pele.node_tree.nodes["Principled BSDF"]
    pb.inputs["Base Color"].default_value = (0.86, 0.6, 0.45, 1)
    pb.inputs["Roughness"].default_value = 1.0
    corpo.data.materials.clear()
    corpo.data.materials.append(pele)
    for p in corpo.data.polygons:
        p.use_smooth = True

    # 3. rig oficial nas juntas medidas
    arm_data = bpy.data.armatures.new("rig")
    rig = bpy.data.objects.new("rig", arm_data)
    cena.collection.objects.link(rig)
    bpy.context.view_layer.objects.active = rig
    with ctx(rig):
        bpy.ops.object.mode_set(mode="EDIT")
        for nome, pai, *_ in OSSOS:
            b = arm_data.edit_bones.new(nome)
            b.head, b.tail = Vector(juntas[nome][0]), Vector(juntas[nome][1])
            if pai:
                b.parent = arm_data.edit_bones[pai]
        for nome, (pai, h, t) in SOCKETS.items():
            b = arm_data.edit_bones.new(nome)
            b.head, b.tail = Vector(h), Vector(t)
            b.parent = arm_data.edit_bones[pai]
            b.use_deform = False
        bpy.ops.object.mode_set(mode="OBJECT")

    # 4. corta a cabeça (vira peça rígida do slot "cabeca")
    cabeca = corpo.copy()
    cabeca.data = corpo.data.copy()
    cabeca.name = cabeca.data.name = "cabeca_humano"
    cena.collection.objects.link(cabeca)
    for ob, manter in ((corpo, lambda c: c.z <= CORTE_CABECA), (cabeca, lambda c: c.z > CORTE_CABECA)):
        bm = bmesh.new()
        bm.from_mesh(ob.data)
        bmesh.ops.delete(bm, geom=[f for f in bm.faces if not manter(f.calc_center_median())], context="FACES")
        bm.to_mesh(ob.data)
        bm.free()

    # 5. rosto: cópia das faces da frente da cara, 1,5 mm para fora, UV plano 0..1
    x0, x1, z0, z1 = ROSTO
    rosto = cabeca.copy()
    rosto.data = cabeca.data.copy()
    rosto.name = rosto.data.name = "rosto"
    cena.collection.objects.link(rosto)
    bm = bmesh.new()
    bm.from_mesh(rosto.data)
    bm.normal_update()

    def na_cara(f):
        c = f.calc_center_median()
        return x0 <= c.x <= x1 and z0 <= c.z <= z1 and f.normal.y < -0.25

    bmesh.ops.delete(bm, geom=[f for f in bm.faces if not na_cara(f)], context="FACES")
    for v in bm.verts:
        v.co += v.normal * 0.0015
    uv = bm.loops.layers.uv.new("UVMap")
    for f in bm.faces:
        for lp in f.loops:
            lp[uv].uv = ((lp.vert.co.x - x0) / (x1 - x0), (lp.vert.co.z - z0) / (z1 - z0))
    bm.to_mesh(rosto.data)
    bm.free()
    mat_rosto = bpy.data.materials.new("rosto")
    rosto.data.materials.clear()
    rosto.data.materials.append(mat_rosto)
    print(f"rosto: {len(rosto.data.polygons)} faces")

    # 6. pesos: automático do Blender; se falhar, por distância ao osso
    for o in bpy.context.view_layer.objects:
        o.select_set(False)
    corpo.select_set(True)
    rig.select_set(True)
    bpy.context.view_layer.objects.active = rig
    try:
        with bpy.context.temp_override(active_object=rig, object=rig, selected_objects=[corpo, rig],
                                       selected_editable_objects=[corpo, rig]):
            bpy.ops.object.parent_set(type="ARMATURE_AUTO")
        sem = sum(1 for v in corpo.data.vertices if not v.groups)
        assert sem < len(corpo.data.vertices) * 0.01, f"{sem} vértices sem peso"
        metodo = "automático"
    except Exception as e:  # noqa: BLE001
        print("pesos automáticos falharam:", e)
        corpo.vertex_groups.clear()
        pesar_distancia(corpo, juntas)
        corpo.parent = rig
        corpo.modifiers.new("Armature", "ARMATURE").object = rig
        metodo = "distância"
    with ctx(corpo):
        bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=4)
        bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)
    print("pesos:", metodo)

    # 7. regiões: cada face vai para a região do osso mais próximo do seu centro
    segs = {n: (Vector(h), Vector(t)) for n, (h, t) in juntas.items()}

    def osso_mais_perto(p):
        melhor, dmin = None, 9
        for n, (a, b) in segs.items():
            ab = b - a
            t = max(0.0, min(1.0, (p - a).dot(ab) / ab.length_squared))
            d = (a + ab * t - p).length
            if d < dmin:
                melhor, dmin = n, d
        return melhor

    por_regiao = {}
    for p in corpo.data.polygons:
        reg = regiao_do_osso(osso_mais_perto(p.center))
        por_regiao.setdefault(reg, []).append(p.index)
    regioes = []
    for reg, faces in sorted(por_regiao.items()):
        ob = corpo.copy()
        ob.data = corpo.data.copy()
        ob.name = ob.data.name = reg
        cena.collection.objects.link(ob)
        manter = set(faces)
        bm = bmesh.new()
        bm.from_mesh(ob.data)
        bm.faces.ensure_lookup_table()
        bmesh.ops.delete(bm, geom=[f for f in bm.faces if f.index not in manter], context="FACES")
        bm.to_mesh(ob.data)
        bm.free()
        ob.parent = rig
        ob.matrix_parent_inverse.identity()
        if not any(m.type == "ARMATURE" for m in ob.modifiers):
            ob.modifiers.new("Armature", "ARMATURE").object = rig
        regioes.append(ob)
        print(f"  {reg}: {len(ob.data.polygons)} faces")
    bpy.data.objects.remove(corpo)

    # 8. âncoras
    def ancora(nome, pos, osso=None):
        e = bpy.data.objects.new(nome, None)
        e.empty_display_type = "SPHERE"
        e.empty_display_size = 0.01
        cena.collection.objects.link(e)
        if osso:
            e.parent = rig
            e.parent_type = "BONE"
            e.parent_bone = osso
        bpy.context.view_layer.update()
        e.matrix_world.translation = Vector(pos)
        return e

    anc_corpo = [ancora(n, p, o) for n, (o, p) in ANCORAS_CORPO.items()]
    anc_cab = [ancora(n, p) for n, p in ANCORAS_CAB.items()]
    assert set(ANCORAS_CAB) == set(ANCORAS_CABECA)

    def exportar(caminho, objs):
        for o in bpy.context.view_layer.objects:
            o.select_set(o in objs)
        bpy.ops.export_scene.gltf(filepath=caminho, export_format="GLB", use_selection=True, export_yup=True,
                                  export_skins=True, export_animations=False, export_apply=False)

    # cabeça e rosto vão num arquivo à parte (slot "cabeca")
    exportar(os.path.join(saida, "cabeca_humano.glb"), [cabeca, rosto] + anc_cab)
    for o in [cabeca, rosto] + anc_cab:
        bpy.data.objects.remove(o)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(os.path.join(saida, "base.blend")))
    exportar(os.path.join(saida, "base.glb"), [rig] + regioes + anc_corpo)
    print("ok:", saida)


def pesar_distancia(ob, juntas):
    segs = {n: (Vector(h), Vector(t)) for n, (h, t) in juntas.items()}
    grupos = {n: ob.vertex_groups.new(name=n) for n in segs}
    for v in ob.data.vertices:
        ds = []
        for n, (a, b) in segs.items():
            ab = b - a
            t = max(0.0, min(1.0, (v.co - a).dot(ab) / ab.length_squared))
            ds.append((max((a + ab * t - v.co).length, 1e-4), n))
        ds.sort()
        ws = [(1.0 / d ** 4, n) for d, n in ds[:3]]
        s = sum(w for w, _ in ws)
        for w, n in ws:
            grupos[n].add([v.index], w / s, "REPLACE")


main()
