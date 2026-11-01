"""Transforma o corpo gerado no Tripo/Meshy no boneco base do mothh.

    blender --background --python tools/avatar/blender/preparar_base.py -- ^
        assets/models/modelo-base-with-rig.glb SAIDA_DIR [--tris 3800] [--juntas juntas.json]
        [--refazer-pesos]

Entra: um .glb do corpo pelado, 1,0 m, pés no chão, olhando para -Y (o Tripo já
exporta assim quando a referência é de frente). Pode vir sem rig ou com o rig
automático do Tripo/Mixamo (ossos "mixamorig:*").
Sai em SAIDA_DIR:
  base.blend         arquivo mestre (rig + 10 regiões + âncoras do corpo)
  base.glb           o mesmo, para o app
  cabeca_humano.glb  cabeça separada, com o objeto "rosto" (UV 0..1) e as 6 âncoras
  juntas.json        onde ficaram as juntas (para conferir com o comum.py)

Passos: reduz triângulos, cria o rig oficial (17 ossos), pesa, corta a cabeça,
separa o corpo nas 10 regiões pelo osso mais próximo e cria as âncoras.

Com rig Mixamo: as juntas saem do próprio rig (simetrizadas) e os pesos dele são
aproveitados, juntando os ossos que o mothh não tem (dedos -> mão, clavícula ->
peito, Spine1/Spine2 -> chest, dedão -> pé). --refazer-pesos ignora os pesos dele.
Sem rig: as juntas vêm do comum.py (ou de --juntas) e os pesos são automáticos
(se falhar, por distância).
"""
import json
import os
import sys

import bpy  # noqa: I001
import bmesh
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import ANCORAS_CABECA, ANCORAS_CORPO, MIXAMO, OSSOS, nome_mixamo, osso_dos_pesos  # noqa: E402
from comum import SOCKETS as SOCKETS_C  # noqa: E402

# Juntas, sockets e âncoras do corpo vêm do comum.py (contrato). Para um corpo
# com outras medidas, passe --juntas com um JSON {"osso": [[x,y,z],[x,y,z]], ...}.
JUNTAS = {n: (h, t) for n, _, h, t in OSSOS if not n.startswith("right")}
SOCKETS = {n: (pai, h, t) for n, pai, h, t in SOCKETS_C}
ANCORAS_CAB = {
    "anc_topo": (0, 0, 1.0), "anc_testa": (0, -0.135, 0.90), "anc_orelha_l": (0.172, 0, 0.80),
    "anc_orelha_r": (-0.172, 0, 0.80), "anc_nuca": (0, 0.145, 0.80), "anc_olhos": (0, -0.14, 0.80),
}
CORTE_CABECA = 0.71      # faces acima disso viram a cabeça (rígida): parte mais fina do pescoço
ROSTO = (-0.12, 0.12, 0.70, 0.94)  # x0, x1, z0, z1 do quadrado do rosto (vista de frente)

OSSO_REGIAO = {
    "hips": "corpo_quadril", "spine": "corpo_barriga", "chest": "corpo_peito", "neck": "corpo_pescoco",
    "head": "corpo_pescoco",  # o que sobra do pescoço abaixo do corte da cabeça
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
    os.makedirs(saida, exist_ok=True)

    bpy.ops.wm.read_factory_settings(use_empty=True)
    cena = bpy.context.scene
    bpy.ops.import_scene.gltf(filepath=entrada)
    rig_src = next((o for o in cena.objects if o.type == "ARMATURE"), None)
    # o importador cria uma malha de enfeite para desenhar os ossos: não é corpo
    formas = {pb.custom_shape for o in cena.objects if o.type == "ARMATURE" for pb in o.pose.bones}
    malhas = [o for o in cena.objects if o.type == "MESH" and o not in formas]

    juntas = JUNTAS
    pesos_prontos = False
    if rig_src:
        juntas = juntas_do_mixamo(rig_src, malhas)
        print("rig Mixamo: juntas medidas no rig")
        if "--refazer-pesos" not in argv:
            for m in malhas:
                converter_pesos(m)
            pesos_prontos = True
    if "--juntas" in argv:
        with open(argv[argv.index("--juntas") + 1], encoding="utf-8") as f:
            juntas = {k: tuple(map(tuple, v)) for k, v in json.load(f).items()}
    juntas = espelhar(juntas)
    faltam = {n for n, *_ in OSSOS} - set(juntas)
    assert not faltam, f"faltam juntas: {faltam}"
    conferir_contrato(juntas)
    with open(os.path.join(saida, "juntas.json"), "w", encoding="utf-8") as f:
        json.dump({n: [list(h), list(t)] for n, (h, t) in juntas.items()}, f, indent=1)

    # tira as malhas do rig importado sem mexer onde elas estão no mundo
    for m in malhas:
        mw = m.matrix_world.copy()
        m.parent = None
        m.matrix_world = mw
        for md in list(m.modifiers):
            m.modifiers.remove(md)
    for o in list(cena.objects):
        if o not in malhas:
            bpy.data.objects.remove(o)
    if len(malhas) > 1:
        with ctx(malhas[0], malhas):
            bpy.ops.object.join()
    corpo = malhas[0]
    corpo.name = corpo.data.name = "corpo"
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
    tris = contar_tris(corpo)
    reduzir_maos_e_pes(corpo, juntas, alvo_tris)
    print(f"triângulos (com a cabeça, antes de reduzir a cabeça): {tris} -> {contar_tris(corpo)}")

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
        # corta no plano exato: sem isso, triângulos compridos deixam pontas acima do pescoço
        bmesh.ops.bisect_plane(bm, geom=bm.verts[:] + bm.edges[:] + bm.faces[:],
                               plane_co=(0, 0, CORTE_CABECA), plane_no=(0, 0, 1))
        bmesh.ops.delete(bm, geom=[f for f in bm.faces if not manter(f.calc_center_median())], context="FACES")
        bm.to_mesh(ob.data)
        bm.free()

    cabeca.vertex_groups.clear()  # peça rígida: segue o osso head inteira
    if contar_tris(cabeca) > TRIS_CABECA:
        antes = contar_tris(cabeca)
        decimar(cabeca, TRIS_CABECA / antes)
        print(f"cabeça: {antes} -> {contar_tris(cabeca)} triângulos")

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

    # 6. pesos: os do rig pronto; senão automático do Blender; se falhar, por distância ao osso
    for o in bpy.context.view_layer.objects:
        o.select_set(False)
    corpo.select_set(True)
    rig.select_set(True)
    bpy.context.view_layer.objects.active = rig
    if pesos_prontos:
        corpo.parent = rig
        corpo.modifiers.new("Armature", "ARMATURE").object = rig
        sem = sum(1 for v in corpo.data.vertices if not v.groups)
        assert sem < len(corpo.data.vertices) * 0.01, f"{sem} vértices sem peso"
        metodo = "do rig pronto (Mixamo)"
    else:
        metodo = pesar_auto(corpo, rig, juntas)
    with ctx(corpo):
        bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=4)
        bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)
    print("pesos:", metodo)


    # 7. regiões: cada face vai para a região do osso que mais pesa nos vértices dela
    #    (pelo centro da face, triângulos compridos do braço caíam no antebraço)
    nomes_grupo = {g.index: g.name for g in corpo.vertex_groups}
    por_regiao = {}
    for p in corpo.data.polygons:
        soma = {}
        for i in p.vertices:
            for g in corpo.data.vertices[i].groups:
                n = nomes_grupo[g.group]
                soma[n] = soma.get(n, 0.0) + g.weight
        osso = max(soma, key=soma.get) if soma else osso_mais_perto(p.center, juntas)
        reg = regiao_do_osso(osso)
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


TRIS_CABECA = 1000  # alvo da cabeça humana (slot cabeca: alvo 900, máximo 1400)


def contar_tris(ob):
    return sum(len(p.vertices) - 2 for p in ob.data.polygons)


def osso_mais_perto(p, juntas):
    melhor, dmin = None, 9
    for n, (a, b) in juntas.items():
        a, b = Vector(a), Vector(b)
        ab = b - a
        t = max(0.0, min(1.0, (p - a).dot(ab) / ab.length_squared))
        d = (a + ab * t - p).length
        if d < dmin:
            melhor, dmin = n, d
    return melhor


def decimar(ob, ratio, faces=None):
    """Reduz triângulos (simétrico em X). Com `faces`, só nessas faces: o resto fica igual."""
    for p in ob.data.polygons:
        p.select = faces is None or p.index in faces
    for v in ob.data.vertices:
        v.select = False
    for e in ob.data.edges:
        e.select = False
    for p in ob.data.polygons:
        if p.select:
            for i in p.vertices:
                ob.data.vertices[i].select = True
    bpy.context.view_layer.objects.active = ob
    with ctx(ob):
        bpy.ops.object.mode_set(mode="EDIT")  # a seleção vem das faces marcadas acima
        bpy.ops.mesh.decimate(ratio=ratio, use_symmetry=True, symmetry_axis="X")
        bpy.ops.object.mode_set(mode="OBJECT")


def reduzir_maos_e_pes(corpo, juntas, alvo_tris):
    """Chega perto de `alvo_tris` (com a cabeça já no alvo dela) reduzindo só mãos e pés.

    Braços, pernas e tronco são tubos com poucos triângulos: reduzir ali cria lascas
    que vão do ombro ao antebraço, dobram mal e bagunçam as regiões. Os dedos e os
    pés redondos é que têm triângulo sobrando.
    """
    grupos = {g.index: g.name for g in corpo.vertex_groups}

    def dono(v):
        soma = {}
        for g in v.groups:
            soma[grupos[g.group]] = soma.get(grupos[g.group], 0.0) + g.weight
        return max(soma, key=soma.get) if soma else osso_mais_perto(v.co, juntas)

    donos = [dono(v) or "" for v in corpo.data.vertices]
    zona = {}
    for p in corpo.data.polygons:
        if p.center.z > CORTE_CABECA:
            zona[p.index] = "cabeca"
        elif all(donos[i].endswith("Hand") for i in p.vertices):
            zona[p.index] = "mao"
        elif all(donos[i].endswith("Foot") for i in p.vertices):
            zona[p.index] = "pe"
        else:
            zona[p.index] = "resto"
    t = {z: sum(len(corpo.data.polygons[i].vertices) - 2 for i, zz in zona.items() if zz == z)
         for z in ("cabeca", "mao", "pe", "resto")}
    sobra = alvo_tris - min(t["cabeca"], TRIS_CABECA) - t["resto"]
    ratio = max(0.15, min(1.0, sobra / max(t["mao"] + t["pe"], 1)))
    print(f"triângulos por zona: {t}; mãos e pés x{ratio:.2f}")
    if ratio < 1.0:
        decimar(corpo, ratio, {i for i, z in zona.items() if z in ("mao", "pe")})


def pesar_auto(corpo, rig, juntas):
    """Pesos automáticos do Blender; se falharem, por distância ao osso."""
    try:
        with bpy.context.temp_override(active_object=rig, object=rig, selected_objects=[corpo, rig],
                                       selected_editable_objects=[corpo, rig]):
            bpy.ops.object.parent_set(type="ARMATURE_AUTO")
        sem = sum(1 for v in corpo.data.vertices if not v.groups)
        assert sem < len(corpo.data.vertices) * 0.01, f"{sem} vértices sem peso"
        return "automático"
    except Exception as e:  # noqa: BLE001
        print("pesos automáticos falharam:", e)
        corpo.vertex_groups.clear()
        pesar_distancia(corpo, juntas)
        corpo.parent = rig
        corpo.modifiers.new("Armature", "ARMATURE").object = rig
        return "distância"


def juntas_do_mixamo(rig, malhas):
    """Juntas do mothh medidas num rig Mixamo (mundo do Blender), já simétricas.

    Cada osso vai da junta Mixamo correspondente até a do filho (comum.MIXAMO).
    O pescoço termina no corte da cabeça (parte mais fina), para a cabeça rígida
    girar em volta da emenda, e a cabeça termina no topo da malha.
    """
    pos = {}
    for b in rig.data.bones:
        pos[nome_mixamo(b.name)] = rig.matrix_world @ b.head_local
        if not b.children:  # ponta (HeadTop_End, dedos...): vale também a cauda
            pos.setdefault(nome_mixamo(b.name) + "#cauda", rig.matrix_world @ b.tail_local)
    faltam = {j for _, a, b in MIXAMO.values() for j in (a, b)} - set(pos)
    assert not faltam, f"o rig não parece Mixamo, faltam: {sorted(faltam)}"
    topo = max((m.matrix_world @ v.co).z for m in malhas for v in m.data.vertices)

    juntas = {n: (pos[a].copy(), pos[b].copy()) for n, (_, a, b) in MIXAMO.items()}
    pescoco, cabeca = pos["Neck"], pos["Head"]
    t = (CORTE_CABECA - pescoco.z) / (cabeca.z - pescoco.z)
    corte = pescoco.lerp(cabeca, max(0.0, min(1.0, t)))
    juntas["neck"] = (pescoco, corte)
    juntas["head"] = (corte, Vector((corte.x, pos["HeadTop_End"].y, topo)))

    # simetria: centro em x=0; esquerda = média da esquerda com o espelho da direita
    def esp(v):
        return Vector((-v.x, v.y, v.z))

    out = {}
    for n, (h, t) in juntas.items():
        if n.startswith("right"):
            continue
        if n.startswith("left"):
            hr, tr = juntas["right" + n[4:]]
            h, t = (h + esp(hr)) / 2, (t + esp(tr)) / 2
        else:
            h, t = Vector((0, h.y, h.z)), Vector((0, t.y, t.z))
        out[n] = (tuple(round(c, 3) for c in h), tuple(round(c, 3) for c in t))
    return out


def converter_pesos(ob):
    """Renomeia os grupos Mixamo para os ossos do mothh, somando os que viram um só."""
    nomes = {g.index: g.name for g in ob.vertex_groups}
    destino = {i: osso_dos_pesos(n) for i, n in nomes.items()}
    perdidos = sorted({nomes[i] for i, d in destino.items() if d is None})
    if perdidos:
        print("aviso: grupos sem osso no mothh (pesos descartados):", perdidos)
    pesos = []
    for v in ob.data.vertices:
        acc = {}
        for g in v.groups:
            d = destino.get(g.group)
            if d and g.weight > 0:
                acc[d] = acc.get(d, 0.0) + g.weight
        pesos.append(acc)
    ob.vertex_groups.clear()
    grupos = {n: ob.vertex_groups.new(name=n) for n in sorted({d for d in destino.values() if d})}
    for i, acc in enumerate(pesos):
        for n, w in acc.items():
            grupos[n].add([i], w, "REPLACE")
    print(f"pesos convertidos: {len(nomes)} grupos Mixamo -> {len(grupos)} ossos do mothh")


def conferir_contrato(juntas, tol=0.01):
    """Avisa se as juntas usadas fogem das do comum.py (o contrato das peças)."""
    longe = []
    for n, _, h, t in OSSOS:
        for a, b in ((h, juntas[n][0]), (t, juntas[n][1])):
            d = (Vector(a) - Vector(b)).length
            if d > tol:
                longe.append(f"{n} {d * 100:.1f} cm")
    if longe:
        print("AVISO: juntas diferentes do comum.py (atualize o contrato ou passe --juntas):")
        for x in longe:
            print("  ", x)
    else:
        print("juntas batem com o comum.py")


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
