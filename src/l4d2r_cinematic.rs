//! l4d2r map I/O and animated-attachment intro camera, adapted to Bevy.
use crate::{game_menu::Frontend, game_state::Match, source_assets::SourceAssets};
use bevy::prelude::*;
use l4d2_formats::bsp::Bsp;
use l4d2r_game_core::{entities::EntityWorld, mapents};
use std::{cell::RefCell, collections::HashMap, io, path::Path, sync::Arc};
#[derive(Component)]
pub struct MapEntity(pub usize);

fn configure_doors(
    world: &mut EntityWorld,
    bounds: &HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
) {
    use l4d2r_game_core::entities::{DoorState, Kind};
    for (id, e) in world.ents.iter_mut().enumerate() {
        if !e.me.classname.starts_with("prop_door_rotating") {
            continue;
        }
        let Kind::Door(d) = &mut e.kind else {
            continue;
        };
        // Source's right vector is -local Y. The leaf farthest from the hinge
        // determines whether forward/backward yaw must be swapped.
        let hinge_left = bounds.get(&id).is_some_and(|(lo, hi)| {
            let lo = l4d2r_math::Vec3::new(lo.x, lo.y, 0.);
            let hi = l4d2r_math::Vec3::new(hi.x, hi.y, 0.);
            let far = if hi.length_squared() > lo.length_squared() {
                hi
            } else {
                lo
            };
            far.y < 0.
        });
        d.forward_sign = if hinge_left { 1. } else { -1. };
        let spawn = e.me.kv_i32("spawnpos").unwrap_or(0);
        let backwards = spawn == 2 || (spawn == 0 && e.me.kv_i32("opendir") == Some(2));
        d.dist = d.dist.abs() * d.forward_sign * if backwards { -1. } else { 1. };
        if spawn == 3 {
            let angles =
                e.me.kv("ajarangles")
                    .unwrap_or("0 0 0")
                    .split_whitespace()
                    .filter_map(|v| v.parse::<f32>().ok())
                    .collect::<Vec<_>>();
            if let Some(yaw) = angles.get(1) {
                let delta = (yaw - e.me.angles.y + 180.).rem_euclid(360.) - 180.;
                d.dist = d.dist.abs().copysign(delta);
                d.frac = (delta.abs() / d.dist.abs().max(0.001)).clamp(0., 1.);
                d.goal = d.frac;
                d.state = if d.frac > 0. {
                    DoorState::Open
                } else {
                    DoorState::Closed
                };
            }
        }
    }
}
#[derive(Component)]
pub struct ScriptedDraw {
    pub model: String,
    pub draw: usize,
    pub ent: usize,
}
#[derive(Component)]
pub(crate) struct IntroDraw {
    pub ent: usize,
    pub model: String,
    pub mesh_model: String,
    pub draw: usize,
}
#[derive(Resource)]
pub struct MapLogic {
    /// Bumped by `restart`: the entity list was rebuilt, so anything keyed
    /// by entity index (particle batches) must be rebuilt too.
    pub generation: u64,
    pub mode: crate::bsp::MapMode,
    pub world: EntityWorld,
    ghosts: HashMap<String, l4d2_formats::anim::Skeleton>,
    pub active: bool,
    pub prop_bounds: HashMap<usize, (l4d2r_math::Vec3, l4d2r_math::Vec3)>,
    pub prop_shapes: HashMap<usize, Arc<Vec<l4d2_formats::phy::Convex>>>,
    pub prop_collision_pose: HashMap<usize,l4d2r_math::Mat4>,
    skip: bool,
    pub triggers: Vec<usize>,
    contacts: std::collections::HashSet<(usize, u64)>,
    hurt_clock: f32,
    team_contacts: std::collections::HashSet<usize>,
    capture_elevator: bool,
    look_progress: HashMap<usize, f32>,
    pub dialogue: Option<crate::source_dialogue::Dialogue>,
    pub physics: crate::prop_physics::Props,
    pub hud_hint: Option<(usize, f32)>,
    durability: HashMap<usize, l4d2r_game_core::entities::Durability>,
    pub door_damage: HashMap<usize, crate::source_breakables::DoorDamage>,
    pub break_debris: crate::source_break_debris::Debris,
    pub break_surfaces: HashMap<usize, crate::source_break_surfaces::Surface>,
}
pub fn transform(m: l4d2r_math::Mat4) -> Transform {
    let c = Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let source = Mat4::from_cols_array(&m.to_cols_array());
    let rotation = c * source * c.inverse();
    Transform::from_translation(Vec3::from_array(crate::binary::source_position(
        m.w_axis.truncate().to_array(),
    )))
    .with_rotation(Quat::from_mat3(&Mat3::from_mat4(rotation)).normalize())
}
impl MapLogic {
    pub fn verify_triggers(&self, collision: &crate::collision::Collision) -> io::Result<()> {
        let mut checked = 0;
        let mut unsupported = 0;
        let mut ghost_only = 0;
        let mut missed = vec![];
        for &id in &self.triggers {
            let e = &self.world.ents[id];
            if e.me.classname == "trigger_hurt_ghost" {
                ghost_only += 1;
            }
            if e.me.classname != "trigger_hurt_ghost"
                && !matches!(
                    e.kind,
                    l4d2r_game_core::entities::Kind::Trigger { .. }
                        | l4d2r_game_core::entities::Kind::Hurt { .. }
                        | l4d2r_game_core::entities::Kind::Push { .. }
                )
            {
                unsupported += 1;
            }
            let Some((lo, hi)) = self.bounds(id) else {
                return Err(io::Error::other("Trigger has no brush bounds"));
            };
            let mat = self.world.world_matrix(id);
            // Box triggers (EDIT WORLD) have no brush to verify.
            let Some(model) = e.me.brush else { continue };
            let outside = hi + Vec3::splat(10.);
            if collision.overlaps_model(model, mat, outside, 0.01) {
                return Err(io::Error::other("Trigger overlap leaks beyond bounds"));
            }
            let mut inside = false;
            for x in [0.1, 0.5, 0.9] {
                for y in [0.1, 0.5, 0.9] {
                    for z in [0.1, 0.5, 0.9] {
                        let p = lo + (hi - lo) * Vec3::new(x, y, z);
                        inside |= collision.overlaps_model(model, mat, p, 0.01);
                    }
                }
            }
            if !inside {
                let m = &self.world.bsp.models[model];
                for face in self
                    .world
                    .bsp
                    .faces
                    .iter()
                    .skip(m.first_face.max(0) as usize)
                    .take(m.num_faces.max(0) as usize)
                {
                    let mut center = l4d2r_math::Vec3::ZERO;
                    let mut count = 0.;
                    for index in 0..face.num_edges.max(0) as usize {
                        let Some(&se) = self
                            .world
                            .bsp
                            .surfedges
                            .get(face.first_edge.max(0) as usize + index)
                        else {
                            continue;
                        };
                        let Some(edge) = self.world.bsp.edges.get(se.unsigned_abs() as usize)
                        else {
                            continue;
                        };
                        if let Some(vertex) = self
                            .world
                            .bsp
                            .vertexes
                            .get(edge[usize::from(se < 0)] as usize)
                        {
                            center += *vertex;
                            count += 1.;
                        }
                    }
                    if count > 0. {
                        let normal = self.world.bsp.planes[face.plane as usize].normal
                            * if face.side == 0 { 1. } else { -1. };
                        let p = Vec3::from_array(crate::binary::source_position(
                            mat.transform_point3(center / count - normal).to_array(),
                        ));
                        inside |= collision.overlaps_model(model, mat, p, 0.01);
                    }
                }
            }
            if !inside {
                // Some invisible compound triggers have no render faces. Build
                // an interior witness from their actual convex brush planes.
                let bsp = &self.world.bsp;
                let mut stack = vec![bsp.models[model].head_node];
                let mut nodes = std::collections::HashSet::new();
                let mut brushes = std::collections::HashSet::new();
                while let Some(node) = stack.pop() {
                    if !nodes.insert(node) {
                        continue;
                    }
                    if node >= 0 {
                        if let Some(n) = bsp.nodes.get(node as usize) {
                            stack.extend(n.children);
                        }
                    } else if let Some(leaf) = bsp.leafs.get((-node - 1) as usize) {
                        for id in bsp
                            .leaf_brushes
                            .iter()
                            .skip(leaf.first_leaf_brush as usize)
                            .take(leaf.num_leaf_brushes as usize)
                        {
                            brushes.insert(*id as usize);
                        }
                    }
                }
                for id in brushes {
                    let brush = &bsp.brushes[id];
                    let planes = bsp
                        .brush_sides
                        .iter()
                        .skip(brush.first_side.max(0) as usize)
                        .take(brush.num_sides.max(0) as usize)
                        .map(|s| bsp.planes[s.plane as usize])
                        .collect::<Vec<_>>();
                    if planes.len() > 64 {
                        continue;
                    }
                    let mut sum = l4d2r_math::Vec3::ZERO;
                    let mut count = 0.;
                    for a in 0..planes.len() {
                        for b in a + 1..planes.len() {
                            for c in b + 1..planes.len() {
                                let (p, q, r) = (planes[a], planes[b], planes[c]);
                                let det = p.normal.dot(q.normal.cross(r.normal));
                                if det.abs() < 0.00001 {
                                    continue;
                                }
                                let v = (p.dist * q.normal.cross(r.normal)
                                    + q.dist * r.normal.cross(p.normal)
                                    + r.dist * p.normal.cross(q.normal))
                                    / det;
                                if v.is_finite()
                                    && planes.iter().all(|p| p.normal.dot(v) <= p.dist + 0.1)
                                {
                                    sum += v;
                                    count += 1.;
                                }
                            }
                        }
                    }
                    if count > 0. {
                        let p = Vec3::from_array(crate::binary::source_position(
                            mat.transform_point3(sum / count).to_array(),
                        ));
                        inside |= collision.overlaps_model(model, mat, p, 0.01);
                    }
                }
            }
            if inside {
                checked += 1;
            } else {
                missed.push(format!("{id}:{} model={model}", e.me.classname));
            }
        }
        println!(
            "Trigger geometry: {checked} brush volumes verified; {unsupported} volumes with unfinished class behavior; {ghost_only} infected-ghost volumes excluded from survivor activation; {} missed volumes",
            missed.len()
        );
        if missed.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(missed.join(", ")))
        }
    }
    fn accepts_survivor(&self, id: usize) -> bool {
        self.accepts_actor(id, false)
    }
    fn accepts_actor(&self, id: usize, infected: bool) -> bool {
        fn filter(logic: &MapLogic, name: &str, depth: usize, infected: bool) -> bool {
            if depth > 8 {
                return false;
            }
            let Some(&id) = logic.world.find(name).first() else {
                return false;
            };
            let e = &logic.world.ents[id];
            let accepted = match e.me.classname.as_str() {
                "filter_activator_team" => {
                    e.me.kv_i32("filterteam") == Some(if infected { 3 } else { 2 })
                }
                "filter_activator_class" => {
                    if infected {
                        e.me.kv("filterclass") == Some("infected")
                    } else {
                        matches!(e.me.kv("filterclass"), Some("player" | "survivor"))
                    }
                }
                "filter_multi" => {
                    let children = (1..=5)
                        .filter_map(|i| e.me.kv(&format!("filter{i:02}")))
                        .collect::<Vec<_>>();
                    if children.is_empty() {
                        false
                    } else if e.me.kv_i32("filtertype") == Some(1) {
                        children
                            .iter()
                            .any(|n| filter(logic, n, depth + 1, infected))
                    } else {
                        children
                            .iter()
                            .all(|n| filter(logic, n, depth + 1, infected))
                    }
                }
                _ => return false,
            };
            if e.me.kv_i32("negated") == Some(1) {
                !accepted
            } else {
                accepted
            }
        }
        self.world.ents[id]
            .me
            .kv("filtername")
            .filter(|s| !s.is_empty())
            .is_none_or(|name| filter(self, name, 0, infected))
    }
    pub fn load(root: &Path, path: &Path, skip: bool) -> io::Result<Self> {
        Self::load_mode(root, path, skip, crate::bsp::MapMode::Campaign)
    }
    pub fn load_mode(
        root: &Path,
        path: &Path,
        skip: bool,
        mode: crate::bsp::MapMode,
    ) -> io::Result<Self> {
        let bsp = Arc::new(
            Bsp::parse(crate::bsp::load_bytes_for_mode(path, root, mode)?)
                .map_err(|e| crate::binary::invalid(e.to_string()))?,
        );
        let mut world = EntityWorld::new(bsp.clone(), mapents::collect(&bsp), 1);
        world.map_spawn(mode.output());
        for i in 0..world.ents.len() {
            if world.ents[i].me.classname == "info_director" {
                world.fire_output_pub(i, "OnGameplayStart");
            }
        }
        world.flush_io();
        let assets = RefCell::new(SourceAssets::new(root, vec![])?);
        let mut ghosts = HashMap::new();
        let auditing = std::env::args().any(|a| a.starts_with("--verify-triggers"));
        for e in &world.ents {
            if !auditing
                && (e.me.classname.starts_with("prop_dynamic") || e.me.classname == "prop_health_cabinet")
            {
                if let Some(model) = &e.me.mdl {
                    if !ghosts.contains_key(model) {
                        if let Ok(s) = l4d2_formats::anim::Skeleton::load_with(model, |p| {
                            assets.borrow_mut().read(p).ok()
                        }) {
                            ghosts.insert(model.clone(), s);
                        }
                    }
                }
            }
        }
        let mut prop_bounds = HashMap::new();
        let mut prop_shapes = HashMap::new();
        let mut shape_cache = HashMap::<String, Option<Arc<Vec<l4d2_formats::phy::Convex>>>>::new();
        for (id, e) in world.ents.iter().enumerate() {
            if !auditing
                && (e.me.classname.starts_with("prop_physics")
                    || e.me.classname == "prop_health_cabinet"
                    || (e.me.classname.starts_with("prop_dynamic") &&(e.parent.is_some() || e.me.kv("DefaultAnim").is_some()))
                    || matches!(
                        e.kind,
                        l4d2r_game_core::entities::Kind::Door(_)
                            | l4d2r_game_core::entities::Kind::Breakable { .. }
                            // Finale props (radios, stage buttons) are used
                            // through their model bounds.
                            | l4d2r_game_core::entities::Kind::Finale { .. }
                            // Car alarms are set off by shots and touches
                            // on their collision model.
                            | l4d2r_game_core::entities::Kind::CarAlarm { .. }
                    ))
            {
                if let Some(model) = &e.me.mdl {
                    let shape = shape_cache.entry(model.clone()).or_insert_with(|| {
                        let bytes = assets
                            .borrow_mut()
                            .read(&model.replace(".mdl", ".phy"))
                            .ok()?;
                        let pieces: Vec<_> = l4d2_formats::phy::parse_phy(&bytes)
                            .ok()?
                            .into_iter()
                            .flat_map(|s| s.convexes)
                            .collect();
                        (!pieces.is_empty()).then(|| Arc::new(pieces))
                    });
                    if let Some(shape) = shape {
                        prop_shapes.insert(id, shape.clone());
                    }
                    if let Ok(bytes) = assets.borrow_mut().read(model) {
                        if let Some((lo, hi, vlo, vhi)) =
                            l4d2_formats::mdl::read_header_bounds(&bytes)
                        {
                            let (lo, hi) = if lo == hi { (vlo, vhi) } else { (lo, hi) };
                            prop_bounds.insert(
                                id,
                                (
                                    l4d2r_math::Vec3::from_array(lo),
                                    l4d2r_math::Vec3::from_array(hi),
                                ),
                            );
                        }
                    }
                }
            }
        }
        for (&id, pieces) in &prop_shapes {
            if world.ents[id].me.classname.starts_with("prop_physics") {
                let lo = pieces
                    .iter()
                    .fold(l4d2r_math::Vec3::splat(f32::MAX), |a, c| a.min(c.mins));
                let hi = pieces
                    .iter()
                    .fold(l4d2r_math::Vec3::splat(f32::MIN), |a, c| a.max(c.maxs));
                prop_bounds.insert(id, (lo, hi));
            }
        }
        let triggers = world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (e.me.classname.starts_with("trigger_") || e.me.classname == "info_changelevel")
                    && e.me.brush.is_some()
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        configure_doors(&mut world, &prop_bounds);
        let durability = crate::source_breakables::configure(&mut assets.borrow_mut(), &mut world);
        let door_damage = crate::source_breakables::load_doors(&mut assets.borrow_mut(), &world);
        let break_surfaces = crate::source_break_surfaces::load(&world);
        let break_debris = crate::source_break_debris::Debris::load(&mut assets.borrow_mut(), &world, &door_damage);
        let physics = if auditing {
            crate::prop_physics::Props::default()
        } else {
            crate::prop_physics::Props::load(&world, &mut assets.borrow_mut())
        };
        let mut report = String::from("entity\tclassname\ttargetname\tstatus\n");
        for (id, e) in world.ents.iter().enumerate().filter(|(_, e)| {
            e.me.classname.starts_with("trigger_")
                || e.me.classname.starts_with("logic_")
                || e.me.classname.starts_with("filter_")
        }) {
            let status = if e.me.classname == "trigger_hurt_ghost" {
                "infected-ghost-only volume; excluded for survivors"
            } else if e.me.classname == "logic_director_query" {
                "provisional anger response; Director parity unfinished"
            } else if e.me.classname.starts_with("filter_") {
                "partial survivor filter adapter"
            } else if matches!(e.kind, l4d2r_game_core::entities::Kind::None) {
                "unimplemented behavior"
            } else {
                "registered"
            };
            report += &format!(
                "{id}\t{}\t{}\t{status}\n",
                e.me.classname,
                e.me.kv("targetname").unwrap_or("")
            );
        }
        std::fs::create_dir_all("local-research")?;
        std::fs::create_dir_all("local-research/trigger-audits")?;
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        std::fs::write(format!("local-research/trigger-audits/{name}.tsv"), &report)?;
        if !std::env::args().any(|a| a == "--verify-triggers-all") {
            std::fs::write("local-research/map-trigger-audit.tsv", report)?;
        }
        println!(
            "Map trigger volumes: {} (unsupported behaviors recorded in local-research/map-trigger-audit.tsv)",
            triggers.len()
        );
        println!(
            "l4d2r map I/O: {} entities, {} animation skeletons",
            world.ents.len(),
            ghosts.len()
        );
        crate::projectiles::prepare(&mut world);
        crate::bullet_impacts::prepare(&mut world);
        if let Ok(registry) = crate::infected_gore::Registry::load(&mut assets.borrow_mut()) {
            crate::infected_gore::prepare(&mut world, &registry);
        }
        crate::special_infected::spitter::prepare(&mut world);
        crate::special_infected::boomer::prepare(&mut world);
        crate::special_infected::death_effects::prepare(&mut world);
        // A single native collision solid follows its named animated bone.
        // Keep unrelated/multi-solid models on their existing collision path.
        let mut prop_collision_pose=HashMap::new();
        // Rotating doors' physics hulls are authored in their root bone's
        // frame too (metal_door_112 lies flat, checkpoint doors are turned
        // 90 degrees), so their collision follows the closed/idle pose.
        let mut door_skeletons=HashMap::new();
        for (id,e) in world.ents.iter().enumerate() {
            let Some(model)=&e.me.mdl else {continue};
            let door=e.me.classname.starts_with("prop_door_rotating");
            let sk=if door {
                let Some(sk)=door_skeletons.entry(model.clone()).or_insert_with(|| {
                    l4d2_formats::anim::Skeleton::load_with(model,|p| assets.borrow_mut().read(p).ok()).ok()
                }).as_ref() else {continue};
                sk
            } else {
                let Some(sk)=ghosts.get(model) else {continue};
                sk
            };
            let seq=match e.me.kv("DefaultAnim").filter(|n| !n.is_empty()) {
                Some(name)=>sk.find_sequence(name),
                None if door=>["closed","idle"].iter().find_map(|n| sk.find_sequence(n)).or(Some(0)),
                None=>None,
            };
            let Some(seq)=seq else {continue};
            let Ok(profile)=crate::physics_data::Profile::load(&mut assets.borrow_mut(),model) else {continue};
            if profile.geometry.len()!=1 || profile.solids.len()!=1 {continue;}
            if let Some(bone)=sk.bone_index(&profile.solids[0].name) {
                prop_collision_pose.insert(id,sk.pose_sequence(seq,0.,false)[bone]);
            }
        }
        let mut result = Self {
            generation: 0,
            mode,
            world,
            ghosts,
            active: false,
            prop_bounds,
            prop_shapes,
            prop_collision_pose,
            skip,
            triggers,
            contacts: Default::default(),
            team_contacts: Default::default(),
            capture_elevator: std::env::args().any(|a| a == "--capture-hotel-elevator"),
            hurt_clock: 0.,
            look_progress: Default::default(),
            dialogue: if auditing {
                None
            } else {
                Some(crate::source_dialogue::Dialogue::load(root)?)
            },
            physics,
            hud_hint: None,
            durability,
            door_damage,
            break_debris,
            break_surfaces,
        };
        result.prepare_skip();
        Ok(result)
    }
    /// CPU fast-forward belongs to map preparation, not the first playable tick.
    fn prepare_skip(&mut self) {
        if !self.skip {
            return;
        }
        for _ in 0..2700 {
            self.world.tick(1. / 60.);
        }
        self.world.cine.view_control = None;
        self.world.cine.view_movement = None;
        self.world.cine.intro = false;
        self.skip = false;
        self.world.cine.scene_starts.clear();
        self.world.cine.concepts.clear();
    }
    pub fn restart(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        let runtime_effects: Vec<_> = self
            .world
            .ents
            .iter()
            .filter(|e| {
                e.me.targetname.starts_with("__upgrade_")
                    || e.me.targetname.starts_with("__rewrite_gore_")
                    || e.me.targetname.starts_with("__rewrite_muzzle_fp_")
                    || e.me.targetname.starts_with("__rewrite_smoker_")
            })
            .cloned()
            .collect();
        let selected_survivor = self.world.cine.selected_survivor.clone();
        let bsp = self.world.bsp.clone();
        self.world = EntityWorld::new(bsp.clone(), mapents::collect(&bsp), 1);
        self.world.cine.selected_survivor = selected_survivor;
        crate::projectiles::prepare(&mut self.world);
        crate::bullet_impacts::prepare(&mut self.world);
        crate::special_infected::spitter::prepare(&mut self.world);
        crate::special_infected::boomer::prepare(&mut self.world);
        crate::special_infected::death_effects::prepare(&mut self.world);
        for mut effect in runtime_effects {
            if self.world.find(&effect.me.targetname).is_empty() {
                effect.kind = l4d2r_game_core::entities::Kind::Particle {
                    active: false,
                    started: 0.,
                };
                effect.alive = true;
                self.world.add_runtime_entity(effect);
            }
        }

        crate::source_breakables::reset_doors(self);
        crate::source_break_debris::reset(self);
        crate::source_break_surfaces::reset(self);
        configure_doors(&mut self.world, &self.prop_bounds);
        self.world.durability = self.durability.clone();
        for (&id, d) in &self.durability {
            if !matches!(
                self.world.ents[id].kind,
                l4d2r_game_core::entities::Kind::Door(_)
            ) {
                self.world.ents[id].kind =
                    l4d2r_game_core::entities::Kind::Breakable { health: d.maximum };
            }
        }
        self.physics.reset(&self.world);
        self.hud_hint = None;
        self.world.map_spawn(self.mode.output());
        for i in 0..self.world.ents.len() {
            if self.world.ents[i].me.classname == "info_director" {
                self.world.fire_output_pub(i, "OnGameplayStart");
            }
        }
        self.active = false;
        self.contacts.clear();
        self.team_contacts.clear();
        self.capture_elevator = std::env::args().any(|a| a == "--capture-hotel-elevator");
        self.hurt_clock = 0.;
        self.look_progress.clear();
        if let Some(dialogue) = &mut self.dialogue {
            dialogue.reset();
        }
    }
    pub fn open_near(
        &mut self,
        eye: Vec3,
        direction: Vec3,
        collision: &crate::collision::Collision,
    ) {
        let closed=self.world.ents.iter().enumerate().filter(|(_,e)| e.alive && matches!(&e.kind,l4d2r_game_core::entities::Kind::Door(d) if !d.locked && d.state==l4d2r_game_core::entities::DoorState::Closed)).filter_map(|(id,_)| {
            let (lo,hi)=self.bounds(id)?;
            let distance=crate::ray_box(eye,direction,lo-Vec3::splat(0.04),hi+Vec3::splat(0.04))?;
            if distance<2.44 {Some((id,distance))}else{None}
        }).min_by(|a,b|a.1.total_cmp(&b.1));
        if let Some((id, _)) = closed {
            self.open_away_from(id, eye);
            self.world.use_ent(id, None);
            collision.sync_entities(self);
        }
    }
    /// Multiplayer: another game used entity `id` (a door opened from
    /// `opener`, Source units). Replayed here without logging it again.
    pub fn remote_use(&mut self, id: usize, opener: Option<l4d2r_math::Vec3>, collision: &crate::collision::Collision) {
        if id >= self.world.ents.len() || !self.world.ents[id].alive {
            return;
        }
        if opener.is_some() {
            self.world.door_open_away_from(id, opener);
        }
        let before = self.world.uses.len();
        self.world.use_ent(id, None);
        self.world.uses.truncate(before);
        collision.sync_entities(self);
    }
    fn open_away_from(&mut self, id: usize, eye: Vec3) {
        let source = l4d2r_math::Vec3::new(eye.x, -eye.z, eye.y) / 0.0254;
        self.world.door_open_away_from(id, Some(source));
    }
    pub fn scripted_models(&self) -> Vec<String> {
        self.ghosts
            .keys()
            .filter(|p| !p.ends_with("ghostanim.mdl"))
            .cloned()
            .collect()
    }
    pub fn bounds(&self, id: usize) -> Option<(Vec3, Vec3)> {
        let e = self.world.ents.get(id)?;
        let (lo, hi) = if let Some(model) = e.me.brush {
            let m = self.world.bsp.models.get(model)?;
            (m.mins, m.maxs)
        } else {
            *self.prop_bounds.get(&id)?
        };
        let mat = self.world.world_matrix(id);
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for i in 0..8 {
            let p = l4d2r_math::Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            let p = Vec3::from_array(crate::binary::source_position(
                mat.transform_point3(p).to_array(),
            ));
            min = min.min(p);
            max = max.max(p);
        }
        Some((min, max))
    }
    pub fn use_near(
        &mut self,
        eye: Vec3,
        forward: Vec3,
        collision: &crate::collision::Collision,
    ) -> bool {
        let target = self
            .world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive
                    && matches!(
                        e.kind,
                        l4d2r_game_core::entities::Kind::Door(_)
                            | l4d2r_game_core::entities::Kind::Button { .. }
                            | l4d2r_game_core::entities::Kind::Finale { started: false, disabled: false, .. }
                    )
            })
            .filter_map(|(id, _)| {
                let (lo, hi) = self.bounds(id)?;
                let distance =
                    crate::ray_box(eye, forward, lo - Vec3::splat(0.04), hi + Vec3::splat(0.04))?;
                let wall = collision
                    .trace(eye, eye + forward * 2.44, Vec3::ZERO)
                    .fraction
                    * 2.44;
                if distance <= 2.44 && distance <= wall + 0.06 {
                    Some((id, distance))
                } else {
                    None
                }
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = target {
            self.open_away_from(id, eye);
            self.world.use_ent(id, None);
            collision.sync_entities(self);
            true
        } else {
            false
        }
    }
    /// Retail CPropHealthCabinet::Use (server 102c1640): a survivor opens a
    /// closed cabinet under the view ray (ACT_DOOR_OPEN, "Doors.Medkit_Cabinet").
    /// Returns its position, HealthCount clamped to 1..4 (102c1770) and the
    /// world transforms of its item1..item4 attachments.
    pub fn open_cabinet(
        &mut self,
        eye: Vec3,
        forward: Vec3,
        collision: &crate::collision::Collision,
    ) -> Option<(usize, Vec3, usize, Vec<Transform>)> {
        let (id, _) = self
            .world
            .ents
            .iter()
            .enumerate()
            .filter(|(id, e)| {
                e.alive && e.me.classname == "prop_health_cabinet" && !self.world.cine.anims.contains_key(id)
            })
            .filter_map(|(id, _)| {
                let (lo, hi) = self.bounds(id)?;
                let distance = crate::ray_box(eye, forward, lo - Vec3::splat(0.04), hi + Vec3::splat(0.04))?;
                let wall = collision.trace(eye, eye + forward * 2.44, Vec3::ZERO).fraction * 2.44;
                (distance <= 2.44 && distance <= wall + 0.3).then_some((id, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let e = &self.world.ents[id];
        let count = e.me.kv_i32("HealthCount").unwrap_or(1).clamp(1, 4) as usize;
        let matrix = self.world.world_matrix(id);
        let slots = e
            .me
            .mdl
            .as_ref()
            .and_then(|m| self.ghosts.get(m))
            .map(|sk| {
                let pose = sk.pose_sequence(0, 0., false);
                (1..=4)
                    .filter_map(|i| sk.attachment_matrix(&format!("item{i}"), &pose))
                    .map(|a| transform(matrix * a))
                    .collect()
            })
            .unwrap_or_default();
        let open = self.world.time;
        self.world.cine.anims.insert(id, ("Open".into(), open));
        Some((id, transform(matrix).translation, count, slots))
    }
    pub fn pound_obstacle(
        &mut self,
        feet: Vec3,
        direction: Vec3,
        collision: &crate::collision::Collision,
    ) -> bool {
        // Damaged door stages have authored holes; probe the body's height
        // range so a hole at eye level does not hide a door that still blocks.
        let mut target: Option<(usize, f32, Vec3)> = None;
        for height in [0.35, 0.7, 1.0, 1.4] {
            let eye = feet + Vec3::Y * height;
            let wall = collision.trace(eye, eye + direction * 1.4, Vec3::ZERO).fraction * 1.4;
            let hit = self
                .world
                .ents
                .iter()
                .enumerate()
                .filter(|(id, e)| {
                    e.alive
                        && self.world.durability.contains_key(id)
                        && matches!(
                            e.kind,
                            l4d2r_game_core::entities::Kind::Door(_)
                                | l4d2r_game_core::entities::Kind::Breakable { .. }
                        )
                })
                .filter_map(|(id, _)| {
                    let d = self.breakable_hit_distance(id, eye, direction, 1.4)?;
                    (d < 1.4 && d <= wall + 0.06).then_some((id, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((id, d)) = hit {
                if target.is_none_or(|(_, best, _)| d < best) {
                    target = Some((id, d, eye));
                }
            }
        }
        if let Some((id, distance, eye)) = target {
            // Retail server.dll: z_door_pound_damage constructor default string at 1062e54c.
            // Infected pounds are melee (club) hits. The Hotel door's authored
            // final damage stage sets dmg.bullets 0, so bullet-typed pounds
            // stalled every door at its last stage (160 of 1000 health) and
            // commons could never break through.
            let kind = l4d2r_game_core::entities::DamageKind::Club;
            if std::env::var_os("L4D2R_COMMON_LOG").is_some() {
                let e = &self.world.ents[id];
                println!(
                    "L4D2RPOUND ent={id} class={} name={} health/max/club={:?}",
                    e.me.classname, e.me.targetname, self.world.durability.get(&id).map(|d| (d.health, d.maximum, d.club))
                );
            }
            if self.break_surfaces.contains_key(&id) {
                crate::source_break_surfaces::damage_at(self,id,60.,kind,eye+direction*distance);
            } else {self.world.damage_entity(id, 60., kind);}
            crate::source_breakables::sync_doors(self);
            if self.door_damage.contains_key(&id) {
                self.world.events.push(l4d2r_game_core::entities::EntEvent::DoorSound { ent: id, output: "Pound".into() });
            }
            collision.sync_entities(self);
            return true;
        }
        false
    }
    pub(crate) fn breakable_hit_distance(&self, id: usize, eye: Vec3, direction: Vec3, range: f32) -> Option<f32> {
        let e = self.world.ents.get(id)?;
        let inverse = self.world.world_matrix(id).inverse();
        let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / crate::collision::UNIT;
        let start = inverse.transform_point3(source(eye));
        let end = inverse.transform_point3(source(eye + direction * range));
        if let Some(surface)=self.break_surfaces.get(&id).filter(|s|s.initialized) {
            return crate::source_breakables::ray_shapes(start,end,&surface.shapes).map(|t|t*range);
        }
        if let Some(shapes) = self.prop_shapes.get(&id).filter(|s| !s.is_empty()) {
            return crate::source_breakables::ray_shapes(start, end, shapes).map(|t| t * range);
        }
        let (lo, hi) = if let Some(model) = e.me.brush {
            let m = self.world.bsp.models.get(model)?; (m.mins, m.maxs)
        } else { *self.prop_bounds.get(&id)? };
        let distance = crate::ray_box(Vec3::from_array(start.to_array()), Vec3::from_array((end-start).to_array()),
            Vec3::from_array(lo.to_array()), Vec3::from_array(hi.to_array()))?;
        (distance <= 1.).then_some(distance * range)
    }
    /// Armed, untriggered prop_car_alarm entities.
    fn armed_car_alarms(&self) -> Vec<usize> {
        use l4d2r_game_core::entities::Kind;
        self.world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive && matches!(e.kind, Kind::CarAlarm { armed: true, triggered: false, .. }))
            .map(|(id, _)| id)
            .collect()
    }
    fn car_alarm_origin(&self, id: usize) -> Vec3 {
        Vec3::from_array(crate::binary::source_position(self.world.ents[id].me.origin.to_array()))
    }
    /// A survivor's bullet or swing reaching a car alarm sets it off when the
    /// survivor is within car_alarm_distance (owned CCarAlarm OnTakeDamage
    /// 101a7c70 -> 1019e4e0).
    pub fn shoot_car_alarm(
        &mut self,
        eye: Vec3,
        forward: Vec3,
        range: f32,
        collision: &crate::collision::Collision,
    ) -> bool {
        let wall = collision.trace(eye, eye + forward * range, Vec3::ZERO).fraction * range;
        let hit = self
            .armed_car_alarms()
            .into_iter()
            .filter_map(|id| Some((id, self.breakable_hit_distance(id, eye, forward, range)?)))
            .filter(|(_, d)| *d <= wall + 0.1)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((id, _)) = hit else { return false };
        let reach = l4d2r_game_core::entities::CAR_ALARM_DISTANCE * crate::collision::UNIT;
        eye.distance(self.car_alarm_origin(id)) <= reach && self.world.trigger_car_alarm(id)
    }
    /// Chirps for survivors near an armed car (car_alarm_chirp_distance), and
    /// the player bumping into or standing on one sets it off (CCarAlarm
    /// touch, 1019e870). Survivor bots never set alarms off.
    pub fn car_alarm_proximity(&mut self, player: Option<Vec3>, survivors: &[Vec3]) {
        let chirp = l4d2r_game_core::entities::CAR_ALARM_CHIRP_DISTANCE * crate::collision::UNIT;
        for id in self.armed_car_alarms() {
            let origin = self.car_alarm_origin(id);
            if let Some(feet) = player {
                if feet.distance(origin) < 8. {
                    // Standing on it: the car right under the feet.
                    let on = self.breakable_hit_distance(id, feet + Vec3::Y * 0.3, Vec3::NEG_Y, 0.45).is_some();
                    // Walking into it: within the hull radius (16) plus a little.
                    let waist = feet + Vec3::Y * 0.6;
                    let against = (0..8).any(|i| {
                        let a = i as f32 * std::f32::consts::FRAC_PI_4;
                        self.breakable_hit_distance(id, waist, Vec3::new(a.cos(), 0., a.sin()), 0.5).is_some()
                    });
                    if (on || against) && self.world.trigger_car_alarm(id) {
                        continue;
                    }
                }
            }
            if survivors.iter().any(|p| p.distance(origin) < chirp) {
                self.world.chirp_car_alarm(id);
            }
        }
    }
    pub fn shoot_breakable(
        &mut self,
        eye: Vec3,
        forward: Vec3,
        range: f32,
        damage: f32,
        kind: l4d2r_game_core::entities::DamageKind,
        collision: &crate::collision::Collision,
    ) -> Option<f32> {
        let wall = collision
            .trace(eye, eye + forward * range, Vec3::ZERO)
            .fraction
            * range;
        let target = self
            .world
            .ents
            .iter()
            .enumerate()
            .filter(|(id, e)| {
                e.alive
                    && (self.world.durability.contains_key(id)
                        || matches!(e.kind, l4d2r_game_core::entities::Kind::Breakable { .. })
                        // Unbreakable props still take damage events
                        // (testscript buttons: OnTakeDamage).
                        || e.conns.iter().any(|c| c.output.eq_ignore_ascii_case("OnTakeDamage")))
            })
            .filter_map(|(id, _)| {
                let distance = self.breakable_hit_distance(id, eye, forward, range)?;
                if distance <= wall + 0.1 && distance <= range {
                    Some((id, distance))
                } else {
                    None
                }
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, distance)) = target {
            if self.break_surfaces.contains_key(&id) {
                crate::source_break_surfaces::damage_at(self,id,damage,kind,eye+forward*distance);
            } else {self.world.damage_entity(id, damage, kind);}
            crate::source_breakables::sync_doors(self);
            collision.sync_entities(self);
            Some(distance)
        } else {
            None
        }
    }
    pub fn shoot_prop(
        &mut self,
        eye: Vec3,
        direction: Vec3,
        range: f32,
        impulse: f32,
        collision: &crate::collision::Collision,
    ) {
        let wall = collision
            .trace(eye, eye + direction * range, Vec3::ZERO)
            .fraction
            * range;
        let target = self
            .physics
            .0
            .keys()
            .filter_map(|&id| {
                if !self.world.ents[id].alive {
                    return None;
                }
                let (lo, hi) = self.bounds(id)?;
                let distance = crate::ray_box(eye, direction, lo, hi)?;
                (distance <= range && distance <= wall + 0.08).then_some((
                    id,
                    distance,
                    self.physics.center_world(id,&self.world).unwrap_or((lo+hi)*0.5),
                ))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, distance, center)) = target {
            self.physics
                .impulse(id, direction, impulse, eye + direction * distance - center);
        }
    }
    pub fn camera(&self) -> Option<(Transform, f32)> {
        let cam = self.world.cine.view_control?;
        let ce = &self.world.ents[cam];
        let pi = ce.parent.or_else(|| {
            self.world
                .find(ce.me.kv("parentname")?.split(',').next()?)
                .first()
                .copied()
        })?;
        let pe = &self.world.ents[pi];
        let sk = self.ghosts.get(pe.me.mdl.as_ref()?)?;
        let sample_time = self
            .world
            .cine
            .view_movement
            .map_or(self.world.time, |(_, start, _)| start);
        let (seq, t) = self
            .world
            .cine
            .anims
            .get(&pi)
            .and_then(|(name, start)| sk.find_sequence(name).map(|i| (i, sample_time - start)))
            .unwrap_or((0, 0.));
        let pose = sk.pose_sequence(seq, t, false);
        let name = self
            .world
            .cine
            .parent_attach
            .get(&cam)
            .map(String::as_str)
            .unwrap_or("Attachment_1");
        let mat = self.world.world_matrix(pi) * sk.attachment_matrix(name, &pose)?;
        let position = Vec3::from_array(crate::binary::source_position(
            mat.w_axis.truncate().to_array(),
        ));
        let direction = Vec3::from_array(crate::binary::source_normal(
            mat.x_axis.truncate().normalize().to_array(),
        ));
        Some((
            Transform::from_translation(position).looking_to(
                direction,
                Vec3::from_array(crate::binary::source_normal(
                    mat.z_axis.truncate().normalize().to_array(),
                )),
            ),
            ce.me.kv_f32("fov").unwrap_or(90.),
        ))
    }
}
#[derive(Component)]
pub struct CinematicFade;
/// A map entity's pose at the last two logic ticks, for drawing between them.
#[derive(Clone, Copy, Debug)]
pub struct MoverPose {
    previous: Transform,
    current: Transform,
    time: f32,
}
impl MoverPose {
    /// Record `latest` when the logic clock has ticked, then return the pose
    /// `alpha` of the way from the previous tick to the current one. Jumps
    /// over a metre in one tick (teleports, respawns) are not blended.
    pub fn advance(&mut self, latest: Transform, time: f32, alpha: f32) -> Transform {
        if time != self.time {
            self.previous = self.current;
            self.time = time;
        }
        self.current = latest;
        if self.previous.translation.distance_squared(latest.translation) > 1. {
            self.previous = latest;
        }
        // At rest: exactly the latest pose. Slerping a rotation with itself
        // is not bit-exact, so every static map entity (pickups, sleeping
        // props) read as moved each frame and re-ran transform propagation.
        if self.previous == latest {
            return latest;
        }
        Transform {
            translation: self.previous.translation.lerp(latest.translation, alpha),
            rotation: self.previous.rotation.slerp(latest.rotation, alpha),
            scale: self.previous.scale.lerp(latest.scale, alpha),
        }
    }
}
pub fn setup(
    mut commands: Commands,
    logic: Res<MapLogic>,
    mut navigation: ResMut<crate::l4d2r_navigation::Navigation>,
) {
    // func_simpleladder "team" (0 any, 1 survivors, 2 infected) gates which
    // NAV ladders each side's AI may route through.
    let ladder_teams: Vec<_> = logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.me.classname == "func_simpleladder")
        .filter_map(|(id, e)| Some((logic.bounds(id)?, e.me.kv_i32("team").unwrap_or(0))))
        .filter(|(_, team)| *team != 0)
        .collect();
    navigation.set_ladder_teams(&ladder_teams);
    for (id, e) in logic.world.ents.iter().enumerate() {
        if e.alive
            && matches!(
                e.kind,
                l4d2r_game_core::entities::Kind::NavBlocker { blocked: true }
            )
        {
            if let Some(bounds) = logic.bounds(id) {
                navigation.map_blocker_team(
                    id,
                    bounds,
                    true,
                    e.me.kv_i32("teamToBlock").unwrap_or(0),
                );
            }
        }
    }
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: percent(100.),
            height: percent(100.),
            ..default()
        },
        BackgroundColor(Color::NONE),
        ZIndex(100),
        CinematicFade,
    ));
}
fn team_touch(
    world: &mut EntityWorld,
    contacts: &mut std::collections::HashSet<usize>,
    id: usize,
    complete: bool,
) {
    if complete {
        if contacts.insert(id) {
            world.fire_output_pub(id, "OnEntireTeamStartTouch");
        }
    } else if contacts.remove(&id) {
        world.fire_output_pub(id, "OnEntireTeamEndTouch");
    }
}
pub(crate) fn sound_position(world: &EntityWorld, id: usize) -> Option<Vec3> {
    let e = world.ents.get(id)?;
    if e.me.spawnflags() & 1 != 0 {
        return None;
    }
    let speaker =
        e.me.kv("SourceEntityName")
            .and_then(|n| world.find(n).first().copied())
            .unwrap_or(id);
    Some(Vec3::from_array(crate::binary::source_position(
        world.world_matrix(speaker).w_axis.truncate().to_array(),
    )))
}
/// Which func_elevator the human and each bot stand on this tick
/// (retail CTerrorPlayer ground entity, for "Joining team leader on a
/// platform").
#[derive(Resource, Default)]
pub struct PlatformRiders {
    pub player: Option<usize>,
    pub bots: Vec<Option<usize>>,
}
pub fn tick(
    time: Res<Time<Fixed>>,
    mut s: ResMut<Match>,
    mut bodies: Query<&mut crate::game::Body>,
    mut squad: ResMut<crate::game_state::Squad>,
    frontend: Res<Frontend>,
    session: Res<crate::campaign_settings::SessionSettings>,
    mut logic: ResMut<MapLogic>,
    collision: Res<crate::collision::Collision>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    playing: Query<(Entity, &crate::game_audio::MapSound)>,
    mut commands: Commands,
    mut infected: Query<(&Transform, &mut crate::game::Enemy)>,
    mut navigation: Option<ResMut<crate::l4d2r_navigation::Navigation>>,
    native_audio: Res<crate::native_movement_audio::NativeAudio>,
    mut unlocked: Local<(usize, std::collections::HashSet<usize>)>,
    mut riders_on: Option<ResMut<PlatformRiders>>,
) {
    let _timing = crate::performance::scope("l4d2r_cinematic::tick");
    // Checkpoint doors already opened on this map (entity ids are per map).
    let map = Arc::as_ptr(&logic.world.bsp) as usize;
    if unlocked.0 != map {
        *unlocked = (map, Default::default());
    }
    let unlocked_checkpoints = &mut unlocked.1;
    if s.paused || !frontend.playing() {
        return;
    }
    // logic_director_query anger: standing, living survivors' health.
    let standing = |m: &Match| if m.dead || m.incap { 0. } else { m.health.max(0.) };
    let health = if s.infected { 0. } else { standing(&s) } + squad.0.iter().map(|m| standing(&m.state)).sum::<f32>();
    logic.world.survivor_health = Some(health);
    {
        let player = (!s.dead && !s.infected).then(|| bodies.single().ok().map(|b| b.feet)).flatten();
        let mut near: Vec<Vec3> = player.into_iter().collect();
        near.extend(squad.0.iter().filter(|m| !m.state.dead).map(|m| m.position));
        logic.car_alarm_proximity(player, &near);
    }
    // Sample support before moving the car; carry riders by its actual delta.
    let platforms = logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter_map(|(id, e)| {
            if !e.alive
                || !matches!(
                    e.kind,
                    l4d2r_game_core::entities::Kind::Elevator { .. }
                        | l4d2r_game_core::entities::Kind::Train { .. }
                )
            {
                return None;
            }
            let model = e.me.brush?;
            let matrix = logic.world.world_matrix(id);
            let supports = |p: Vec3| {
                collision.overlaps_model(
                    model,
                    matrix,
                    p - Vec3::Y * crate::collision::UNIT * 0.5,
                    crate::collision::UNIT,
                ) && !collision.overlaps_model(
                    model,
                    matrix,
                    p + Vec3::Y * crate::collision::UNIT,
                    crate::collision::UNIT,
                )
            };
            let player = bodies
                .single()
                .ok()
                .filter(|b| !s.dead && b.grounded && supports(b.feet))
                .map(|b| b.feet);
            let riders = squad
                .0
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.state.dead && m.vertical.abs() < 0.01 && supports(m.position))
                .map(|(i, m)| (i, m.position))
                .collect::<Vec<_>>();
            Some((id, matrix, player, riders))
        })
        .collect::<Vec<_>>();
    if let Some(on) = riders_on.as_mut() {
        on.player = platforms.iter().find(|p| p.2.is_some()).map(|p| p.0);
        on.bots = vec![None; squad.0.len()];
        for (id, _, _, riders) in &platforms {
            for (i, _) in riders {
                on.bots[*i] = Some(*id);
            }
        }
    }
    {
        let _timing = crate::performance::scope("map::entity_tick");
        logic.world.tick(time.delta_secs());
    }
    if crate::source_breakables::sync_doors(&mut logic) { collision.sync_entities(&logic); }
    if crate::source_break_surfaces::update(&mut logic) {collision.sync_entities(&logic);}
    for (id, previous, player, riders) in platforms {
        let next = logic.world.world_matrix(id);
        if next == previous {
            continue;
        }
        let motion = next * previous.inverse();
        let moved = |p: Vec3| {
            Vec3::from_array(crate::binary::source_position(
                motion
                    .transform_point3(
                        l4d2r_math::Vec3::new(p.x, -p.z, p.y) / crate::collision::UNIT,
                    )
                    .to_array(),
            ))
        };
        if let Some(p) = player {
            if let Ok(mut body) = bodies.single_mut() {
                body.feet = moved(p);
            }
        }
        for (i, p) in riders {
            squad.0[i].position = moved(p);
        }
    }
    if let Some(nav) = navigation.as_mut() {
        nav.sync_map_time(logic.world.time);
        nav.rescue_vehicle_access(logic.world.cine.rescue_vehicle_nav_unblocked);
        for (id, e) in logic.world.ents.iter().enumerate() {
            if let l4d2r_game_core::entities::Kind::NavBlocker { blocked } = e.kind {
                if let Some(bounds) = logic.bounds(id) {
                    nav.map_blocker_team(
                        id,
                        bounds,
                        blocked && e.alive,
                        e.me.kv_i32("teamToBlock").unwrap_or(0),
                    );
                }
            }
        }
    }
    let pending = std::mem::take(&mut logic.world.unhandled);
    for (id, input, param) in pending {
        let nav_input =
            logic.world.ents.get(id).is_some_and(|e| {
                matches!(e.kind, l4d2r_game_core::entities::Kind::NavBlocker { .. })
            });
        if nav_input
            && (input.eq_ignore_ascii_case("BlockNav") || input.eq_ignore_ascii_case("UnblockNav"))
        {
            if let (Some(nav), Some(bounds)) = (navigation.as_mut(), logic.bounds(id)) {
                let e = &logic.world.ents[id];
                nav.map_blocker_team(
                    id,
                    bounds,
                    e.alive && input.eq_ignore_ascii_case("BlockNav"),
                    e.me.kv_i32("teamToBlock").unwrap_or(0),
                );
            }
        } else {
            logic.world.unhandled.push((id, input, param));
        }
    }
    let mut physics = std::mem::take(&mut logic.physics);
    let inner = &mut *logic;
    let moved_props = physics.tick(
        &mut inner.world,
        &inner.prop_bounds,
        &collision,
        time.delta_secs(),
    );
    logic.physics = physics;
    if moved_props {
        collision.sync_entities(&logic);
        // Props landing on or hitting survivors (retail impact damage).
        let mut survivors = vec![];
        if !s.dead && !s.infected {
            if let Ok(b) = bodies.single() {
                survivors.push((0, b.feet));
            }
        }
        survivors.extend(squad.0.iter().enumerate().filter(|(_, m)| !m.state.dead).map(|(i, m)| (i + 1, m.position)));
        let mut hits = vec![];
        let punch = crate::special_infected::tank::punch_damage(false, crate::special_infected::ai::difficulty_index(session.difficulty), false);
        crate::prop_impact::apply(time.elapsed_secs(), punch, &logic, &survivors, |i, d| hits.push((i, d)));
        for (i, d) in hits {
            println!("Prop impact: survivor {i} takes {d} crush damage");
            if i == 0 {
                s.damage(d);
            } else if let Some(m) = squad.0.get_mut(i - 1) {
                m.state.damage(d);
            }
        }
    }
    if !logic.world.cine.intro {
        let _timing = crate::performance::scope("map::trigger_contacts");
        let mut positions = squad
            .0
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.state.dead)
            .map(|(i, b)| (Some(i), b.position))
            .collect::<Vec<_>>();
        let mut player = bodies.single_mut().ok();
        if !s.dead {
            if let Some(body) = player.as_ref() {
                positions.insert(0, (None, body.feet));
            }
        }
        let damage_tick = logic.hurt_clock <= logic.world.time;
        if damage_tick {
            logic.hurt_clock = logic.world.time + 0.5;
        }
        for id in logic.triggers.clone() {
            let e = &logic.world.ents[id];
            let enabled = e.alive
                && match &e.kind {
                    l4d2r_game_core::entities::Kind::Trigger { disabled, .. }
                    | l4d2r_game_core::entities::Kind::Hurt { disabled, .. }
                    | l4d2r_game_core::entities::Kind::Push { disabled, .. } => !*disabled,
                    _ => false,
                };
            let matrix = logic.world.world_matrix(id);
            // EDIT WORLD box triggers have no brush model: their mins/maxs
            // box (prop_bounds, tested by `nearby`) is the whole volume.
            let model = e.me.brush;
            let allowed = logic.accepts_survivor(id);
            let bounds = logic.bounds(id);
            let nearby = |p: Vec3| {
                bounds.is_some_and(|(lo, hi)| {
                    p.x + crate::collision::RADIUS >= lo.x
                        && p.x - crate::collision::RADIUS <= hi.x
                        && p.z + crate::collision::RADIUS >= lo.z
                        && p.z - crate::collision::RADIUS <= hi.z
                        && p.y + crate::collision::HEIGHT >= lo.y
                        && p.y <= hi.y
                })
            };
            // Reuse the same occupancy snapshot for damage, touch and enter/exit.
            // Positions and trigger transforms are fixed for this iteration.
            let touching = positions
                .iter()
                .filter(|(_, p)| {
                    enabled
                        && allowed
                        && nearby(*p)
                        && model.is_none_or(|model| collision.overlaps_model(model, matrix, *p, crate::collision::HEIGHT))
                })
                .collect::<Vec<_>>();
            let touched = !touching.is_empty();
            if touched {
                let kind = logic.world.ents[id].kind.clone();
                for &&(actor, _p) in &touching {
                    match kind {
                        l4d2r_game_core::entities::Kind::Hurt { damage, .. } if damage_tick => {
                            if let Some(index) = actor {
                                squad.0[index].state.damage(damage * 0.5);
                            } else {
                                s.damage(damage * 0.5);
                            }
                        }
                        l4d2r_game_core::entities::Kind::Push { dir, speed, .. } => {
                            let push = Vec3::from_array(crate::binary::source_normal(dir.to_array()))
                                * speed
                                * crate::collision::UNIT
                                * time.delta_secs();
                            // Like the engine's base velocity push, it also
                            // frees a survivor wedged against a door frame:
                            // a slide that cannot start moves straight to a
                            // clear spot.
                            let shove = |from: Vec3| {
                                let slid = collision.slide(from, push, crate::collision::HEIGHT);
                                if slid.distance(from) >= push.length() * 0.05 {
                                    return slid;
                                }
                                let step = push.normalize_or_zero() * 0.05;
                                (1..=20)
                                    .map(|k| from + step * k as f32)
                                    .find(|p| collision.clear_hull(*p, crate::collision::HEIGHT))
                                    .unwrap_or(slid)
                            };
                            if let Some(index) = actor {
                                // Survivor bots are pushed too (c1m4's
                                // push_elevator sweeps the doorway into the
                                // car before it leaves).
                                let m = &mut squad.0[index];
                                m.position = shove(m.position);
                            } else if let Some(body) = player.as_mut() {
                                body.feet = shove(body.feet);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if enabled && damage_tick && logic.accepts_actor(id, true) {
                if let l4d2r_game_core::entities::Kind::Hurt { damage, .. } =
                    logic.world.ents[id].kind
                {
                    let burns = logic.world.ents[id].me.kv_i32("damagetype").unwrap_or(0) & 8 != 0;
                    for (transform, mut enemy) in &mut infected {
                        if enemy.health > 0.
                            && nearby(transform.translation)
                            && model.is_none_or(|model| collision.overlaps_model(
                                model,
                                matrix,
                                transform.translation,
                                crate::collision::HEIGHT,
                            ))
                        {
                            enemy.health -= damage * 0.5;
                            if burns {
                                enemy.burning = 0.6;
                            }
                            if enemy.health <= 0. {
                                enemy.record_death(
                                    if burns {
                                        crate::infected_death::Cause::Fire
                                    } else {
                                        crate::infected_death::Cause::Blunt
                                    },
                                    transform,
                                    Vec3::ZERO,
                                    1.,
                                );
                                enemy.corpse_time = 12.;
                            }
                        }
                    }
                }
            }
            // Individual enters/exits remain distinct while TouchAll uses aggregate occupancy.
            let now_touching = touching
                .iter()
                .map(|(actor, _)| actor.map_or(0, |id| id as u64 + 1))
                .collect::<std::collections::HashSet<_>>();
            if logic.world.ents[id].me.kv_i32("entireteam").unwrap_or(0) > 0 {
                let allow_incap = logic.world.ents[id].me.kv_i32("allowincap") == Some(1);
                let incapacitated =
                    (!s.dead && s.incap) || squad.0.iter().any(|m| !m.state.dead && m.state.incap);
                let complete = enabled
                    && !positions.is_empty()
                    && now_touching.len() == positions.len()
                    && (allow_incap || !incapacitated);
                let inner = &mut *logic;
                team_touch(&mut inner.world, &mut inner.team_contacts, id, complete);
            }
            let previous = logic
                .contacts
                .iter()
                .filter(|(ent, _)| *ent == id)
                .map(|(_, actor)| *actor)
                .collect::<Vec<_>>();
            for actor in &now_touching {
                logic.contacts.insert((id, *actor));
                logic.world.touch_actor(id, *actor, None);
            }
            for actor in previous
                .into_iter()
                .filter(|actor| !now_touching.contains(actor))
            {
                logic.contacts.remove(&(id, actor));
                logic.world.untouch_actor(id, actor, None);
            }
            if logic.world.ents[id].me.classname == "trigger_look" {
                let looking = player
                    .as_ref()
                    .and_then(|body| {
                        if !touched
                            || !model.is_none_or(|model| collision.overlaps_model(
                                model,
                                matrix,
                                body.feet,
                                crate::collision::HEIGHT,
                            ))
                        {
                            return None;
                        }
                        let target = logic.world.ents[id].me.kv("target")?;
                        let target = *logic.world.find(target).first()?;
                        let eye = body.feet + Vec3::Y * crate::collision::EYE;
                        let point = Vec3::from_array(crate::binary::source_position(
                            logic
                                .world
                                .world_matrix(target)
                                .w_axis
                                .truncate()
                                .to_array(),
                        ));
                        let direction =
                            Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
                        Some(
                            direction.dot((point - eye).normalize_or_zero())
                                >= logic.world.ents[id].me.kv_f32("FieldOfView").unwrap_or(0.9)
                                && collision.trace(eye, point, Vec3::ZERO).fraction >= 0.99,
                        )
                    })
                    .unwrap_or(false);
                let threshold = logic.world.ents[id]
                    .me
                    .kv_f32("LookTime")
                    .unwrap_or(0.5)
                    .max(0.01);
                let progress = logic.look_progress.entry(id).or_default();
                if looking {
                    *progress += time.delta_secs();
                } else {
                    *progress = 0.;
                }
                if *progress >= threshold {
                    *progress = 0.;
                    logic.world.fire_output_pub(id, "OnTrigger");
                    if logic.world.ents[id].me.spawnflags() & 128 != 0 {
                        logic.world.ents[id].alive = false;
                    }
                }
            }
        }
    }
    if logic.capture_elevator {
        if let Some(&button) = logic.world.find("elevator_button").first() {
            if matches!(
                logic.world.ents[button].kind,
                l4d2r_game_core::entities::Kind::Button { locked: false, .. }
            ) {
                logic.world.use_ent(button, None);
                logic.capture_elevator = false;
                println!(
                    "Hotel elevator fixture: whole-team gate unlocked; pressed authored button"
                );
            }
        }
    }
    if logic.world.events.iter().any(|e| match e {
        l4d2r_game_core::entities::EntEvent::Moved { ent, .. } => {
            crate::collision::collider_move_is_relevant(&logic.world, *ent)
        }
        l4d2r_game_core::entities::EntEvent::Solid { .. }
        | l4d2r_game_core::entities::EntEvent::Spawned { .. } => true,
        _ => false,
    }) {
        collision.sync_entities(&logic);
    }
    let mut events = HashMap::new();
    for (entity, sound) in &playing {
        if let Some(position) = sound_position(&logic.world, sound.0) {
            commands
                .entity(entity)
                .insert(Transform::from_translation(position));
        }
        // Gameplay cues (music, voices) tag their sounds with ids counted
        // down from usize::MAX; only real map entities end with the map's.
        if sound.0 < crate::game_audio::GAMEPLAY_SOUND_IDS
            && logic.world.ents.get(sound.0).is_none_or(|e| !e.alive)
        {
            events.insert(sound.0, false);
        }
    }
    for event in &logic.world.events {
        if let l4d2r_game_core::entities::EntEvent::Broken { ent } = event {
            if let Some(d) = logic.world.durability.get(ent) {
                if d.sound.is_empty() { continue; }
                let p = logic.world.world_matrix(*ent).w_axis.truncate();
                sounds.play(
                    &d.sound,
                    Some(Vec3::from_array(crate::binary::source_position(
                        p.to_array(),
                    ))),
                );
            } else {
                let e = &logic.world.ents[*ent];
                if matches!(e.kind, l4d2r_game_core::entities::Kind::Breakable { .. }) {
                    let sound = crate::source_breakables::material_sound(e.me.kv_i32("material").unwrap_or(0));
                    if !sound.is_empty() { sounds.play(sound, logic.bounds(*ent).map(|(lo, hi)| (lo + hi) * 0.5)); }
                }
            }
        }
        if let l4d2r_game_core::entities::EntEvent::Hurt { ent, damage } = event {
            let e = &logic.world.ents[*ent];
            if let Some(model) = e.me.brush {
                let matrix = logic.world.world_matrix(*ent);
                if let Ok(body) = bodies.single_mut() {
                    if collision.overlaps_model(model, matrix, body.feet, crate::collision::HEIGHT)
                    {
                        s.damage(*damage);
                    }
                }
                for member in &mut squad.0 {
                    if !member.state.dead
                        && collision.overlaps_model(
                            model,
                            matrix,
                            member.position,
                            crate::collision::HEIGHT,
                        )
                    {
                        member.state.damage(*damage);
                    }
                }
            }
        }
        if let l4d2r_game_core::entities::EntEvent::DoorSound { ent, output } = event {
            let p = logic.world.world_matrix(*ent).w_axis.truncate();
            let at = Some(Vec3::from_array(crate::binary::source_position(
                p.to_array(),
            )));
            if output == "Pound" {
                if let Some(profile) = logic.door_damage.get(ent) { sounds.play(&profile.pound, at); }
                continue;
            }
            // A checkpoint door plays its unlock clunk the first time it opens.
            if output == "OnOpen"
                && logic.world.ents.get(*ent).is_some_and(|e| {
                    e.me.classname == "prop_door_rotating_checkpoint"
                })
                && unlocked_checkpoints.insert(*ent)
            {
                if let Some(sound) = native_audio.door(*ent, "Unlock") {
                    sounds.play(sound, at);
                }
            }
            if let Some(sound) = native_audio.door(*ent, output) {
                sounds.play(sound, at);
            }
        }
        if let l4d2r_game_core::entities::EntEvent::Sound { ent, play } = event {
            events.insert(*ent, *play);
        }
    }
    for (id, play) in events {
        if !play {
            sounds.1.push(id);
        }
        for (entity, sound) in &playing {
            if sound.0 == id {
                commands.entity(entity).despawn();
            }
        }
        let Some(e) = logic.world.ents.get(id) else {
            continue;
        };
        if play && e.alive {
            if let Some(name) = e.me.kv("message") {
                sounds.0.push_back(crate::game_audio::Request {
                    name: name.into(),
                    // SF_AMBIENT_SOUND_EVERYWHERE bypasses positional attenuation.
                    position: sound_position(&logic.world, id),
                    looping: e.me.spawnflags() & 32 == 0,
                    // CAmbientGeneric: volume = health / 10 (default 10).
                    gain: (e.me.kv_f32("health").unwrap_or(10.) * 10.).clamp(0., 100.) / 100.,
                    map_entity: Some(id),
                    // ComputeSoundlevel: 40 dB + 20 log10(radius / 36); none
                    // for play-everywhere or no radius. A raw .wav uses it;
                    // soundscript names keep their scripted level.
                    soundlevel: if name.to_ascii_lowercase().ends_with(".wav") || name.to_ascii_lowercase().ends_with(".mp3") {
                        let radius = e.me.kv_f32("radius").unwrap_or(1250.);
                        Some(if radius > 0. && e.me.spawnflags() & 1 == 0 {
                            (40. + 20. * (radius / 36.).log10()).trunc()
                        } else {
                            0.
                        })
                    } else {
                        None
                    },
                });
            }
        }
    }
    if let Some(mut dialogue) = logic.dialogue.take() {
        // Only survivors present in the world speak: the player's survivor
        // while on the survivor team, plus the bots. On the infected team the
        // selected survivor is absent and the player's health is an
        // infected's, so it must not drive survivor barks.
        let mut roster: Vec<&str> = squad
            .0
            .iter()
            .filter(|m| !m.state.dead)
            .map(|m| m.model.trim_start_matches("models/survivors/survivor_").trim_end_matches(".mdl"))
            .collect();
        if !s.infected && !s.dead {
            roster.insert(0, session.survivor.character());
        }
        if !logic.active {
            let now = logic.world.time;
            if !s.infected {
                dialogue.observe(session.survivor.character(), s.health, s.reload > 0., now);
            }
            for (who, concept) in crate::source_dialogue::take_requests() {
                dialogue.speak(&who, &concept, now);
            }
            for member in &squad.0 {
                let who = member
                    .model
                    .trim_start_matches("models/survivors/survivor_")
                    .trim_end_matches(".mdl");
                dialogue.observe(who, member.state.health, member.state.reload > 0., now);
            }
        }
        dialogue.set_map(&session.map.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        // Map I/O actor ids: 0 is the player, n is squad member n - 1.
        dialogue.set_actors(
            std::iter::once(session.survivor.character().to_string())
                .chain(squad.0.iter().map(|m| {
                    m.model.trim_start_matches("models/survivors/survivor_").trim_end_matches(".mdl").to_string()
                }))
                .collect(),
        );
        dialogue.tick(&mut logic.world, &mut sounds, &roster);
        logic.dialogue = Some(dialogue);
    }
    let hint_events = logic
        .world
        .events
        .iter()
        .filter_map(|e| {
            if let l4d2r_game_core::entities::EntEvent::HudHint { ent, show } = e {
                Some((*ent, *show))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for (id, show) in hint_events {
        if show {
            let duration = logic.world.ents[id]
                .me
                .kv_f32("hint_timeout")
                .unwrap_or(10.);
            logic.hud_hint = Some((
                id,
                logic.world.time
                    + if duration > 0. {
                        duration
                    } else {
                        f32::INFINITY
                    },
            ));
        } else if logic.hud_hint.is_some_and(|(active, _)| active == id) {
            logic.hud_hint = None;
        }
    }
    if logic
        .hud_hint
        .is_some_and(|(id, end)| !logic.world.ents[id].alive || logic.world.time >= end)
    {
        logic.hud_hint = None;
    }
    if crate::source_break_debris::update(&mut logic) { collision.sync_entities(&logic); }
    logic.world.events.clear();
    logic.active = logic.world.cine.intro || logic.world.cine.view_control.is_some();
}
pub fn presentation(
    logic: Res<MapLogic>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    frontend: Res<Frontend>,
    settings: Res<crate::performance::DisplaySettings>,
    mut camera: Single<
        (&mut Transform, &mut Projection, &crate::game::Body),
        With<crate::viewer::GameCamera>,
    >,
    mut entities: Query<
        (
            Entity,
            &MapEntity,
            Option<&crate::game::Taken>,
            Option<&crate::loot::PickupPlacement>,
            &mut Transform,
            &mut Visibility,
        ),
        Without<crate::viewer::GameCamera>,
    >,
    mut hud: Query<
        &mut Visibility,
        (
            With<crate::game_ui::HudRoot>,
            Without<MapEntity>,
            Without<CinematicFade>,
            Without<crate::viewer::GameCamera>,
        ),
    >,
    mut held: Query<
        &mut Visibility,
        (
            With<crate::game::HeldWeapon>,
            Without<crate::game_ui::HudRoot>,
            Without<MapEntity>,
            Without<crate::viewer::GameCamera>,
        ),
    >,
    mut fade_overlay: Query<&mut BackgroundColor, With<CinematicFade>>,
    scripts: Query<(&ScriptedDraw, &Mesh3d, &ViewVisibility)>,
    mut intro_draws: Query<
        (&IntroDraw, &Mesh3d, &mut Transform, &mut Visibility),
        (
            Without<MapEntity>,
            Without<crate::viewer::GameCamera>,
            Without<crate::game_ui::HudRoot>,
            Without<crate::game::HeldWeapon>,
            Without<crate::game::CompanionActor>,
        ),
    >,
    mut companions: Query<
        (&crate::game::CompanionActor, &mut Visibility),
        (
            With<crate::game::CompanionActor>,
            Without<IntroDraw>,
            Without<MapEntity>,
            Without<crate::viewer::GameCamera>,
            Without<crate::game_ui::HudRoot>,
            Without<crate::game::HeldWeapon>,
        ),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut script_cache: Local<(usize,HashMap<bevy::asset::AssetId<Mesh>,(String,f32)>)>,
    mut motion: (Option<Res<Time<Fixed>>>, Local<HashMap<Entity, MoverPose>>),
) {
    let _timing = crate::performance::scope("l4d2r_cinematic::presentation");
    if !frontend.playing() {
        return;
    }
    let hide_capture_bots = std::env::args().any(|a| a == "--capture-game")
        && std::env::args().any(|a| a == "--capture-no-bots");
    for (actor, mut visible) in &mut companions {
        visible.set_if_neq(if logic.active || hide_capture_bots || crate::game_state::kicked(actor.0) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }
    let intro_clock = logic
        .world
        .cine
        .anims
        .iter()
        .find_map(|(id, (_, start))| {
            logic
                .world
                .ents
                .get(*id)
                .filter(|e| e.me.mdl.as_deref().is_some_and(|m| m.contains("ghostanim")))
                .map(|_| (logic.world.time - start).max(0.))
        })
        .unwrap_or(0.);
    for (draw, mesh, mut transform, mut visible) in &mut intro_draws {
        let e = &logic.world.ents[draw.ent];
        let local = e.me.kv("survivorname").is_some_and(|name| {
            logic
                .world
                .cine
                .selected_survivor
                .as_deref()
                .is_some_and(|selected| name.eq_ignore_ascii_case(selected))
        });
        let entering_eye = local
            && logic
                .world
                .cine
                .view_movement
                .is_some_and(|(_, start, duration)| (logic.world.time - start) / duration >= 0.9);
        let showing = logic.active && !entering_eye;
        visible.set_if_neq(if showing {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if showing {
            transform.set_if_neq(crate::l4d2r_cinematic::transform(
                logic.world.world_matrix(draw.ent),
            ));
            // Posed on the GPU by `scripted_skinning`.
            let _ = (mesh, intro_clock);
        }
    }
    // Map logic advances in the 30 Hz FixedUpdate while the camera is drawn
    // between the last two ticks (`sync_camera`). Draw moving brushes and
    // props the same way, or an elevator steps against the smooth view.
    let alpha = motion.0.as_ref().map_or(1., |f| f.overstep_fraction());
    let poses = &mut motion.1;
    poses.retain(|entity, _| entities.contains(*entity));
    for (entity, id, taken, placement, mut t, mut v) in &mut entities {
        let Some(e) = logic.world.ents.get(id.0) else {
            continue;
        };
        let mut latest = transform(logic.world.world_matrix(id.0));
        if let Some(placement) = placement {
            latest.translation += placement.0;
        }
        let next = poses
            .entry(entity)
            .or_insert(MoverPose { previous: latest, current: latest, time: logic.world.time })
            .advance(latest, logic.world.time, alpha);
        if *t != next {
            *t = next;
        }
        let visible = if taken.is_none()
            && e.alive
            && !matches!(
                e.kind,
                l4d2r_game_core::entities::Kind::Brush { enabled: false, .. }
            ) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != visible {
            *v = visible;
        }
    }
    if let Some((mut t, mut fov)) = logic.camera() {
        if let Some((_, start, duration)) = logic.world.cine.view_movement {
            let body = camera.2;
            let destination = Transform::from_translation(body.feet + Vec3::Y * body.stance.eye())
                .with_rotation(Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.));
            (t, fov) = camera_handoff(
                t,
                fov,
                destination,
                settings.camera_fov,
                (logic.world.time - start) / duration,
            );
        }
        camera.0.set_if_neq(t);
        set_camera_fov(
            &mut camera.1,
            2. * ((fov.to_radians() * 0.5).tan() / (4. / 3.)).atan(),
        );
    } else {
        set_camera_fov(
            &mut camera.1,
            crate::performance::viewmodel_vertical_fov(settings.camera_fov),
        );
    }
    for mut v in &mut hud {
        v.set_if_neq(if logic.active {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }
    if logic.active {
        for mut v in &mut held {
            v.set_if_neq(Visibility::Hidden);
        }
    }
    // engine_post blends the view fade into the frame under the HUD; the
    // overlay is only the fallback without that pass.
    let in_post = std::env::var_os("L4D2R_NO_POST").is_none();
    for mut color in &mut fade_overlay {
        if in_post {
            color.set_if_neq(BackgroundColor(Color::NONE));
            continue;
        }
        color.set_if_neq(BackgroundColor(logic.world.cine.fade.map_or(
            Color::NONE,
            |f| {
                Color::srgba(
                    f.color[0],
                    f.color[1],
                    f.color[2],
                    f.alpha_at(logic.world.time),
                )
            },
        )));
    }
    let _ = (&scripts, &mut meshes, &mut script_cache, &library);
}
/// Intro survivors and scripted map models (`ScriptedDraw`) are posed on the
/// GPU: their meshes are converted to skinned meshes once and only joint
/// transforms change per frame (`AnimationLibrary::scripted_gpu`).
#[allow(clippy::too_many_arguments)]
pub fn scripted_skinning(
    mut commands: Commands,
    logic: Res<MapLogic>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    frontend: Res<Frontend>,
    intro: Query<(Entity, &IntroDraw, &Mesh3d, &Visibility, Option<&crate::l4d2r_animation::GpuSkin>)>,
    scripts: Query<(Entity, &ScriptedDraw, &Mesh3d, &ViewVisibility, Option<&crate::l4d2r_animation::GpuSkin>)>,
    mut joints: Query<&mut Transform, With<crate::l4d2r_animation::GpuBone>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
) {
    let _timing = crate::performance::scope("l4d2r_cinematic::scripted_skinning");
    if !frontend.playing() {
        return;
    }
    let intro_clock = logic
        .world
        .cine
        .anims
        .iter()
        .find_map(|(id, (_, start))| {
            logic
                .world
                .ents
                .get(*id)
                .filter(|e| e.me.mdl.as_deref().is_some_and(|m| m.contains("ghostanim")))
                .map(|_| (logic.world.time - start).max(0.))
        })
        .unwrap_or(0.);
    for (entity, draw, mesh, visibility, gpu) in &intro {
        if *visibility == Visibility::Hidden {
            continue;
        }
        let Some(name) = logic.world.ents[draw.ent].me.kv("survivorintrosequence") else {
            continue;
        };
        library.scripted_gpu(
            &mut commands, entity, gpu, &mut joints, &mut meshes, &mut bindposes, &mesh.0,
            (&draw.model, &draw.mesh_model, draw.draw),
            (name, intro_clock),
        );
    }
    for (entity, d, mesh, visible, gpu) in &scripts {
        if !visible.get() && gpu.is_some() {
            continue;
        }
        if let Some((name, start)) = logic.world.cine.anims.get(&d.ent) {
            library.scripted_gpu(
                &mut commands, entity, gpu, &mut joints, &mut meshes, &mut bindposes, &mesh.0,
                (&d.model, &d.model, d.draw),
                (name, logic.world.time - start),
            );
        }
    }
}
fn camera_handoff(
    from: Transform,
    from_fov: f32,
    to: Transform,
    to_fov: f32,
    fraction: f32,
) -> (Transform, f32) {
    let u = fraction.clamp(0., 1.);
    let u = u * u * (3. - 2. * u);
    (
        Transform::from_translation(from.translation.lerp(to.translation, u))
            .with_rotation(from.rotation.slerp(to.rotation, u)),
        from_fov + (to_fov - from_fov) * u,
    )
}
fn set_camera_fov(projection: &mut Mut<'_, Projection>, fov: f32) {
    // Reading via DerefMut would invalidate projection/frustum work even when
    // the FOV is unchanged, so compare through the immutable view first.
    if matches!(&**projection, Projection::Perspective(p) if p.fov != fov) {
        if let Projection::Perspective(p) = &mut **projection {
            p.fov = fov;
        }
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;
    #[test]
    fn c5m2_ceda_trailer_exit_unlocks_once_all_inside_with_the_door_closed() {
        // logic_branch_listener: entrance closed + entire team inside ->
        // OnAllTrue -> finale_cleanse_exit_door Unlock; anyone leaving locks
        // it again (OnMixed).
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic = MapLogic::load(root, &root.join("left4dead2/maps/c5m2_park.bsp"), true).unwrap();
        let pump = |logic: &mut MapLogic| {
            for _ in 0..30 {
                logic.world.tick(1. / 60.);
            }
        };
        let exit = logic.world.find("finale_cleanse_exit_door")[0];
        let locked = |logic: &MapLogic| matches!(&logic.world.ents[exit].kind, Kind::Door(d) if d.locked);
        assert!(locked(&logic), "the exit starts locked");
        let closed = logic.world.find("ceda_trailer_doorclosed_branch")[0];
        let inside = logic.world.find("ceda_trailer_allinside_branch")[0];
        logic.world.input(closed, "SetValueTest", "1", None, None);
        pump(&mut logic);
        assert!(locked(&logic), "door closed but team outside: still locked");
        logic.world.input(inside, "SetValueTest", "1", None, None);
        pump(&mut logic);
        assert!(!locked(&logic), "all inside with the door closed unlocks the exit");
        logic.world.input(inside, "SetValueTest", "0", None, None);
        pump(&mut logic);
        assert!(locked(&logic), "someone leaving locks it again");
    }
    #[test]
    fn c5m4_parade_float_rolls_to_the_end_of_its_track() {
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c5m4_quarter.bsp");
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        let pump = |logic: &mut MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                logic.world.tick(1. / 60.);
            }
        };
        pump(&mut logic, 1.);
        let float = logic.world.find("float_tracktrain")[0];
        let state = |logic: &MapLogic| match logic.world.ents[float].kind {
            Kind::Train { offset, moving, .. } => (offset, moving),
            _ => panic!("float_tracktrain is not a train"),
        };
        assert!(!state(&logic).1);
        let relay = logic.world.find("tractor_start_relay")[0];
        logic.world.fire_output_pub(relay, "OnTrigger");
        pump(&mut logic, 10.);
        assert!(state(&logic).1, "the tractor pulls the float");
        pump(&mut logic, 60.);
        let (offset, moving) = state(&logic);
        // float_path1 -> float_path2 is 656 units along -y (Source +y).
        assert!(!moving && (offset.y - 656.).abs() < 1., "reached float_path2 ({offset:?})");
        let push = logic.world.find("float_push");
        assert!(push.iter().all(|&p| matches!(logic.world.ents[p].kind, Kind::Push { disabled: true, .. })), "the push turns off at the end");
    }
    #[test]
    fn c3m1_ferry_comes_when_called_and_crosses_with_the_team() {
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c3m1_plankcountry.bsp");
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        let pump = |logic: &mut MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                logic.world.tick(1. / 60.);
            }
        };
        pump(&mut logic, 1.);
        let ferry = logic.world.find("ferry_tram")[0];
        let state = |logic: &MapLogic| match logic.world.ents[ferry].kind {
            Kind::Train { offset, moving, .. } => (offset, moving),
            _ => panic!("ferry_tram is not a train"),
        };
        let open = |logic: &MapLogic, name: &str| {
            logic.world.find(name).iter().all(|&d| matches!(&logic.world.ents[d].kind, Kind::Door(d) if d.frac > 0.5))
        };
        assert_eq!(state(&logic), (l4d2r_math::Vec3::ZERO, false), "waits at the far dock");
        // Call it from the dock (ferry_button -> stick relay -> SetSpeed .5 after 2+2 s).
        let button = logic.world.find("ferry_button")[0];
        logic.world.use_ent(button, None);
        pump(&mut logic, 6.);
        assert!(state(&logic).1, "the called ferry is moving");
        pump(&mut logic, 80.);
        let (offset, moving) = state(&logic);
        println!("ferry arrived at offset {offset:?}");
        assert!(!moving && (offset.x + 752.).abs() < 1., "reached ferry_path_2 ({offset:?})");
        assert!(open(&logic, "ferry_door_left_entrance") && open(&logic, "ferry_door_right_entrance"), "boarding doors open");
        // The doors ride along with it.
        let door = logic.world.find("ferry_door_left_entrance")[0];
        let door_origin = logic.world.world_matrix(door).transform_point3(l4d2r_math::Vec3::ZERO);
        assert!(door_origin.x < logic.world.ents[door].me.origin.x - 700., "{door_origin:?}");
        // The deck you stand on is the train's own brush, so riders are carried.
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        collision.sync_entities(&logic);
        let model = logic.world.ents[ferry].me.brush.unwrap();
        let matrix = logic.world.world_matrix(ferry);
        let mut decks = 0;
        for dx in [-120., -60., 0., 60., 120.] {
            for dy in [-60., 0., 60.] {
                let top = Vec3::from_array(crate::binary::source_position([-5248. + dx, 6064. + dy, 80.]));
                let hit = collision.trace(top, top - Vec3::Y * 3., Vec3::new(0.3, 0.01, 0.3));
                if hit.fraction >= 1. {
                    continue;
                }
                let feet = top - Vec3::Y * 3. * hit.fraction;
                let carried = collision.overlaps_model(model, matrix, feet - Vec3::Y * crate::collision::UNIT * 0.5, crate::collision::UNIT);
                println!("deck {dx} {dy}: feet {:?} carried {carried}", feet / crate::collision::UNIT);
                decks += carried as usize;
            }
        }
        assert!(decks > 0, "no deck spot is on the ferry brush");
        // Whole team aboard unlocks the ferry's button; it carries them back.
        let gate = logic.world.find("ferry_tram_button_trigger")[0];
        let inner = &mut logic;
        team_touch(&mut inner.world, &mut inner.team_contacts, gate, true);
        pump(&mut logic, 0.1);
        let launch = logic.world.find("ferry_tram_button")[0];
        assert!(matches!(logic.world.ents[launch].kind, Kind::Button { locked: false, .. }));
        logic.world.use_ent(launch, None);
        pump(&mut logic, 1.);
        assert!(state(&logic).1, "launched");
        assert!(!open(&logic, "ferry_door_left_entrance"), "boarding doors close");
        pump(&mut logic, 80.);
        let (offset, moving) = state(&logic);
        assert!(!moving && offset.length() < 1., "back across at ferry_path_1 ({offset:?})");
        assert!(open(&logic, "ferry_door_left_exit") && open(&logic, "ferry_door_right_exit"), "exit doors open");
    }
    #[test]
    fn c1m4_elevator_unlocks_for_the_whole_team_and_rides_down_to_the_finale() {
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m4_atrium.bsp");
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let pump = |logic: &mut MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                logic.world.tick(1. / 60.);
            }
        };
        pump(&mut logic, 1.);
        let car = logic.world.find("elevator")[0];
        let button = logic.world.find("button_elev_3rdfloor")[0];
        let offset = |logic: &MapLogic| match logic.world.ents[car].kind {
            Kind::Elevator { offset, moving, .. } => (offset, moving),
            _ => panic!("not an elevator"),
        };
        assert!(matches!(logic.world.ents[button].kind, Kind::Button { locked: true, .. }), "locked until the team is inside");
        // The player presses the elevator panel from inside the car.
        let panel = Vec3::from_array(crate::binary::source_position([-3944., -3478., 592.]));
        let eye = Vec3::from_array(crate::binary::source_position([-3990., -3420., 596.]));
        let at_panel = (panel - eye).normalize();
        collision.sync_entities(&logic);
        logic.use_near(eye, at_panel, &collision);
        pump(&mut logic, 3.);
        assert!(!offset(&logic).1, "a locked button does not start the car");
        // Whole team inside: unlocked; the panel starts the ride.
        let gate = logic
            .world
            .ents
            .iter()
            .position(|e| e.me.kv_i32("entireteam") == Some(2) && e.conns.iter().any(|c| c.target == "button_elev_3rdfloor"))
            .unwrap();
        let inner = &mut logic;
        team_touch(&mut inner.world, &mut inner.team_contacts, gate, true);
        pump(&mut logic, 0.1);
        assert!(matches!(logic.world.ents[button].kind, Kind::Button { locked: false, .. }));
        collision.sync_entities(&logic);
        assert!(logic.use_near(eye, at_panel, &collision), "the elevator panel responds to use");
        pump(&mut logic, 3.);
        assert!(offset(&logic).1, "the car is moving");
        let start = offset(&logic).0;
        pump(&mut logic, 40.);
        let (end, moving) = offset(&logic);
        println!("elevator offset {start:?} -> {end:?}");
        assert!(!moving && (end - start).length() > 400., "reached the bottom ({end:?})");
        let doors = logic.world.find("door_elevator");
        assert!(doors.iter().all(|&d| matches!(&logic.world.ents[d].kind, Kind::Door(d) if d.frac > 0.5)), "bottom doors open");
        assert!(!logic.world.ents[button].alive, "the button is used up at the bottom");
    }
    #[test]
    fn c1m3_mall_alarm_starts_from_the_door_or_the_glass_and_the_button_ends_it() {
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m3_mall.bsp");
        let pump = |logic: &mut MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                logic.world.tick(1. / 60.);
            }
        };
        let scripts = |logic: &mut MapLogic| -> Vec<String> {
            std::mem::take(&mut logic.world.cine.director_commands)
                .into_iter()
                .filter(|(c, _)| c.eq_ignore_ascii_case("beginscript"))
                .map(|(_, p)| p.to_lowercase())
                .collect()
        };
        let alarm_playing = |logic: &MapLogic| {
            let id = logic.world.find("sound_alarm")[0];
            matches!(logic.world.ents[id].kind, Kind::Ambient { playing: true })
        };
        // The door route: opening the hallway door starts the onslaught.
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        pump(&mut logic, 1.);
        scripts(&mut logic);
        let door = logic.world.find("door_hallway_lower4")[0];
        logic.world.fire_output_pub(door, "OnOpen");
        pump(&mut logic, 1.5);
        assert!(scripts(&mut logic).contains(&"c1_mall_onslaught".into()));
        assert!(alarm_playing(&logic), "the alarm sounds");
        // The button switches it off: the cooldown script, the sound stops,
        // the button is used up.
        let button = logic
            .world
            .ents
            .iter()
            .position(|e| e.me.classname == "func_button" && e.conns.iter().any(|c| c.param.to_lowercase().contains("c1_mall_crescendo_cooldown")))
            .expect("alarm button");
        logic.world.input(button, "Press", "", None, None);
        pump(&mut logic, 1.5);
        assert!(scripts(&mut logic).contains(&"c1_mall_crescendo_cooldown".into()));
        assert!(!alarm_playing(&logic), "the alarm stops");
        assert!(!logic.world.ents[button].alive);
        // The glass route: breaking a shop window starts the crescendo, then
        // its follow-up wave 15 s later.
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        pump(&mut logic, 1.);
        scripts(&mut logic);
        // A high-anger (healthy) team gets the stairwell route, which spawns
        // the breakable shop windows.
        logic.world.queue_input("relay_stairwell_close", "Trigger", "", 0., None);
        pump(&mut logic, 0.2);
        let glass = logic.world.find("breakble_glass_minifinale")[0];
        assert!(logic.world.ents[glass].alive, "the windows spawn on the stairwell route");
        // Invulnerable until a survivor walks into the shop entrance trigger,
        // which sets the bullet-or-melee filter on the windows.
        assert!(!logic.world.damage_entity(glass, 10., l4d2r_game_core::entities::DamageKind::Bullet));
        let entrance = logic.world.find("trigger_glass_breakable")[0];
        logic.world.fire_output_pub(entrance, "OnStartTouch");
        pump(&mut logic, 0.1);
        assert!(logic.world.damage_entity(glass, 10., l4d2r_game_core::entities::DamageKind::Bullet));
        pump(&mut logic, 1.);
        assert!(scripts(&mut logic).contains(&"c1_mall_crescendo".into()));
        assert!(alarm_playing(&logic));
        pump(&mut logic, 15.);
        assert!(scripts(&mut logic).contains(&"c1_mall_crescendo_wave".into()));
    }
    #[test]
    fn c1m2_car_alarm_arms_by_anger_chirps_near_and_fires_once_on_a_shot() {
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let pump = |logic: &mut MapLogic, seconds: f32| {
            for _ in 0..(seconds * 60.) as usize {
                logic.world.tick(1. / 60.);
            }
        };
        pump(&mut logic, 1.);
        let car1 = logic.world.find("caralarm_car1-car1_alarm")[0];
        let car2 = logic.world.find("caralarm_car1-car2_alarm")[0];
        let armed = |logic: &MapLogic, id: usize| matches!(logic.world.ents[id].kind, Kind::CarAlarm { armed: true, .. });
        assert!(!armed(&logic, car1) && !armed(&logic, car2), "both cars start disarmed");
        // A full-health team is at the top of the 1..3 anger range: car 1.
        logic.world.survivor_health = Some(400.);
        let mut tries = 0;
        while !armed(&logic, car1) && tries < 20 {
            // Noise (25%) can pick another case; ask again like a new round.
            logic.world.queue_input("director_query", "HowAngry", "", 0., None);
            pump(&mut logic, 0.2);
            tries += 1;
        }
        assert!(armed(&logic, car1), "anger 3 arms car 1");
        collision.sync_entities(&logic);
        let origin = logic.car_alarm_origin(car1);
        // Within car_alarm_chirp_distance: one chirp, then not again for 3 s.
        let near = origin + Vec3::X * 6.;
        logic.car_alarm_proximity(None, &[near]);
        assert!(matches!(logic.world.ents[car1].kind, Kind::CarAlarm { chirp_until, .. } if chirp_until > 0.));
        pump(&mut logic, 1.2);
        assert!(matches!(logic.world.ents[car1].kind, Kind::CarAlarm { chirp_until, triggered: false, .. } if chirp_until == 0.));
        // A shot from 5 m sets it off once: a panic event and a 15 s alarm.
        logic.world.cine.director_commands.clear();
        let eye = origin + Vec3::new(5., 1., 0.);
        let toward = (origin + Vec3::Y * 0.6 - eye).normalize();
        assert!(logic.breakable_hit_distance(car1, eye, toward, 10.).is_some(), "the shot reaches the car body");
        assert!(logic.shoot_car_alarm(eye, toward, 10., &collision));
        assert!(logic.world.cine.director_commands.iter().any(|(c, _)| c == "PanicEvent"));
        assert!(!logic.shoot_car_alarm(eye, toward, 10., &collision), "an alarm goes off once");
        pump(&mut logic, 14.);
        assert!(matches!(logic.world.ents[car1].kind, Kind::CarAlarm { until, .. } if until > 0.));
        pump(&mut logic, 1.5);
        assert!(matches!(logic.world.ents[car1].kind, Kind::CarAlarm { until, .. } if until == 0.), "OnCarAlarmEnd after 15 s");
        // Car 2 stayed disarmed: shooting it does nothing.
        let o2 = logic.car_alarm_origin(car2);
        let eye2 = o2 + Vec3::new(5., 1., 0.);
        assert!(!logic.shoot_car_alarm(eye2, (o2 + Vec3::Y * 0.6 - eye2).normalize(), 10., &collision));
        // The player climbing onto an armed car sets it off; walking past
        // (5 m from the middle, clear of the bumpers) does not.
        logic.world.queue_input("caralarm_car1-car2_alarm", "Enable", "", 0., None);
        pump(&mut logic, 0.1);
        let above = o2 + Vec3::Y * 4.;
        let roof = above.y - logic.breakable_hit_distance(car2, above, Vec3::NEG_Y, 4.).expect("car roof");
        logic.car_alarm_proximity(Some(o2 + Vec3::new(5., 0., 0.)), &[]);
        assert!(matches!(logic.world.ents[car2].kind, Kind::CarAlarm { triggered: false, .. }));
        logic.car_alarm_proximity(Some(Vec3::new(o2.x, roof + 0.02, o2.z)), &[]);
        assert!(matches!(logic.world.ents[car2].kind, Kind::CarAlarm { triggered: true, .. }), "standing on the car");
    }
    #[test]
    fn c1m2_medical_cabinet_opens_on_use_with_its_item_slots() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        if !path.exists() {
            return;
        }
        let mut logic = MapLogic::load(root, &path, true).unwrap();
        let collision = crate::collision::Collision::load(&path).unwrap();
        let id = (0..logic.world.ents.len())
            .find(|&i| logic.world.ents[i].me.classname == "prop_health_cabinet")
            .unwrap();
        let (lo, hi) = logic.bounds(id).expect("cabinet bounds for the use ray");
        let center = (lo + hi) * 0.5;
        // Stand 1 m in front (the cabinet faces Source yaw 270, -y = Bevy +z).
        let eye = center + Vec3::Z * 1.;
        let (opened, position, count, slots) =
            logic.open_cabinet(eye, (center - eye).normalize(), &collision).expect("use opens it");
        assert_eq!(opened, id);
        assert!(position.distance(center) < 1.);
        assert_eq!(count, 2, "HealthCount 2");
        assert_eq!(slots.len(), 4, "item1..item4 attachments");
        for s in &slots {
            assert!(s.translation.distance(center) < 0.6, "slot {:?} inside the cabinet", s.translation);
        }
        assert_eq!(logic.world.cine.anims.get(&id).map(|a| a.0.as_str()), Some("Open"));
        assert!(logic.open_cabinet(eye, (center - eye).normalize(), &collision).is_none(), "opens once");
        // The items sit inside the cabinet's collision model: a plain reach
        // trace stops at the cabinet; ignoring the cabinet reaches them.
        let mut collision = collision;
        collision.enable_l4d2r(root, &logic).unwrap();
        collision.sync_entities(&logic);
        for s in &slots {
            let target = s.translation + Vec3::Y * 0.12;
            let plain = collision.trace(eye, target, Vec3::ZERO).fraction;
            let through = collision.prop_trace(id, eye, target, Vec3::ZERO).fraction;
            println!("slot {:?}: plain {plain:.2}, ignoring the cabinet {through:.2}", s.translation);
            assert!(through >= 0.99, "an item in the open cabinet is reachable");
        }
    }
    #[test]
    #[ignore = "sweeps every retail map"]
    fn every_medical_cabinet_item_is_reachable_once_open() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let Ok(dir) = std::fs::read_dir(root.join("left4dead2/maps")) else { return };
        let mut failures = vec![];
        for path in dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "bsp")) {
            let Ok(mut logic) = MapLogic::load(root, &path, true) else { continue };
            let ids: Vec<usize> = (0..logic.world.ents.len())
                .filter(|&i| logic.world.ents[i].me.classname == "prop_health_cabinet")
                .collect();
            if ids.is_empty() {
                continue;
            }
            let Ok(mut collision) = crate::collision::Collision::load(&path) else { continue };
            collision.enable_l4d2r(root, &logic).unwrap();
            collision.sync_entities(&logic);
            for id in ids {
                let Some((lo, hi)) = logic.bounds(id) else {
                    failures.push(format!("{path:?} #{id}: no bounds"));
                    continue;
                };
                let center = (lo + hi) * 0.5;
                let m = logic.world.world_matrix(id);
                let mut facing = Vec3::from_array(m.x_axis.truncate().to_array()).with_y(0.).normalize_or_zero();
                let room = |d: Vec3| collision.prop_trace(id, center, center + d * 1.5, Vec3::ZERO).fraction;
                if room(-facing) > room(facing) {
                    facing = -facing;
                }
                let eye = center + facing * 0.9;
                let opened = logic.open_cabinet(eye, (center - eye).normalize(), &collision);
                let Some((_, _, count, slots)) = opened else {
                    failures.push(format!("{path:?} #{id}: use from {eye:?} does not open it"));
                    continue;
                };
                for s in slots.iter().take(count.max(slots.len())) {
                    let target = s.translation + Vec3::Y * 0.12;
                    let t = collision.prop_trace(id, eye, target, Vec3::ZERO);
                    let aim = crate::game::pickup_aim_score(eye, (s.translation - eye).normalize(), s.translation);
                    println!("{:?} #{id} slot {:?}: reach {:.2} aim {aim:?} eye_src {:?}", path.file_stem().unwrap(), s.translation, t.fraction, eye / crate::collision::UNIT);
                    if t.fraction < 0.99 || aim.is_none() {
                        failures.push(format!("{path:?} #{id} slot {:?}: reach {:.2}", s.translation, t.fraction));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
    #[test]
    fn restart_brings_back_every_particle_emitter_and_flags_a_rebuild() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() {
            return;
        }
        let mut logic = MapLogic::load(root, &path, false).unwrap();
        // Runtime emitters created during play (here the Smoker cloud).
        let cloud = logic.world.ents.len();
        let mut emitter = logic.world.ents[logic.world.find("__rewrite_gore_blood_impact_infected_01_0").first().copied().unwrap_or(cloud - 1)].clone();
        emitter.me.targetname = "__rewrite_smoker_smokecloud".into();
        logic.world.add_runtime_entity(emitter);
        let particles = |l: &MapLogic| {
            let mut v: Vec<(String, String)> = l
                .world
                .ents
                .iter()
                .filter(|e| matches!(e.kind, l4d2r_game_core::entities::Kind::Particle { .. } | l4d2r_game_core::entities::Kind::Fire { .. }))
                .map(|e| (e.me.targetname.clone(), e.me.kv("effect_name").unwrap_or("").to_string()))
                .collect();
            v.sort();
            v
        };
        let indices = |l: &MapLogic| -> Vec<(usize, String)> {
            l.world
                .ents
                .iter()
                .enumerate()
                .filter(|(_, e)| e.me.targetname.starts_with("__rewrite_"))
                .map(|(i, e)| (i, e.me.targetname.clone()))
                .collect()
        };
        let before = particles(&logic);
        let before_indices = indices(&logic);
        let generation = logic.generation;
        logic.restart();
        assert_ne!(logic.generation, generation, "particle batches must be rebuilt");
        assert_eq!(particles(&logic), before, "every particle emitter returns after a restart");
        // Indices do move (which is why the batches are rebuilt, not kept).
        assert_ne!(indices(&logic), before_indices);
    }
    #[test]
    fn moving_brushes_draw_between_logic_ticks_and_snap_on_teleport() {
        let at = |y: f32| Transform::from_xyz(0., y, 0.);
        let mut pose = MoverPose { previous: at(10.), current: at(10.), time: 0. };
        // A tick moves the car down 0.05: frames in between blend.
        assert_eq!(pose.advance(at(9.95), 1. / 30., 0.).translation.y, 10.);
        assert!((pose.advance(at(9.95), 1. / 30., 0.5).translation.y - 9.975).abs() < 1e-5);
        assert!((pose.advance(at(9.95), 1. / 30., 1.).translation.y - 9.95).abs() < 1e-5);
        // Next tick: the previous pose becomes the last tick's.
        assert!((pose.advance(at(9.9), 2. / 30., 0.5).translation.y - 9.925).abs() < 1e-5);
        // A teleport is drawn at once.
        assert_eq!(pose.advance(at(-50.), 3. / 30., 0.2).translation.y, -50.);
    }
    #[test]
    fn camera_handoff_reaches_player_eye_and_fov_without_endpoint_snap() {
        let from = Transform::from_xyz(2., 3., 4.).with_rotation(Quat::from_rotation_y(2.));
        let to = Transform::from_xyz(8., 1., -2.).with_rotation(Quat::from_rotation_y(-0.5));
        let (start, fov) = camera_handoff(from, 35., to, 110., 0.);
        assert_eq!(start, from);
        assert_eq!(fov, 35.);
        let (end, fov) = camera_handoff(from, 35., to, 110., 1.);
        assert!(end.translation.distance(to.translation) < 1e-6);
        assert!(end.rotation.dot(to.rotation).abs() > 0.99999);
        assert_eq!(fov, 110.);
        let (near_end, _) = camera_handoff(from, 35., to, 110., 0.999);
        assert!(near_end.translation.distance(to.translation) < 0.0001);
    }
    #[test]
    fn hotel_selects_each_survivor_camera_and_retains_it_through_handoff() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let bytes =
            crate::bsp::load_bytes(&root.join("left4dead2/maps/c1m1_hotel.bsp"), root).unwrap();
        let bsp = Arc::new(Bsp::parse(bytes).unwrap());
        for (name, camera) in [
            ("coach", "camera_intro_survivor_01"),
            ("ellis", "camera_intro_survivor_02"),
            ("rochelle", "camera_intro_survivor_03"),
            ("nick", "camera_intro_survivor_04"),
        ] {
            let mut w = EntityWorld::new(bsp.clone(), mapents::collect(&bsp), 7);
            w.cine.selected_survivor = Some(name.into());
            let relay = w.find("relay_intro_start")[0];
            w.input(relay, "Trigger", "", None, None);
            w.flush_io();
            assert_eq!(w.cine.view_control, Some(w.find(camera)[0]), "{name}");
            for _ in 0..930 {
                w.tick(1. / 60.);
            }
            assert!(
                w.cine.intro && w.cine.view_movement.is_some(),
                "{name} time {}",
                w.time
            );
            assert_eq!(w.cine.view_control, Some(w.find(camera)[0]));
            for _ in 0..50 {
                w.tick(1. / 60.);
            }
            assert!(
                !w.cine.intro && w.cine.view_control.is_none() && w.cine.view_movement.is_none()
            );
        }
    }
    #[test]
    fn laboratory_button_routes_panic_to_director_and_releases_spawn_budget() {
        let path = crate::test_lab::path();
        let bsp = Arc::new(Bsp::parse(std::fs::read(path).unwrap()).unwrap());
        let ents = l4d2r_game_core::mapents::collect(&bsp);
        let mut w = EntityWorld::new(bsp, ents, 7);
        let id = w.find("lab_panic_button")[0];
        let mut d = crate::source_director::Director::default();
        d.command("Disable", "");
        w.use_ent(id, None);
        w.flush_io();
        for (input, param) in std::mem::take(&mut w.cine.director_commands) {
            d.command(&input, &param);
        }
        assert!(d.pending > 0);
        // The mob spawns once STAGE_INITIAL_DELAY (1 to 2 s) and the mob
        // timer (2 to 4 s) have passed.
        assert_eq!(d.tick(1. / 30., 100., 0, 0, Vec3::ZERO), 0);
        assert!(d.tick(6., 100., 0, 0, Vec3::ZERO) > 0);
    }
    #[test]
    fn hotel_team_gate_unlocks_elevator_and_reaches_bottom() {
        let root = Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.exists() {
            return;
        }
        let bsp = Arc::new(
            Bsp::parse(
                crate::bsp::load_bytes_for_mode(&path, root, crate::bsp::MapMode::Campaign)
                    .unwrap(),
            )
            .unwrap(),
        );
        let ents = l4d2r_game_core::mapents::collect(&bsp);
        let mut w = EntityWorld::new(bsp, ents, 7);
        w.map_spawn("OnCoop");
        w.flush_io();
        let button = w.find("elevator_button")[0];
        let car = w.find("elevator_1")[0];
        let gate = w
            .ents
            .iter()
            .position(|e| {
                e.me.kv_i32("entireteam") == Some(2)
                    && e.me.ent.0.iter().any(|(k, v)| {
                        k == "OnEntireTeamStartTouch" && v.starts_with("elevator_button")
                    })
            })
            .unwrap();
        let mut contacts = Default::default();
        assert!(matches!(
            w.ents[button].kind,
            Kind::Button { locked: true, .. }
        ));
        team_touch(&mut w, &mut contacts, gate, true);
        w.flush_io();
        assert!(matches!(
            w.ents[button].kind,
            Kind::Button { locked: false, .. }
        ));
        team_touch(&mut w, &mut contacts, gate, false);
        w.flush_io();
        assert!(matches!(
            w.ents[button].kind,
            Kind::Button { locked: true, .. }
        ));
        team_touch(&mut w, &mut contacts, gate, true);
        w.flush_io();
        w.use_ent(button, None);
        for _ in 0..1000 {
            w.tick(1. / 30.);
        }
        match w.ents[car].kind {
            Kind::Elevator { offset, moving, .. } => {
                assert!(!moving);
                assert!((offset.z + 1280.).abs() < 0.01, "{offset:?}");
            }
            _ => panic!("missing car"),
        }
        assert!(w.events.iter().any(|e| matches!(e, EntEvent::Fired { src, output, .. } if *src == car && output == "OnReachedBottom")));
        let exit = w.find("elevator_door_button1")[0];
        assert!(matches!(
            w.ents[exit].kind,
            Kind::Button { locked: false, .. }
        ));
        let shadows = w.find("elevator_1_doors_shadow").to_vec();
        assert!(!shadows.is_empty());
        assert!(
            shadows
                .iter()
                .all(|id| w.ents[*id].me.kv_i32("glowstate") == Some(3))
        );
        let speaker = w.find("target_speaker")[0];
        let ambient = w.find("elevator_movement_sound")[0];
        let actual = sound_position(&w, ambient).unwrap();
        let expected = Vec3::from_array(crate::binary::source_position(
            w.world_matrix(speaker).w_axis.truncate().to_array(),
        ));
        assert!(actual.distance(expected) < 0.001);
        assert!(w.events.iter().any(
            |e| matches!(e,EntEvent::DoorSound{ent,output} if *ent==car && output=="OnMoveStarted")
        ));
        w.use_ent(exit, None);
        w.flush_io();
        assert!(
            shadows
                .iter()
                .all(|id| w.ents[*id].me.kv_i32("glowstate") == Some(0))
        );
    }
    use l4d2r_game_core::{
        entities::{EntEvent, Kind},
        mapents::MapEnt,
    };
    #[test]
    fn native_entity_parser_accepts_last_stand_cr_only_patch() {
        let entries = l4d2_formats::bsp::parse_entities(
            "{\r\"classname\" \"trigger_once\"\r\"OnTrigger\" \"a,Enable,,0,-1\"\r\"OnTrigger\" \"b,Enable,,0,-1\"\r}\r",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].classname(), "trigger_once");
        assert_eq!(
            entries[0]
                .0
                .iter()
                .filter(|(k, _)| k == "OnTrigger")
                .count(),
            2
        );
    }
    fn world(class: &str) -> EntityWorld {
        let mut data = vec![0u8; 1036];
        data[..4].copy_from_slice(b"VBSP");
        data[4..8].copy_from_slice(&21i32.to_le_bytes());
        let bsp = Arc::new(Bsp::parse(data).unwrap());
        let entity = |index, classname: &str, targetname: &str, keys: Vec<(&str, &str)>| {
            let mut values = vec![
                ("classname".into(), classname.into()),
                ("targetname".into(), targetname.into()),
            ];
            values.extend(keys.into_iter().map(|(k, v)| (k.into(), v.into())));
            MapEnt {
                index,
                classname: classname.into(),
                targetname: targetname.into(),
                origin: l4d2r_math::Vec3::ZERO,
                angles: l4d2r_math::Vec3::ZERO,
                mdl: None,
                brush: None,
                ent: l4d2_formats::bsp::Entity(values),
            }
        };
        EntityWorld::new(
            bsp,
            vec![
                entity(
                    0,
                    class,
                    "volume",
                    vec![
                        ("wait", "0.2"),
                        ("OnTrigger", "counter,Add,1,0,-1"),
                        ("OnEndTouchAll", "counter,Add,0,0,-1"),
                    ],
                ),
                entity(
                    1,
                    "math_counter",
                    "counter",
                    vec![
                        ("min", "0"),
                        ("max", "10"),
                        ("OutValue", "copy,SetValue,,0,-1"),
                    ],
                ),
                entity(2, "math_counter", "copy", vec![("min", "0"), ("max", "10")]),
            ],
            1,
        )
    }
    #[test]
    fn decorative_motion_does_not_rebuild_collision_but_parented_solids_do() {
        let mut w = world("prop_physics");
        assert!(crate::collision::collider_move_is_relevant(&w, 0));
        let decorative = 1;
        assert!(!crate::collision::collider_move_is_relevant(&w, decorative));
        w.ents[0].parent = Some(decorative);
        assert!(crate::collision::collider_move_is_relevant(&w, decorative));
        assert!(!crate::collision::collider_move_is_relevant(&w, 10000));
    }
    fn count(w: &EntityWorld) -> f32 {
        if let Kind::Counter { value, .. } = w.ents[1].kind {
            value
        } else {
            panic!("missing counter")
        }
    }
    #[test]
    fn map_hint_inputs_create_and_end_the_native_hint() {
        for class in ["env_hudhint", "env_instructor_hint"] {
            let mut w = world(class);
            w.input(0, "ShowHint", "", None, None);
            w.input(0, "EndHint", "", None, None);
            assert!(matches!(
                w.events[w.events.len() - 2],
                EntEvent::HudHint { ent: 0, show: true }
            ));
            assert!(matches!(
                w.events[w.events.len() - 1],
                EntEvent::HudHint {
                    ent: 0,
                    show: false
                }
            ));
            assert!(w.unhandled.is_empty());
        }
    }
    #[test]
    fn particle_start_stop_and_actor_contacts_are_distinct() {
        let mut w = world("info_particle_system");
        w.tick(0.25);
        w.input(0, "Start", "", None, None);
        assert!(
            matches!(w.ents[0].kind, Kind::Particle { active: true, started } if started == 0.25)
        );
        w.input(0, "Stop", "", None, None);
        assert!(matches!(
            w.ents[0].kind,
            Kind::Particle { active: false, .. }
        ));
        let mut w = world("trigger_multiple");
        w.touch_actor(0, 1, None);
        w.touch_actor(0, 2, None);
        w.untouch_actor(0, 1, None);
        w.tick(0.3);
        w.touch_actor(0, 2, None);
        w.tick(0.01);
        assert_eq!(count(&w), 2.);
        w.untouch_actor(0, 2, None);
        w.tick(0.3);
        w.touch_actor(0, 3, None);
        w.tick(0.01);
        assert_eq!(count(&w), 3.);
    }
    #[test]
    fn damaged_door_keeps_door_behavior_until_one_break_event() {
        use l4d2r_game_core::entities::{DamageKind, Durability};
        let mut w = door_world("0", "0", false);
        w.durability.insert(
            0,
            Durability {
                health: 1000.,
                maximum: 1000.,
                bullets: 1.,
                club: 1.25,
                explosive: 1.5,
                fire: 1.,
                sound: "Breakable.Crate".into(),
            },
        );
        w.damage_entity(0, 400., DamageKind::Club);
        assert!(matches!(w.ents[0].kind, Kind::Door(_)));
        assert!(w.ents[0].alive);
        assert_eq!(w.durability[&0].health, 500.);
        w.damage_entity(0, 500., DamageKind::Bullet);
        w.damage_entity(0, 500., DamageKind::Bullet);
        assert!(!w.ents[0].alive);
        assert_eq!(
            w.events
                .iter()
                .filter(|e| matches!(e, EntEvent::Broken { ent: 0 }))
                .count(),
            1
        );
    }
    #[test]
    fn ambient_volume_does_not_become_breakable_health() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let mut w = world("ambient_generic");
        w.ents[0].me.ent.0.push(("health".into(), "10".into()));
        assert!(crate::source_breakables::configure(&mut assets, &mut w).is_empty());
        assert!(matches!(w.ents[0].kind, Kind::Ambient { .. }));
        w.input(0, "PlaySound", "", None, None);
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e, EntEvent::Sound { ent: 0, play: true }))
        );
    }
    fn door_world(spawn: &str, direction: &str, negative_y: bool) -> EntityWorld {
        let w = world("prop_door_rotating");
        let mut me = w.ents[0].me.clone();
        me.ent.0.extend([
            ("spawnpos".into(), spawn.into()),
            ("opendir".into(), direction.into()),
        ]);
        let mut w = EntityWorld::new(w.bsp.clone(), vec![me], 1);
        let (lo, hi) = if negative_y {
            (
                l4d2r_math::Vec3::new(-4., -55., 0.),
                l4d2r_math::Vec3::new(4., 0., 112.),
            )
        } else {
            (
                l4d2r_math::Vec3::new(-4., 0., 0.),
                l4d2r_math::Vec3::new(4., 55., 112.),
            )
        };
        configure_doors(&mut w, &HashMap::from([(0, (lo, hi))]));
        w
    }
    #[test]
    fn native_door_start_positions_and_hinge_sides() {
        for (left, expected) in [(false, -90.), (true, 90.)] {
            for (spawn, sign, fraction) in [("0", 1., 0.), ("1", 1., 1.), ("2", -1., 1.)] {
                let w = door_world(spawn, "0", left);
                let Kind::Door(d) = &w.ents[0].kind else {
                    panic!()
                };
                assert_eq!(d.dist, expected * sign);
                assert_eq!(d.frac, fraction);
                let hinge = w.door_matrix(0).transform_point3(l4d2r_math::Vec3::ZERO);
                assert_eq!(hinge, l4d2r_math::Vec3::ZERO);
            }
        }
    }
    #[test]
    fn door_sound_events_do_not_require_map_output_connections() {
        let mut w = door_world("0", "0", false);
        w.events.clear();
        w.ents[0].conns.clear();
        w.input(0, "Open", "", None, None);
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e, EntEvent::DoorSound {ent:0,output} if output=="OnOpen"))
        );
        for _ in 0..180 {
            w.tick(1. / 60.);
        }
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e, EntEvent::DoorSound {ent:0,output} if output=="OnFullyOpen"))
        );
        w.input(0, "Close", "", None, None);
        for _ in 0..180 {
            w.tick(1. / 60.);
        }
        assert!(
            w.events.iter().any(
                |e| matches!(e, EntEvent::DoorSound {ent:0,output} if output=="OnFullyClosed")
            )
        );
        let mut w = world("info_director");
        w.input(0, "BeginScript", "director_quiet", None, None);
        w.input(0, "ForcePanicEvent", "", None, None);
        assert_eq!(
            w.cine.director_commands,
            vec![
                ("beginscript".into(), "director_quiet".into()),
                ("forcepanicevent".into(), "".into())
            ]
        );
    }
    #[test]
    fn tilted_prop_door_changes_yaw_without_tipping_its_hinge_axis() {
        let mut w = door_world("1", "0", false);
        w.ents[0].me.angles = l4d2r_math::Vec3::new(20., 35., 10.);
        let expected = l4d2r_game_core::mapents::entity_matrix(
            l4d2r_math::Vec3::ZERO,
            l4d2r_math::Vec3::new(20., -55., 10.),
        );
        let actual = w.door_matrix(0);
        for (a, b) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!((a - b).abs() < 0.0001);
        }
    }
    #[test]
    fn native_door_opens_away_and_negative_swing_moves_at_normal_speed() {
        for (side, direction, expected) in [
            (-10., "0", -90.),
            (10., "0", 90.),
            (10., "1", -90.),
            (-10., "2", 90.),
        ] {
            let mut w = door_world("0", direction, false);
            w.door_open_away_from(0, Some(l4d2r_math::Vec3::new(side, 0., 50.)));
            w.use_ent(0, None);
            w.tick(1. / 60.);
            let Kind::Door(d) = &w.ents[0].kind else {
                panic!()
            };
            assert_eq!(d.dist, expected);
            assert!(
                d.frac > 0. && d.frac < 0.1,
                "door must animate rather than jump open"
            );
            let sign = d.dist;
            w.door_open_away_from(0, Some(l4d2r_math::Vec3::new(-side, 0., 50.)));
            let Kind::Door(d) = &w.ents[0].kind else {
                panic!()
            };
            assert_eq!(
                d.dist, sign,
                "an already moving door keeps its hinge trajectory"
            );
            for _ in 0..90 {
                w.tick(1. / 60.);
            }
            w.input(0, "Close", "", None, None);
            for _ in 0..90 {
                w.tick(1. / 60.);
            }
            let Kind::Door(d) = &w.ents[0].kind else {
                panic!()
            };
            assert_eq!(d.frac, 0.);
            assert_eq!(d.state, l4d2r_game_core::entities::DoorState::Closed);
        }
    }
    #[test]
    fn trigger_multiple_enters_refires_and_exits() {
        let mut w = world("trigger_multiple");
        w.touch(0, None);
        w.tick(0.01);
        assert_eq!(count(&w), 1.);
        w.touch(0, None);
        w.tick(0.01);
        assert_eq!(count(&w), 1.);
        w.tick(0.2);
        w.touch(0, None);
        w.tick(0.01);
        assert_eq!(count(&w), 2.);
        w.untouch(0, None);
        assert!(
            w.events
                .iter()
                .any(|e| matches!(e,EntEvent::Fired{output,..} if output=="OnEndTouchAll"))
        );
    }
    #[test]
    fn counter_outputs_carry_the_updated_value_through_map_io() {
        let mut w = world("trigger_multiple");
        w.input(1, "SetValue", "7", None, None);
        w.tick(0.01);
        if let Kind::Counter { value, .. } = w.ents[2].kind {
            assert_eq!(value, 7.);
        } else {
            panic!("No receiving counter");
        }
    }
    #[test]
    fn trigger_once_does_not_refire() {
        let mut w = world("trigger_once");
        w.touch(0, None);
        w.tick(0.01);
        w.untouch(0, None);
        w.touch(0, None);
        w.tick(1.);
        assert_eq!(count(&w), 1.);
        assert!(!w.ents[0].alive);
    }
    #[test]
    fn disabled_trigger_does_not_activate() {
        let mut w = world("trigger_multiple");
        w.input(0, "Disable", "", None, None);
        w.touch(0, None);
        w.tick(0.1);
        assert_eq!(count(&w), 0.);
        w.input(0, "Enable", "", None, None);
        w.touch(0, None);
        w.tick(0.1);
        assert_eq!(count(&w), 1.);
    }
    #[test]
    fn look_trigger_requires_gaze_instead_of_contact_alone() {
        let mut w = world("trigger_look");
        w.touch(0, None);
        w.tick(1.);
        assert_eq!(count(&w), 0.);
        w.fire_output_pub(0, "OnTrigger");
        w.tick(0.01);
        assert_eq!(count(&w), 1.);
    }
}

#[cfg(test)]
mod infected_obstacle_tests {
    #[test]
    fn infected_break_lab_glass_and_clear_its_collider() {
        use bevy::prelude::*;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = crate::test_lab::path();
        let mut logic = super::MapLogic::load(root, &path, false).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let id = logic.world.find("lab_glass")[0];
        let feet = Vec3::from_array(crate::binary::source_position([2900., 1175., 4.]));
        assert!(logic.pound_obstacle(feet, Vec3::NEG_Z, &collision));
        assert!(!logic.world.ents[id].alive);
        let eye = feet + Vec3::Y;
        assert!(collision.trace(eye, eye + Vec3::NEG_Z, Vec3::ZERO).fraction >= 1.);
    }
}
