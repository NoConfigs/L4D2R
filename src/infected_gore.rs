//! Owned damage-cutout definitions and bounded client gib effects.
//! Wound selection follows the owned server (CInfected damage dispatch
//! 10405d90, pickers 104050a0/10404f20/104051b0, SetWound 104034b0); the
//! mesh cutout and gib contact are reconstructions.
use crate::{infected_death::Cause, kv, source_assets::SourceAssets};
use bevy::{camera::visibility::NoFrustumCulling, mesh::VertexAttributeValues, prelude::*};
use std::{
    collections::{BTreeSet, VecDeque},
    io,
};
/// Projection coordinates must never share the visible vertex-colour channel:
/// mesh and material uploads can reach the render world on different frames.
pub(crate) const WOUND_BIND_POSITION: bevy::mesh::MeshVertexAttribute =
    bevy::mesh::MeshVertexAttribute::new("SourceWoundBindPosition", 91341,
        bevy::render::render_resource::VertexFormat::Float32x3);

#[derive(Clone, Debug)]
pub struct Volume {
    pub center: Vec3,
    pub scale: Vec3,
    pub look: Vec3,
    pub up: Vec3,
}
impl Volume {
    pub fn contains(&self, point: Vec3) -> bool {
        let z = self.look.normalize_or_zero();
        let x = z.cross(self.up.normalize_or_zero());
        let y = x.cross(z);
        let p = point - self.center;
        let p = Vec3::new(p.dot(x), p.dot(y), p.dot(z)) / self.scale;
        p.length_squared() < 1.
    }
}
#[derive(Clone, Debug)]
pub struct Wound {
    pub profile: String,
    pub model: String,
    pub types: Vec<String>,
    pub boxes: Vec<i32>,
    pub fatal: bool,
    pub exclusions: Vec<String>,
    pub gibs: Vec<String>,
    pub effects: Vec<(String, u32)>,
    pub attachment: String,
    pub volumes: Vec<Volume>,
    /// Native Type bits (cutout parser 103523a0): gunshot 1, explosion 2,
    /// dismember 4, slash 8, blunt 0x10, chainsaw 0x20, decapitate 0x40,
    /// headshot 0x80.
    pub flags: u32,
    /// SwingStart/SwingEnd compass directions (N S E W NE SE SW NW = 0..7,
    /// 10351970); None when absent (native -1).
    pub swing: (Option<u8>, Option<u8>),
    /// "Back": the slash wound is cut from behind.
    pub back: bool,
    /// Native volume count: 2 with a Secondary block, otherwise 1, even
    /// when the Primary block is empty (slash wounds).
    pub volume_count: u8,
    /// "DeathThroes" sequence the death plays instead (10415d80).
    pub death_throes: String,
}
pub const WOUND_GUNSHOT: u32 = 1;
pub const WOUND_EXPLOSION: u32 = 2;
pub const WOUND_DISMEMBER: u32 = 4;
pub const WOUND_SLASH: u32 = 8;
pub const WOUND_BLUNT: u32 = 0x10;
pub const WOUND_CHAINSAW: u32 = 0x20;
pub const WOUND_DECAPITATE: u32 = 0x40;
pub const WOUND_HEADSHOT: u32 = 0x80;
/// Native damage-type bits the wound dispatch reads.
pub const DMG_BULLET: u32 = 0x2;
pub const DMG_SLASH: u32 = 0x4;
pub const DMG_BLAST: u32 = 0x40;
pub const DMG_CLUB: u32 = 0x80;
pub const DMG_MELEE: u32 = 0x20_0000;
/// AMMO_TYPE_M60 / AMMO_TYPE_MINIGUN: every hit takes a random explosion
/// wound (10405d90, 10404c10).
pub const DMG_EXPLOSIVE_BULLET: u32 = 0x100_0000;
pub const DMG_CHAINSAW: u32 = 0x400_0000;
pub const DMG_BUCKSHOT: u32 = 0x2000_0000;
pub const DMG_HEADSHOT: u32 = 0x4000_0000;
pub const DMG_DISMEMBER: u32 = 0x8000_0000;
/// The owned ammo table (102cfef0) damage types by weapon class.
pub fn bullet_damage_type(class: &str) -> u32 {
    match class {
        "weapon_pistol_magnum" | "weapon_rifle" | "weapon_rifle_ak47" | "weapon_rifle_desert"
        | "weapon_rifle_sg552" | "weapon_hunting_rifle" | "weapon_sniper_military"
        | "weapon_sniper_scout" | "weapon_sniper_awp" => DMG_DISMEMBER | DMG_BULLET,
        "weapon_pumpshotgun" | "weapon_shotgun_chrome" | "weapon_autoshotgun"
        | "weapon_shotgun_spas" => DMG_DISMEMBER | DMG_BUCKSHOT | DMG_BULLET,
        "weapon_rifle_m60" | "prop_minigun" | "prop_minigun_l4d1" | "prop_mounted_machine_gun" => {
            DMG_DISMEMBER | DMG_EXPLOSIVE_BULLET | DMG_BULLET
        }
        _ => DMG_BULLET,
    }
}
fn compass(name: &str) -> Option<u8> {
    Some(match name.to_ascii_uppercase().as_str() {
        "N" => 0,
        "S" => 1,
        "E" => 2,
        "W" => 3,
        "NE" => 4,
        "SE" => 5,
        "SW" => 6,
        "NW" => 7,
        _ => return None,
    })
}
#[derive(Default)]
pub struct Registry {
    pub wounds: Vec<Wound>,
}
fn profile(model: &str) -> &'static str {
    if model.contains("ceda") {
        "ceda"
    } else if model.contains("riot") {
        "laughriot"
    } else if model.contains("female") && model.contains("l4d1") {
        "female_l4d1"
    } else if model.contains("female") {
        "female"
    } else if model.contains("l4d1") {
        "l4d1"
    } else {
        "male"
    }
}
impl Registry {
    pub fn load(assets: &mut SourceAssets) -> io::Result<Self> {
        let mut out = Self::default();
        for p in ["male", "female", "l4d1", "female_l4d1", "ceda", "laughriot"] {
            let path = if p == "male" {
                "scripts/damagecutout.txt".into()
            } else {
                format!("scripts/damagecutout_{p}.txt")
            };
            let bytes = assets.read(&path)?;
            // The Windows definitions contain conditional duplicate Gibs
            // keys. Drop the Xbox branch before the simple KV parser sees it.
            // A repeated DamageCutoutN keeps its first block, as the native
            // FindKey does; the simple parser would keep the last.
            let mut seen = BTreeSet::new();
            let text = String::from_utf8_lossy(&bytes)
                .lines()
                .filter(|line| !line.contains("[$X360]"))
                .map(|line| {
                    let line = line.replace("[$WIN32]", "");
                    let key = line.trim().trim_matches('"').to_ascii_lowercase();
                    if key.starts_with("damagecutout") && !key.contains(char::is_whitespace)
                        && !seen.insert(key.clone())
                    {
                        line.replacen('"', "\"duplicate_", 1)
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let block = kv::blocks(&text);
            let Some(data) = block.children.get("damagecutoutdata") else {
                continue;
            };
            // Native table order: DamageCutout1..DamageCutout50, each entry
            // only with Model, HitBoxes, ExclusionList, Type and Fatal keys.
            for n in 1..=50 {
                let Some(b) = data.children.get(&format!("damagecutout{n}")) else {
                    continue;
                };
                if !["hitboxes", "exclusionlist", "type", "fatal"].iter().all(|k| b.values.contains_key(*k)) {
                    continue;
                }
                let Some(name) = b.values.get("model") else {
                    continue;
                };
                let model = format!("models/infected/{name}.mdl");
                if !assets.exists(&model) {
                    continue;
                }
                let words = |key: &str| {
                    b.values
                        .get(key)
                        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
                        .unwrap_or_default()
                };
                let effects = words("particleeffect")
                    .chunks_exact(2)
                    .filter_map(|p| Some((p[0].clone(), p[1].parse().ok()?)))
                    .collect();
                let volumes = ["primary", "secondary"]
                    .into_iter()
                    .filter_map(|key| {
                        let v = b.children.get(key)?;
                        let vector = |name| Vec3::from_array(kv::vector(v.values.get(name)));
                        let scale = vector("scale");
                        // Negative axes mirror the native wound texture. They
                        // are valid dimensions, not absent/invalid volumes.
                        (scale.is_finite() && scale.abs().min_element() > 1e-6).then(|| Volume {
                            center: vector("center"),
                            scale,
                            look: vector("lookat"),
                            up: vector("up"),
                        })
                    })
                    .collect();
                let gibs = words("gibs")
                    .into_iter()
                    .map(|g| format!("models/infected/limbs/{g}.mdl"))
                    .filter(|g| assets.exists(g))
                    .collect();
                let types = words("type");
                let flags = types.iter().fold(0, |f, t| f | match t.to_ascii_lowercase().as_str() {
                    "gunshot" => WOUND_GUNSHOT,
                    "explosion" => WOUND_EXPLOSION,
                    "dismember" => WOUND_DISMEMBER,
                    "slash" => WOUND_SLASH,
                    "blunt" => WOUND_BLUNT,
                    "chainsaw" => WOUND_CHAINSAW,
                    "decapitate" => WOUND_DECAPITATE,
                    "headshot" => WOUND_HEADSHOT,
                    _ => 0,
                });
                out.wounds.push(Wound {
                    profile: p.into(),
                    model,
                    flags,
                    swing: (
                        b.values.get("swingstart").and_then(|s| compass(s)),
                        b.values.get("swingend").and_then(|s| compass(s)),
                    ),
                    back: b.values.get("back").is_some_and(|s| s.trim().parse::<i32>().unwrap_or(0) != 0),
                    volume_count: if b.children.contains_key("secondary") { 2 } else { 1 },
                    death_throes: b.values.get("deaththroes").cloned().unwrap_or_default(),
                    types,
                    boxes: words("hitboxes")
                        .iter()
                        .filter_map(|s| s.parse().ok())
                        .collect(),
                    fatal: b.values.get("fatal").is_some_and(|s| s == "1"),
                    exclusions: words("exclusionlist"),
                    gibs,
                    effects,
                    attachment: b.values.get("particleattach").cloned().unwrap_or_default(),
                    volumes,
                });
            }
        }
        Ok(out)
    }
    pub fn paths(&self) -> Vec<String> {
        self.wounds
            .iter()
            .flat_map(|w| std::iter::once(w.model.clone()).chain(w.gibs.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    /// The owned CInfected wound dispatch (10405d90) for one hit, then the
    /// native SetWound admission (104034b0). `lethal` is the dispatch's
    /// "damage >= health" test.
    pub fn choose(&self, model: &str, hit: &Hit, lethal: bool, used: &[usize]) -> Option<usize> {
        if !model.contains("/common_") || used.len() >= 2 {
            return None;
        }
        let table = self
            .wounds
            .iter()
            .enumerate()
            .filter(|(_, w)| w.profile == profile(model))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let first = used.first().copied();
        let pick = |candidates: Vec<usize>| -> Option<usize> {
            // Native: at most eight candidates, then RandomInt over them.
            let candidates = &candidates[..candidates.len().min(8)];
            (!candidates.is_empty()).then(|| candidates[(hit.seed % candidates.len() as u64) as usize])
        };
        let on_box = |w: &Wound, b: i32| w.boxes.iter().any(|x| *x >= 0 && *x == b);
        let box_index = hit.box_index as i32;
        let damage = hit.damage_type;
        let with = |bits: u32| -> Vec<usize> {
            table.iter().copied().filter(|i| self.wounds[*i].flags & bits != 0).collect()
        };
        let decapitation = if hit.decapitates {
            // Melee weapons with "decapitates" try a decapitate wound at the
            // hit box first (104051b0(0x40, hitbox)).
            pick(with(WOUND_DECAPITATE).into_iter().filter(|i| on_box(&self.wounds[*i], box_index)).collect())
        } else {
            None
        };
        let chosen = match decapitation {
            Some(i) => i,
            None if damage & (DMG_BLAST | DMG_EXPLOSIVE_BULLET) != 0 => pick(with(WOUND_EXPLOSION))?,
            None if damage & DMG_CHAINSAW != 0 => pick(with(WOUND_CHAINSAW))?,
            None if damage & DMG_MELEE != 0 => {
                // 10404f20: slash damage takes dismember/slash cutouts, club
                // damage blunt ones; a slash cutout also needs the swing's
                // directions and side, and never stacks on another slash.
                pick(table.iter().copied().filter(|i| {
                    let w = &self.wounds[*i];
                    (damage & DMG_SLASH == 0 || w.flags & (WOUND_DISMEMBER | WOUND_SLASH) != 0)
                        && (damage & DMG_CLUB == 0 || w.flags & WOUND_BLUNT != 0)
                        && (w.flags & WOUND_SLASH == 0
                            || (hit.swing.is_some_and(|(a, b)| w.swing == (Some(a), Some(b)))
                                && w.back == hit.back
                                && first.is_none_or(|f| self.wounds[f].flags & WOUND_SLASH == 0)))
                        && on_box(w, box_index)
                }).collect())?
            }
            None => {
                // Bullets: only dismembering ammo (z_head_damage_causes_wounds
                // defaults to 0, so a pistol or SMG headshot cuts nothing).
                if damage & DMG_DISMEMBER == 0 {
                    return None;
                }
                let mut box_index = box_index;
                if hit.sniper && (box_index == 15 || box_index == 16) {
                    box_index = 14;
                }
                let fatal = lethal || damage & DMG_HEADSHOT != 0;
                pick(table.iter().copied().filter(|i| {
                    let w = &self.wounds[*i];
                    (!w.fatal || fatal) && w.flags & WOUND_GUNSHOT != 0 && on_box(w, box_index)
                }).collect())?
            }
        };
        self.admits(used, chosen).then_some(chosen)
    }
    /// SetWound (104034b0): a second wound needs both wounds single-volume,
    /// must differ from the first and must not be on the first's exclusion
    /// list.
    pub fn admits(&self, used: &[usize], wound: usize) -> bool {
        match used {
            [] => true,
            [first] => {
                let (a, b) = (&self.wounds[*first], &self.wounds[wound]);
                let name = b.model.rsplit('/').next().unwrap_or("").trim_end_matches(".mdl");
                *first != wound
                    && a.volume_count <= 1
                    && b.volume_count <= 1
                    && !a.exclusions.iter().any(|s| s.eq_ignore_ascii_case(name))
            }
            _ => false,
        }
    }
}
#[derive(Clone)]
pub struct Hit {
    pub entity: Entity,
    pub position: Vec3,
    pub direction: Vec3,
    /// Unscaled native damage impulse, kg*Source-inch/s, when measured/known.
    /// A direction alone cannot recover the weapon's native damage force.
    pub damage_force_source: Option<Vec3>,
    pub box_index: usize,
    pub cause: Cause,
    pub distance: f32,
    /// Native damage-type bits (DMG_*), including DMG_HEADSHOT for a head hit.
    pub damage_type: u32,
    /// AMMO_TYPE_HUNTINGRIFLE / AMMO_TYPE_SNIPERRIFLE: hit boxes 15 and 16
    /// count as 14 for wounds.
    pub sniper: bool,
    /// Melee: the attack's swing directions and whether it struck from
    /// behind (attacker and victim facing within 90 degrees).
    pub swing: Option<(u8, u8)>,
    pub back: bool,
    /// Melee script "decapitates".
    pub decapitates: bool,
    pub seed: u64,
}
/// Network form of a hit's melee fields: sniper 1, back 2, decapitates 4,
/// swing present 8, start << 4, end << 8.
pub fn pack_melee(h: &Hit) -> u64 {
    let swing = h.swing.map_or(0, |(a, b)| 8 | (a as u64) << 4 | (b as u64) << 8);
    h.sniper as u64 | (h.back as u64) << 1 | (h.decapitates as u64) << 2 | swing
}
pub fn unpack_melee(h: &mut Hit, v: u64) {
    h.sniper = v & 1 != 0;
    h.back = v & 2 != 0;
    h.decapitates = v & 4 != 0;
    h.swing = (v & 8 != 0).then(|| ((v >> 4 & 0xf) as u8, (v >> 8 & 0xf) as u8));
}
impl Hit {
    /// Damage type for a bullet of `class` striking hit group `group`.
    pub fn bullet_type(class: &str, group: Option<i32>) -> u32 {
        bullet_damage_type(class) | if group == Some(1) { DMG_HEADSHOT } else { 0 }
    }
    pub fn is_sniper(class: &str) -> bool {
        matches!(class, "weapon_hunting_rifle" | "weapon_sniper_military" | "weapon_sniper_scout" | "weapon_sniper_awp")
    }
}

/// Ordinary bullet impulse at the stock phys_pushscale=1. The owned server's
/// ammo table (102cfef0) feeds CalculateBulletDamageForce (101e8f40), called
/// with 0.4 by the player/bot hit path 103c4b50. Units remain kg*Source-inch/s;
/// wound scaling and conversion to metres happen once when queuing a fragment.
/// This is per pellet, independent of range falloff and health damage.
pub(crate) fn bullet_damage_force_source(class: &str, direction: Vec3) -> Option<Vec3> {
    let ammo_force = match class {
        "weapon_smg" | "weapon_smg_silenced" | "weapon_smg_mp5" => 2000.,
        "weapon_pumpshotgun" | "weapon_shotgun_chrome" | "weapon_autoshotgun"
            | "weapon_shotgun_spas" => 800.,
        "weapon_pistol" | "weapon_pistol_dual" | "weapon_pistol_magnum"
            | "weapon_rifle" | "weapon_rifle_ak47" | "weapon_rifle_desert"
            | "weapon_rifle_sg552" | "weapon_rifle_m60" | "weapon_hunting_rifle"
            | "weapon_sniper_military" | "weapon_sniper_scout" | "weapon_sniper_awp" => 2400.,
        _ => return None,
    };
    direction.try_normalize().filter(|v| v.is_finite()).map(|v| v * ammo_force * 0.4)
}
#[derive(Component, Default)]
pub struct Wounds(pub Vec<usize>);
#[derive(Component)]
struct Cap {
    root: Entity,
    model: String,
    draw: usize,
    pose_hash: Option<u64>,
}
#[derive(Component)]
struct CutVersion(usize);
#[derive(Component)]
struct Gib {
    velocity: Vec3,
    spin: Vec3,
    life: f32,
    resting: bool,
    half_extents: Vec3,
    body: Option<crate::ragdoll::Body>,
    model: String,
    meshes: Vec<(Handle<Mesh>, usize)>,
}
struct PendingGib {
    root: Entity,
    parent_model: String,
    model: String,
    transform: Transform,
    direction: Vec3,
    impulse: Option<Vec3>,
    scatter: bool,
    seed: u64,
    frozen: Option<std::sync::Arc<crate::l4d2r_animation::FrozenGorePose>>,
    boomer_force: Option<Vec3>,
}
#[derive(Resource, Default)]
struct State {
    pending: VecDeque<PendingGib>,
    live: VecDeque<Entity>,
    effects: Vec<(usize, f32)>,
    cursor: usize,
    gib_cursor: usize,
    fixture_started: bool,
    scatter_rng: l4d2r_game_core::random::UniformRandomStream,
}

pub fn prepare(world: &mut l4d2r_game_core::entities::EntityWorld, registry: &Registry) {
    use l4d2r_game_core::{
        entities::{Ent, Kind},
        mapents::MapEnt,
    };
    let effects = registry
        .wounds
        .iter()
        .flat_map(|w| w.effects.iter().map(|e| e.0.clone()))
        .collect::<BTreeSet<_>>();
    for effect in effects {
        for slot in 0..2 {
            let name = format!("__rewrite_gore_{effect}_{slot}");
            if !world.find(&name).is_empty() {
                continue;
            }
            world.add_runtime_entity(Ent {
                me: MapEnt {
                    index: world.ents.len(),
                    classname: "info_particle_system".into(),
                    targetname: name.clone(),
                    origin: l4d2r_math::Vec3::ZERO,
                    angles: l4d2r_math::Vec3::ZERO,
                    mdl: None,
                    brush: None,
                    ent: l4d2_formats::bsp::Entity(vec![
                        ("classname".into(), "info_particle_system".into()),
                        ("targetname".into(), name),
                        ("effect_name".into(), effect.clone()),
                    ]),
                },
                conns: vec![],
                kind: Kind::Particle {
                    active: false,
                    started: 0.,
                },
                alive: true,
                templated: false,
                parent: None,
            });
        }
    }
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub(crate) struct GoreUpdate;
pub fn install(app: &mut App) {
    app.init_resource::<State>()
        .add_systems(First, reserve_physics.after(crate::ragdoll::refill_budget).after(bevy::time::TimeSystems))
        .add_systems(
        PostUpdate,
        update
            .in_set(GoreUpdate)
            .after(crate::l4d2r_animation::update)
            .before(bevy::transform::TransformSystems::Propagate),
    );
}
fn reserve_physics(time: Res<Time>, gibs: Query<&Gib>) {
    let demand = gibs.iter().filter_map(|g| g.body.as_ref())
        .map(|b| b.steps_due(time.delta_secs())).sum::<u32>();
    crate::ragdoll::reserve_gib_steps(demand);
}
pub fn reset(world: &mut World) {
    let gibs = world
        .query_filtered::<Entity, With<Gib>>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in gibs {
        world.despawn(entity);
    }
    world.insert_resource(State::default());
}
#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    time: Res<Time>,
    game: Res<crate::game_state::Match>,
    frontend: Res<crate::game_menu::Frontend>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    mut playback: ResMut<crate::l4d2r_animation::PlaybackState>,
    models: Res<crate::viewer::RenderModels>,
    collision: Res<crate::collision::Collision>,
    mut impacts: ResMut<crate::bullet_impacts::Impacts>,
    mut state: ResMut<State>,
    render_assets: (ResMut<Assets<Mesh>>, ResMut<Assets<crate::viewer::SourceMaterial>>, Option<ResMut<crate::special_infected::death_effects::Pool>>),
    mut enemies: Query<
        (Entity, &mut crate::game::Enemy, &Transform, Option<&Wounds>),
        Without<Gib>,
    >,
    mut caps: Query<(&mut Cap, &Mesh3d)>,
    mut bodies: Query<(
        Entity,
        &crate::l4d2r_animation::AnimatedDraw,
        &mut Mesh3d,
        Option<&CutVersion>,
        &mut MeshMaterial3d<crate::viewer::SourceMaterial>,
        Has<crate::l4d2r_animation::SharedSkin>,
    ), Without<Cap>>,
    mut gibs: Query<(Entity, &mut Gib, &mut Transform), Without<crate::game::Enemy>>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
) {
    let (mut meshes, mut materials, mut death_effects) = render_assets;
    if !frontend.playing() || game.paused {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    let now = logic.world.time;
    // Native Boomer corpses use an exploded lower-body model plus five
    // separately bonemerged chunks, rather than the intact living mesh.
    for (root, mut enemy, transform, _) in &mut enemies {
        if enemy.health > 0. { continue; }
        let Some(corpse) = crate::special_infected::boomer_gibs::corpse(&enemy.model) else { continue };
        let ready = std::iter::once(corpse).chain(crate::special_infected::boomer_gibs::PARTS)
            .all(|p| models.0.get(&(p.to_owned(), 0, 0)).is_some_and(|d| !d.is_empty() && d.iter().all(|(m,_)| meshes.contains(m.id()))));
        if !ready { continue; }
        let parent = enemy.model.clone();
        let Some(snapshot) = library.freeze_gore_pose(&parent, root, &playback) else { continue };
        let Some(mapped) = library.remap_gore_pose(&parent, &snapshot, corpse) else { continue };
        let snapshot = std::sync::Arc::new(snapshot);
        let Some(body) = library.frozen_gib_body(&parent, &snapshot, corpse, transform, None) else { continue };
        // Install body and meshes together. New draws are already posed,
        // avoiding one bind-pose frame before GPU joints are created.
        for (entity, draw, _, _, _, _) in &bodies {
            if draw.root == root { commands.entity(entity).despawn(); }
        }
        for (draw, (mesh, material)) in models.0[&(corpse.to_owned(), 0, 0)].iter().enumerate() {
            let mut mesh = meshes.get(mesh).unwrap().clone();
            library.merge_frozen_gore(&parent, &snapshot, corpse, draw, &mut mesh);
            commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone()),
                Transform::default(), ChildOf(root), NoFrustumCulling,
                crate::l4d2r_animation::AnimatedDraw { root, model: corpse.into(), draw },
                crate::viewer::DynamicModelLighting::default()));
        }
        let corpse_smoke_position = body.center();
        library.replace_corpse(corpse, root, body, transform, &mut playback);
        enemy.model = corpse.into();
        if let Some(pool) = death_effects.as_mut() {
            if let Some(smoke) = pool.attach(&mut logic.world, "boomer_leg_smoke", root, corpse_smoke_position) {
                commands.entity(root).insert(smoke);
            }
        }
        while state.pending.len() + 5 > 32 { state.pending.pop_back(); }
        let snapshot = std::sync::Arc::new(mapped);
        let forces = crate::special_infected::boomer_gibs::forces(&mut state.scatter_rng);
        // Low Violence: no exploded_boomer_* limb chunks (client 10224210).
        let parts = if crate::low_violence::on() { &[][..] } else { &crate::special_infected::boomer_gibs::PARTS[..] };
        for (model, force) in parts.iter().copied().zip(forces).rev() {
            state.pending.push_front(PendingGib { root, parent_model: corpse.into(), model: model.into(),
                transform: *transform, direction: Vec3::from_array(crate::binary::source_normal(force.to_array())).normalize_or_zero(),
                impulse: None, scatter: false, seed: 0, frozen: Some(snapshot.clone()), boomer_force: Some(force) });
        }
        if std::env::args().any(|a| a == "--capture-ai-special") {
            println!("Boomer native split: root={root:?} corpse={corpse} chunks=5");
        }
    }
    static FIXTURE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    let fixture_delay = FIXTURE.get_or_init(|| {
        let args = std::env::args().collect::<Vec<_>>();
        args.iter().any(|a| a == "--capture-gore").then(|| {
            args.windows(2)
                .find(|a| a[0] == "--capture-gore-delay")
                .and_then(|a| a[1].parse::<f32>().ok())
                .unwrap_or(1.)
        })
    });
    if fixture_delay.is_some_and(|delay| game.elapsed > delay) && !state.fixture_started {
        let mut count = 0;
        for (entity, mut enemy, transform, _) in &mut enemies {
            if !enemy.model.contains("/common_") || enemy.health <= 0. {
                continue;
            }
            let cause = [Cause::Bullet, Cause::Shotgun, Cause::Slash][count];
            if !std::env::args().any(|a| a == "--capture-gore-alive") {
                enemy.health = 0.;
                enemy.corpse_time = 12.;
                enemy.record_death(cause, transform, Vec3::NEG_Z, 1.);
            }
            impacts.gore.push(Hit {
                entity,
                position: transform.translation + Vec3::Y,
                direction: Vec3::NEG_Z,
                damage_force_source: std::env::args().collect::<Vec<_>>().windows(2)
                    .find(|p| p[0] == "--capture-gore-force-weapon")
                    .and_then(|p| bullet_damage_force_source(&p[1], Vec3::NEG_Z)),
                box_index: [13, 20, 5][count],
                cause,
                distance: 2.,
                damage_type: [DMG_DISMEMBER | DMG_BULLET, DMG_DISMEMBER | DMG_BUCKSHOT | DMG_BULLET,
                    DMG_MELEE | DMG_SLASH | DMG_DISMEMBER][count],
                sniper: false,
                swing: None,
                back: false,
                decapitates: count == 2,
                seed: count as u64,
            });
            if std::env::args().any(|a| a == "--capture-gore-wound2") {
                let extra = impacts.gore.last().unwrap().clone();
                impacts.gore.push(extra);
            }
            count += 1;
            if count == 3 {
                break;
            }
        }
        state.fixture_started = count > 0;
    }
    state.effects.retain(|(id, until)| {
        if now < *until {
            return true;
        }
        if let Some(e) = logic.world.ents.get_mut(*id) {
            if let l4d2r_game_core::entities::Kind::Particle { active, .. } = &mut e.kind {
                *active = false;
            }
        }
        false
    });
    let mut changed = std::collections::HashMap::<Entity, Vec<usize>>::new();
    // Resolve hit events once; pellets cannot queue unbounded duplicate wounds.
    if crate::net_play::host() {
        let copies: Vec<Hit> = impacts.gore.iter().take(64).cloned().collect();
        impacts.net_gore.extend(copies);
    }
    // Fatal blasts per frame (10405d90): past z_fatal_blast_max_ragdolls
    // (16) a blast cuts no further wound this frame.
    let mut blasts = 0;
    let mut limp = Vec::new();
    for hit in std::mem::take(&mut impacts.gore).into_iter().take(64) {
        let Ok((_, enemy, transform, wounds)) = enemies.get(hit.entity) else {
            continue;
        };
        let used = changed
            .entry(hit.entity)
            .or_insert_with(|| wounds.map_or_else(Vec::new, |w| w.0.clone()));
        static FIXTURE_WOUND: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        let forced = FIXTURE_WOUND.get_or_init(|| {
            let args = std::env::args().collect::<Vec<_>>();
            if !args.iter().any(|a| a == "--capture-gore") { return vec![]; }
            ["--capture-gore-wound", "--capture-gore-wound2"].iter().filter_map(|key|
                args.windows(2).find(|a| a[0] == *key).map(|a| a[1].clone())).collect()
        });
        let selected = if crate::low_violence::on() {
            // Low Violence: SetWound (104034b0) never cuts a wound.
            None
        } else if !forced.is_empty() {
            forced.get(used.len()).and_then(|selector| {
                // Several native cutouts share one model (e.g. gunshot neck
                // bursts versus intact melee decapitation). Opt-in fixture
                // selector "model:type" makes those tests unambiguous.
                let (name, kind) = selector.split_once(':').map_or((selector.as_str(), None),
                    |(name, kind)| (name, Some(kind)));
                library.gore.wounds.iter().position(|w| w.profile == profile(&enemy.model)
                    && w.model.ends_with(&format!("/{name}.mdl"))
                    && kind.is_none_or(|kind| w.types.iter().any(|t| t == kind)))
            })
        } else if hit.damage_type & DMG_BLAST != 0 && {
            blasts += 1;
            blasts > 17
        } {
            None
        } else {
            library.gore.choose(&enemy.model, &hit, enemy.health <= 0., used)
        };
        if std::env::var_os("L4D2R_GORE_LOG").is_some() {
            println!("L4D2RGORE hit box={} type={:#x} lethal={} used={:?} wound={:?} ready={:?}",
                hit.box_index, hit.damage_type, enemy.health <= 0., used,
                selected.map(|i| &library.gore.wounds[i].model),
                selected.map(|i| models.0.get(&(library.gore.wounds[i].model.clone(), 0, 0)).map(|d| d.len())));
        }
        let Some(index) = selected else { continue; };
        let wound = &library.gore.wounds[index];
        // A cut without its interior mesh creates a see-through body. Wait for
        // a renderable cap before committing any damage cutout.
        if !models.0.get(&(wound.model.clone(), 0, 0))
            .is_some_and(|draws| !draws.is_empty() && draws.iter().all(|(m, _)| meshes.contains(m.id()))) {
            continue;
        }
        used.push(index);
        // DeathThroes "none": this body dies without an animation (10415d80).
        if wound.death_throes.eq_ignore_ascii_case("none") {
            limp.push(hit.entity);
        }
        if fixture_delay.is_some() {
            println!("Gore capture: root={:?} wound={} volumes={} caps={} elapsed={:.3} damage_force_source={:?}",
                hit.entity, wound.model, wound.volumes.len(), models.0[&(wound.model.clone(), 0, 0)].len(), game.elapsed, hit.damage_force_source);
        }
        let total = wound.effects.iter().map(|e| e.1).sum::<u32>();
        if total > 0 {
            let mut choice = hit.seed as u32 % total;
            let effect = wound
                .effects
                .iter()
                .find(|(_, weight)| {
                    if choice < *weight {
                        true
                    } else {
                        choice -= *weight;
                        false
                    }
                })
                .map(|e| &e.0);
            if let Some(effect) = effect {
                let name = format!("__rewrite_gore_{effect}_{}", state.cursor % 2);
                state.cursor += 1;
                if let Some(&id) = logic.world.find(&name).first() {
                    let position = library
                        .gore_attachment(&enemy.model, hit.entity, &playback, &wound.attachment)
                        .map_or(hit.position, |p| transform.transform_point(p));
                    let e = &mut logic.world.ents[id];
                    e.me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y)
                        / crate::collision::UNIT;
                    let d = hit.direction.normalize_or_zero();
                    e.me.angles = l4d2r_math::Vec3::new(
                        -d.y.asin().to_degrees(),
                        (-d.z).atan2(d.x).to_degrees(),
                        0.,
                    );
                    e.kind = l4d2r_game_core::entities::Kind::Particle {
                        active: true,
                        started: now,
                    };
                    state.effects.retain(|(old, _)| *old != id);
                    state.effects.push((id, now + 0.35));
                    if state.effects.len() > 16 {
                        let (old, _) = state.effects.remove(0);
                        if let Some(e) = logic.world.ents.get_mut(old) {
                            if let l4d2r_game_core::entities::Kind::Particle { active, .. } =
                                &mut e.kind
                            {
                                *active = false;
                            }
                        }
                    }
                }
            }
        }
        if let Some(draws) = models.0.get(&(wound.model.clone(), 0, 0)) {
            for (draw, (mesh, material)) in draws.iter().enumerate() {
                let Some(mut mesh) = meshes.get(mesh).cloned() else {
                    continue;
                };
                // The cutout is visible this frame. Attach its interior in the
                // same pose before spawning it; waiting for the Cap query on
                // the following frame exposes a bind-pose gap during motion.
                library.merge_gore_mesh(
                    &enemy.model, hit.entity, &playback, &wound.model, draw, &mut mesh,
                );
                let mesh = meshes.add(mesh);
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material.clone()),
                    Transform::default(),
                    ChildOf(hit.entity),
                    Cap {
                        root: hit.entity,
                        model: wound.model.clone(),
                        draw,
                        pose_hash: library.gore_pose_hash(hit.entity, &playback),
                    },
                    NoFrustumCulling,
                    crate::viewer::DynamicModelLighting::default(),
                ));
            }
        }
        for model in &wound.gibs {
            if state.pending.len() < 32 {
                state.pending.push_back(PendingGib {
                    root: hit.entity,
                    parent_model: enemy.model.clone(),
                    model: model.clone(),
                    transform: *transform,
                    direction: hit.direction,
                    impulse: hit.damage_force_source.map(|force| force *
                        if hit.cause == Cause::Explosion { 0.1 } else { 10. }
                        * crate::collision::UNIT),
                    scatter: wound.gibs.len() > 4,
                    seed: hit.seed,
                    frozen: None,
                    boomer_force: None,
                });
            }
        }
    }
    for (root, wounds) in &changed {
        commands.entity(*root).insert(Wounds(wounds.clone()));
    }
    for entity in limp {
        if let Ok((_, mut enemy, _, _)) = enemies.get_mut(entity) {
            enemy.death.ragdoll = true;
        }
    }
    for (entity, draw, mut mesh, version, mut material, shared) in &mut bodies {
        let Ok((_, _, _, wounds)) = enemies.get(draw.root) else {
            continue;
        };
        let Some(used) = changed.get(&draw.root).or_else(|| wounds.map(|w| &w.0)) else {
            continue;
        };
        if version.is_some_and(|v| v.0 == used.len()) {
            continue;
        }
        // Bevy's absent optional texture is white. Activating cutouts without
        // an authored mask would therefore discard the entire body draw.
        if !materials.get(&material.0).is_some_and(|m| m.extension.has_wound_mask()) {
            continue;
        }
        if shared {
            crate::l4d2r_animation::take_private_skin(&mut commands, entity, &mut mesh, &mut meshes);
        }
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            if let Some(rows) = library.cut_wounds(&draw.model, draw.draw, used, &mut mesh) {
                if let Some(native) = materials.get(&material.0) {
                    let mut native = native.clone();
                    native.extension.wound_rows = rows;
                    for (slot, index) in used.iter().take(2).enumerate() {
                        let w = &library.gore.wounds[*index];
                        native.extension.wound.params[slot].z = 1.;
                        native.extension.wound.params[slot].w = if w.volumes.len() > 1 { 1. } else { 0. };
                        // Combined texture: left half bullet, right half slash.
                        // Native client tests Type bit 8 (slash), not the
                        // independent decapitate flag, when selecting the atlas.
                        let slash = w.types.iter().any(|t| t == "slash");
                        native.extension.wound.atlas[slot].z = if slash && native.extension.wound.atlas[slot].x == 0.5 { 0.5 } else { 0. };
                    }
                    // MAY_DISCARD must also select a fragment stage for the
                    // depth/shadow prepass. A zero alpha threshold preserves
                    // opaque albedo while our wound shader performs the cut.
                    if native.base.alpha_mode == AlphaMode::Opaque {
                        native.base.alpha_mode = AlphaMode::Mask(0.);
                    }
                    material.0 = materials.add(native);
                    commands.entity(entity).insert(CutVersion(used.len()));
                }
            }
        }
    }
    let mut pose_hashes = std::collections::HashMap::new();
    for (mut cap, mesh) in &mut caps {
        // Parent GPU joints keep advancing off-screen. Keep the cap on that
        // same pose too: previous-frame visibility otherwise reveals a stale
        // interior for one frame as the actor returns to view.
        let Some(hash) = *pose_hashes
            .entry(cap.root)
            .or_insert_with(|| library.gore_pose_hash(cap.root, &playback))
        else {
            continue;
        };
        if cap.pose_hash == Some(hash) {
            continue;
        }
        let Ok((_, enemy, _, _)) = enemies.get(cap.root) else {
            continue;
        };
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            library.merge_gore_mesh(
                &enemy.model,
                cap.root,
                &playback,
                &cap.model,
                cap.draw,
                &mut mesh,
            );
            cap.pose_hash = Some(hash);
        }
    }
    // Retail z_gibs_per_frame=2 and z_gib_despawn_time=10. Visual pieces
    // have no survivor/common collision. Native fragment PHYs supply masses,
    // hulls and joints; the bounded local solver is not Valve IVP.
    state.live.retain(|e| gibs.contains(*e));
    let immediate = state.pending.iter().filter(|g| g.frozen.is_some()).count();
    for _ in 0..(2 + immediate) {
        let Some(mut gib) = state.pending.pop_front() else {
            break;
        };
        // Queued pieces must use the same current actor transform as the pose
        // being copied; the original hit transform can already be stale.
        if gib.frozen.is_none() {
            let Ok((_, _, transform, _)) = enemies.get(gib.root) else { continue };
            gib.transform = *transform;
        }
        let Some(draws) = models.0.get(&(gib.model.clone(), 0, 0)) else {
            continue;
        };
        let mut baked = Vec::new();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (draw, (handle, material)) in draws.iter().enumerate() {
            let Some(mut mesh) = meshes.get(handle).cloned() else {
                continue;
            };
            if let Some(snapshot) = &gib.frozen {
                library.merge_frozen_gore(&gib.parent_model, snapshot, &gib.model, draw, &mut mesh);
            } else {
                library.merge_gore_mesh(&gib.parent_model, gib.root, &playback, &gib.model, draw, &mut mesh);
            }
            if let Some(VertexAttributeValues::Float32x3(p)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            {
                for p in p {
                    let p = Vec3::from_array(*p);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            baked.push((mesh, material.clone(), draw));
        }
        if !lo.is_finite() || !hi.is_finite() {
            continue;
        }
        // The native limb force is an impulse applied independently to every
        // PHY body, not a normalized launch velocity or an upward kick. Unknown
        // weapon forces and fragments without PHY retain the directional fallback.
        let velocity = gib.direction.normalize_or_zero() * 3.;
        let mut body = if let Some(snapshot) = &gib.frozen {
            library.frozen_gib_body(&gib.parent_model, snapshot, &gib.model, &gib.transform, gib.boomer_force)
        } else { library.gib_body(
            &gib.parent_model,
            gib.root,
            &playback,
            &gib.model,
            &gib.transform,
            if gib.impulse.is_some() { Vec3::ZERO } else { velocity },
        ) };
        if let Some(body) = &mut body {
            let launch = body.launch_fragment(gib.impulse, gib.scatter, &mut state.scatter_rng);
            if fixture_delay.is_some() {
                println!("Gib launch: model={} scatter={} impulse_m={launch:?}", gib.model, gib.scatter);
            }
        }
        let center = if body.is_some() {
            Vec3::ZERO
        } else {
            (lo + hi) * 0.5
        };
        let mut gib_meshes = Vec::new();

        let seed = gib.seed.wrapping_mul(747796405);
        let spin = Vec3::new(3., (seed % 11) as f32 - 5., 4.);
        let baked = baked
            .into_iter()
            .map(|(mut mesh, material, draw)| {
                if let Some(VertexAttributeValues::Float32x3(p)) =
                    mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
                {
                    for p in p {
                        *p = (Vec3::from_array(*p) - center).to_array();
                    }
                }
                let handle = meshes.add(mesh);
                gib_meshes.push((handle.clone(), draw));
                (handle, material)
            })
            .collect::<Vec<_>>();
        let smoke_position = body.as_ref().map_or(gib.transform.transform_point(center), |body| body.center());
        let root = commands
            .spawn((
                Transform::from_translation(gib.transform.transform_point(center))
                    .with_rotation(gib.transform.rotation),
                Visibility::default(),
                Gib {
                    velocity,
                    spin,
                    life: 10.,
                    resting: false,
                    half_extents: ((hi - lo) * 0.5).max(Vec3::splat(0.02)),
                    body,
                    model: gib.model.clone(),
                    meshes: gib_meshes,
                },
            ))
            .id();
        for (mesh, material) in baked {
            commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::default(),
                ChildOf(root),
                NoFrustumCulling,
                crate::viewer::DynamicModelLighting::default(),
            ));
        }
        if gib.frozen.is_some() && !gib.model.ends_with("_head.mdl") {
            if let Some(pool) = death_effects.as_mut() {
                if let Some(smoke) = pool.attach(&mut logic.world, "boomer_chunk_smoke", root, smoke_position) {
                    commands.entity(root).insert(smoke);
                }
            }
        }
        state.live.push_back(root);
        if state.live.len() > 32 {
            if let Some(old) = state.live.pop_front() {
                commands.entity(old).despawn();
            }
        }
    }
    let ids = gibs.iter().map(|(id, _, _)| id).collect::<Vec<_>>();
    let start = state.gib_cursor % ids.len().max(1);
    let mut deferred = None;
    for k in 0..ids.len() {
        let index = (start + k) % ids.len();
        let Ok((entity, mut gib, mut transform)) = gibs.get_mut(ids[index]) else { continue };
        gib.life -= time.delta_secs();
        if gib.life <= 0. {
            commands.entity(entity).despawn();
            continue;
        }
        let was_sleeping = gib.body.as_ref().is_some_and(|b| b.sleeping());
        if let Some(body) = &mut gib.body {
            if !crate::ragdoll::take_gib_budget(body.steps_due(time.delta_secs())) {
                body.defer(time.delta_secs());
                deferred.get_or_insert(index);
                continue;
            }
            body.tick(time.delta_secs(), &collision);
            if !was_sleeping || !body.sleeping() {
                // Keep lighting/culling at the moving fragment, not the original
                // infected spawn point. Pose stays relative to this render root.
                transform.translation = body.center();
                let pose = body.pose(&transform);
                for (handle, draw) in &gib.meshes {
                    if let Some(mut mesh) = meshes.get_mut(handle) {
                        library.skin_gib(&gib.model, *draw, &pose, &mut mesh);
                    }
                }
            }
            continue;
        }
        if gib.resting {
            continue;
        }
        gib.velocity.y -= 800. * crate::collision::UNIT * dt;
        let start = transform.translation;
        let end = start + gib.velocity * dt;
        let axes = Mat3::from_quat(transform.rotation);
        let extents = axes.x_axis.abs() * gib.half_extents.x
            + axes.y_axis.abs() * gib.half_extents.y
            + axes.z_axis.abs() * gib.half_extents.z;
        let hit = collision.trace(start, end, extents);
        transform.translation = start.lerp(end, hit.fraction);
        transform.rotate(Quat::from_scaled_axis(gib.spin * dt));
        if hit.start_solid {
            gib.resting = true;
        } else if hit.fraction < 1. {
            let normal_speed = gib.velocity.dot(hit.normal);
            if normal_speed < 0. {
                gib.velocity -= hit.normal * normal_speed * 1.25;
            }
            gib.velocity *= 0.8;
            if hit.normal.y > 0.7 && gib.velocity.length() < 0.8 {
                gib.resting = true;
            }
        }
    }
    state.gib_cursor = deferred.unwrap_or(0);
    static AUDIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *AUDIT.get_or_init(|| std::env::args().any(|a| a == "--audit-gibs")) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true)
            .open("local-research/parity/gib-bugs-1021-motion.tsv") {
            for (entity, gib, transform) in &gibs {
                let p = transform.translation;
                let _ = writeln!(file, "{:.4}\t{entity:?}\t{}\t{:.5}\t{:.5}\t{:.5}\t{}",
                    game.elapsed, gib.model, p.x, p.y, p.z, gib.body.as_ref().is_some_and(|b| b.sleeping()));
            }
        }
    }
}

/// Affine rows mapping pelvis bind coordinates to each authored ellipsoid.
/// Zero rows disable unused slots. Negative scales preserve authored mirroring.
pub(crate) fn wound_rows<'a>(volumes: impl IntoIterator<Item = &'a Volume>) -> [Vec4; 12] {
    let mut rows = [Vec4::ZERO; 12];
    for (volume, output) in volumes.into_iter().take(4).zip(rows.chunks_exact_mut(3)) {
        let z = volume.look.normalize_or_zero();
        // stdshader_dx9 10017e40 normalizes LookAt/Up independently,
        // then forms Side = LookAt x Up and corrected Up = Side x LookAt.
        let x = z.cross(volume.up.normalize_or_zero());
        let y = x.cross(z);
        for ((axis, scale), row) in [x, y, z].into_iter().zip(volume.scale.to_array()).zip(output) {
            let axis = axis / scale;
            *row = axis.extend(-axis.dot(volume.center));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_bullet_impulses_distinguish_ammo_and_normalize_each_pellet() {
        use super::bullet_damage_force_source as force;
        use bevy::prelude::Vec3;
        for (classes, magnitude) in [
            (&["weapon_smg", "weapon_smg_silenced", "weapon_smg_mp5"][..], 800.),
            (&["weapon_pumpshotgun", "weapon_shotgun_chrome", "weapon_autoshotgun", "weapon_shotgun_spas"][..], 320.),
            (&["weapon_pistol", "weapon_pistol_dual", "weapon_pistol_magnum", "weapon_rifle", "weapon_rifle_ak47", "weapon_rifle_desert", "weapon_rifle_sg552", "weapon_rifle_m60", "weapon_hunting_rifle", "weapon_sniper_military", "weapon_sniper_scout", "weapon_sniper_awp"][..], 960.),
        ] {
            for class in classes {
                let impulse = force(class, Vec3::new(3., 0., 4.)).unwrap();
                assert!((impulse - Vec3::new(0.6, 0., 0.8) * magnitude).length() < 0.001, "{class}");
                // An eight-pellet volley must not put the entire volley force
                // into each separate hit; this helper returns one pellet only.
                assert!((impulse.length() - magnitude).abs() < 0.001);
            }
        }
        for class in ["weapon_melee_fireaxe", "weapon_chainsaw", "weapon_grenade_launcher", "weapon_pipe_bomb", "unknown"] {
            assert!(force(class, Vec3::X).is_none());
        }
        for direction in [Vec3::ZERO, Vec3::splat(f32::NAN), Vec3::splat(f32::INFINITY)] {
            assert!(force("weapon_rifle", direction).is_none());
        }
    }
    use super::*;
    fn shader_contains(rows: &[Vec4; 12], p: Vec3) -> bool {
        rows.chunks_exact(3).any(|r| {
            r[0].truncate().length_squared() > 0. && Vec3::new(
                r[0].dot(p.extend(1.)), r[1].dot(p.extend(1.)), r[2].dot(p.extend(1.))
            ).length_squared() < 1.
        })
    }
    #[test]
    fn fragment_cutout_preserves_skin_outside_a_crossing_triangle() {
        let v = Volume { center: Vec3::ZERO, scale: Vec3::ONE, look: Vec3::Z, up: Vec3::Y };
        let rows = wound_rows([&v]);
        // Old any-corner triangle removal discarded this entire surface.
        assert!(shader_contains(&rows, Vec3::ZERO));
        assert!(!shader_contains(&rows, Vec3::new(1.5, 0., 0.)));
        // Conversely the centre of a large triangle must be cut even if all
        // three corners fall outside the volume.
        for p in [Vec3::new(-2., -2., 0.), Vec3::new(2., -2., 0.), Vec3::new(0., 2., 0.)] {
            assert!(!shader_contains(&rows, p));
        }
        assert!(shader_contains(&rows, Vec3::new(0., -0.2, 0.)));
        assert!(!shader_contains(&[Vec4::ZERO; 12], Vec3::ZERO));
    }
    #[test]
    fn fragment_rows_match_all_owned_wound_volumes() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for wound in Registry::load(&mut assets).unwrap().wounds {
            assert!(wound.volumes.len() <= 2, "{}", wound.model);
            let rows = wound_rows(&wound.volumes);
            for volume in &wound.volumes {
                for x in -4..=4 {
                    for y in -4..=4 {
                        let p = volume.center + Vec3::new(x as f32 * 3.17, y as f32 * 2.73, x as f32 * 1.13);
                        assert_eq!(shader_contains(&rows, p), wound.volumes.iter().any(|v| v.contains(p)), "{} {p:?}", wound.model);
                    }
                }
            }
        }
    }
    #[test]
    fn native_wound_basis_preserves_side_handedness_and_corrected_up() {
        let v = Volume { center: Vec3::new(1., 2., 3.), scale: Vec3::new(-2., 4., 8.), look: Vec3::Z, up: Vec3::new(0., 1., 1.) };
        let rows = wound_rows([&v]);
        // Native CPU setup normalizes LookAt/Up independently and crosses
        // LookAt x Up. It does not normalize the resulting Side again.
        let side = Vec3::Z.cross(v.up.normalize());
        let up = side.cross(Vec3::Z);
        let p = Vec3::new(5., 8., 11.);
        assert!((rows[0].dot(p.extend(1.)) - side.dot(p-v.center)/-2.).abs() < 1e-6);
        assert!((rows[1].dot(p.extend(1.)) - up.dot(p-v.center)/4.).abs() < 1e-6);
        assert!((rows[2].dot(p.extend(1.)) - Vec3::Z.dot(p-v.center)/8.).abs() < 1e-6);
    }
    #[test]
    fn mirrored_native_cutout_axes_retain_the_geometry_envelope() {
        let mut volume = Volume {
            center: Vec3::new(2., 4., 6.),
            scale: Vec3::new(3., -4., 5.),
            look: Vec3::Z,
            up: Vec3::Y,
        };
        let points = [
            volume.center,
            volume.center + Vec3::X * 2.,
            volume.center + Vec3::Y * 8.,
        ];
        let mirrored = points.map(|p| volume.contains(p));
        volume.scale = volume.scale.abs();
        assert_eq!(mirrored, points.map(|p| volume.contains(p)));
        assert_eq!(mirrored, [true, true, false]);
    }
    #[test]
    fn map_or_round_reset_discards_fragment_entities_and_pending_work() {
        let mut world = World::new();
        world.init_resource::<State>();
        let gib = world
            .spawn(Gib {
                velocity: Vec3::ZERO,
                spin: Vec3::ZERO,
                life: 10.,
                resting: true,
                half_extents: Vec3::splat(0.1),
                body: None,
                model: String::new(),
                meshes: Vec::new(),
            })
            .id();
        let child = world.spawn(ChildOf(gib)).id();
        world.resource_mut::<State>().live.push_back(gib);
        world.resource_mut::<State>().fixture_started = true;
        reset(&mut world);
        assert!(world.get_entity(gib).is_err());
        assert!(world.get_entity(child).is_err());
        assert!(world.resource::<State>().live.is_empty());
        assert!(!world.resource::<State>().fixture_started);
    }
    #[test]
    fn saturated_corpse_budget_still_advances_every_detached_gib() {
        use bevy::ecs::system::RunSystemOnce;
        struct Restore;
        impl Drop for Restore { fn drop(&mut self) { crate::ragdoll::unlimit_budget_for_test(); } }
        let _restore = Restore;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let assets = std::cell::RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = l4d2_formats::anim::Skeleton::load_with("models/infected/limbs/common_infected_w_l_arm_lower_gib.mdl", |p| assets.borrow_mut().read(p).ok()).unwrap();
        let pose = sk.pose_sequence(0, 0., false);
        let mut world = World::new();
        let mut game = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        game.paused = false;
        world.insert_resource(game);
        world.insert_resource(crate::game_menu::Frontend { page: crate::game_menu::Page::Game, mode: 0, restart: false, started: true });
        world.init_resource::<Time>();
        world.init_resource::<State>();
        world.init_resource::<crate::l4d2r_animation::AnimationLibrary>();
        world.init_resource::<crate::l4d2r_animation::PlaybackState>();
        world.insert_resource(crate::viewer::RenderModels(Default::default()));
        world.insert_resource(crate::collision::Collision::test_floor());
        world.init_resource::<crate::bullet_impacts::Impacts>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<crate::viewer::SourceMaterial>>();
        world.insert_resource(crate::l4d2r_cinematic::MapLogic::load(root, std::path::Path::new("maps/rewrite_lab.bsp"), false).unwrap());
        let mut starts = vec![];
        for index in 0..12 {
            let transform = Transform::from_translation(Vec3::new(index as f32 * 2., 10., 0.));
            let body = crate::ragdoll::Body::new(&sk, &pose, &transform, Vec3::ZERO);
            let height = body.center().y;
            let id = world.spawn((transform, Gib { velocity: Vec3::ZERO, spin: Vec3::ZERO,
                life: 10., resting: false, half_extents: Vec3::splat(0.1), body: Some(body),
                model: String::new(), meshes: vec![] })).id();
            starts.push((id, height));
        }
        let system = world.register_system(update);
        for _ in 0..36 {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f64(1. / 60.));
            crate::ragdoll::refill_budget();
            world.run_system_once(reserve_physics).unwrap();
            while crate::ragdoll::take_budget(1) {} // saturated corpse batch runs first
            world.run_system(system).unwrap();
        }
        for (id, height) in starts {
            let gib = world.get::<Gib>(id).unwrap();
            let after = gib.body.as_ref().unwrap().center().y;
            assert!(after < height - 0.03, "gib {id:?} starved at {after}, started {height}");
        }
    }
    #[test]
    fn owned_cutouts_preserve_windows_gibs_profiles_hitboxes_and_exclusions() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let registry = Registry::load(&mut assets).unwrap();
        assert!(
            registry
                .wounds
                .iter()
                .filter(|w| w.profile == "male")
                .count()
                >= 40
        );
        let mirrored = registry
            .wounds
            .iter()
            .flat_map(|w| &w.volumes)
            .filter(|v| v.scale.min_element() < 0.)
            .collect::<Vec<_>>();
        assert!(
            mirrored.len() >= 10,
            "native mirrored cutouts were silently dropped"
        );
        assert!(mirrored.iter().all(|v| v.contains(v.center)));
        let half = registry
            .wounds
            .iter()
            .find(|w| w.profile == "male" && w.model.ends_with("common_infected_w_l_half.mdl"))
            .unwrap();
        assert_eq!(
            half.gibs.len(),
            2,
            "Windows gibs must not be overwritten by the Xbox branch"
        );
        assert_eq!(half.boxes, vec![-1]);
        assert!(half.effects.iter().any(|e| e.0 == "gore_wound_fullbody_1"));
        let male = "models/infected/common_male01.mdl";
        let name = |i: usize| registry.wounds[i].model.rsplit('/').next().unwrap().trim_end_matches(".mdl").to_owned();
        let hit = Hit {
            entity: Entity::PLACEHOLDER,
            position: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            damage_force_source: None,
            box_index: 13,
            cause: Cause::Bullet,
            distance: 1.,
            damage_type: Hit::bullet_type("weapon_rifle", Some(2)),
            sniper: false,
            swing: None,
            back: false,
            decapitates: false,
            seed: 0,
        };
        // A lethal rifle round at the neck box takes the fatal neck burst.
        let index = registry.choose(male, &hit, true, &[]).unwrap();
        let head = &registry.wounds[index];
        assert!(head.model.contains("neck"));
        assert!(!head.gibs.is_empty());
        assert!(head.flags & WOUND_GUNSHOT != 0 && head.fatal);
        // Not lethal and not a headshot: fatal cutouts are off the table.
        assert!(registry.choose(male, &hit, false, &[]).is_none_or(|i| !registry.wounds[i].fatal));
        // Pistols and SMGs carry no DMG_DISMEMBER: no bullet wound at all,
        // even through the head (z_head_damage_causes_wounds 0).
        for class in ["weapon_pistol", "weapon_smg", "weapon_smg_silenced"] {
            let mut small = hit.clone();
            small.damage_type = Hit::bullet_type(class, Some(1));
            assert!(registry.choose(male, &small, true, &[]).is_none(), "{class}");
        }
        // Range does not matter for a rifle.
        let mut far = hit.clone();
        far.distance = 4000. * crate::collision::UNIT;
        assert!(registry.choose(male, &far, true, &[]).is_some());
        // "-1" hit boxes never match a bullet: whole-body halves only come
        // from blasts and chainsaws.
        for seed in 0..64 {
            let mut any = hit.clone();
            any.seed = seed;
            for b in 0..24 {
                any.box_index = b;
                if let Some(i) = registry.choose(male, &any, true, &[]) {
                    assert!(!registry.wounds[i].boxes.contains(&-1), "{}", name(i));
                }
            }
        }
        // Edged melee with "decapitates" takes the decapitation at the neck.
        let mut axe = hit.clone();
        axe.damage_type = DMG_MELEE | DMG_SLASH | DMG_DISMEMBER;
        axe.decapitates = true;
        axe.swing = Some((4, 6));
        let i = registry.choose(male, &axe, true, &[]).unwrap();
        assert!(registry.wounds[i].flags & WOUND_DECAPITATE != 0);
        // Without the flag it takes dismember or a direction-matched slash.
        axe.decapitates = false;
        for seed in 0..64 {
            axe.seed = seed;
            for b in 0..24 {
                axe.box_index = b;
                for back in [false, true] {
                    axe.back = back;
                    if let Some(i) = registry.choose(male, &axe, true, &[]) {
                        let w = &registry.wounds[i];
                        assert!(w.flags & (WOUND_DISMEMBER | WOUND_SLASH) != 0, "{}", name(i));
                        if w.flags & WOUND_SLASH != 0 {
                            assert_eq!(w.swing, (Some(4), Some(6)));
                            assert_eq!(w.back, back);
                        }
                    }
                }
            }
        }
        assert!(registry.wounds.iter().any(|w| w.flags & WOUND_SLASH != 0 && w.swing == (Some(4), Some(6))));
        // Club damage needs blunt cutouts, which the scripts do not author.
        let mut bat = axe.clone();
        bat.damage_type = DMG_MELEE | DMG_CLUB;
        for b in 0..24 {
            bat.box_index = b;
            assert!(registry.choose(male, &bat, true, &[]).is_none());
        }
        // Blasts and M60 rounds take a random explosion cutout anywhere.
        let mut blast = hit.clone();
        blast.damage_type = DMG_BLAST;
        blast.box_index = 999;
        let i = registry.choose(male, &blast, false, &[]).unwrap();
        assert!(registry.wounds[i].flags & WOUND_EXPLOSION != 0);
        let mut m60 = hit.clone();
        m60.damage_type = Hit::bullet_type("weapon_rifle_m60", Some(3));
        assert!(registry.choose(male, &m60, false, &[]).is_some_and(|i| registry.wounds[i].flags & WOUND_EXPLOSION != 0));
        // SetWound: two wounds at most, never the first again, never one on
        // the first's exclusion list or with a two-volume cutout.
        assert!(registry.choose(male, &hit, true, &[index, index]).is_none());
        assert!(!registry.admits(&[index], index));
        for (j, w) in registry.wounds.iter().enumerate().filter(|(_, w)| w.profile == "male") {
            if head.exclusions.iter().any(|s| s.eq_ignore_ascii_case(&name(j))) || w.volume_count > 1 || head.volume_count > 1 {
                assert!(!registry.admits(&[index], j), "{}", name(j));
            }
        }
        assert!(
            registry
                .choose("models/infected/hunter.mdl", &hit, true, &[])
                .is_none()
        );
        for path in registry.paths() {
            assert!(assets.exists(&path), "{path}");
        }
    }
    #[test]
    fn owned_neck_cutout_removes_head_geometry_but_keeps_body() {
        use l4d2_formats::anim::Skeleton;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = std::cell::RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let registry = Registry::load(&mut assets.borrow_mut()).unwrap();
        let wound = registry
            .wounds
            .iter()
            .find(|w| w.profile == "male" && w.model.ends_with("common_infected_w_neck.mdl"))
            .unwrap();
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let pelvis = sk
            .bones
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case("ValveBiped.Bip01_Pelvis"))
            .unwrap();
        let draws = crate::studio::load(
            &mut assets.borrow_mut(),
            "models/infected/common_male01.mdl",
            0,
            0,
        )
        .unwrap();
        let mut total = 0;
        let mut kept = 0;
        for d in draws {
            let points = d
                .positions
                .iter()
                .map(|p| {
                    let source = l4d2r_math::Vec3::new(p[0], -p[2], p[1]) / 0.0254;
                    Vec3::from_array(pelvis.pose_to_bone.transform_point3(source).to_array())
                })
                .collect::<Vec<_>>();
            total += points.len();
            kept += points.iter().filter(|p| !wound.volumes.iter().any(|v| v.contains(**p))).count();
        }
        assert!(
            kept < total && kept > total / 2,
            "neck cutout kept {kept}/{total} vertices"
        );
    }
}
