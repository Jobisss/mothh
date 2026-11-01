"""Passa animações do Mixamo para o esqueleto do mothh (retarget) e gera os clipes do app.

    blender --background --python tools/avatar/blender/importar_animacao.py -- ^
        --base assets/avatar/base.blend --saida assets/avatar ^
        --pasta assets/animacoes/mixamo
    (ou clipes soltos: --clipe andar=C:\\Downloads\\Walking.fbx --clipe parado=Idle.fbx)

Entra: arquivos do Mixamo (.fbx; .glb/.gltf também servem), baixados com ou sem
skin, em T-pose (bonecos do Mixamo) ou A-pose (o nosso, subido no Mixamo).
O nome do clipe é o do arquivo ("Andar Feliz.fbx" -> "andar_feliz") ou o do --clipe.

Sai em --saida:
  animacoes.glb   só o rig do mothh com um clipe glTF por animação (nada de malha)
  animacoes.json  lista dos clipes: duração, se repete, velocidade do passo...

Como funciona (por quadro, para cada um dos 17 ossos, comum.MIXAMO):
  rotação no mundo do osso Mixamo, relativa à pose de descanso dele, aplicada no
  osso do mothh já alinhado à direção do osso Mixamo em descanso. Assim T-pose x
  A-pose e ossos de tamanhos diferentes não importam; dedos, clavículas e Spine1
  ficam de fora (a direção do braço e do peito já inclui o efeito deles).
  O quadril anda na escala do boneco (altura do quadril do mothh / do Mixamo).

Raiz: por padrão o clipe fica no lugar (o app é que move o boneco): o
deslocamento do quadril no chão é tirado e vira "velocidade" no manifesto, para
o app andar na mesma velocidade dos pés. --raiz-livre nome1,nome2 mantém.
"""
import argparse
import json
import os
import re
import shutil
import sys
import tempfile
import unicodedata

import bpy
from mathutils import Matrix, Vector

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from comum import MIXAMO, OSSOS, RIG_VERSION, SCHEMA_VERSION, nome_mixamo  # noqa: E402

EXTENSOES = (".fbx", ".glb", ".gltf")


def args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser()
    p.add_argument("--base", required=True, help="base.blend gerado pelo preparar_base.py")
    p.add_argument("--saida", required=True)
    p.add_argument("--pasta", help="pasta com os arquivos do Mixamo (um clipe por arquivo)")
    p.add_argument("--clipe", action="append", default=[], help="nome=arquivo, ou só o arquivo")
    p.add_argument("--raiz-livre", default="", help="clipes que mantêm o deslocamento do quadril")
    p.add_argument("--sem-loop", default="", help="clipes que tocam uma vez (acenar, sentar...)")
    p.add_argument("--fps", type=float, default=30.0, help="quadros por segundo dos arquivos (Mixamo: 30)")
    return p.parse_args(argv)


def lista(txt):
    return [x for x in txt.split(",") if x]


def slug(txt):
    txt = unicodedata.normalize("NFKD", txt).encode("ascii", "ignore").decode()
    return re.sub(r"[^a-z0-9]+", "_", txt.lower()).strip("_") or "clipe"


def clipes_pedidos(a):
    pedidos = []
    if a.pasta:
        for arq in sorted(os.listdir(a.pasta)):
            if arq.lower().endswith(EXTENSOES):
                pedidos.append((slug(os.path.splitext(arq)[0]), os.path.join(a.pasta, arq)))
    for c in a.clipe:
        nome, _, arq = c.rpartition("=") if "=" in c else ("", "", c)
        pedidos.append((slug(nome or os.path.splitext(os.path.basename(arq))[0]), arq))
    nomes = [n for n, _ in pedidos]
    repetidos = {n for n in nomes if nomes.count(n) > 1}
    assert not repetidos, f"clipes com o mesmo nome: {sorted(repetidos)}"
    assert pedidos, "nenhum clipe: use --pasta ou --clipe"
    return pedidos


def rot(m):
    """Só a rotação de uma matriz (o Mixamo vem com escala 0,01 no objeto)."""
    return m.to_3x3().normalized().to_quaternion()


def importar(caminho):
    antes = set(bpy.data.objects)
    acoes_antes = set(bpy.data.actions)
    ext = os.path.splitext(caminho)[1].lower()
    if ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=caminho, use_anim=True, ignore_leaf_bones=False,
                                 automatic_bone_orientation=False)
    else:
        bpy.ops.import_scene.gltf(filepath=caminho)
    novos = [o for o in bpy.data.objects if o not in antes]
    return novos, [x for x in bpy.data.actions if x not in acoes_antes]


def achar_rig_mixamo(objs):
    for o in objs:
        if o.type != "ARMATURE":
            continue
        ossos = {nome_mixamo(b.name): b.name for b in o.data.bones}
        if all(m in ossos for m, _, _ in MIXAMO.values()):
            return o, ossos
    raise SystemExit("nenhum esqueleto Mixamo no arquivo (precisa de Hips, Spine, LeftArm...)")


class Alvo:
    """Pose de descanso do rig do mothh (mundo e espaço do rig)."""

    def __init__(self, rig):
        self.rig = rig
        self.ordem = [n for n, *_ in OSSOS]  # pais antes dos filhos
        mw = rig.matrix_world
        self.mw_rot = rot(mw)
        self.rest = {n: rig.data.bones[n].matrix_local.copy() for n in self.ordem}
        self.pai = {n: (rig.data.bones[n].parent.name if rig.data.bones[n].parent else None) for n in self.ordem}
        self.rot_mundo = {n: rot(mw @ self.rest[n]) for n in self.ordem}
        self.dir_mundo = {}
        for n in self.ordem:
            b = rig.data.bones[n]
            self.dir_mundo[n] = ((mw @ b.tail_local) - (mw @ b.head_local)).normalized()
        self.quadril = mw @ rig.data.bones["hips"].head_local


def retarget(alvo, src, ossos, quadros, fps, livre):
    """Amostra o rig Mixamo quadro a quadro. Devolve (poses por quadro, velocidade m/s)."""
    mw = src.matrix_world
    pos = {nome_mixamo(b.name): mw @ b.head_local for b in src.data.bones}
    rest_s = {n: rot(mw @ src.data.bones[ossos[m]].matrix_local) for n, (m, _, _) in MIXAMO.items()}
    # alinhamento: leva a direção de descanso do osso do mothh à do osso Mixamo
    alinha = {}
    for n, (m, ini, fim) in MIXAMO.items():
        if fim in pos and (pos[fim] - pos[ini]).length > 1e-5:
            d_s = (pos[fim] - pos[ini]).normalized()
        else:  # sem o osso da ponta (Mixamo sem dedos): eixo do próprio osso
            d_s = (mw.to_3x3() @ src.data.bones[ossos[m]].matrix_local.to_3x3().col[1]).normalized()
        alinha[n] = alvo.dir_mundo[n].rotation_difference(d_s)
    quadril_s = pos["Hips"]
    escala = alvo.quadril.z / quadril_s.z if quadril_s.z > 1e-4 else 1.0

    cena = bpy.context.scene
    amostras = []
    for f in quadros:
        cena.frame_set(f)
        rots = {}
        for n, (m, _, _) in MIXAMO.items():
            w_s = rot(mw @ src.pose.bones[ossos[m]].matrix)
            rots[n] = w_s @ rest_s[n].inverted() @ alinha[n] @ alvo.rot_mundo[n]
        p_s = mw @ src.pose.bones[ossos["Hips"]].matrix.translation
        amostras.append((rots, alvo.quadril + (p_s - quadril_s) * escala))

    # no chão, o clipe começa onde o boneco está (o descanso do arquivo pode estar em outro lugar)
    inicio = amostras[0][1] - alvo.quadril
    inicio.z = 0
    for _, p in amostras:
        p -= inicio
    # tira o deslocamento no chão (tendência do primeiro ao último quadro) e guarda a velocidade
    desloc = amostras[-1][1] - amostras[0][1]
    desloc.z = 0
    dur = (quadros[-1] - quadros[0]) / fps
    velocidade = desloc.length / dur if dur > 0 else 0.0
    if not livre and len(amostras) > 1:
        ult = len(amostras) - 1
        for i, (_, p) in enumerate(amostras):
            p -= desloc * (i / ult)
    return amostras, velocidade, escala


def gravar_acao(alvo, nome, quadros, amostras):
    """Converte as rotações no mundo em poses locais e grava as chaves numa ação."""
    rig = alvo.rig
    rig.animation_data_create()
    acao = bpy.data.actions.new(nome)
    rig.animation_data.action = acao
    mw_inv = alvo.rig.matrix_world.inverted()
    anterior = {}
    for f, (rots, quadril) in zip(quadros, amostras):
        pose = {}  # matriz no espaço do rig, por osso
        for n in alvo.ordem:
            r = (alvo.mw_rot.inverted() @ rots[n]).to_matrix().to_4x4()
            pai = alvo.pai[n]
            if pai is None:
                p = mw_inv @ quadril
                rel = alvo.rest[n]
                base = Matrix.Identity(4)
            else:
                rel = alvo.rest[pai].inverted() @ alvo.rest[n]
                base = pose[pai]
                p = (base @ rel).translation
            pose[n] = Matrix.Translation(p) @ r
            local = (base @ rel).inverted() @ pose[n]
            loc, q, _ = local.decompose()
            if n in anterior and anterior[n].dot(q) < 0:
                q.negate()  # continuidade: não deixa o quaternion trocar de sinal entre quadros
            anterior[n] = q
            pb = rig.pose.bones[n]
            pb.rotation_mode = "QUATERNION"
            pb.rotation_quaternion = q
            pb.keyframe_insert("rotation_quaternion", frame=f, group=n)
            if pai is None:
                pb.location = loc
                pb.keyframe_insert("location", frame=f, group=n)
    # guarda a ação numa trilha NLA com o nome do clipe (o exportador faz um clipe por trilha)
    rig.animation_data.action = None
    trilha = rig.animation_data.nla_tracks.new()
    trilha.name = nome
    trilha.strips.new(nome, int(quadros[0]), acao)
    trilha.mute = False
    for pb in rig.pose.bones:  # volta para o descanso
        pb.location = (0, 0, 0)
        pb.rotation_quaternion = (1, 0, 0, 0)
    return acao


def main():
    a = args()
    pedidos = clipes_pedidos(a)
    bpy.ops.wm.open_mainfile(filepath=os.path.abspath(a.base))
    rig = bpy.data.objects.get("rig")
    assert rig and rig.type == "ARMATURE", "o base.blend não tem o objeto 'rig'"
    alvo = Alvo(rig)
    cena = bpy.context.scene
    cena.render.fps = round(a.fps)
    cena.render.fps_base = round(a.fps) / a.fps
    livres, sem_loop = set(lista(a.raiz_livre)), set(lista(a.sem_loop))
    for x in (livres | sem_loop) - {n for n, _ in pedidos}:
        print(f"aviso: --raiz-livre/--sem-loop cita '{x}', que não está nos clipes")

    manifesto = []
    for nome, arq in pedidos:
        objs, acoes = importar(arq)
        src, ossos = achar_rig_mixamo(objs)
        acao_src = src.animation_data.action if src.animation_data else None
        assert acao_src, f"{arq}: o esqueleto não tem animação"
        ini, fim = (int(round(x)) for x in acao_src.frame_range)
        quadros = list(range(ini, fim + 1))
        amostras, vel, escala = retarget(alvo, src, ossos, quadros, a.fps, nome in livres)
        gravar_acao(alvo, nome, quadros, amostras)
        dur = (fim - ini) / a.fps
        manifesto.append({
            "nome": nome,
            "fonte": os.path.basename(arq),
            "quadros": len(quadros),
            "duracao": round(dur, 3),
            "loop": nome not in sem_loop,
            "raiz": "livre" if nome in livres else "no_lugar",
            "velocidade": round(vel, 3),
        })
        print(f"{nome}: {len(quadros)} quadros, {dur:.2f} s, escala do quadril {escala:.3f}, "
              f"velocidade {vel:.2f} m/s ({'livre' if nome in livres else 'no lugar'})")
        # limpa o que veio do arquivo para não ir junto no export
        for o in objs:
            bpy.data.objects.remove(o)
        for x in acoes:
            bpy.data.actions.remove(x)
        bpy.data.orphans_purge(do_recursive=True)

    os.makedirs(a.saida, exist_ok=True)
    cena.frame_set(0)
    with tempfile.TemporaryDirectory() as tmp:
        glb = os.path.join(tmp, "animacoes.glb")
        for o in bpy.context.view_layer.objects:
            o.select_set(o == rig)
        bpy.context.view_layer.objects.active = rig
        bpy.ops.export_scene.gltf(
            filepath=glb, export_format="GLB", use_selection=True, export_yup=True,
            export_animations=True, export_animation_mode="NLA_TRACKS", export_force_sampling=True,
            export_frame_step=1, export_skins=True, export_def_bones=False, export_apply=False,
            export_optimize_animation_size=False,
        )
        dados = {
            "schema_version": SCHEMA_VERSION,
            "rig_version": RIG_VERSION,
            "arquivo": "animacoes.glb",
            "fps": a.fps,
            "clipes": manifesto,
        }
        with open(os.path.join(tmp, "animacoes.json"), "w", encoding="utf-8") as f:
            json.dump(dados, f, ensure_ascii=False, indent=2)
            f.write("\n")
        for arq in ("animacoes.glb", "animacoes.json"):
            shutil.move(os.path.join(tmp, arq), os.path.join(a.saida, arq))
    print("ok:", os.path.join(a.saida, "animacoes.glb"), f"({len(manifesto)} clipes)")


main()
