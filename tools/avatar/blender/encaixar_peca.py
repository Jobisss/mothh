"""Encaixa uma peça gerada (Tripo/Meshy/feita à mão) no boneco base do mothh.

    blender --background --python tools/avatar/blender/encaixar_peca.py -- \
        --base base.blend --peca cabelo_gerado.glb --slot cabelo --nome cabelo_bob \
        --saida pecas/ [--alinhar] [--tris 800] [--esconde corpo_tronco,corpo_bracos]

O que faz:
 1. abre o base.blend e importa a peça (.glb, .gltf, .fbx ou .obj);
 2. junta tudo num objeto só, apaga esqueletos/empties que vieram junto, aplica escala;
 3. (--alinhar) encaixa pela caixa: escala e posiciona sobre a região do slot;
 4. reduz para o orçamento de triângulos do slot (ou --tris);
 5. deixa as texturas com filtro "Closest" (pixel nítido);
 6. peça skinada: copia os pesos do corpo (vértice mais próximo) e liga no rig;
    peça rígida: só exporta no lugar (o app prende no osso do slot);
 7. confere tudo e exporta <nome>.glb + <nome>.json (manifesto).
"""
import argparse
import json
import os
import sys

import bpy
from mathutils import Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import NOMES_OSSOS, ORCAMENTO_TRIS, REGIOES, SLOTS  # noqa: E402

# Região do corpo usada para alinhar cada slot (caixa de referência).
ALVO_ALINHAR = {
    "cabeca": ["@head"], "cabelo": ["@head"], "cabeca_acc": ["@head"],
    "superior": ["corpo_tronco", "corpo_bracos"], "inferior": ["corpo_quadril", "corpo_coxas", "corpo_canelas"],
    "conjunto": ["corpo_tronco", "corpo_bracos", "corpo_quadril", "corpo_coxas", "corpo_canelas"],
    "meias": ["corpo_canelas", "corpo_pes"], "calcados": ["corpo_pes"], "cintura": ["corpo_quadril"],
}
# Caixa da cabeça padrão (todas as cabeças cabem nela; chapéus são feitos para ela).
CAIXA_CABECA = (Vector((-0.18, -0.17, 0.62)), Vector((0.18, 0.17, 0.98)))


def args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--base")
    p.add_argument("--peca", required=True)
    p.add_argument("--slot", required=True, choices=sorted(SLOTS))
    p.add_argument("--nome", required=True)
    p.add_argument("--saida", default=".")
    p.add_argument("--alinhar", action="store_true")
    p.add_argument("--tris", type=int)
    p.add_argument("--esconde", default="")
    p.add_argument("--folga", type=float, default=0.004, help="empurra a peça skinada para fora (m)")
    return p.parse_args(argv)


def caixa_mundo(objs):
    pts = [o.matrix_world @ Vector(c) for o in objs for c in o.bound_box]
    return (Vector(min(p[i] for p in pts) for i in range(3)), Vector(max(p[i] for p in pts) for i in range(3)))


def tris(ob):
    return sum(len(p.vertices) - 2 for p in ob.data.polygons)


def main():
    a = args()
    if a.base:
        bpy.ops.wm.open_mainfile(filepath=a.base)
    rig = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    faltando = NOMES_OSSOS - {b.name for b in rig.data.bones}
    assert not faltando, f"base.blend sem os ossos {faltando}"
    corpo = [bpy.data.objects[n] for n in REGIOES if n in bpy.data.objects]

    antes = set(bpy.data.objects)
    ext = os.path.splitext(a.peca)[1].lower()
    {".glb": bpy.ops.import_scene.gltf, ".gltf": bpy.ops.import_scene.gltf,
     ".fbx": bpy.ops.import_scene.fbx, ".obj": bpy.ops.wm.obj_import}[ext](filepath=a.peca)
    novos = [o for o in bpy.data.objects if o not in antes]
    malhas = [o for o in novos if o.type == "MESH"]
    assert malhas, "a peça não tem malha"

    # 2. junta e limpa
    for o in malhas:
        mw = o.matrix_world.copy()
        o.parent = None
        o.matrix_world = mw
        o.modifiers.clear()
        o.vertex_groups.clear()
    for o in novos:
        if o.type != "MESH":
            bpy.data.objects.remove(o)
    with bpy.context.temp_override(active_object=malhas[0], selected_editable_objects=malhas, selected_objects=malhas):
        if len(malhas) > 1:
            bpy.ops.object.join()
    peca = malhas[0]
    peca.name = peca.data.name = a.nome
    with bpy.context.temp_override(active_object=peca, selected_editable_objects=[peca], selected_objects=[peca]):
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)

    tipo, osso = SLOTS[a.slot]

    # 3. alinhar pela caixa (escala uniforme pela altura, centraliza em X/Y, base no Z)
    if a.alinhar and a.slot in ALVO_ALINHAR:
        alvo = ALVO_ALINHAR[a.slot]
        lo, hi = CAIXA_CABECA if alvo == ["@head"] else caixa_mundo([bpy.data.objects[n] for n in alvo])
        plo, phi = caixa_mundo([peca])
        s = (hi.z - lo.z) / max(phi.z - plo.z, 1e-6)
        if a.slot == "cabelo":
            s *= 1.06  # o cabelo envolve a cabeça
        centro_p = (plo + phi) / 2
        centro_a = (lo + hi) / 2
        for v in peca.data.vertices:
            p = (v.co - centro_p) * s
            v.co = Vector((centro_a.x + p.x, centro_a.y + p.y, centro_a.z + p.z))
        print(f"alinhado: escala {s:.3f}")

    # 4. orçamento de triângulos
    limite = a.tris or ORCAMENTO_TRIS[a.slot]
    if tris(peca) > limite:
        m = peca.modifiers.new("dec", "DECIMATE")
        m.ratio = limite / tris(peca)
        with bpy.context.temp_override(object=peca, active_object=peca):
            bpy.ops.object.modifier_apply(modifier=m.name)
    print(f"triângulos: {tris(peca)} (limite {limite})")

    # 5. pixel nítido
    for slot in peca.material_slots:
        if slot.material and slot.material.node_tree:
            slot.material.name = f"{a.nome}_{slot.material.name}"
            for n in slot.material.node_tree.nodes:
                if n.type == "TEX_IMAGE":
                    n.interpolation = "Closest"

    # 6. pesos
    exportar = [peca]
    if tipo == "skinado":
        if a.folga:
            for v in peca.data.vertices:
                v.co += v.normal * a.folga
        fonte = juntar_corpo(corpo)
        dt = peca.modifiers.new("pesos", "DATA_TRANSFER")
        dt.object = fonte
        dt.use_vert_data = True
        dt.data_types_verts = {"VGROUP_WEIGHTS"}
        dt.vert_mapping = "POLYINTERP_NEAREST"
        dt.layers_vgroup_select_src = "ALL"
        dt.layers_vgroup_select_dst = "NAME"
        with bpy.context.temp_override(object=peca, active_object=peca):
            bpy.ops.object.datalayout_transfer(modifier=dt.name)
            bpy.ops.object.modifier_apply(modifier=dt.name)
        bpy.data.objects.remove(fonte)
        with bpy.context.temp_override(object=peca, active_object=peca, selected_editable_objects=[peca]):
            bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=4)
            bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)
        for g in list(peca.vertex_groups):
            if g.name not in NOMES_OSSOS:
                peca.vertex_groups.remove(g)
        sem_peso = [v.index for v in peca.data.vertices if not v.groups]
        assert not sem_peso, f"{len(sem_peso)} vértices sem peso"
        peca.parent = rig
        peca.modifiers.new("Armature", "ARMATURE").object = rig
        exportar = [rig, peca]

    # 7. exporta
    os.makedirs(a.saida, exist_ok=True)
    for o in bpy.context.view_layer.objects:
        o.select_set(o in exportar)
    glb = os.path.join(a.saida, a.nome + ".glb")
    bpy.ops.export_scene.gltf(filepath=glb, export_format="GLB", use_selection=True, export_yup=True,
                              export_skins=True, export_animations=False, export_apply=False)
    manifesto = {
        "id": a.nome, "slot": a.slot, "arquivo": a.nome + ".glb", "tipo": tipo,
        "osso": osso, "esconde": [r for r in a.esconde.split(",") if r], "tris": tris(peca),
    }
    with open(os.path.join(a.saida, a.nome + ".json"), "w", encoding="utf-8") as f:
        json.dump(manifesto, f, ensure_ascii=False, indent=2)
    print("ok:", glb)


def juntar_corpo(corpo):
    """Cópia temporária do corpo inteiro numa malha só (fonte dos pesos)."""
    copias = []
    for o in corpo:
        c = o.copy()
        c.data = o.data.copy()
        c.modifiers.clear()
        c.parent = None
        c.matrix_world = o.matrix_world
        bpy.context.scene.collection.objects.link(c)
        copias.append(c)
    with bpy.context.temp_override(active_object=copias[0], selected_editable_objects=copias, selected_objects=copias):
        if len(copias) > 1:
            bpy.ops.object.join()
    return copias[0]


main()
