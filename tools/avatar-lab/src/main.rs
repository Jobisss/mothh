//! Protótipo dos bonequinhos modulares (Fase 3).
//!
//! Mostra as três peças do pipeline descrito em docs/produto/personagem-modular.md:
//! - corpo base skinado (base.glb) com o esqueleto oficial;
//! - peça rígida (cabeça, cabelo) presa no osso `head`;
//! - peça skinada (camiseta) que reaproveita os ossos do corpo (troca de juntas por nome);
//! - rosto por textura: um atlas de expressões, trocado em runtime pelo `uv_transform`.
//!
//! - animações do Mixamo passadas para o nosso rig (animacoes.glb + animacoes.json, gerados
//!   por tools/avatar/blender/importar_animacao.py); sem esse arquivo, uma animação de teste
//!   feita em código.
//!
//! Teclas: 1-6 expressão de todos, H cabelo, C camiseta, M moletom (o moletom esconde a camiseta),
//! W anda/para, N próximo clipe, F custo de cada boneco.
//! As peças vêm dos manifestos .json em assets/avatar (contrato v1).
//! Os modelos em assets/avatar são placeholders gerados por
//! tools/avatar/blender/criar_base_placeholder.py e encaixar_peca.py.

use std::collections::HashMap;
use std::time::Duration;

use bevy::animation::{AnimatedBy, AnimationTargetId};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::math::Affine2;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};
use serde::Deserialize;

/// Quantos bonecos na tela (o alvo da sala é até ~10).
const BONECOS: usize = 10;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(Startup, iniciar)
        .add_systems(Update, (animar, teclado, visibilidade, piscar, foto))
        .add_systems(Update, (montar_grafo, ligar_animacao, trocar_clipe, mover).chain())
        .run();
}

// ---------------------------------------------------------------------------
// Manifesto das peças (no app vem do JSON gerado pelo encaixar_peca.py)

/// Versões do contrato que este app entende (iguais às de tools/avatar/blender/comum.py).
const SCHEMA_VERSION: u32 = 1;
const RIG_VERSION: &str = "mothh-humanoide-v1";

/// Manifesto v1 de uma peça (só os campos que o protótipo usa).
#[derive(Clone, Deserialize)]
struct Item {
    schema_version: u32,
    id: String,
    slot: String,
    arquivo: String,
    /// "rigido" (presa no `osso`) ou "skinado" (usa os ossos do corpo).
    tipo: String,
    osso: Option<String>,
    rig_version: String,
    /// Regiões do corpo escondidas debaixo da peça.
    #[serde(default)]
    esconde: Vec<String>,
    /// Slots escondidos por esta peça (moletom esconde a camiseta de baixo).
    #[serde(default)]
    esconde_slots: Vec<String>,
    tris: u32,
    /// Passou da validação só com --forcar: a loja não aceita.
    #[serde(default)]
    forcado: bool,
}

/// Clipes de animação (animacoes.json, gerado pelo importar_animacao.py).
#[derive(Clone, Deserialize)]
struct ManifestoAnimacoes {
    schema_version: u32,
    rig_version: String,
    arquivo: String,
    clipes: Vec<Clipe>,
}

#[derive(Clone, Deserialize)]
struct Clipe {
    nome: String,
    /// Repete (andar, parado) ou toca uma vez (acenar, sentar).
    #[serde(rename = "loop", default = "sim")]
    loop_: bool,
    /// Velocidade do passo no clipe original (m/s): o app anda nessa velocidade
    /// para os pés não escorregarem. 0 para clipes parados.
    #[serde(default)]
    velocidade: f32,
}

fn sim() -> bool {
    true
}

/// Nomes procurados para o clipe parado e o de andar (o primeiro que existir).
const NOMES_PARADO: [&str; 3] = ["parado", "idle", "respirando"];
const NOMES_ANDAR: [&str; 4] = ["andar", "andando", "walking", "walk"];

/// Lê assets/<pasta>/animacoes.json, se existir e for deste rig.
fn ler_animacoes() -> Option<ManifestoAnimacoes> {
    let caminho = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(pasta_avatar())
        .join("animacoes.json");
    let txt = std::fs::read_to_string(&caminho).ok()?;
    match serde_json::from_str::<ManifestoAnimacoes>(&txt) {
        Ok(m) if m.schema_version != SCHEMA_VERSION || m.rig_version != RIG_VERSION => {
            warn!("animacoes.json: contrato {} / {} não é o deste app; usando a animação de teste", m.schema_version, m.rig_version);
            None
        }
        Ok(m) => {
            info!("animações: {}", m.clipes.iter().map(|c| c.nome.as_str()).collect::<Vec<_>>().join(", "));
            Some(m)
        }
        Err(e) => {
            warn!("animacoes.json inválido: {e}");
            None
        }
    }
}

/// Teto de triângulos de um boneco montado (doc, seção 7), contando só o que aparece.
const TETO_TRIS: u32 = 6500;

/// Pasta dos modelos dentro de assets/: "avatar" (caixas) ou, com
/// `AVATAR_PASTA=avatar_real`, o corpo gerado no Tripo.
fn pasta_avatar() -> String {
    std::env::var("AVATAR_PASTA").unwrap_or_else(|_| "avatar".into())
}

/// Lê todos os manifestos de assets/avatar e recusa os de outra versão do contrato.
fn ler_manifestos() -> Vec<Item> {
    let pasta = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join(pasta_avatar());
    let mut itens = Vec::new();
    for arq in std::fs::read_dir(&pasta).into_iter().flatten().flatten() {
        // animacoes.json e juntas.json moram na mesma pasta, mas não são peças
        let nome = arq.file_name();
        if arq.path().extension().is_none_or(|e| e != "json") || nome == "animacoes.json" || nome == "juntas.json" {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(arq.path()) else { continue };
        match serde_json::from_str::<Item>(&txt) {
            Ok(i) if i.schema_version != SCHEMA_VERSION || i.rig_version != RIG_VERSION => {
                warn!("{}: contrato {} / {} não é o deste app; ignorada", i.id, i.schema_version, i.rig_version);
            }
            Ok(mut i) => {
                if i.forcado {
                    warn!("{}: peça forçada (reprovou na validação); ok no protótipo, não na loja", i.id);
                }
                i.arquivo = format!("{}/{}", pasta_avatar(), i.arquivo);
                itens.push(i);
            }
            Err(e) => warn!("{}: manifesto inválido: {e}", arq.path().display()),
        }
    }
    itens.sort_by(|a, b| a.id.cmp(&b.id));
    itens
}

// ---------------------------------------------------------------------------
// Componentes

/// Raiz de um boneco. `ossos` fica pronto quando o base.glb termina de carregar.
#[derive(Component, Default)]
struct Boneco {
    ossos: HashMap<String, Entity>,
    /// Pose de descanso de cada osso, para a animação de teste.
    descanso: HashMap<Entity, Transform>,
    /// Regiões do corpo (corpo_tronco, ...) por nome.
    regioes: HashMap<String, Entity>,
    /// Peça equipada por slot: entidade, manifesto e se a pessoa deixou ligada.
    pecas: HashMap<String, (Entity, Item, bool)>,
    rosto: Handle<StandardMaterial>,
    fase: f32,
    /// Triângulos e malhas de cada região do corpo (lidos das malhas carregadas).
    custo_regiao: HashMap<String, (u32, u32)>,
    /// Malhas (≈ draw calls) de cada peça, por slot.
    malhas_peca: HashMap<String, u32>,
    /// Já ganhou o AnimationPlayer (os clipes do animacoes.glb).
    animado: bool,
    /// Clipe tocando agora (índice em `Animacoes::clipes`).
    clipe: usize,
    /// Onde o boneco começou: andando, ele volta para cá depois de uns passos.
    origem: Vec3,
}

/// Custo do boneco como ele aparece: só peças e regiões visíveis.
#[derive(Debug, Default)]
struct Custo {
    tris: u32,
    malhas: u32,
    skinadas: u32,
}

impl Boneco {
    /// Regra de camadas: (slots que aparecem, regiões escondidas).
    /// Uma peça ligada esconde as regiões e os slots que pede; o moletom
    /// (superior_meio) esconde a camiseta (superior_base) e os braços.
    fn estado(&self) -> (Vec<&str>, Vec<&str>) {
        let slots_escondidos: Vec<&str> = self
            .pecas
            .values()
            .filter(|(_, _, ligada)| *ligada)
            .flat_map(|(_, i, _)| i.esconde_slots.iter().map(String::as_str))
            .collect();
        let mut aparecem = Vec::new();
        let mut regioes = Vec::new();
        for (_, item, ligada) in self.pecas.values() {
            if *ligada && !slots_escondidos.contains(&item.slot.as_str()) {
                aparecem.push(item.slot.as_str());
                regioes.extend(item.esconde.iter().map(String::as_str));
            }
        }
        (aparecem, regioes)
    }

    fn custo(&self) -> Custo {
        let (aparecem, escondidas) = self.estado();
        let mut c = Custo::default();
        for (nome, (t, m)) in &self.custo_regiao {
            if !escondidas.contains(&nome.as_str()) {
                c.tris += t;
                c.malhas += m;
                c.skinadas += m;
            }
        }
        for slot in aparecem {
            let (_, item, _) = &self.pecas[slot];
            let m = self.malhas_peca.get(slot).copied().unwrap_or(1);
            c.tris += item.tris;
            c.malhas += m;
            if item.tipo == "skinado" {
                c.skinadas += m;
            }
        }
        c
    }

    /// Liga uma peça só se o boneco continuar dentro do teto.
    fn ligar(&mut self, slot: &str, ligar: bool) {
        let Some(p) = self.pecas.get_mut(slot) else { return };
        let antes = p.2;
        p.2 = ligar;
        if ligar && self.custo().tris > TETO_TRIS {
            let c = self.custo();
            self.pecas.get_mut(slot).unwrap().2 = antes;
            warn!("{slot}: não equipada, o boneco ficaria com {} triângulos (teto {TETO_TRIS})", c.tris);
        }
    }
}

/// Marca a cena de uma peça enquanto ela carrega.
#[derive(Component)]
struct Peca {
    boneco: Entity,
    item: Item,
}

/// Clipes carregados do animacoes.glb, num grafo só (todos os bonecos usam o mesmo).
#[derive(Resource, Default)]
struct Animacoes {
    manifesto: Option<ManifestoAnimacoes>,
    gltf: Option<Handle<Gltf>>,
    grafo: Option<Handle<AnimationGraph>>,
    /// Nó do grafo e dados de cada clipe, na ordem do manifesto.
    clipes: Vec<(AnimationNodeIndex, Clipe)>,
    parado: usize,
    andar: Option<usize>,
}

impl Animacoes {
    fn achar(&self, nomes: &[&str]) -> Option<usize> {
        nomes.iter().find_map(|n| self.clipes.iter().position(|(_, c)| c.nome == *n))
    }
}

#[derive(Resource)]
struct Recursos {
    itens: Vec<Item>,
    base: Handle<WorldAsset>,
    atlas_rosto: Handle<Image>,
}

// ---------------------------------------------------------------------------
// Rosto: atlas 4x2 de expressões, 32x32 px cada célula.

const CEL: u32 = 32;
const COLS: u32 = 4;
const LINHAS: u32 = 2;
const PISCANDO: usize = 4;

/// `uv_transform` que mostra só a célula `i` do atlas no quadrado do rosto.
fn celula(i: usize) -> Affine2 {
    let (c, l) = ((i as u32 % COLS) as f32, (i as u32 / COLS) as f32);
    Affine2::from_scale_angle_translation(
        Vec2::new(1.0 / COLS as f32, 1.0 / LINHAS as f32),
        0.0,
        Vec2::new(c / COLS as f32, l / LINHAS as f32),
    )
}

/// Desenha o atlas em código. No app, cada item de olhos/boca é um PNG
/// neste mesmo formato (ver o doc); aqui é tudo gerado para não depender de arte.
fn desenhar_atlas() -> Image {
    let (w, h) = (CEL * COLS, CEL * LINHAS);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut pinta = |cel: usize, x: i32, y: i32, cor: [u8; 4]| {
        let ox = (cel as u32 % COLS) * CEL;
        let oy = (cel as u32 / COLS) * CEL;
        if (0..CEL as i32).contains(&x) && (0..CEL as i32).contains(&y) {
            let i = (((oy + y as u32) * w + ox + x as u32) * 4) as usize;
            px[i..i + 4].copy_from_slice(&cor);
        }
    };
    let preto = [20, 14, 20, 255];
    let branco = [245, 240, 235, 255];
    let rosa = [235, 120, 150, 255];
    // Olhos: dois blocos 5x7 em (8,10) e (19,10). Boca perto de y=24.
    for cel in 0..8usize {
        for (ex, _) in [(8, 0), (19, 0)] {
            match cel {
                // 0 neutro, 3 bravo: olho cheio com brilho
                0 | 3 => {
                    for y in 10..17 {
                        for x in ex..ex + 5 {
                            pinta(cel, x, y, preto);
                        }
                    }
                    pinta(cel, ex + 1, 11, branco);
                }
                // 1 feliz: ^ ^
                1 | 5 => {
                    for d in 0..3 {
                        pinta(cel, ex + d, 14 - d, preto);
                        pinta(cel, ex + 4 - d, 14 - d, preto);
                    }
                }
                // 2 surpreso: O O
                2 => {
                    for y in 9..18 {
                        for x in ex - 1..ex + 6 {
                            let borda = y == 9 || y == 17 || x == ex - 1 || x == ex + 5;
                            pinta(cel, x, y, if borda { preto } else { branco });
                        }
                    }
                    pinta(cel, ex + 2, 13, preto);
                }
                // 4 piscando: traço
                4 => {
                    for x in ex..ex + 5 {
                        pinta(cel, x, 14, preto);
                    }
                }
                _ => {}
            }
        }
        // sobrancelha brava
        if cel == 3 {
            for d in 0..5 {
                pinta(cel, 7 + d, 6 + d / 2, preto);
                pinta(cel, 24 - d, 6 + d / 2, preto);
            }
        }
        // bochechas no feliz
        if cel == 1 || cel == 5 {
            for x in [5, 6, 25, 26] {
                pinta(cel, x, 19, rosa);
            }
        }
        // boca
        match cel {
            1 | 5 => {
                for x in 13..19 {
                    pinta(cel, x, 23 + i32::from(x == 13 || x == 18) * -1, preto);
                }
            }
            2 => {
                for y in 22..26 {
                    for x in 14..18 {
                        pinta(cel, x, y, preto);
                    }
                }
            }
            0 | 3 | 4 => {
                for x in 14..18 {
                    pinta(cel, x, 24, preto);
                }
            }
            _ => {}
        }
    }
    Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

// ---------------------------------------------------------------------------

fn iniciar(mut cmd: Commands, assets: Res<AssetServer>, mut imagens: ResMut<Assets<Image>>) {
    let mut atlas = desenhar_atlas();
    atlas.sampler = ImageSampler::nearest();
    let rec = Recursos {
        itens: ler_manifestos(),
        base: assets.load(GltfAssetLabel::Scene(0).from_asset(format!("{}/base.glb", pasta_avatar()))),
        atlas_rosto: imagens.add(atlas),
    };

    for i in 0..BONECOS {
        let x = (i % 5) as f32 * 0.7 - 1.4;
        let z = (i / 5) as f32 * -0.9;
        let boneco = cmd
            .spawn((
                Name::new(format!("boneco {i}")),
                Boneco { fase: i as f32 * 0.7, origem: Vec3::new(x, 0.0, z), ..default() },
                Transform::from_xyz(x, 0.0, z),
                Visibility::default(),
            ))
            .id();
        cmd.spawn((WorldAssetRoot(rec.base.clone()), ChildOf(boneco)))
            .observe(base_pronta);
    }
    cmd.insert_resource(rec);

    let manifesto = ler_animacoes();
    let gltf = manifesto.as_ref().map(|m| assets.load(format!("{}/{}", pasta_avatar(), m.arquivo)));
    cmd.insert_resource(Animacoes { manifesto, gltf, ..default() });

    cmd.spawn((Camera3d::default(), Transform::from_xyz(0.0, 1.3, 3.2).looking_at(Vec3::new(0.0, 0.45, -0.45), Vec3::Y)));
    cmd.spawn((
        DirectionalLight { illuminance: 6000.0, ..default() },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    cmd.insert_resource(GlobalAmbientLight { brightness: 600.0, ..default() });
}

/// O corpo carregou: guarda os ossos e regiões e pede as peças.
fn base_pronta(
    ev: On<WorldInstanceReady>,
    mut cmd: Commands,
    pais: Query<&ChildOf>,
    filhos: Query<&Children>,
    nomes: Query<(&Name, &Transform)>,
    mut bonecos: Query<&mut Boneco>,
    rec: Res<Recursos>,
    malhas: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
) {
    let raiz = ev.entity;
    let Ok(boneco) = pais.get(raiz).map(ChildOf::parent) else { return };
    let Ok(mut b) = bonecos.get_mut(boneco) else { return };
    for e in filhos.iter_descendants(raiz) {
        if let Ok((nome, t)) = nomes.get(e) {
            // O Bevy cria o nó "corpo_x" e, embaixo, a malha "corpo_x.0" (ou parecido):
            // só o nó conta, senão a região entra duas vezes no custo.
            let ja_tem = b.regioes.keys().any(|k| nome.starts_with(k.as_str()));
            if nome.starts_with("corpo_") && ja_tem {
                continue;
            }
            if nome.starts_with("corpo_") {
                b.regioes.insert(nome.to_string(), e);
                let c = medir(e, &filhos, &malhas, &meshes);
                b.custo_regiao.insert(nome.to_string(), c);
            } else {
                b.ossos.insert(nome.to_string(), e);
                b.descanso.insert(e, *t);
            }
        }
    }
    for item in &rec.itens {
        equipar(&mut cmd, boneco, item.clone());
    }
}

/// (triângulos, malhas) de uma entidade e descendentes.
fn medir(e: Entity, filhos: &Query<&Children>, malhas: &Query<&Mesh3d>, meshes: &Assets<Mesh>) -> (u32, u32) {
    let mut c = (0, 0);
    for x in std::iter::once(e).chain(filhos.iter_descendants(e)) {
        if let Ok(m) = malhas.get(x) {
            c.1 += 1;
            if let Some(mesh) = meshes.get(&m.0) {
                c.0 += mesh.indices().map_or(mesh.count_vertices(), |i| i.len()) as u32 / 3;
            }
        }
    }
    c
}

fn equipar(cmd: &mut Commands, boneco: Entity, item: Item) {
    cmd.queue(move |world: &mut World| {
        let cena = world.resource::<AssetServer>().load(GltfAssetLabel::Scene(0).from_asset(item.arquivo.clone()));
        // Começa escondida; aparece quando estiver encaixada (evita piscar fora do lugar).
        world
            .spawn((WorldAssetRoot(cena), Peca { boneco, item }, ChildOf(boneco), Visibility::Hidden))
            .observe(peca_pronta);
    });
}

/// Transform do osso em relação à raiz do boneco, na pose de descanso.
fn pose_descanso(osso: Entity, boneco: Entity, pais: &Query<&ChildOf>, ts: &Query<&Transform>) -> Transform {
    let mut acc = Transform::IDENTITY;
    let mut e = osso;
    while e != boneco {
        let t = ts.get(e).copied().unwrap_or_default();
        acc = t * acc;
        let Ok(ChildOf(p)) = pais.get(e) else { break };
        e = *p;
    }
    acc
}

#[allow(clippy::too_many_arguments)]
fn peca_pronta(
    ev: On<WorldInstanceReady>,
    mut cmd: Commands,
    pecas: Query<&Peca>,
    pais: Query<&ChildOf>,
    filhos: Query<&Children>,
    nomes: Query<&Name>,
    ts: Query<&Transform>,
    mut skins: Query<&mut SkinnedMesh>,
    mut bonecos: Query<&mut Boneco>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    rec: Res<Recursos>,
    malhas: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
) {
    let raiz = ev.entity;
    let Ok(peca) = pecas.get(raiz) else { return };
    let (boneco_e, item) = (peca.boneco, peca.item.clone());
    let Ok(mut b) = bonecos.get_mut(boneco_e) else { return };

    match (item.tipo.as_str(), item.osso.as_deref()) {
        ("rigido", Some(osso)) => {
            // A peça foi modelada no lugar (espaço do boneco). Pendurar no osso e
            // desfazer a pose de descanso do osso deixa ela exatamente onde foi modelada,
            // e daí em diante ela segue o osso.
            let Some(&osso_e) = b.ossos.get(osso) else {
                warn!("{}: osso {osso} não existe", item.id);
                return;
            };
            let rest = pose_descanso(osso_e, boneco_e, &pais, &ts);
            let local = Transform::from_matrix(rest.to_matrix().inverse());
            cmd.entity(raiz).insert((ChildOf(osso_e), local));
        }
        _ => {
            // A peça veio com uma cópia do esqueleto. Troca cada junta pela junta
            // de mesmo nome do corpo: a peça passa a deformar com os ossos do boneco.
            for e in filhos.iter_descendants(raiz) {
                if let Ok(mut skin) = skins.get_mut(e) {
                    for j in skin.joints.iter_mut() {
                        let nome = nomes.get(*j).map(|n| n.as_str()).unwrap_or("");
                        match b.ossos.get(nome) {
                            Some(&novo) => *j = novo,
                            None => warn!("{}: osso {nome} não existe no corpo", item.arquivo),
                        }
                    }
                }
            }
        }
    }

    // Rosto: troca o material do objeto "rosto" por um material próprio deste boneco.
    if item.slot == "cabeca" {
        let mat = mats.add(StandardMaterial {
            base_color_texture: Some(rec.atlas_rosto.clone()),
            alpha_mode: AlphaMode::Mask(0.5),
            unlit: true,
            uv_transform: celula(0),
            ..default()
        });
        for e in filhos.iter_descendants(raiz) {
            let dentro_do_rosto = std::iter::once(e)
                .chain(pais.iter_ancestors(e))
                .take_while(|&a| a != raiz)
                .any(|a| nomes.get(a).is_ok_and(|n| n.as_str() == "rosto"));
            if dentro_do_rosto {
                cmd.entity(e).insert(MeshMaterial3d(mat.clone()));
            }
        }
        b.rosto = mat;
    }

    let slot = item.slot.clone();
    b.malhas_peca.insert(slot.clone(), medir(raiz, &filhos, &malhas, &meshes).1);
    b.pecas.insert(slot.clone(), (raiz, item, false));
    b.ligar(&slot, true); // respeita o teto de triângulos do boneco
    // Visibility é aplicada pelo sistema `visibilidade` (o comando acima ainda não rodou).
}

/// Aplica `Boneco::estado` nas entidades (só muda o que mudou).
fn visibilidade(bonecos: Query<&Boneco>, mut vis: Query<&mut Visibility>) {
    for b in &bonecos {
        let (aparecem, escondidas) = b.estado();
        for (slot, (e, _, _)) in &b.pecas {
            if let Ok(mut v) = vis.get_mut(*e) {
                let sim = aparecem.contains(&slot.as_str());
                v.set_if_neq(if sim { Visibility::Inherited } else { Visibility::Hidden });
            }
        }
        for (nome, e) in &b.regioes {
            if let Ok(mut v) = vis.get_mut(*e) {
                let esconde = escondidas.contains(&nome.as_str());
                v.set_if_neq(if esconde { Visibility::Hidden } else { Visibility::Inherited });
            }
        }
    }
}

/// Animação de teste, procedural: balança cabeça, tronco e braços (só sem animacoes.json).
fn animar(time: Res<Time>, bonecos: Query<&Boneco>, mut ts: Query<&mut Transform>, anim: Res<Animacoes>) {
    if anim.manifesto.is_some() {
        return;
    }
    let t = time.elapsed_secs();
    for b in &bonecos {
        let s = (t * 2.0 + b.fase).sin();
        for (osso, eixo, amp) in [
            ("head", Vec3::Z, 0.15),
            ("spine", Vec3::Z, 0.06),
            ("leftUpperArm", Vec3::X, 0.5),
            ("rightUpperArm", Vec3::X, -0.5),
            ("leftUpperLeg", Vec3::X, 0.25),
            ("rightUpperLeg", Vec3::X, -0.25),
        ] {
            let Some(&e) = b.ossos.get(osso) else { continue };
            let Ok(mut tr) = ts.get_mut(e) else { continue };
            tr.rotation = b.descanso[&e].rotation * Quat::from_axis_angle(eixo, s * amp);
        }
    }
}

/// Quando o animacoes.glb termina de carregar, monta um grafo com todos os clipes.
fn montar_grafo(mut anim: ResMut<Animacoes>, assets: Res<AssetServer>, gltfs: Res<Assets<Gltf>>, mut grafos: ResMut<Assets<AnimationGraph>>) {
    if anim.grafo.is_some() {
        return;
    }
    let (Some(h), Some(m)) = (anim.gltf.clone(), anim.manifesto.clone()) else { return };
    if !assets.is_loaded_with_dependencies(&h) {
        return;
    }
    let Some(gltf) = gltfs.get(&h) else { return };
    let mut handles = Vec::new();
    let mut dados = Vec::new();
    for c in &m.clipes {
        match gltf.named_animations.get(c.nome.as_str()) {
            Some(clip) => {
                handles.push(clip.clone());
                dados.push(c.clone());
            }
            None => warn!("clipe {} está no animacoes.json mas não no {}", c.nome, m.arquivo),
        }
    }
    if handles.is_empty() {
        warn!("{}: nenhum clipe; usando a animação de teste", m.arquivo);
        anim.manifesto = None;
        return;
    }
    let (grafo, nos) = AnimationGraph::from_clips(handles);
    anim.clipes = nos.into_iter().zip(dados).collect();
    // AVATAR_CLIPE=andar começa com esse clipe (útil com AVATAR_FOTO)
    let inicial = std::env::var("AVATAR_CLIPE").ok();
    anim.parado = inicial
        .as_deref()
        .and_then(|n| anim.achar(&[n]))
        .or_else(|| anim.achar(&NOMES_PARADO))
        .unwrap_or(0);
    anim.andar = anim.achar(&NOMES_ANDAR);
    anim.grafo = Some(grafos.add(grafo));
}

/// Liga os ossos do corpo aos clipes. O base.glb não tem animação, então o glTF não
/// marcou os ossos: o app marca cada um com o mesmo id que o clipe usa (o caminho de
/// nomes desde o nó "rig": rig/hips/spine/...) e põe o AnimationPlayer no "rig".
/// As peças skinadas já usam esses ossos e as rígidas estão penduradas neles,
/// então tudo anima junto.
fn ligar_animacao(
    mut cmd: Commands,
    mut bonecos: Query<&mut Boneco>,
    anim: Res<Animacoes>,
    pais: Query<&ChildOf>,
    nomes: Query<&Name>,
) {
    let Some(grafo) = anim.grafo.clone() else { return };
    for mut b in &mut bonecos {
        if b.animado {
            continue;
        }
        let Some(&rig) = b.ossos.get("rig") else { continue };
        for &osso in b.ossos.values() {
            let mut caminho = Vec::new();
            let mut e = osso;
            loop {
                let Ok(n) = nomes.get(e) else { break };
                caminho.push(n.clone());
                if e == rig {
                    break;
                }
                let Ok(ChildOf(p)) = pais.get(e) else { break };
                e = *p;
            }
            if e != rig {
                continue; // não está debaixo do rig
            }
            caminho.reverse();
            cmd.entity(osso).insert((AnimationTargetId::from_names(caminho.iter()), AnimatedBy(rig)));
        }
        let mut player = AnimationPlayer::default();
        let mut transicoes = AnimationTransitions::new();
        let (no, clipe) = &anim.clipes[anim.parado];
        let tocando = transicoes.play(&mut player, *no, Duration::ZERO);
        if clipe.loop_ {
            tocando.repeat();
        }
        tocando.seek_to(b.fase); // cada boneco num ponto do clipe, para não ficarem iguais
        cmd.entity(rig).insert((player, transicoes, AnimationGraphHandle(grafo.clone())));
        b.animado = true;
        b.clipe = anim.parado;
    }
}

/// W alterna andar/parado; N passa para o próximo clipe (para ver todos).
fn trocar_clipe(
    teclas: Res<ButtonInput<KeyCode>>,
    anim: Res<Animacoes>,
    mut bonecos: Query<&mut Boneco>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    if anim.clipes.is_empty() {
        return;
    }
    let andar = teclas.just_pressed(KeyCode::KeyW);
    let proximo = teclas.just_pressed(KeyCode::KeyN);
    if !andar && !proximo {
        return;
    }
    if andar && anim.andar.is_none() {
        warn!("nenhum clipe de andar ({})", NOMES_ANDAR.join(", "));
        return;
    }
    for mut b in &mut bonecos {
        let Some(&rig) = b.ossos.get("rig") else { continue };
        let Ok((mut player, mut transicoes)) = players.get_mut(rig) else { continue };
        let novo = if andar {
            let a = anim.andar.unwrap_or(0);
            if b.clipe == a { anim.parado } else { a }
        } else {
            (b.clipe + 1) % anim.clipes.len()
        };
        let (no, clipe) = &anim.clipes[novo];
        let tocando = transicoes.play(&mut player, *no, Duration::from_millis(250));
        if clipe.loop_ {
            tocando.repeat();
        }
        b.clipe = novo;
    }
    if let Some(b) = bonecos.iter().next() {
        info!("clipe: {}", anim.clipes[b.clipe].1.nome);
    }
}

/// Clipe com velocidade (andar): o boneco vai para a frente (+Z, para a câmera) nessa
/// velocidade e, depois de uns passos, volta para onde começou.
fn mover(time: Res<Time>, anim: Res<Animacoes>, mut bonecos: Query<(&Boneco, &mut Transform)>) {
    for (b, mut t) in &mut bonecos {
        let v = anim.clipes.get(b.clipe).filter(|_| b.animado).map_or(0.0, |(_, c)| c.velocidade);
        if v <= 0.0 {
            t.translation = b.origem;
            continue;
        }
        t.translation.z += v * time.delta_secs();
        if t.translation.z - b.origem.z > 0.8 {
            t.translation.z = b.origem.z;
        }
    }
}

fn teclado(
    teclas: Res<ButtonInput<KeyCode>>,
    mut bonecos: Query<&mut Boneco>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let exp = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6]
        .iter()
        .position(|k| teclas.just_pressed(*k));
    for mut b in &mut bonecos {
        if let Some(i) = exp
            && let Some(mut m) = mats.get_mut(&b.rosto)
        {
            m.uv_transform = celula(i);
        }
        for (tecla, slot) in [(KeyCode::KeyH, "cabelo"), (KeyCode::KeyC, "superior_base"), (KeyCode::KeyM, "superior_meio")] {
            if teclas.just_pressed(tecla)
                && let Some(&(_, _, ligada)) = b.pecas.get(slot)
            {
                b.ligar(slot, !ligada);
            }
        }
    }
    // F: custo de cada boneco como ele aparece agora.
    if teclas.just_pressed(KeyCode::KeyF) {
        for b in &bonecos {
            let c = b.custo();
            info!("{:?}  ({}/{TETO_TRIS} tris)", c, c.tris);
        }
    }
}

/// Pisca sozinho: a cada ~3 s troca para a célula "piscando" por 0,12 s e volta.
fn piscar(time: Res<Time>, bonecos: Query<&Boneco>, mut mats: ResMut<Assets<StandardMaterial>>, mut antes: Local<HashMap<AssetId<StandardMaterial>, Affine2>>) {
    let t = time.elapsed_secs();
    for b in &bonecos {
        let ciclo = (t + b.fase * 1.3) % 3.0;
        let Some(mut m) = mats.get_mut(&b.rosto) else { continue };
        let fechado = m.uv_transform == celula(PISCANDO);
        if ciclo < 0.12 && !fechado {
            antes.insert(b.rosto.id(), m.uv_transform);
            m.uv_transform = celula(PISCANDO);
        } else if ciclo >= 0.12 && fechado {
            m.uv_transform = antes.get(&b.rosto.id()).copied().unwrap_or(celula(0));
        }
    }
}

/// `AVATAR_FOTO=arquivo.png cargo run` salva um print depois de 3 s e fecha (para revisão).
fn foto(mut cmd: Commands, time: Res<Time>, mut feito: Local<u8>, mut sair: MessageWriter<AppExit>, bonecos: Query<&Boneco>) {
    let Ok(caminho) = std::env::var("AVATAR_FOTO") else { return };
    let t = time.elapsed_secs();
    if *feito == 0 && t > 3.0 {
        *feito = 1;
        if let Some(b) = bonecos.iter().next() {
            info!("custo de um boneco: {:?}", b.custo());
        }
        cmd.spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(caminho));
    } else if *feito == 1 && t > 5.0 {
        sair.write(AppExit::Success);
    }
}
