"""Encaixa uma peça gerada (Tripo/Meshy/feita à mão) no boneco base do mothh.

    blender --background --python tools/avatar/blender/encaixar_peca.py -- \
        --base base.blend --peca moletom_gerado.glb --slot superior_meio --nome moletom_largo \
        --saida pecas/ --alinhar --perfil larga --esconde corpo_peito,corpo_barriga,corpo_braco_sup \
        (valida as poses sozinho; --sem-validar pula)

Passos:
 1. abre o base.blend e importa a peça (.glb, .gltf, .fbx ou .obj);
 2. junta tudo num objeto só, apaga esqueletos/pesos/empties que vieram junto
    (guarda as âncoras anc_* se a peça trouxer);
    com --molde (peça gerada vestida no molde do corpo): põe a figura na altura do
    corpo e tira o corpo dela, sobrando só a roupa (precisa de --cabeca);
 3. encaixe (--alinhar): por âncoras da peça (melhor) ou pelo perfil de encaixe do slot
    medido nas âncoras do corpo/cabeça; depois aplica --ajuste (fica gravado no manifesto);
 4. confere o orçamento de triângulos (alvo avisa, máximo recusa; --reduzir corta até o alvo);
 5. texturas em "Closest" (pixel nítido);
 6. pesos pelo perfil de rigging (justa, larga, saia, rigida_osso);
 7. põe o rig em poses de teste: mede o quanto a malha estica e se atravessa o corpo
    visível; reprovada, sai sem escrever nada;
 8. exporta <nome>.glb + <nome>.json (manifesto v1) numa pasta temporária e só então publica.
"""
import argparse
import json
import math
import os
import sys
import tempfile

import bpy
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import (  # noqa: E402
    ANCORAS_CABECA, CORPO_PERFIL, NOMES_OSSOS, ORCAMENTO_TRIS, PERFIS_CABECA, PERFIS_RIG,
    REGIOES, RIG_VERSION, SCHEMA_VERSION, SLOTS,
)

SLOTS_DE_CABECA = {s for s, (_, osso, *_r) in SLOTS.items() if osso == "head"}

# Perfil de encaixe por slot, quando a peça não traz âncoras próprias.
# escala: (medida na peça, medida nas âncoras); ponto: onde a peça encosta.
PERFIL_ENCAIXE = {
    "cabelo":  {"escala": ("largura", ("anc_orelha_l", "anc_orelha_r"), 1.12), "topo": "anc_topo", "folga_topo": 0.012},
    "chapeu":  {"escala": ("largura", ("anc_orelha_l", "anc_orelha_r"), 1.08), "base_em": "anc_testa", "folga_topo": 0.0},
    "fone":    {"escala": ("largura", ("anc_orelha_l", "anc_orelha_r"), 1.15), "centro_z": "anc_orelha_l"},
    "oculos":  {"escala": ("largura", ("anc_orelha_l", "anc_orelha_r"), 1.02), "centro_z": "anc_olhos", "frente": "anc_olhos"},
    "superior_base": {"escala": ("altura", ("anc_pescoco", "anc_cintura"), 1.05), "topo": "anc_pescoco", "folga_topo": 0.0},
    "superior_meio": {"escala": ("altura", ("anc_pescoco", "anc_cintura"), 1.15), "topo": "anc_pescoco", "folga_topo": 0.0},
    "superior_fora": {"escala": ("altura", ("anc_pescoco", "anc_cintura"), 1.20), "topo": "anc_pescoco", "folga_topo": 0.0},
    "inferior": {"escala": ("altura", ("anc_cintura", "anc_tornozelo_l"), 1.0), "topo": "anc_cintura", "folga_topo": 0.0},
    "conjunto": {"escala": ("altura", ("anc_pescoco", "anc_tornozelo_l"), 1.0), "topo": "anc_pescoco", "folga_topo": 0.0},
    # o par de calçados cobre a caixa dos pés: mesma largura (um pouco maior), sola no chão
    "calcados": {"caixa": "corpo_pes", "fator": 1.04},
}

# Poses de teste: osso -> (eixo local, graus). "sentado" é a pose da sala.
POSES = {
    "bracos_cima": {"leftUpperArm": ("X", 70), "rightUpperArm": ("X", 70)},
    "bracos_frente": {"leftUpperArm": ("Z", 70), "rightUpperArm": ("Z", -70), "leftLowerArm": ("Z", 40), "rightLowerArm": ("Z", -40)},
    "sentado": {"leftUpperLeg": ("X", -90), "rightUpperLeg": ("X", -90), "leftLowerLeg": ("X", 90), "rightLowerLeg": ("X", 90)},
    "andando": {"leftUpperLeg": ("X", 35), "rightUpperLeg": ("X", -35), "spine": ("Y", 12)},
}
ESTICA_AVISO, ESTICA_ERRO = 1.6, 2.2  # razão (p98) comprimento posado / descanso das arestas
# Fração dos vértices da roupa que ficam DENTRO de uma parte visível do corpo.
DENTRO_AVISO, DENTRO_ERRO = 0.01, 0.04
DENTRO_TOL = 0.002  # 2 mm de tolerância


def args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--base")
    p.add_argument("--peca", required=True)
    p.add_argument("--slot", required=True, choices=sorted(SLOTS))
    p.add_argument("--nome", required=True)
    p.add_argument("--saida", default=".")
    p.add_argument("--cabeca", help="cabeça (.glb com âncoras) usada para encaixar itens de cabeça")
    p.add_argument("--alinhar", action="store_true")
    p.add_argument("--molde", action="store_true",
                   help="a peça veio vestida no boneco (gerada do molde): encaixa pela figura e tira o corpo")
    p.add_argument("--molde-dist", type=float, default=0.012,
                   help="com --molde: superfície a menos disso do corpo visível ou da cabeça é corpo (m)")
    p.add_argument("--ajuste", default="", help="dx,dy,dz,giro_z_graus,escala depois do encaixe")
    p.add_argument("--perfil", choices=sorted(PERFIS_RIG), default="justa")
    p.add_argument("--osso", help="osso do perfil rigida_osso")
    p.add_argument("--folga", type=float, default=0.004, help="empurra a peça skinada para fora (m)")
    p.add_argument("--reduzir", action="store_true", help="decima até o alvo de triângulos do slot")
    p.add_argument("--textura", type=int, choices=(64, 128, 256), help="reduz as texturas para este lado (px)")
    p.add_argument("--forcar", action="store_true", help="aceita passar do máximo (não usar em item de loja)")
    p.add_argument("--esconde", default="", help="regiões do corpo, separadas por vírgula")
    p.add_argument("--mascara", help="PNG de máscara do corpo (para quando região inteira é demais)")
    p.add_argument("--esconde-slots", default="", help="slots que esta peça esconde (chapéu -> cabelo)")
    p.add_argument("--conflitos", default="", help="slots que não podem estar junto")
    p.add_argument("--cabecas", default="*", help="perfis de cabeça compatíveis, ou *")
    p.add_argument("--variante", action="append", default=[], help="perfil=arquivo.glb para outra cabeça")
    p.add_argument("--perfil-cabeca", choices=sorted(PERFIS_CABECA), help="só para slot cabeca")
    p.add_argument("--sem-validar", action="store_true", help="pula as poses de teste (só para rascunho)")
    return p.parse_args(argv)


def lista(txt):
    return [x for x in txt.split(",") if x]


def caixa_mundo(objs):
    pts = [o.matrix_world @ Vector(c) for o in objs for c in o.bound_box]
    return (Vector(min(p[i] for p in pts) for i in range(3)), Vector(max(p[i] for p in pts) for i in range(3)))


def tris(ob):
    return sum(len(p.vertices) - 2 for p in ob.data.polygons)


def importar(caminho):
    antes = set(bpy.data.objects)
    ext = os.path.splitext(caminho)[1].lower()
    {".glb": bpy.ops.import_scene.gltf, ".gltf": bpy.ops.import_scene.gltf,
     ".fbx": bpy.ops.import_scene.fbx, ".obj": bpy.ops.wm.obj_import}[ext](filepath=caminho)
    return [o for o in bpy.data.objects if o not in antes]


def ctx(ob, objs=None):
    objs = objs or [ob]
    return bpy.context.temp_override(object=ob, active_object=ob, selected_objects=objs, selected_editable_objects=objs)


# --- encaixe ------------------------------------------------------------------

def nao_colineares(pts):
    """3+ pontos que não estão numa linha (senão a rotação fica indefinida)."""
    import numpy as np
    if len(pts) < 3:
        return False
    a = np.array(pts)
    sv = np.linalg.svd(a - a.mean(0), compute_uv=False)
    return sv[1] > 1e-3 * max(sv[0], 1e-9)


def similaridade(src, dst):
    """Escala uniforme + rotação + translação que leva src em dst (Umeyama)."""
    import numpy as np
    a, b = np.array(src), np.array(dst)
    ma, mb = a.mean(0), b.mean(0)
    a0, b0 = a - ma, b - mb
    u, d, vt = np.linalg.svd(b0.T @ a0)
    sgn = np.eye(3)
    sgn[2, 2] = np.sign(np.linalg.det(u @ vt))
    r = u @ sgn @ vt
    s = np.trace(np.diag(d) @ sgn) / (a0 ** 2).sum()
    m = Matrix.Identity(4)
    for i in range(3):
        for j in range(3):
            m[i][j] = s * r[i, j]
    t = mb - s * r @ ma
    m.translation = Vector(t)
    return m


def encaixar(peca, ancoras_peca, alvo, slot, corpo=()):
    """Devolve a matriz de encaixe e como foi feito."""
    comuns = sorted(set(ancoras_peca) & set(alvo))
    if comuns and not nao_colineares([ancoras_peca[n] for n in comuns]):
        print(f"AVISO: só {len(comuns)} âncora(s) em comum ou em linha ({comuns}); "
              "precisa de 3 não colineares. Usando o perfil do slot.")
        comuns = []
    if comuns:
        m = similaridade([ancoras_peca[n] for n in comuns], [alvo[n] for n in comuns])
        return m, f"ancoras:{','.join(comuns)}"
    perfil = PERFIL_ENCAIXE.get(slot)
    regiao = [o for o in corpo if perfil and o.name == perfil.get("caixa")]
    if regiao:
        lo, hi = caixa_mundo([peca])
        rlo, rhi = caixa_mundo(regiao)
        s = (rhi.x - rlo.x) * perfil["fator"] / max(hi.x - lo.x, 1e-6)
        ref = Vector(((lo.x + hi.x) / 2, (lo.y + hi.y) / 2, lo.z))
        destino = Vector(((rlo.x + rhi.x) / 2, (rlo.y + rhi.y) / 2, rlo.z))
        m = Matrix.Translation(destino) @ Matrix.Scale(s, 4) @ Matrix.Translation(-ref)
        return m, f"perfil:{slot}"
    if not perfil or "escala" not in perfil or any(n not in alvo for n in perfil["escala"][1]):
        lo, hi = caixa_mundo([peca])
        print(f"AVISO: {slot} sem perfil/âncoras; mantendo onde está (encaixe manual + --ajuste)")
        return Matrix.Identity(4), "manual"
    lo, hi = caixa_mundo([peca])
    medida, (a1, a2), fator = perfil["escala"]
    if medida == "largura":
        s = abs(alvo[a1].x - alvo[a2].x) * fator / max(hi.x - lo.x, 1e-6)
    else:
        s = abs(alvo[a1].z - alvo[a2].z) * fator / max(hi.z - lo.z, 1e-6)
    centro = (lo + hi) / 2
    ref = Vector((0.0, centro.y, centro.z))
    destino = Vector((0.0, 0.0, 0.0))
    if "topo" in perfil:
        ref = Vector((0.0, centro.y, hi.z))
        destino = alvo[perfil["topo"]] + Vector((0, 0, perfil["folga_topo"]))
        destino.y = (alvo.get("anc_testa", destino).y + alvo.get("anc_nuca", destino).y) / 2 if slot in SLOTS_DE_CABECA else 0.0
        destino.x = 0.0
    if "base_em" in perfil:
        ref = Vector((0.0, centro.y, lo.z))
        destino = Vector((0.0, (alvo["anc_testa"].y + alvo["anc_nuca"].y) / 2, alvo[perfil["base_em"]].z))
    if "centro_z" in perfil:
        destino = Vector((0.0, 0.0, alvo[perfil["centro_z"]].z))
        if "frente" in perfil:
            ref = Vector((0.0, lo.y, centro.z))
            destino.y = alvo[perfil["frente"]].y - 0.01
    m = Matrix.Translation(destino) @ Matrix.Scale(s, 4) @ Matrix.Translation(-ref)
    return m, f"perfil:{slot}"


def figura_no_corpo(peca, corpo, visiveis, malhas_cabeca):
    """Figura inteira (corpo vestido, como o Tripo entrega): mesma altura do corpo base
    (o Tripo normaliza o tamanho), sola no chão e centrada; depois um ajuste fino só de
    translação cola a pele à mostra da figura no corpo base (roupa volumosa desloca a caixa)."""
    from mathutils.bvhtree import BVHTree
    lo, hi = caixa_mundo([peca])
    pts = [o.matrix_world @ Vector(c) for o in corpo for c in o.bound_box]
    pts += [v for vs, _ in malhas_cabeca for v in vs]
    blo = Vector(min(p[i] for p in pts) for i in range(3))
    bhi = Vector(max(p[i] for p in pts) for i in range(3))
    s = (bhi.z - blo.z) / max(hi.z - lo.z, 1e-6)
    ref = Vector(((lo.x + hi.x) / 2, (lo.y + hi.y) / 2, lo.z))
    destino = Vector(((blo.x + bhi.x) / 2, (blo.y + bhi.y) / 2, blo.z))
    peca.data.transform(Matrix.Translation(destino) @ Matrix.Scale(s, 4) @ Matrix.Translation(-ref))
    print(f"molde: figura de {hi.z - lo.z:.3f} -> {bhi.z - blo.z:.3f} m")

    # ajuste fino: a mediana do quanto os vértices perto da pele visível estão fora dela
    sup = [BVHTree.FromPolygons([o.matrix_world @ v.co for v in o.data.vertices],
                                [tuple(f.vertices) for f in o.data.polygons]) for o in visiveis]
    sup += [BVHTree.FromPolygons(vs, fs) for vs, fs in malhas_cabeca]
    amostra = peca.data.vertices[::max(1, len(peca.data.vertices) // 3000)]
    total = Vector()
    for _ in range(8):
        difs = []
        for v in amostra:
            melhor = None
            for arv in sup:
                loc, _, _, d = arv.find_nearest(v.co + total, 0.03)
                if loc is not None and (melhor is None or d < melhor[0]):
                    melhor = (d, loc - (v.co + total))
            if melhor:
                difs.append(melhor[1])
        if len(difs) < 20:
            break
        passo = Vector(sorted(d[i] for d in difs)[len(difs) // 2] for i in range(3))
        total += passo
        if passo.length < 0.0003:
            break
    peca.data.transform(Matrix.Translation(total))
    peca.data.update()
    print(f"molde: ajuste fino {total.x * 1000:+.1f}, {total.y * 1000:+.1f}, {total.z * 1000:+.1f} mm")


def tirar_corpo(peca, visiveis, malhas_cabeca, dist):
    """Apaga da figura o que é corpo: faces perto (ou dentro) do corpo visível e da cabeça.
    O que fica sobre as regiões escondidas pela peça é a roupa. Depois tira as ilhas
    pequenas (pedaços de pele que o Tripo desenhou um pouco fora do lugar)."""
    import bmesh
    from mathutils.bvhtree import BVHTree
    sup = [BVHTree.FromPolygons([o.matrix_world @ v.co for v in o.data.vertices],
                                [tuple(f.vertices) for f in o.data.polygons]) for o in visiveis]
    sup += [BVHTree.FromPolygons(vs, fs) for vs, fs in malhas_cabeca]
    bm = bmesh.new()
    bm.from_mesh(peca.data)
    bm.normal_update()

    def e_corpo(f):
        c = f.calc_center_median()
        for arv in sup:
            loc, normal, _, d = arv.find_nearest(c, dist * 4)
            if loc is None:
                continue
            if d < dist or (c - loc).dot(normal) < 0:  # perto, ou dentro da pele
                return True
        return False

    total = len(bm.faces)
    bmesh.ops.delete(bm, geom=[f for f in bm.faces if e_corpo(f)], context="FACES")
    # ilhas: grupos de faces ligadas; fica só o que tem pelo menos 3% das faces
    sobra, ilhas = set(bm.faces), []
    while sobra:
        semente = sobra.pop()
        ilha, pilha = {semente}, [semente]
        while pilha:
            f = pilha.pop()
            for e in f.edges:
                for g in e.link_faces:
                    if g in sobra:
                        sobra.discard(g)
                        ilha.add(g)
                        pilha.append(g)
        ilhas.append(ilha)
    restantes = sum(len(i) for i in ilhas)
    pequenas = [f for i in ilhas if len(i) < 0.03 * restantes for f in i]
    bmesh.ops.delete(bm, geom=pequenas, context="FACES")
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
    print(f"molde: {total} faces -> {len(bm.faces)} de roupa "
          f"({len(ilhas)} ilhas, {len(pequenas)} faces em ilhas pequenas apagadas)")
    assert bm.faces, "não sobrou roupa: confira --esconde (as regiões cobertas pela peça) e --molde-dist"
    bm.to_mesh(peca.data)
    bm.free()


def matriz_ajuste(txt):
    if not txt:
        return Matrix.Identity(4), None
    v = [float(x) for x in txt.split(",")] + [0, 0, 0, 0, 1][len(txt.split(",")):]
    dx, dy, dz, rz, esc = v[:5]
    m = Matrix.Translation((dx, dy, dz)) @ Matrix.Rotation(math.radians(rz), 4, "Z") @ Matrix.Scale(esc, 4)
    return m, {"dx": dx, "dy": dy, "dz": dz, "giro_z": rz, "escala": esc}


# --- pesos --------------------------------------------------------------------

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
    if len(copias) > 1:
        with ctx(copias[0], copias):
            bpy.ops.object.join()
    return copias[0]


def pesar(peca, corpo, perfil, osso):
    if perfil == "rigida_osso":
        assert osso in NOMES_OSSOS, "--osso precisa ser um osso do rig"
        peca.vertex_groups.new(name=osso).add(range(len(peca.data.vertices)), 1.0, "REPLACE")
        return
    fonte = juntar_corpo(corpo)
    dt = peca.modifiers.new("pesos", "DATA_TRANSFER")
    dt.object = fonte
    dt.use_vert_data = True
    dt.data_types_verts = {"VGROUP_WEIGHTS"}
    dt.vert_mapping = "POLYINTERP_NEAREST"
    dt.layers_vgroup_select_src = "ALL"
    dt.layers_vgroup_select_dst = "NAME"
    with ctx(peca):
        bpy.ops.object.datalayout_transfer(modifier=dt.name)
        bpy.ops.object.modifier_apply(modifier=dt.name)
    bpy.data.objects.remove(fonte)

    if perfil == "saia":
        # Saia não pode seguir canela nem braço: só quadril, coluna e coxas.
        permitidos = {"hips", "spine", "leftUpperLeg", "rightUpperLeg"}
        for g in list(peca.vertex_groups):
            if g.name not in permitidos:
                peca.vertex_groups.remove(g)
        hips = peca.vertex_groups.get("hips") or peca.vertex_groups.new(name="hips")
        hips.add([v.index for v in peca.data.vertices if not v.groups], 1.0, "REPLACE")
    if perfil in ("larga", "saia"):
        # Roupa larga: suavizar evita que um vértice da manga "pule" para o osso vizinho.
        suavizar(peca, 0.6, 6 if perfil == "larga" else 10)
    with ctx(peca):
        bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=4)
        bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)


def suavizar(peca, fator, repeticoes):
    """Suaviza os pesos de todos os grupos pela vizinhança (Laplaciano simples)."""
    me = peca.data
    viz = [[] for _ in me.vertices]
    for e in me.edges:
        a, b = e.vertices
        viz[a].append(b)
        viz[b].append(a)
    grupos = {g.index: g for g in peca.vertex_groups}
    pesos = [{ge.group: ge.weight for ge in v.groups} for v in me.vertices]
    for _ in range(repeticoes):
        novos = []
        for i, w in enumerate(pesos):
            if not viz[i]:
                novos.append(w)
                continue
            media = {}
            for j in viz[i]:
                for g, x in pesos[j].items():
                    media[g] = media.get(g, 0.0) + x / len(viz[i])
            chaves = set(w) | set(media)
            novos.append({g: (1 - fator) * w.get(g, 0.0) + fator * media.get(g, 0.0) for g in chaves})
        pesos = novos
    for i, w in enumerate(pesos):
        for g, x in w.items():
            if x > 1e-4:
                grupos[g].add([i], x, "REPLACE")
            else:
                grupos[g].remove([i])


def validar(peca, rig, corpo, corpo_visivel):
    """Põe o rig em cada pose de teste e mede:
    - esticada: quanto as arestas da peça esticam (p98);
    - dentro_corpo: fração dos vértices da peça que ficam dentro de uma parte
      visível do corpo (roupa atravessando braço/tronco). Usa a normal da face
      mais próxima do corpo: vértice atrás dela está dentro.
    Também testa a pose de descanso. Não substitui olhar no Blender."""
    from mathutils.bvhtree import BVHTree
    me = peca.data
    descanso = [(me.vertices[e.vertices[0]].co - me.vertices[e.vertices[1]].co).length for e in me.edges]
    rel = {}
    for nome, pose in {"descanso": {}, **POSES}.items():
        for pb in rig.pose.bones:
            pb.rotation_mode = "XYZ"
            pb.rotation_euler = (0, 0, 0)
        for osso, (eixo, graus) in pose.items():
            rig.pose.bones[osso].rotation_euler["XYZ".index(eixo)] = math.radians(graus)
        bpy.context.view_layer.update()
        dg = bpy.context.evaluated_depsgraph_get()
        # As árvores do corpo antes do to_mesh() da peça (FromObject invalida a malha temporária).
        # Todas as regiões, não só as visíveis: cada região é um retalho aberto, e a
        # borda de um retalho vizinho dava falso "dentro". Vale a superfície do corpo
        # mais próxima; só conta se ela for de uma região visível.
        arvores = [BVHTree.FromObject(o, dg) for o in corpo]
        mw_corpo = [o.matrix_world for o in corpo]
        ev = peca.evaluated_get(dg)
        m = ev.to_mesh()
        razoes = []
        for e, l0 in zip(m.edges, descanso):
            if l0 < 1e-6:
                continue
            l1 = (m.vertices[e.vertices[0]].co - m.vertices[e.vertices[1]].co).length
            razoes.append(max(l1 / l0, l0 / max(l1, 1e-6)))
        razoes.sort()
        # p98: ignora as poucas arestas do sovaco, que sempre esticam um pouco.
        p98 = razoes[int(len(razoes) * 0.98)] if razoes else 1.0

        dentro = 0
        por_regiao = {}
        for v in m.vertices:
            pw = peca.matrix_world @ v.co
            perto = None
            for arv, mw, reg in zip(arvores, mw_corpo, corpo):
                local = mw.inverted() @ pw
                loc, normal, _, dist = arv.find_nearest(local, 0.05)
                if loc is not None and (perto is None or dist < perto[0]):
                    perto = (dist, (local - loc).dot(normal), reg)
            if perto and perto[1] < -DENTRO_TOL and perto[2] in corpo_visivel:
                dentro += 1
                por_regiao[perto[2].name] = por_regiao.get(perto[2].name, 0) + 1
        pior_reg = max(por_regiao, key=por_regiao.get) if por_regiao else "-"
        frac = dentro / max(len(m.vertices), 1)
        ev.to_mesh_clear()
        rel[nome] = {"esticada": round(p98, 2), "dentro_corpo": round(frac, 3)}
        ruim = p98 > ESTICA_ERRO or frac > DENTRO_ERRO
        meio = p98 > ESTICA_AVISO or frac > DENTRO_AVISO
        estado = "ERRO" if ruim else "AVISO" if meio else "ok"
        print(f"pose {nome:14s} esticada p98 {p98:.2f}x  dentro do corpo {frac:5.1%} (mais em {pior_reg})  {estado}")
    for pb in rig.pose.bones:
        pb.rotation_euler = (0, 0, 0)
    bpy.context.view_layer.update()
    return rel


# --- principal ------------------------------------------------------------------

def main():
    a = args()
    if a.base:
        bpy.ops.wm.open_mainfile(filepath=a.base)
    rig = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    faltando = NOMES_OSSOS - {b.name for b in rig.data.bones}
    assert not faltando, f"base.blend sem os ossos {faltando}"
    corpo = [bpy.data.objects[n] for n in REGIOES if n in bpy.data.objects]
    for r in lista(a.esconde):
        assert r in REGIOES, f"região desconhecida: {r} (use {REGIOES})"
    tipo, osso, *_ = SLOTS[a.slot]

    # Âncoras de destino: corpo + cabeça de referência.
    bpy.context.view_layer.update()
    alvo = {o.name: o.matrix_world.translation.copy() for o in bpy.data.objects if o.type == "EMPTY" and o.name.startswith("anc_")}
    malhas_cabeca = []  # (vértices no mundo, faces) da cabeça, para o --molde
    if a.cabeca:
        for o in importar(a.cabeca):
            if o.type == "EMPTY" and o.name.split(".")[0] in ANCORAS_CABECA:
                alvo[o.name.split(".")[0]] = o.matrix_world.translation.copy()
            if o.type == "MESH":
                malhas_cabeca.append(([o.matrix_world @ v.co for v in o.data.vertices],
                                      [tuple(f.vertices) for f in o.data.polygons]))
            bpy.data.objects.remove(o)
    elif a.slot in SLOTS_DE_CABECA and a.slot != "cabeca" and a.alinhar:
        print("AVISO: item de cabeça sem --cabeca; o encaixe vai cair no manual")
    assert not a.molde or malhas_cabeca, "--molde precisa de --cabeca (a cabeça sai junto com o corpo)"

    novos = importar(a.peca)
    # Cabeça: o objeto "rosto" e as âncoras da cabeça ficam separados e vão junto no .glb.
    extras = []
    if a.slot == "cabeca":
        extras = [o for o in novos if o.name.split(".")[0] == "rosto"
                  or (o.type == "EMPTY" and o.name.split(".")[0] in ANCORAS_CABECA)]
        nomes = {o.name.split(".")[0] for o in extras}
        assert "rosto" in nomes, "cabeça sem o objeto 'rosto' (UV 0..1 do rosto)"
        faltam = set(ANCORAS_CABECA) - nomes
        assert not faltam, f"cabeça sem as âncoras {faltam}"
        for o in extras:
            mw = o.matrix_world.copy()
            o.parent = None
            o.matrix_world = mw
            o.name = o.name.split(".")[0]
    malhas = [o for o in novos if o.type == "MESH" and o not in extras]
    assert malhas, "a peça não tem malha"
    ancoras_peca = {o.name.split(".")[0]: o.matrix_world.translation.copy() for o in novos
                    if o.type == "EMPTY" and o.name.startswith("anc_")}
    for o in malhas:
        mw = o.matrix_world.copy()
        o.parent = None
        o.matrix_world = mw
        o.modifiers.clear()
        o.vertex_groups.clear()
    for o in novos:
        if o.type != "MESH" and o not in extras:
            bpy.data.objects.remove(o)
    if len(malhas) > 1:
        with ctx(malhas[0], malhas):
            bpy.ops.object.join()
    peca = malhas[0]
    peca.name = peca.data.name = a.nome
    with ctx(peca):
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)

    encaixe = "no_lugar"
    m = Matrix.Identity(4)
    if a.molde:
        visiveis = [o for o in corpo if o.name not in lista(a.esconde)]
        figura_no_corpo(peca, corpo, visiveis, malhas_cabeca)
        tirar_corpo(peca, visiveis, malhas_cabeca, a.molde_dist)
        encaixe = "molde"
    elif a.alinhar:
        m, encaixe = encaixar(peca, ancoras_peca, alvo, a.slot, corpo)
    m_aj, ajuste = matriz_ajuste(a.ajuste)
    total = m_aj @ m
    # A mesma transformação vale para a peça e para o que anda junto com ela
    # (na cabeça: o objeto "rosto" e as âncoras), senão o rosto sai do lugar.
    peca.data.transform(total)
    peca.data.update()
    for o in extras:
        if o.type == "MESH":
            o.data.transform(o.matrix_world.inverted() @ total @ o.matrix_world)
            o.data.update()
        else:
            o.matrix_world = total @ o.matrix_world
    bpy.context.view_layer.update()
    print(f"encaixe: {encaixe}  ajuste: {ajuste}")

    alvo_t, max_t = ORCAMENTO_TRIS[a.slot]
    if a.reduzir and tris(peca) > alvo_t:
        d = peca.modifiers.new("dec", "DECIMATE")
        d.ratio = alvo_t / tris(peca)
        with ctx(peca):
            bpy.ops.object.modifier_apply(modifier=d.name)
    t = tris(peca)
    print(f"triângulos: {t} (alvo {alvo_t}, máximo {max_t})")
    if t > max_t and not a.forcar:
        sys.exit(f"ERRO: {t} triângulos passa do máximo {max_t} do slot {a.slot}; use --reduzir")
    if t > alvo_t:
        print("AVISO: acima do alvo do slot")

    resolucao = 0
    for s in peca.material_slots:
        if s.material and s.material.node_tree:
            s.material.name = f"{a.nome}_{s.material.name}"
            for n in s.material.node_tree.nodes:
                if n.type == "TEX_IMAGE":
                    n.interpolation = "Closest"
                    if n.image and a.textura and max(n.image.size) > a.textura:
                        # o Tripo entrega 1024-2048 px; o boneco usa 128-256 (pixel grande)
                        w, h = n.image.size
                        k = a.textura / max(w, h)
                        n.image.scale(max(1, round(w * k)), max(1, round(h * k)))
                        n.image.pack()
                    if n.image:
                        resolucao = max(resolucao, *n.image.size)
    if resolucao > 256:
        print(f"AVISO: textura {resolucao}px; o padrão é 128 ou 256 (use --textura 256)")

    exportar = [peca] + extras
    rel = None
    reprovou = t > max_t
    if tipo == "skinado":
        if a.folga:
            for v in peca.data.vertices:
                v.co += v.normal * a.folga
        pesar(peca, corpo, a.perfil, a.osso)
        for g in list(peca.vertex_groups):
            if g.name not in NOMES_OSSOS:
                peca.vertex_groups.remove(g)
        sem_peso = [v.index for v in peca.data.vertices if not v.groups]
        assert not sem_peso, f"{len(sem_peso)} vértices sem peso"
        peca.parent = rig
        peca.modifiers.new("Armature", "ARMATURE").object = rig
        exportar = [rig, peca]
        if not a.sem_validar:
            visiveis = [o for o in corpo if o.name not in lista(a.esconde)]
            rel = validar(peca, rig, corpo, visiveis)
            reprovou = [n for n, r in rel.items() if r["esticada"] > ESTICA_ERRO or r["dentro_corpo"] > DENTRO_ERRO]
            if reprovou and not a.forcar:
                # Sai antes de exportar: peça reprovada não deixa arquivo na saída.
                sys.exit(f"ERRO: reprovada nas poses {reprovou}. Roupa larga: --perfil larga; atravessando o corpo: "
                         "--folga maior, --esconde a região, ou corrigir no Blender. Nada foi escrito.")

    # Exporta numa pasta temporária e só publica (move) depois de tudo certo.
    os.makedirs(a.saida, exist_ok=True)
    tmp = tempfile.mkdtemp(prefix=".tmp_", dir=a.saida)
    for o in bpy.context.view_layer.objects:
        o.select_set(o in exportar)
    glb = os.path.join(a.saida, a.nome + ".glb")
    bpy.ops.export_scene.gltf(filepath=os.path.join(tmp, a.nome + ".glb"), export_format="GLB", use_selection=True, export_yup=True,
                              export_skins=True, export_animations=False, export_apply=False)
    manifesto = {
        "schema_version": SCHEMA_VERSION,
        "id": a.nome,
        "slot": a.slot,
        "arquivo": a.nome + ".glb",
        "tipo": tipo,
        "osso": osso,
        "rig_version": RIG_VERSION,
        "corpo_perfil": CORPO_PERFIL,
        "perfil_rig": a.perfil if tipo == "skinado" else None,
        "esconde": lista(a.esconde),
        "mascara": os.path.basename(a.mascara) if a.mascara else None,
        "esconde_slots": lista(a.esconde_slots),
        "conflitos": lista(a.conflitos),
        "cabecas": ["*"] if a.cabecas == "*" else lista(a.cabecas),
        "variantes": dict(v.split("=", 1) for v in a.variante),
        "encaixe": encaixe,
        "ajuste": ajuste,
        "textura": resolucao or None,
        "tris": t,
        "validacao": rel,
        # Passou do limite com --forcar: o app de loja deve recusar item forçado.
        "forcado": bool(a.forcar and reprovou),
    }
    if a.slot == "cabeca":
        manifesto["perfil_cabeca"] = a.perfil_cabeca or "humano"
    with open(os.path.join(tmp, a.nome + ".json"), "w", encoding="utf-8") as f:
        json.dump(manifesto, f, ensure_ascii=False, indent=2)
    for ext in (".glb", ".json"):
        os.replace(os.path.join(tmp, a.nome + ext), os.path.join(a.saida, a.nome + ext))
    os.rmdir(tmp)
    print("ok:", glb)


main()
