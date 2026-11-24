//! Swept axis-aligned survivor hulls against native solid BSP brushes.
use crate::binary::*;
use bevy::prelude::*;
use std::io;
pub const UNIT: f32 = 0.0254;
pub const RADIUS: f32 = 16. * UNIT;
pub const HEIGHT: f32 = 71. * UNIT;
pub const DUCK_HEIGHT: f32 = 55. * UNIT;
pub const EYE: f32 = 62. * UNIT;
#[derive(Clone)]
struct Plane {
    normal: Vec3,
    distance: f32,
}
#[derive(Clone)]
struct Brush {
    planes: Vec<Plane>,
}
#[derive(Resource, Default, Clone)]
pub struct Collision {
    brushes: Vec<Brush>,
    terrain: Vec<[Vec3; 3]>,
    native: Option<std::sync::Arc<std::sync::Mutex<l4d2r_game_core::trace::Tracer>>>,
    movement_team: u8,
    clip_filters: std::sync::Arc<std::sync::Mutex<[std::collections::HashSet<usize>; 5]>>,
    checkpoints: std::sync::Arc<Vec<l4d2_formats::nav::NavArea>>,
    contact_surfaces: std::sync::Arc<Vec<crate::physics_data::Surface>>,
}
#[derive(Debug, Clone, Copy)]
pub struct Trace {
    pub fraction: f32,
    pub normal: Vec3,
    pub start_solid: bool,
    pub plane_distance: Option<f32>,
}
#[derive(Clone, Copy)]
pub(crate) struct ContactPatch(l4d2r_game_core::trace::ContactPatch);
impl ContactPatch {
    pub fn trace(&self, start: Vec3, end: Vec3) -> Option<Trace> {
        let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
        let t = self.0.trace_from(source(start), source(end))?;
        Some(Trace {
            fraction: t.fraction,
            normal: Vec3::from_array(source_normal(t.normal.to_array())),
            start_solid: t.start_solid,
            plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
        })
    }
}
pub fn collider_move_is_relevant(
    world: &l4d2r_game_core::entities::EntityWorld,
    moved: usize,
) -> bool {
    use l4d2r_game_core::entities::Kind;
    world.ents.iter().enumerate().any(|(id, e)| {
        let collider = if e.me.brush.is_some() {
            matches!(
                e.kind,
                Kind::Brush { .. } | Kind::Door(_) | Kind::Breakable { .. } | Kind::Elevator { .. }
            ) || matches!(
                e.me.classname.as_str(),
                "func_wall" | "func_physbox" | "func_movelinear" | "func_tracktrain"
            )
        } else {
            e.me.classname.starts_with("prop_physics") || matches!(e.kind, Kind::Door(_))
        };
        if !collider {
            return false;
        }
        let mut at = Some(id);
        for _ in 0..64 {
            let Some(id) = at else {
                return false;
            };
            if id == moved {
                return true;
            }
            at = world.ents.get(id).and_then(|e| e.parent);
        }
        false
    })
}
impl Collision {
    /// A private trace-filter view for loose bodies; never mutate the shared
    /// actor filters or let a fragment collide with its own registered hull.
    pub(crate) fn without_prop_owners(&self, owners: &std::collections::HashSet<usize>) -> Self {
        let mut view = self.for_actor(false, false);
        let mut filters = self.clip_filters.lock().unwrap().clone();
        filters[4].extend(owners.iter().copied());
        view.clip_filters = std::sync::Arc::new(std::sync::Mutex::new(filters));
        view
    }
    /// A cheap view of the same native tracer, with actor-specific clip rules.
    pub(crate) fn for_actor(&self, infected: bool, ghost: bool) -> Self {
        let mut view = if self.native.is_some() {
            Self {
                native: self.native.clone(),
                clip_filters: self.clip_filters.clone(),
                contact_surfaces: self.contact_surfaces.clone(),
                checkpoints: self.checkpoints.clone(),
                brushes: Vec::new(),
                terrain: Vec::new(),
                movement_team: 0,
            }
        } else {
            self.clone()
        };
        view.movement_team = if !infected {
            0
        } else if ghost {
            2
        } else {
            1
        };
        view
    }
    pub(crate) fn infected_spawn_allowed(&self, position: Vec3) -> bool {
        let p = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / UNIT;
        !self
            .checkpoints
            .iter()
            .any(|area| area.contains_xy(p.x, p.y) && (p.z - area.z_at(p.x, p.y)).abs() <= 72.)
    }
    pub(crate) fn for_common(&self) -> Self {
        let mut view = self.for_actor(true, false);
        view.movement_team = 3;
        view
    }
    #[cfg(test)]
    pub(crate) fn test_native(
        tracer: std::sync::Arc<std::sync::Mutex<l4d2r_game_core::trace::Tracer>>,
    ) -> Self {
        Self {
            native: Some(tracer),
            ..default()
        }
    }
    pub(crate) fn trace_epsilon(&self) -> f32 {
        if self.native.is_some() {
            l4d2r_game_core::trace::DIST_EPSILON * UNIT
        } else {
            0.001
        }
    }
    pub(crate) fn supports_clear_certificates(&self) -> bool {
        self.native.is_some() || self.terrain.is_empty()
    }
    pub(crate) fn certify_contact_patch(
        &self,
        start: Vec3,
        end: Vec3,
        half: Vec3,
        radius: f32,
    ) -> Option<ContactPatch> {
        let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
        let extent = l4d2r_math::Vec3::new(half.x, half.z, half.y) / UNIT;
        let native = self.native.as_ref()?.lock().unwrap();
        native
            .certify_contact_patch(
                source(start),
                source(end),
                extent,
                radius / UNIT,
                l4d2r_game_core::trace::MASK_SOLID | l4d2r_game_core::trace::CONTENTS_DEBRIS,
            )
            .map(ContactPatch)
    }
    pub(crate) fn snapshot_tag(&self) -> Option<(usize, u64)> {
        self.native.as_ref().map(|n| {
            (
                std::sync::Arc::as_ptr(n) as usize,
                n.lock().unwrap().snapshot_epoch(),
            )
        })
    }
    pub(crate) fn environment_tag(&self) -> Option<(usize, u64)> {
        self.native.as_ref().map(|n| {
            (
                std::sync::Arc::as_ptr(n) as usize,
                n.lock().unwrap().environment_epoch(),
            )
        })
    }
    pub(crate) fn worker_snapshot(&self) -> Self {
        if let Some(native) = &self.native {
            Self {
                brushes: Vec::new(),
                terrain: Vec::new(),
                native: Some(std::sync::Arc::new(std::sync::Mutex::new(
                    native.lock().unwrap().clone(),
                ))),
                clip_filters: self.clip_filters.clone(),
                contact_surfaces: self.contact_surfaces.clone(),
                ..default()
            }
        } else {
            self.clone()
        }
    }

    pub fn recover_physics_position(&self, center: Vec3, half: Vec3, maximum: f32) -> Option<Vec3> {
        self.recover_position(center, half, maximum, false)
    }
    pub fn recover_prop_position(&self, owners: &std::collections::HashSet<usize>,
        center: Vec3, half: Vec3, maximum: f32) -> Option<Vec3> {
        if !self.prop_environment_trace(owners, center, center, half).start_solid { return Some(center); }
        for step in 1..=(maximum.clamp(0., 1.) / 0.02).ceil() as usize {
            for axis in [Vec3::Y, Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z, Vec3::NEG_Y] {
                let point = center + axis * (step as f32 * 0.02);
                if !self.prop_environment_trace(owners, point, point, half).start_solid { return Some(point); }
            }
        }
        None
    }
    pub fn recover_corpse_position(&self, center: Vec3, half: Vec3, maximum: f32) -> Option<Vec3> {
        self.recover_position(center, half, maximum, true)
    }
    fn recover_position(
        &self,
        center: Vec3,
        half: Vec3,
        maximum: f32,
        entities: bool,
    ) -> Option<Vec3> {
        if !self
            .trace_physics(center, center, half, entities)
            .start_solid
        {
            return Some(center);
        }
        for step in 1..=(maximum.clamp(0., 1.) / 0.02).ceil() as usize {
            for axis in [
                Vec3::Y,
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Z,
                Vec3::NEG_Z,
                Vec3::NEG_Y,
            ] {
                let point = center + axis * (step as f32 * 0.02);
                if !self.trace_physics(point, point, half, entities).start_solid {
                    return Some(point);
                }
            }
        }
        None
    }
    pub fn physics_trace(&self, start: Vec3, end: Vec3, half: Vec3) -> Trace {
        self.trace_physics(start, end, half, false)
    }
    pub fn prop_trace(&self, owner: usize, start: Vec3, end: Vec3, half: Vec3) -> Trace {
        if let Some(native) = &self.native {
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let size = l4d2r_math::Vec3::new(half.x, half.z, half.y) / UNIT;
            let t = native.lock().unwrap().box_trace_without_owner(
                source(start),
                source(end),
                -size,
                size,
                l4d2r_game_core::trace::MASK_SOLID | l4d2r_game_core::trace::CONTENTS_DEBRIS,
                owner,
            );
            return Trace {
                fraction: t.fraction,
                normal: Vec3::from_array(source_normal(t.normal.to_array())),
                start_solid: t.start_solid,
                plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                    .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
            };
        }
        self.physics_trace(start, end, half)
    }
    pub fn prop_environment_trace(
        &self,
        owners: &std::collections::HashSet<usize>,
        start: Vec3,
        end: Vec3,
        half: Vec3,
    ) -> Trace {
        self.prop_environment_contact(owners, start, end, half).0
    }
    pub fn prop_environment_contact(
        &self,
        owners: &std::collections::HashSet<usize>,
        start: Vec3,
        end: Vec3,
        half: Vec3,
    ) -> (Trace, Option<usize>) {
        if let Some(native) = &self.native {
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let size = l4d2r_math::Vec3::new(half.x, half.z, half.y) / UNIT;
            let t = native.lock().unwrap().box_trace_without_owners(
                source(start),
                source(end),
                -size,
                size,
                l4d2r_game_core::trace::MASK_SOLID | l4d2r_game_core::trace::CONTENTS_DEBRIS,
                owners,
            );
            return (
                Trace {
                    fraction: t.fraction,
                    normal: Vec3::from_array(source_normal(t.normal.to_array())),
                    start_solid: t.start_solid,
                    plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                        .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
                },
                t.texinfo,
            );
        }
        (self.physics_trace(start, end, half), None)
    }
    /// Corpses must collide with current doors, elevators and movable props.
    /// Prop integration uses the environment-only query to avoid hitting itself.
    pub fn corpse_trace(&self, start: Vec3, end: Vec3, half: Vec3) -> Trace {
        self.trace_physics(start, end, half, true)
    }
    pub(crate) fn corpse_contact(&self, start: Vec3, end: Vec3, half: Vec3)
        -> (Trace, Option<crate::physics_data::Surface>) {
        let (hit, texinfo) = self.trace_physics_contact(start, end, half, true);
        (hit, texinfo.and_then(|id| self.contact_surfaces.get(id)).copied())
    }
    fn trace_physics(&self, start: Vec3, end: Vec3, half: Vec3, entities: bool) -> Trace {
        self.trace_physics_contact(start, end, half, entities).0
    }
    fn trace_physics_contact(&self, start: Vec3, end: Vec3, half: Vec3, entities: bool)
        -> (Trace, Option<usize>) {
        if let Some(native) = &self.native {
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let extent = l4d2r_math::Vec3::new(half.x, half.z, half.y) / UNIT;
            let filters = self.clip_filters.lock().unwrap();
            let tracer = native.lock().unwrap();
            let t = if entities {
                tracer.box_trace_without_owners(
                    source(start),
                    source(end),
                    -extent,
                    extent,
                    l4d2r_game_core::trace::MASK_SOLID | l4d2r_game_core::trace::CONTENTS_DEBRIS,
                    &filters[4],
                )
            } else {
                tracer.box_trace_environment(
                    source(start),
                    source(end),
                    -extent,
                    extent,
                    l4d2r_game_core::trace::MASK_SOLID,
                )
            };
            (Trace {
                fraction: t.fraction,
                normal: Vec3::from_array(source_normal(t.normal.to_array())),
                start_solid: t.start_solid,
                plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                    .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
            }, t.texinfo)
        } else {
            (self.trace(start, end, half), None)
        }
    }
    pub fn for_trigger_audit(logic: &crate::l4d2r_cinematic::MapLogic) -> Self {
        Self {
            native: Some(std::sync::Arc::new(std::sync::Mutex::new(
                l4d2r_game_core::trace::Tracer::new(logic.world.bsp.clone()),
            ))),
            ..Default::default()
        }
    }
    /// Exact BSP-brush narrow phase for a stationary survivor hull in a trigger.
    pub fn overlaps_model(
        &self,
        model: usize,
        matrix: l4d2r_math::Mat4,
        feet: Vec3,
        height: f32,
    ) -> bool {
        let Some(native) = &self.native else {
            return false;
        };
        let center = l4d2r_math::Vec3::new(feet.x, -feet.z, feet.y + height * 0.5) / UNIT;
        let inverse = matrix.inverse();
        let local = inverse.transform_point3(center);
        let extent = l4d2r_math::Vec3::new(RADIUS, RADIUS, height * 0.5) / UNIT;
        let rotation = l4d2r_math::Mat3::from_mat4(inverse);
        let extent = rotation.x_axis.abs() * extent.x
            + rotation.y_axis.abs() * extent.y
            + rotation.z_axis.abs() * extent.z;
        native
            .lock()
            .unwrap()
            .trigger_overlap(local, -extent, extent, model)
    }
    pub(crate) fn ladder_trace(&self, model: usize, matrix: l4d2r_math::Mat4,
        feet: Vec3, end: Vec3, height: f32) -> Option<Trace> {
        let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y + height * 0.5) / UNIT;
        let inverse = matrix.inverse();
        let half = l4d2r_math::Vec3::new(RADIUS, RADIUS, height * 0.5) / UNIT;
        let rotation = l4d2r_math::Mat3::from_mat4(inverse);
        let extent = rotation.x_axis.abs() * half.x + rotation.y_axis.abs() * half.y
            + rotation.z_axis.abs() * half.z;
        let hit = self.native.as_ref()?.lock().unwrap().trigger_sweep(
            inverse.transform_point3(source(feet)), inverse.transform_point3(source(end)),
            -extent, extent, model);
        let normal = matrix.transform_vector3(hit.normal).normalize_or_zero();
        Some(Trace {fraction: hit.fraction, start_solid: hit.start_solid,
            normal: Vec3::new(normal.x, normal.z, -normal.y), plane_distance: None})
    }
    #[cfg(test)]
    pub(crate) fn test_add_box(&mut self, lo: Vec3, hi: Vec3) {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut normal = Vec3::ZERO;
            normal[axis] = 1.;
            planes.push(Plane {
                normal,
                distance: hi[axis],
            });
            planes.push(Plane {
                normal: -normal,
                distance: -lo[axis],
            });
        }
        self.brushes.push(Brush { planes });
    }
    #[cfg(test)]
    pub(crate) fn test_solid_floor() -> Self {
        let lo = Vec3::new(-20., -4., -20.);
        let hi = Vec3::new(20., 0., 20.);
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut n = Vec3::ZERO;
            n[axis] = 1.;
            planes.push(Plane {
                normal: n,
                distance: hi[axis],
            });
            n[axis] = -1.;
            planes.push(Plane {
                normal: n,
                distance: -lo[axis],
            });
        }
        Self {
            brushes: vec![Brush { planes }],
            ..Default::default()
        }
    }
    #[cfg(test)]
    pub(crate) fn test_floor() -> Self {
        Self {
            brushes: vec![],
            native: None,
            terrain: vec![
                [
                    Vec3::new(-20., 0., -20.),
                    Vec3::new(20., 0., -20.),
                    Vec3::new(20., 0., 20.),
                ],
                [
                    Vec3::new(-20., 0., -20.),
                    Vec3::new(20., 0., 20.),
                    Vec3::new(-20., 0., 20.),
                ],
            ],
            ..default()
        }
    }
    pub fn enable_l4d2r(
        &mut self,
        root: &std::path::Path,
        logic: &crate::l4d2r_cinematic::MapLogic,
    ) -> io::Result<()> {
        use l4d2r_game_core::{mapents, propcol, trace::Tracer};
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![])?);
        let mut bounds = |p: &str| {
            let b = assets.borrow_mut().read(p).ok()?;
            let (lo, hi, vlo, vhi) = l4d2_formats::mdl::read_header_bounds(&b)?;
            let (lo, hi) = if lo == hi { (vlo, vhi) } else { (lo, hi) };
            Some((
                l4d2r_math::Vec3::from_array(lo),
                l4d2r_math::Vec3::from_array(hi),
            ))
        };
        let mut physics = |p: &str| {
            let b = assets.borrow_mut().read(&p.replace(".mdl", ".phy")).ok()?;
            let pieces: Vec<_> = l4d2_formats::phy::parse_phy(&b)
                .ok()?
                .into_iter()
                .flat_map(|s| s.convexes)
                .collect();
            if pieces.is_empty() {
                None
            } else {
                Some(std::sync::Arc::new(pieces))
            }
        };
        let mapents = mapents::collect(&logic.world.bsp);
        let obbs = propcol::build_obbs(&logic.world.bsp, &mapents, &mut bounds, &mut physics);
        println!(
            "l4d2r physics: {} prop collision instances (native PHY where available)",
            obbs.len()
        );
        let mut tracer = Tracer::new(logic.world.bsp.clone());
        tracer.set_obbs(obbs);
        self.native = Some(std::sync::Arc::new(std::sync::Mutex::new(tracer)));
        self.contact_surfaces = std::sync::Arc::new(logic.physics.environment_surfaces().to_vec());
        self.sync_entities(logic);
        Ok(())
    }
    pub fn sync_entities(&self, logic: &crate::l4d2r_cinematic::MapLogic) {
        let _timing = crate::performance::scope("collision::sync_entities");
        use l4d2r_game_core::{
            entities::Kind,
            trace::{CONTENTS_SOLID, Collider, ColliderShape},
        };
        let Some(native) = &self.native else { return };
        let mut filters: [std::collections::HashSet<usize>; 5] = Default::default();
        for (id, e) in logic.world.ents.iter().enumerate() {
            let blocked = match e.me.classname.as_str() {
                "func_playerinfected_clip" => Some([false, true, true, false]),
                "func_playerghostinfected_clip" => Some([false, false, true, false]),
                _ => None,
            };
            if let Some(blocked) = blocked {
                for (i, blocks) in blocked.into_iter().enumerate() {
                    if !blocks {
                        filters[i].insert(id);
                    }
                }
                filters[4].insert(id); // Invisible clips do not stop weapon/LOS rays.
            }
        }
        *self.clip_filters.lock().unwrap() = filters;
        let tracer = native.lock().unwrap();
        let prop_shape = |id, mins, maxs, contents| {
            let matrix = logic.world.world_matrix(id) * logic.prop_collision_pose.get(&id).copied().unwrap_or(l4d2r_math::Mat4::IDENTITY);
            if let Some(shape) = logic.prop_shapes.get(&id) {
                ColliderShape::Convexes {
                    shape: shape.clone(),
                    matrix,
                    contents,
                }
            } else {
                ColliderShape::OrientedBox {
                    mins,
                    maxs,
                    matrix,
                    contents,
                }
            }
        };
        let colliders = logic
            .world
            .ents
            .iter()
            .enumerate()
            .filter_map(|(id, e)| {
                if let Some(surface)=logic.break_surfaces.get(&id).filter(|s|s.initialized) {
                    return Some((id,Collider {shape:ColliderShape::Convexes {shape:surface.shapes.clone(),
                        matrix:logic.world.world_matrix(id),contents:l4d2r_game_core::trace::CONTENTS_WINDOW},active:e.alive && !surface.shapes.is_empty()}));
                }
                if e.me
                    .mdl
                    .as_deref()
                    .is_some_and(l4d2r_game_core::propcol::is_baked_corpse_model)
                {
                    return None;
                }
                let model = if let Some(model) = e.me.brush {
                    model
                } else {
                    if e.me.classname.starts_with("prop_physics")
                        || logic.prop_collision_pose.contains_key(&id)
                        || (e.parent.is_some() && e.me.classname.starts_with("prop_dynamic")) {
                        let (mins, maxs) = *logic.prop_bounds.get(&id)?;
                        let debris = e.me.classname.starts_with("prop_physics") && (e.me.spawnflags() & 4 != 0
                            || matches!(e.me.kv_i32("physicsmode"), Some(2 | 3)));
                        return Some((
                            id,
                            Collider {
                                shape: prop_shape(
                                    id,
                                    mins,
                                    maxs,
                                    if debris {
                                        l4d2r_game_core::trace::CONTENTS_DEBRIS
                                    } else {
                                        CONTENTS_SOLID
                                    },
                                ),
                                active: e.alive && e.me.kv_i32("solid") != Some(0)
                                    && !matches!(e.kind, Kind::Brush { enabled: false, .. } | Kind::Brush { solid: false, .. }),
                            },
                        ));
                    }
                    if let l4d2r_game_core::entities::Kind::Door(d) = &e.kind {
                        let (mins, maxs) = *logic.prop_bounds.get(&id)?;
                        return Some((
                            id,
                            Collider {
                                shape: prop_shape(id, mins, maxs, CONTENTS_SOLID),
                                active: e.alive && !d.passable,
                            },
                        ));
                    }
                    return None;
                };
                if !matches!(
                    e.kind,
                    Kind::Brush { .. }
                        | Kind::Door(_)
                        | Kind::Breakable { .. }
                        | Kind::Elevator { .. }
                ) && !matches!(
                    e.me.classname.as_str(),
                    "func_wall" | "func_physbox" | "func_movelinear" | "func_tracktrain"
                ) {
                    return None;
                }
                let active = e.alive
                    && match &e.kind {
                        Kind::Brush { enabled, solid } => {
                            *enabled && *solid && e.me.kv_i32("solidity") != Some(1)
                        }
                        Kind::Door(d) => !d.passable,
                        _ => e.me.kv_i32("solid") != Some(0),
                    };
                let matrix = logic.world.world_matrix(id);
                let angles = e.me.angles;
                let rotating = matches!(&e.kind,Kind::Door(d) if d.rotating)
                    || angles.length_squared() > 0.001;
                // Rotated brush models keep their real brushes: an oriented
                // box around them turned the mall's sloped escalator ramps
                // (escalator_*-brush_lift_collision, yawed 45-180 degrees)
                // into walls.
                let shape = if rotating {
                    ColliderShape::RotatedModel { model, matrix }
                } else {
                    ColliderShape::Model {
                        model,
                        origin: matrix.w_axis.truncate(),
                    }
                };
                Some((id, Collider { shape, active }))
            })
            .collect();
        tracer.set_owned_colliders(colliders);
        // MapEntity state is authoritative even before the first event/tick.
        // Template props otherwise leave invisible collision in campaign maps.
        tracer.sync_obb_owner_activity(|id| {
            logic.world.ents.get(id).is_some_and(|e| {
                e.alive
                    && e.me.kv_i32("solid") != Some(0)
                    && !matches!(
                        e.kind,
                        Kind::Brush { enabled: false, .. } | Kind::Brush { solid: false, .. }
                    )
            })
        });
    }
    pub fn load(path: &std::path::Path) -> io::Result<Self> {
        let mut collision = Self::parse(&std::fs::read(path)?)?;
        if let Some(nav) = std::fs::read(path.with_extension("nav"))
            .ok()
            .and_then(|b| l4d2_formats::nav::parse_nav(&b).ok())
        {
            collision.checkpoints = std::sync::Arc::new(
                nav.areas
                    .into_iter()
                    .filter(|a| a.spawn_attributes & 0x800 != 0)
                    .collect(),
            );
        }
        Ok(collision)
    }
    fn parse(bytes: &[u8]) -> io::Result<Self> {
        range(bytes, 0, 1036)?;
        if &bytes[..4] != b"VBSP" || u32_at(bytes, 4)? != 21 {
            return Err(invalid("Expected L4D2 BSP v21"));
        }
        let lump = |id: usize| -> io::Result<&[u8]> {
            let base = 8 + id * 16;
            range(
                bytes,
                u32_at(bytes, base + 4)? as usize,
                u32_at(bytes, base + 8)? as usize,
            )
        };
        let planes = lump(1)?;
        let sides = lump(19)?;
        // Brush submodels have their own node trees and transforms. Only the
        // world model's reachable leaf brushes belong in this static collider.
        let nodes = lump(5)?;
        let leaves = lump(10)?;
        let leaf_brushes = lump(17)?;
        let models = lump(14)?;
        let root = u32_at(range(models, 0, 48)?, 36)? as i32;
        let mut stack = vec![root];
        let mut visited = std::collections::HashSet::new();
        let mut world_brushes = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            if visited.len() > 1_000_000 {
                return Err(invalid("Collision tree too large"));
            }
            if node >= 0 {
                let data = range(nodes, node as usize * 32, 32)?;
                stack.push(u32_at(data, 4)? as i32);
                stack.push(u32_at(data, 8)? as i32);
            } else {
                let index = (-1i64 - node as i64) as usize;
                let leaf = range(leaves, index * 32, 32)?;
                let first = u16_at(leaf, 24)? as usize;
                let count = u16_at(leaf, 26)? as usize;
                for i in first..first + count {
                    world_brushes.insert(u16_at(leaf_brushes, i * 2)? as usize);
                }
            }
        }
        let mut brushes = vec![];
        for (index, brush) in lump(18)?.chunks_exact(12).enumerate() {
            if !world_brushes.contains(&index) {
                continue;
            }
            // Solid, windows and player clip. Water/trigger volumes do not block movement.
            if u32_at(brush, 8)? & (1 | 2 | 0x10000) == 0 {
                continue;
            }
            let first = u32_at(brush, 0)? as usize;
            let count = u32_at(brush, 4)? as usize;
            if count > 256 {
                return Err(invalid("Oversized collision brush"));
            }
            let mut out = vec![];
            for side in range(
                sides,
                first
                    .checked_mul(8)
                    .ok_or_else(|| invalid("Brush overflow"))?,
                count * 8,
            )?
            .chunks_exact(8)
            {
                let p = range(planes, u16_at(side, 0)? as usize * 20, 20)?;
                let n = crate::binary::vec3(p, 0)?;
                out.push(Plane {
                    normal: Vec3::new(n[0], n[2], -n[1]),
                    distance: f32_at(p, 12)? * UNIT,
                });
            }
            if out.len() >= 4 {
                brushes.push(Brush { planes: out });
            }
        }
        Ok(Self {
            brushes,
            terrain: vec![],
            native: None,
            ..default()
        })
    }
    pub fn add_terrain(&mut self, world: &crate::bsp::World) {
        self.terrain = world
            .surfaces
            .iter()
            // "No hull collision" displacements (c13m1's creek water
            // surface) let players wade through, as in retail.
            .filter(|s| s.model == 0 && !s.sky && s.hull)
            .flat_map(|s| s.positions.chunks_exact(3))
            .map(|t| {
                [
                    Vec3::from_array(t[0]),
                    Vec3::from_array(t[1]),
                    Vec3::from_array(t[2]),
                ]
            })
            .filter(|t| (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().y.abs() > 0.6)
            .collect();
        // Visible brush-model floors are absent from the static world leaf tree.
        for brush in world.brushes.iter().filter(|b| {
            !b.sky
                && matches!(
                    b.classname.as_str(),
                    "func_brush"
                        | "func_wall"
                        | "func_wall_toggle"
                        | "func_detail"
                        | "func_elevator"
                        | "func_movelinear"
                )
        }) {
            let rotation = Quat::from_rotation_y(brush.angles[1].to_radians())
                * Quat::from_rotation_z(-brush.angles[0].to_radians())
                * Quat::from_rotation_x(brush.angles[2].to_radians());
            let origin = Vec3::from_array(source_position(brush.origin));
            for surface in world
                .surfaces
                .iter()
                .filter(|s| s.model == brush.model && !s.sky)
            {
                for triangle in surface.positions.chunks_exact(3) {
                    let t =
                        std::array::from_fn(|i| rotation * Vec3::from_array(triangle[i]) + origin);
                    if (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().y.abs() > 0.6 {
                        self.terrain.push(t);
                    }
                }
            }
        }
    }
    pub fn count(&self) -> usize {
        self.brushes.len()
    }
    pub fn supported_spawn(&self, feet: Vec3, height: f32) -> io::Result<Vec3> {
        // Spawn markers can lie slightly inside the displacement support plane.
        for lift in [0.05, 0.15, 0.3, 0.5, 1., 2.] {
            let start = feet + Vec3::Y * lift;
            if !self.clear_hull(start, height) {
                continue;
            }
            let end = self.slide(start, Vec3::NEG_Y * (lift + 1.), height);
            if self.grounded(end, height) && self.clear_hull(end, height) {
                return Ok(end);
            }
        }
        Err(invalid(format!(
            "No clear floor support near spawn {feet:?}"
        )))
    }
    /// Transition landmarks are reference frames, often above the safehouse floor.
    /// Find nearby clear support when launching that chapter without the prior map.
    pub fn supported_spawn_near(&self, feet: Vec3, height: f32) -> io::Result<Vec3> {
        for drop in [0., 1., 2., 3.] {
            for (x, z) in [
                (0., 0.),
                (0.8, 0.),
                (-0.8, 0.),
                (0., 0.8),
                (0., -0.8),
                (1.6, 0.),
                (-1.6, 0.),
                (0., 1.6),
                (0., -1.6),
            ] {
                if let Ok(p) = self.supported_spawn(feet + Vec3::new(x, -drop, z), height) {
                    return Ok(p);
                }
            }
        }
        Err(invalid(format!(
            "No clear floor support around transition spawn {feet:?}"
        )))
    }
    /// A ray in Source units against `mask`: (fraction, end, texinfo,
    /// start solid), or None without the native world tracer.
    /// `props` false traces world brushes only.
    pub fn source_ray(&self, start: [f32; 3], end: [f32; 3], mask: u32, props: bool) -> Option<(f32, [f32; 3], Option<usize>, bool)> {
        let native = self.native.as_ref()?;
        let (start, end, zero) = (l4d2r_math::Vec3::from_array(start), l4d2r_math::Vec3::from_array(end), l4d2r_math::Vec3::ZERO);
        let tracer = native.lock().unwrap();
        let t = if props {
            tracer.box_trace(start, end, zero, zero, mask, 0, zero)
        } else {
            tracer.world_box_trace(start, end, zero, zero, mask)
        };
        Some((t.fraction, t.end_pos.to_array(), t.texinfo, t.start_solid || t.all_solid))
    }
    /// `source_ray` that also returns the hit plane normal (Source axes).
    pub fn source_ray_normal(&self, start: [f32; 3], end: [f32; 3], mask: u32) -> Option<(f32, [f32; 3], [f32; 3])> {
        let native = self.native.as_ref()?;
        let (start, end, zero) = (l4d2r_math::Vec3::from_array(start), l4d2r_math::Vec3::from_array(end), l4d2r_math::Vec3::ZERO);
        let t = native.lock().unwrap().box_trace(start, end, zero, zero, mask, 0, zero);
        Some((t.fraction, t.end_pos.to_array(), t.normal.to_array()))
    }
    /// Source MASK_SOLID_BRUSHONLY | MASK_WATER (solid, moveable, window,
    /// grate, water, slime), as env_soundscape's visibility trace uses.
    /// Returns whether the line is clear.
    pub fn brush_water_clear(&self, start: Vec3, end: Vec3) -> bool {
        if let Some(native) = &self.native {
            let mask = 0x1 | 0x2 | 0x8 | 0x10 | 0x20 | 0x4000;
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let t = native.lock().unwrap().box_trace(
                source(start),
                source(end),
                l4d2r_math::Vec3::ZERO,
                l4d2r_math::Vec3::ZERO,
                mask,
                0,
                l4d2r_math::Vec3::ZERO,
            );
            return t.fraction >= 1. && !t.start_solid;
        }
        let t = self.trace(start, end, Vec3::ZERO);
        t.fraction >= 1. && !t.start_solid
    }
    pub fn sight_trace(&self, start: Vec3, end: Vec3) -> Trace {
        if let Some(native) = &self.native {
            use l4d2r_game_core::trace::{CONTENTS_MOVEABLE, CONTENTS_OPAQUE, CONTENTS_SOLID};
            // Source MASK_VISIBLE: transparent windows/grates and player clips
            // may block bodies or shots without hiding a teammate.
            let mask = CONTENTS_SOLID | CONTENTS_MOVEABLE | CONTENTS_OPAQUE | 0x2000;
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let t = native.lock().unwrap().box_trace(
                source(start),
                source(end),
                l4d2r_math::Vec3::ZERO,
                l4d2r_math::Vec3::ZERO,
                mask,
                0,
                l4d2r_math::Vec3::ZERO,
            );
            return Trace {
                fraction: t.fraction,
                normal: Vec3::from_array(source_normal(t.normal.to_array())),
                start_solid: t.start_solid,
                plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                    .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
            };
        }
        self.trace(start, end, Vec3::ZERO)
    }
    pub fn trace(&self, start: Vec3, end: Vec3, extents: Vec3) -> Trace {
        if let Some(native) = &self.native {
            let source = |p: Vec3| l4d2r_math::Vec3::new(p.x, -p.z, p.y) / UNIT;
            let size = l4d2r_math::Vec3::new(extents.x, extents.z, extents.y) / UNIT;
            let mask = if size.length_squared() > 0. {
                if self.movement_team == 0 {
                    l4d2r_game_core::trace::MASK_PLAYERSOLID
                } else {
                    l4d2r_game_core::trace::MASK_NPCSOLID
                }
            } else {
                l4d2r_game_core::trace::MASK_SHOT
            };
            let filters = self.clip_filters.lock().unwrap();
            let t = native.lock().unwrap().box_trace_without_owners(
                source(start),
                source(end),
                -size,
                size,
                mask,
                &filters[if size.length_squared() > 0. {
                    self.movement_team as usize
                } else {
                    4
                }],
            );
            return Trace {
                fraction: t.fraction,
                normal: Vec3::from_array(source_normal(t.normal.to_array())),
                start_solid: t.start_solid,
                plane_distance: (t.fraction < 1. && t.normal.length_squared() > 0.)
                    .then_some((t.plane_dist + t.normal.dot(t.hit_origin)) * UNIT),
            };
        }
        let mut result = Trace {
            fraction: 1.,
            normal: Vec3::ZERO,
            start_solid: false,
            plane_distance: None,
        };
        for brush in &self.brushes {
            let mut enter = -1.;
            let mut exit: f32 = 1.;
            let mut normal = Vec3::ZERO;
            let mut outside = false;
            let mut rejected = false;
            for plane in &brush.planes {
                let distance = plane.distance + plane.normal.abs().dot(extents);
                let a = plane.normal.dot(start) - distance;
                let b = plane.normal.dot(end) - distance;
                outside |= a > 0.;
                if a > 0. && b >= a {
                    rejected = true;
                    break;
                }
                if a <= 0. && b <= 0. {
                    continue;
                }
                if a > b {
                    let f = (a - 0.001).max(0.) / (a - b);
                    if f > enter {
                        enter = f;
                        normal = plane.normal;
                    }
                } else {
                    exit = exit.min((a + 0.001) / (a - b));
                }
            }
            if rejected {
                continue;
            }
            if !outside {
                result.start_solid = true;
                continue;
            }
            // Contact inside the plane epsilon has a slightly negative entry
            // fraction. Clamp it to zero instead of dropping the floor hit.
            if enter > -1. && enter < exit && enter < result.fraction {
                result.fraction = enter.max(0.);
                result.normal = normal;
            }
        }
        for t in &self.terrain {
            if let Some((fraction, normal)) = triangle_trace(start, end, extents, *t) {
                if fraction < result.fraction {
                    result.fraction = fraction;
                    result.normal = normal;
                }
            }
        }
        result
    }
    pub fn grounded(&self, feet: Vec3, height: f32) -> bool {
        // Centre/half-height conversion at large map coordinates can put the
        // bottom a fraction of a Source unit inside a tilted contact plane.
        let margin = 0.0625 * UNIT;
        let center = feet + Vec3::Y * (height * 0.5 + margin);
        let trace = self.trace(
            center,
            center - Vec3::Y * (2. * UNIT + margin),
            Vec3::new(RADIUS, height * 0.5, RADIUS),
        );
        if trace.fraction < 1. && !trace.start_solid && trace.normal.y >= 0.7 {
            return true;
        }
        // A bevel/steep edge may win the full-hull trace while one quadrant
        // still rests on a walkable floor. Do not move to a quadrant's endpoint.
        [-1., 1.].into_iter().any(|x| {
            [-1., 1.].into_iter().any(|z| {
                let part = center + Vec3::new(x * RADIUS * 0.5, 0., z * RADIUS * 0.5);
                let hit = self.trace(
                    part,
                    part - Vec3::Y * (2. * UNIT + margin),
                    Vec3::new(RADIUS * 0.5, height * 0.5, RADIUS * 0.5),
                );
                !hit.start_solid && hit.fraction < 1. && hit.normal.y >= 0.7
            })
        })
    }
    pub fn snap_ground(&self, feet: Vec3, height: f32) -> Vec3 {
        let offset = Vec3::Y * height * 0.5;
        let drop = Vec3::NEG_Y * (18. * UNIT);
        let hit = self.trace(
            feet + offset,
            feet + offset + drop,
            Vec3::new(RADIUS, height * 0.5, RADIUS),
        );
        if !hit.start_solid && hit.fraction < 1. && hit.normal.y >= 0.7 {
            feet + drop * hit.fraction + hit.normal * (0.03125 * UNIT)
        } else {
            feet
        }
    }
    pub fn clear_hull(&self, feet: Vec3, height: f32) -> bool {
        !self
            .trace(
                feet + Vec3::Y * height * 0.5,
                feet + Vec3::Y * height * 0.5,
                Vec3::new(RADIUS - 0.002, height * 0.5 - 0.002, RADIUS - 0.002),
            )
            .start_solid
    }
    pub fn slide(&self, feet: Vec3, delta: Vec3, height: f32) -> Vec3 {
        let extents = Vec3::new(RADIUS, height * 0.5, RADIUS);
        let offset = Vec3::Y * height * 0.5;
        let mut position = feet + offset;
        let mut remaining = delta;
        for _ in 0..4 {
            let trace = self.trace(position, position + remaining, extents);
            position += remaining * trace.fraction;
            if trace.fraction >= 1. {
                break;
            }
            remaining *= 1. - trace.fraction;
            remaining -= trace.normal * remaining.dot(trace.normal);
            if remaining.length_squared() < 1e-8 {
                break;
            }
        }
        position - offset
    }
    pub fn walk(&self, feet: Vec3, delta: Vec3, height: f32, on_ground: bool) -> Vec3 {
        let flat = self.slide(feet, delta, height);
        if !on_ground || (flat - feet).xz().length_squared() >= delta.xz().length_squared() * 0.99 {
            return flat;
        }
        let raised = self.slide(feet, Vec3::Y * (18. * UNIT), height);
        let moved = self.slide(raised, delta, height);
        let stepped = self.slide(moved, Vec3::NEG_Y * (20. * UNIT), height);
        if self.grounded(stepped, height)
            && (stepped - feet).xz().length_squared() > (flat - feet).xz().length_squared()
        {
            stepped
        } else {
            flat
        }
    }
    pub fn walk_velocity(
        &self,
        feet: Vec3,
        velocity: Vec3,
        dt: f32,
        height: f32,
        on_ground: bool,
    ) -> (Vec3, Vec3) {
        // A thin tilted support can leave a tiny initial overlap. Sweeping
        // from start-solid otherwise returns zero forever, including stepping.
        // The overlap test must use the same hull as the walking sweep. The
        // spawn-clearance hull is intentionally inset and can miss edge snags.
        // Recover only a shallow two-unit overlap, with clear support; deeper
        // embedded hulls must remain blocked rather than crossing solid floors.
        let mut feet = feet;
        let mut velocity = velocity;
        let mut on_ground = on_ground;
        let movement_clear = |feet: Vec3| {
            let center = feet + Vec3::Y * height * 0.5;
            !self
                .trace(center, center, Vec3::new(RADIUS, height * 0.5, RADIUS))
                .start_solid
        };
        if velocity.y <= 0.01 && !movement_clear(feet) {
            for quarter in 1..=8 {
                let candidate = feet + Vec3::Y * (quarter as f32 * 0.25 * UNIT);
                if movement_clear(candidate) && self.grounded(candidate, height) {
                    feet = candidate;
                    on_ground = true;
                    velocity.y = 0.;
                    break;
                }
            }
            if self.movement_team == 3 && !movement_clear(feet) {
                // Common spawn clearance is inset from the walking hull. At
                // a vertical rail/post, lifting cannot clear that shallow
                // lateral overlap. Recover outward by at most two Source
                // units, only to a clear hull with floor support. This cannot
                // transfer the hull across a rail or through a solid wall.
                'recover: for quarter in 1..=8 {
                    for (x, z) in [(-1., 0.), (1., 0.), (0., -1.), (0., 1.),
                        (-1., -1.), (-1., 1.), (1., -1.), (1., 1.)] {
                        let offset = Vec3::new(x, 0., z).normalize() * (quarter as f32 * 0.25 * UNIT);
                        let candidate = feet + offset;
                        if movement_clear(candidate) && self.grounded(candidate, height) {
                            feet = candidate;
                            on_ground = true;
                            velocity.y = 0.;
                            break 'recover;
                        }
                    }
                }
            }
        }
        let delta = velocity * dt;
        let (flat, flat_velocity) = self.slide_velocity(feet, delta, velocity, height);
        if !on_ground || (flat - feet).xz().length_squared() >= delta.xz().length_squared() * 0.99 {
            return (flat, flat_velocity);
        }
        let extents = Vec3::new(RADIUS, height * 0.5, RADIUS);
        let offset = Vec3::Y * height * 0.5;
        let step = (18. + 0.03125) * UNIT;
        let up = self.trace(feet + offset, feet + offset + Vec3::Y * step, extents);
        if up.start_solid {
            return (flat, flat_velocity);
        }
        let raised = feet + Vec3::Y * step * up.fraction;
        let (moved, mut step_velocity) = self.slide_velocity(raised, delta, velocity, height);
        let down = self.trace(moved + offset, moved + offset - Vec3::Y * step, extents);
        if down.start_solid || down.fraction >= 1. || down.normal.y < 0.7 {
            return (flat, flat_velocity);
        }
        let stepped = moved - Vec3::Y * step * down.fraction;
        if (stepped - feet).xz().length_squared() > (flat - feet).xz().length_squared() {
            step_velocity.y = flat_velocity.y;
            (stepped, step_velocity)
        } else {
            (flat, flat_velocity)
        }
    }
    fn slide_velocity(
        &self,
        feet: Vec3,
        delta: Vec3,
        mut velocity: Vec3,
        height: f32,
    ) -> (Vec3, Vec3) {
        let extents = Vec3::new(RADIUS, height * 0.5, RADIUS);
        let offset = Vec3::Y * height * 0.5;
        let mut position = feet + offset;
        let mut remaining = delta;
        let mut planes = [Vec3::ZERO; 4];
        let mut plane_count = 0;
        for _ in 0..4 {
            let hit = self.trace(position, position + remaining, extents);
            if hit.start_solid {
                return (position - offset, Vec3::ZERO);
            }
            position += remaining * hit.fraction;
            if hit.fraction >= 1. {
                break;
            }
            planes[plane_count] = hit.normal;
            plane_count += 1;
            remaining *= 1. - hit.fraction;
            remaining -= hit.normal * remaining.dot(hit.normal);
            velocity -= hit.normal * velocity.dot(hit.normal).min(0.);
            for normal in &planes[..plane_count] {
                if velocity.dot(*normal) < -0.001 {
                    let crease = normal.cross(hit.normal).normalize_or_zero();
                    velocity = crease * velocity.dot(crease);
                }
            }
            if planes[..plane_count]
                .iter()
                .any(|n| velocity.dot(*n) < -0.001)
            {
                velocity = Vec3::ZERO;
            }
            if remaining.length_squared() < 1e-8 {
                break;
            }
        }
        (position - offset, velocity)
    }
    pub fn steer(&self, feet: Vec3, toward: Vec3, height: f32) -> Vec3 {
        let direction = (toward * Vec3::new(1., 0., 1.)).normalize_or_zero();
        if direction == Vec3::ZERO {
            return direction;
        }
        let mut best = Vec3::ZERO;
        let mut score = f32::NEG_INFINITY;
        for angle in [0f32, 30., -30., 60., -60., 90., -90.] {
            let candidate = Quat::from_rotation_y(angle.to_radians()) * direction;
            let end = self.walk(feet, candidate * 0.8, height, true);
            let progress = (end - feet).xz().length();
            // Stay on supported surfaces and skirt obstructions rather than push into them.
            if progress < 0.3 || !self.grounded(self.snap_ground(end, height), height) {
                continue;
            }
            if angle == 0. && progress > 0.79 {
                return direction;
            }
            let value = progress * (0.5 + 0.5 * candidate.dot(direction));
            if value > score {
                score = value;
                best = candidate;
            }
        }
        best
    }
}
// Continuous separating-axis test for a translating AABB and a triangle.
fn triangle_trace(start: Vec3, end: Vec3, extents: Vec3, t: [Vec3; 3]) -> Option<(f32, Vec3)> {
    let lo = t[0].min(t[1]).min(t[2]);
    let hi = t[0].max(t[1]).max(t[2]);
    if (start.min(end) - extents).cmpgt(hi).any() || (start.max(end) + extents).cmplt(lo).any() {
        return None;
    }
    let edges = [t[1] - t[0], t[2] - t[1], t[0] - t[2]];
    let mut axes = vec![Vec3::X, Vec3::Y, Vec3::Z, edges[0].cross(edges[1])];
    for edge in edges {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            axes.push(edge.cross(axis));
        }
    }
    let mut enter: f32 = 0.;
    let mut exit: f32 = 1.;
    let mut normal = Vec3::ZERO;
    for axis in axes {
        if axis.length_squared() < 1e-10 {
            continue;
        }
        let axis = axis.normalize();
        let values = t.map(|p| p.dot(axis));
        let radius = extents.dot(axis.abs());
        let min = values.into_iter().fold(f32::INFINITY, f32::min) - radius;
        let max = values.into_iter().fold(f32::NEG_INFINITY, f32::max) + radius;
        let position = start.dot(axis);
        let speed = (end - start).dot(axis);
        if speed.abs() < 1e-8 {
            if position < min - 1e-5 || position > max + 1e-5 {
                return None;
            }
            continue;
        }
        let a = (min - position) / speed;
        let b = (max - position) / speed;
        let near = a.min(b);
        let far = a.max(b);
        if near >= enter {
            enter = near;
            normal = if speed > 0. { -axis } else { axis };
        }
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }
    if normal == Vec3::ZERO || enter < 0. || enter > 1. {
        return None;
    }
    Some((
        (enter - 0.001 / (end - start).length().max(0.001)).max(0.),
        normal,
    ))
}
#[cfg(test)]
mod tests {
    #[test]
    fn director_query_anger_follows_survivor_health_and_noise() {
        use l4d2r_game_core::entities::director_query_anger as anger;
        // Full health team, no noise roll: the top of the range.
        assert_eq!(anger(400., 100, 25, 1, 8), 8);
        // One survivor's worth of health: true anger 13 -> 2.71 -> 3.
        assert_eq!(anger(100., 100, 25, 1, 8), 3);
        assert_eq!(anger(0., 100, 25, 1, 8), 1);
        // A noise roll (<= noise%) replaces it: roll 1 gives the minimum.
        assert_eq!(anger(400., 1, 25, 1, 8), 1);
        assert_eq!(anger(400., 25, 25, 5, 8), 6);
    }
    #[test]
    fn installed_mall_open_escalators_have_no_invisible_barrier() {
        // Each route's open escalators have a sloped func_brush ramp
        // (escalator_*-brush_lift_collision), most of them yawed. Traced as an
        // oriented box around the brush, the ramp was a wall at the bottom.
        use l4d2r_game_core::entities::Kind;
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m3_mall.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = super::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        for _ in 0..60 {
            logic.world.tick(1. / 60.);
        }
        // Opening the hallway door picks the escalator route.
        logic.world.queue_input("relay_director_set_paths", "Trigger", "", 0., None);
        for _ in 0..120 {
            logic.world.tick(1. / 60.);
        }
        collision.sync_entities(&logic);
        let walk = |from: Vec3, dir: Vec3| {
            let mut p = from + Vec3::Y * 3.;
            let down = collision.trace(p, p - Vec3::Y * 6., Vec3::new(0.4, 0.05, 0.4));
            p += Vec3::NEG_Y * 6. * down.fraction + Vec3::Y * 0.1;
            let start = p;
            let mut vertical = 0.;
            for _ in 0..(60 * 8) {
                let (q, v) = crate::infected_movement::walk(&collision, p, dir * 220. * super::UNIT, vertical, 1. / 60.);
                p = q;
                vertical = v;
            }
            p.y - start.y
        };
        let (mut open, mut closed) = (0, 0);
        for name in ["upper_01", "upper_02", "upper_03", "upper_04", "lower_01", "lower_02", "lower_03", "lower_04"] {
            let railings = logic.world.find(&format!("escalator_{name}-railing"));
            let blocked = railings.iter().any(|&r| matches!(logic.world.ents[r].kind, Kind::Brush { enabled: true, .. }));
            // Top entrance (the railing strip) and the lift prop in the middle.
            let lift = logic.world.find(&format!("escalator_{name}-lift"))[0];
            let middle = Vec3::from_array(crate::binary::source_position(logic.world.ents[lift].me.origin.to_array()));
            let top = railings
                .iter()
                .map(|&r| logic.bounds(r).unwrap())
                .map(|(lo, hi)| (lo + hi) * 0.5)
                .max_by(|a, b| a.y.total_cmp(&b.y))
                .unwrap();
            let flat = (top - middle) * Vec3::new(1., 0., 1.);
            let up = flat.normalize();
            let rise = top.y - middle.y;
            let bottom = Vec3::new(middle.x, middle.y - rise, middle.z) - flat - up * 1.2;
            let climbed = walk(bottom, up);
            let descended = -walk(top + up * 1.2, -up);
            println!("escalator {name}: blocked {blocked}, climbed {climbed:.1} m, descended {descended:.1} m");
            if blocked {
                closed += 1;
                assert!(climbed < 3., "closed escalator {name} must stay blocked ({climbed:.1} m)");
            } else {
                open += 1;
                assert!(climbed > 6. && descended > 6., "open escalator {name}: up {climbed:.1} m, down {descended:.1} m");
            }
        }
        assert!(open >= 3 && closed >= 3, "the route opens some escalators ({open} open, {closed} closed)");
    }
    #[test]
    fn corpse_contacts_preserve_lab_surface_metadata_and_snapshot_geometry() {
        use std::sync::{Arc, Mutex};
        let bytes = std::fs::read("maps/rewrite_lab.bsp").unwrap();
        let bsp = Arc::new(l4d2_formats::bsp::Bsp::parse(bytes).unwrap());
        let surface = crate::physics_data::Surface { friction: 0.25, elasticity: 0.4, density: 2000. };
        let mut collision = Collision::test_native(Arc::new(Mutex::new(
            l4d2r_game_core::trace::Tracer::new(bsp.clone()))));
        collision.contact_surfaces = Arc::new(vec![surface; bsp.texinfo.len()]);
        let start = Vec3::new(1200., 8., -800.) * UNIT;
        let end = Vec3::new(1200., -1., -800.) * UNIT;
        let half = Vec3::splat(0.02 * UNIT);
        for c in [&collision, &collision.worker_snapshot()] {
            let (hit, material) = c.corpse_contact(start, end, half);
            let ordinary = c.corpse_trace(start, end, half);
            assert!(hit.fraction < 1. && !hit.start_solid);
            assert_eq!((hit.fraction, hit.normal), (ordinary.fraction, ordinary.normal));
            let material = material.expect("world brush contact or worker snapshot lost surface metadata");
            assert_eq!((material.friction, material.elasticity), (0.25, 0.4));
        }
        assert!(Collision::test_solid_floor().corpse_contact(Vec3::Y, Vec3::NEG_Y, half).1.is_none());
    }
    #[test]
    fn vphysics_props_without_phy_are_not_solid_tanker_debris() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m2_streets.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let assets = std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let mut bounds = |p: &str| {
            let b = assets.borrow_mut().read(p).ok()?;
            let (lo, hi, vlo, vhi) = l4d2_formats::mdl::read_header_bounds(&b)?;
            let (lo, hi) = if lo == hi { (vlo, vhi) } else { (lo, hi) };
            Some((l4d2r_math::Vec3::from_array(lo), l4d2r_math::Vec3::from_array(hi)))
        };
        let mut physics = |p: &str| {
            let b = assets.borrow_mut().read(&p.replace(".mdl", ".phy")).ok()?;
            let pieces: Vec<_> = l4d2_formats::phy::parse_phy(&b).ok()?.into_iter().flat_map(|s| s.convexes).collect();
            (!pieces.is_empty()).then(|| std::sync::Arc::new(pieces))
        };
        let ents = l4d2r_game_core::mapents::collect(&logic.world.bsp);
        let obbs = l4d2r_game_core::propcol::build_obbs(&logic.world.bsp, &ents, &mut bounds, &mut physics);
        let owners: std::collections::HashSet<_> = obbs.iter().map(|o| o.owner).collect();
        let named = |name: &str| -> Vec<usize> {
            ents.iter().enumerate().filter(|(_, e)| e.targetname == name).map(|(i, _)| i).collect()
        };
        // No .phy: their model bounds must not become invisible walls.
        for name in ["tanker_debris_1", "tanker_debris_4", "tanker_debris_6", "tanker_cables_far", "tanker_cables_near"] {
            for i in named(name) {
                assert!(!owners.contains(&i), "{name} must not be solid without a PHY");
            }
        }
        // Props that do ship a PHY (the concrete barriers by the tanker) stay solid.
        assert!(named("tanker_fence_model").iter().any(|i| owners.contains(i)));
    }
    /// Sweeps survivor-sized boxes along `dir` across a doorway and returns
    /// how many of the offset lanes are blocked before `reach` units.
    fn blocked_lanes(map: &str, start: [f32; 3], dir: [f32; 3], side: [f32; 3], reach: f32) -> usize {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join(format!("left4dead2/maps/{map}.bsp"));
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, false).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let native = collision.native.as_ref().unwrap().lock().unwrap();
        let half = l4d2r_math::Vec3::new(8., 8., 8.);
        let (start, dir, side) = (
            l4d2r_math::Vec3::from_array(start),
            l4d2r_math::Vec3::from_array(dir),
            l4d2r_math::Vec3::from_array(side),
        );
        (-3..=3)
            .filter(|i| {
                let a = start + side * (*i as f32 * 4.);
                let hit = native.box_trace(
                    a,
                    a + dir * reach,
                    -half,
                    half,
                    l4d2r_game_core::trace::MASK_SOLID,
                    0,
                    l4d2r_math::Vec3::ZERO,
                );
                hit.fraction < 1.
            })
            .count()
    }
    #[test]
    fn checkpoint_doors_collide_where_they_are_drawn() {
        if !std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2")
            .exists()
        {
            return;
        }
        // c1m2 starting safe room: checkpoint_exit spawns closed and must
        // block the doorway (its hull used to stay turned 90 degrees).
        let closed = blocked_lanes("c1m2_streets", [2322., 4880., 500.], [0., 1., 0.], [1., 0., 0.], 120.);
        assert_eq!(closed, 7, "closed checkpoint door blocks only {closed}/7 lanes");
        // c1m1 ending safe room: checkpoint_entrance spawns open (spawnpos 2).
        let open = blocked_lanes("c1m1_hotel", [1760., 4591., 1280.], [1., 0., 0.], [0., 1., 0.], 120.);
        assert_eq!(open, 0, "open checkpoint door still blocks {open}/7 lanes");
    }
    #[test]
    fn actor_clip_masks_keep_team_barriers_and_weapon_rays_separate() {
        use super::*;
        use l4d2r_game_core::trace::{
            CONTENTS_MONSTERCLIP, CONTENTS_PLAYERCLIP, CONTENTS_SOLID, Collider, ColliderShape,
            Tracer,
        };
        let mut bytes = vec![0u8; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21i32.to_le_bytes());
        let native = std::sync::Arc::new(std::sync::Mutex::new(Tracer::new(std::sync::Arc::new(
            l4d2_formats::bsp::Bsp::parse(bytes).unwrap(),
        ))));
        let collision = Collision::test_native(native.clone());
        let start = Vec3::new(-1., 0.5, 0.);
        let end = Vec3::new(1., 0.5, 0.);
        for (owner, contents, ignored, expected) in [
            (
                10,
                CONTENTS_PLAYERCLIP,
                [false, false, false, false, false],
                [true, false, false],
            ),
            (
                11,
                CONTENTS_MONSTERCLIP,
                [false, false, false, false, false],
                [false, true, true],
            ),
            (
                12,
                CONTENTS_SOLID,
                [true, false, false, true, true],
                [false, true, true],
            ),
            (
                13,
                CONTENTS_SOLID,
                [true, true, false, true, true],
                [false, false, true],
            ),
            (
                14,
                CONTENTS_SOLID,
                [false, false, false, false, false],
                [true, true, true],
            ),
        ] {
            native.lock().unwrap().set_owned_colliders(vec![(
                owner,
                Collider {
                    active: true,
                    shape: ColliderShape::Box {
                        mins: l4d2r_math::Vec3::new(-2., -50., 0.),
                        maxs: l4d2r_math::Vec3::new(2., 50., 100.),
                        contents,
                    },
                },
            )]);
            *collision.clip_filters.lock().unwrap() = ignored.map(|ignore| {
                if ignore {
                    std::collections::HashSet::from([owner])
                } else {
                    Default::default()
                }
            });
            for (i, (infected, ghost)) in [(false, false), (true, false), (true, true)]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    collision
                        .for_actor(infected, ghost)
                        .trace(start, end, Vec3::splat(0.1))
                        .fraction
                        < 1.,
                    expected[i],
                    "owner={owner} team={i}"
                );
            }
            assert_eq!(
                collision.trace(start, end, Vec3::ZERO).fraction < 1.,
                owner == 14
            );
            assert_eq!(
                collision
                    .corpse_trace(start, end, Vec3::splat(0.1))
                    .fraction
                    < 1.,
                owner == 14
            );
            assert_eq!(
                collision
                    .for_common()
                    .trace(start, end, Vec3::splat(0.1))
                    .fraction
                    < 1.,
                matches!(owner, 11 | 14)
            );
        }
    }
    #[test]
    fn checkpoints_deny_infected_spawns_without_blocking_another_floor() {
        use super::*;
        let nav = l4d2_formats::nav::parse_nav(
            &std::fs::read(crate::test_lab::path().with_extension("nav")).unwrap(),
        )
        .unwrap();
        let mut area = nav.areas[0].clone();
        area.spawn_attributes |= 0x800;
        let p = Vec3::from_array(source_position(area.center()));
        let mut collision = Collision::default();
        collision.checkpoints = std::sync::Arc::new(vec![area]);
        assert!(!collision.infected_spawn_allowed(p));
        assert!(!collision.infected_spawn_allowed(p + Vec3::Y * 0.1));
        assert!(collision.infected_spawn_allowed(p + Vec3::Y * 4.));
        assert!(collision.infected_spawn_allowed(p + Vec3::X * 100.));
    }
    #[test]
    fn worker_snapshot_versions_track_changed_world_colliders() {
        use super::*;
        use l4d2r_game_core::trace::{CONTENTS_SOLID, Collider, ColliderShape, Tracer};
        let mut bytes = vec![0u8; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21i32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(bytes).unwrap());
        let native = std::sync::Arc::new(std::sync::Mutex::new(Tracer::new(bsp)));
        native.lock().unwrap().set_owned_colliders(vec![(
            42,
            Collider {
                shape: ColliderShape::Box {
                    mins: l4d2r_math::Vec3::new(-100., -100., 0.),
                    maxs: l4d2r_math::Vec3::new(100., 100., 8.),
                    contents: CONTENTS_SOLID,
                },
                active: true,
            },
        )]);
        let source = Collision {
            native: Some(native.clone()),
            ..Default::default()
        };
        let old_tag = source.snapshot_tag();
        let old = source.worker_snapshot();
        let hit = |c: &Collision| {
            c.corpse_trace(Vec3::Y, Vec3::NEG_Y, Vec3::splat(0.01))
                .fraction
        };
        assert!(hit(&old) < 1.);
        native.lock().unwrap().set_owned_colliders(Vec::new());
        assert_ne!(old_tag, source.snapshot_tag());
        assert_eq!(hit(&source), 1.);
        assert!(hit(&old) < 1.);
        assert_eq!(hit(&source.worker_snapshot()), 1.);
    }
    #[test]
    fn hotel_rooftop_plank_crossings_recover_shallow_overlap() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = super::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let point = l4d2r_math::Vec3::new(554.4, 5636.4, 2849.8);
        for p in &logic.world.bsp.static_props {
            if p.origin.distance(point) < 150. {
                println!(
                    "NEAR STATIC {} {:?} {:?} solid={}",
                    logic.world.bsp.static_prop_names[p.model_idx as usize],
                    p.origin,
                    p.angles,
                    p.solid
                );
            }
        }
        let center =
            bevy::prelude::Vec3::from_array(crate::binary::source_position(point.to_array()));
        println!(
            "CENTER clear={} grounded={}",
            collision.clear_hull(center, super::HEIGHT),
            collision.grounded(center, super::HEIGHT)
        );
        for units in [0.25, 0.5, 1., 2., 4.] {
            println!(
                "RAISE {units} clear={}",
                collision.clear_hull(
                    center + bevy::prelude::Vec3::Y * (units * super::UNIT),
                    super::HEIGHT
                )
            );
        }
        for p in &logic.world.bsp.static_props {
            let name = &logic.world.bsp.static_prop_names[p.model_idx as usize];
            if name.contains("dead") || name.contains("infected") {
                println!("CORPSE MODEL {name} solid={}", p.solid);
            }
        }
        for direction in [
            bevy::prelude::Vec3::X,
            bevy::prelude::Vec3::NEG_X,
            bevy::prelude::Vec3::Z,
            bevy::prelude::Vec3::NEG_Z,
            (bevy::prelude::Vec3::X + bevy::prelude::Vec3::Z).normalize(),
            (bevy::prelude::Vec3::X - bevy::prelude::Vec3::Z).normalize(),
            (-bevy::prelude::Vec3::X + bevy::prelude::Vec3::Z).normalize(),
            (-bevy::prelude::Vec3::X - bevy::prelude::Vec3::Z).normalize(),
        ] {
            let mut feet = collision.snap_ground(center - direction * 1.5, super::HEIGHT);
            let start = feet;
            for _ in 0..60 {
                let (next, _) = collision.walk_velocity(
                    feet,
                    direction * 3.,
                    1. / 30.,
                    super::HEIGHT,
                    collision.grounded(feet, super::HEIGHT),
                );
                feet = collision.snap_ground(next, super::HEIGHT);
            }
            println!(
                "CROSS {direction:?}: start={start:?} end={feet:?} distance={}",
                (feet - start).xz().length()
            );
            assert!(
                (feet - start).xz().length() > 1.8,
                "Plank blocked {direction:?}"
            );
        }
        let safe = collision.snap_ground(center, super::HEIGHT);
        let embedded = safe - bevy::prelude::Vec3::Y * (0.5 * super::UNIT);
        assert!(!collision.clear_hull(embedded, super::HEIGHT));
        let (recovered, _) = collision.walk_velocity(
            embedded,
            bevy::prelude::Vec3::X,
            1. / 30.,
            super::HEIGHT,
            false,
        );
        assert!(collision.clear_hull(recovered, super::HEIGHT));
        assert!(
            recovered.x > embedded.x + 0.02,
            "A tiny initial overlap must not freeze movement"
        );
        let deep = safe - bevy::prelude::Vec3::Y * (8. * super::UNIT);
        let (blocked, _) =
            collision.walk_velocity(deep, bevy::prelude::Vec3::X, 1. / 30., super::HEIGHT, false);
        assert_eq!(
            blocked, deep,
            "Recovery must not escape deep solid geometry"
        );
    }
    #[test]
    #[ignore = "manual collision-stage profiling"]
    fn profile_hotel_corpse_trace_layers() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let world = l4d2r_game_core::trace::Tracer::new(logic.world.bsp.clone());
        let native = collision.native.as_ref().unwrap().lock().unwrap();
        let half = l4d2r_math::Vec3::splat(0.02);
        let mask = l4d2r_game_core::trace::MASK_SOLID | l4d2r_game_core::trace::CONTENTS_DEBRIS;
        for kind in 0..3 {
            let start = std::time::Instant::now();
            for i in 0..10000 {
                let p = l4d2r_math::Vec3::new(
                    1800. + (i % 50) as f32,
                    6200. + ((i / 50) % 100) as f32,
                    2460. + (i % 35) as f32,
                );
                let end = if i % 2 == 0 {
                    p
                } else {
                    p + l4d2r_math::Vec3::new(0.5, 0.3, -0.1)
                };
                let hit = match kind {
                    0 => world.box_trace(p, end, -half, half, mask, 0, l4d2r_math::Vec3::ZERO),
                    1 => native.box_trace_environment(p, end, -half, half, mask),
                    _ => native.box_trace(p, end, -half, half, mask, 0, l4d2r_math::Vec3::ZERO),
                };
                std::hint::black_box(hit);
            }
            println!(
                "HOTEL TRACE layer={kind} queries=10000 ms={:.3}",
                start.elapsed().as_secs_f64() * 1000.
            );
        }
    }
    #[test]
    fn walking_recovers_full_hull_edge_overlap_but_not_deep_embedded_walls() {
        let c = Collision::test_solid_floor();
        for height in [HEIGHT, DUCK_HEIGHT] {
            for depth in [0.001, 1. * UNIT, 1.9 * UNIT] {
                let feet = Vec3::NEG_Y * depth;
                if depth == 0.001 {
                    assert!(
                        c.clear_hull(feet, height),
                        "inset hull should miss this overlap"
                    );
                }
                let (next, _) = c.walk_velocity(feet, Vec3::X * 3., 1. / 30., height, false);
                assert!(
                    next.x > 0.09 && next.y >= 0.,
                    "depth {depth}, next {next:?}"
                );
            }
            let feet = Vec3::NEG_Y * (19. * UNIT);
            let (next, _) = c.walk_velocity(feet, Vec3::X * 3., 1. / 30., height, false);
            assert_eq!(next, feet, "recovery must not cross more than step height");
        }
    }
    #[test]
    fn repeated_owned_collision_snapshots_preserve_activity_motion_and_owner_filtering() {
        use l4d2r_game_core::trace::{CONTENTS_SOLID, Collider, ColliderShape, MASK_SOLID, Tracer};
        use l4d2r_math::Vec3 as V;
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        let tracer = Tracer::new(std::sync::Arc::new(
            l4d2_formats::bsp::Bsp::parse(bytes).unwrap(),
        ));
        let shape = |x, active| Collider {
            active,
            shape: ColliderShape::Box {
                mins: V::new(x, -5., -5.),
                maxs: V::new(x + 5., 5., 5.),
                contents: CONTENTS_SOLID,
            },
        };
        let hit = |owner| {
            tracer
                .box_trace_without_owner(
                    V::ZERO,
                    V::new(100., 0., 0.),
                    V::ZERO,
                    V::ZERO,
                    MASK_SOLID,
                    owner,
                )
                .fraction
        };
        for _ in 0..50 {
            tracer.set_owned_colliders(vec![(10, shape(20., true))]);
            assert!(hit(99) < 0.3);
            assert_eq!(hit(10), 1.);
        }
        tracer.set_owned_colliders(vec![(10, shape(20., false))]);
        assert_eq!(hit(99), 1.);
        tracer.set_owned_colliders(vec![(10, shape(70., true))]);
        assert!(hit(99) > 0.6 && hit(99) < 0.8);
        tracer.set_owned_colliders(vec![(20, shape(70., true))]);
        assert_eq!(hit(20), 1.);
        assert!(hit(10) < 0.8);
        tracer.set_owned_colliders(vec![]);
        assert_eq!(hit(99), 1.);
    }
    #[test]
    fn dynamic_collision_grid_keeps_nearby_moved_and_oversize_shapes() {
        use l4d2r_game_core::trace::{CONTENTS_SOLID, Collider, ColliderShape, MASK_SOLID, Tracer};
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        let tracer = Tracer::new(std::sync::Arc::new(
            l4d2_formats::bsp::Bsp::parse(bytes).unwrap(),
        ));
        let v = l4d2r_math::Vec3::new;
        let box_at = |x: f32| Collider {
            active: true,
            shape: ColliderShape::Box {
                mins: v(x, -5., 0.),
                maxs: v(x + 10., 5., 30.),
                contents: CONTENTS_SOLID,
            },
        };
        let mut colliders = vec![box_at(10.)];
        colliders.extend((0..200).map(|i| box_at(1000. + i as f32 * 300.)));
        tracer.set_colliders(colliders);
        let trace = |end| {
            tracer.box_trace(
                v(0., 0., 10.),
                end,
                v(0., 0., 0.),
                v(0., 0., 0.),
                MASK_SOLID,
                0,
                v(0., 0., 0.),
            )
        };
        assert!(trace(v(30., 0., 10.)).fraction < 1.);
        tracer.set_collider(0, box_at(-20.));
        assert_eq!(trace(v(30., 0., 10.)).fraction, 1.);
        assert!(trace(v(-30., 0., 10.)).fraction < 1.);
        assert!(trace(v(2_000_000., 0., 10.)).fraction < 1.);
        tracer.set_collider(
            0,
            Collider {
                active: true,
                shape: ColliderShape::Box {
                    mins: v(-1_000_000., -1_000_000., -10.),
                    maxs: v(1_000_000., 1_000_000., 30.),
                    contents: CONTENTS_SOLID,
                },
            },
        );
        assert!(trace(v(30., 0., 10.)).start_solid);
    }
    #[test]
    fn dormant_mode_props_do_not_leave_solid_boxes_and_activate_on_spawn() {
        use l4d2r_game_core::{
            entities::Kind,
            trace::{CONTENTS_SOLID, Obb, Tracer},
        };
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic =
            crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), true).unwrap();
        let owner = 0;
        logic.world.ents[owner].me.classname = "prop_dynamic".into();
        logic.world.ents[owner].me.mdl = Some("test.mdl".into());
        logic.world.ents[owner].kind = Kind::Brush {
            enabled: true,
            solid: true,
        };
        logic.world.ents[owner].alive = false;
        let center = l4d2r_math::Vec3::new(10000., 10000., 300.);
        let mut obb = Obb::new(
            center,
            l4d2r_math::Vec3::splat(16.),
            l4d2r_math::Mat3::IDENTITY,
            CONTENTS_SOLID,
        );
        obb.owner = owner;
        let mut tracer = Tracer::new(logic.world.bsp.clone());
        tracer.set_obbs(vec![obb]);
        let collision = Collision {
            native: Some(std::sync::Arc::new(std::sync::Mutex::new(tracer))),
            ..Default::default()
        };
        let start = Vec3::from_array(crate::binary::source_position(
            (center - l4d2r_math::Vec3::X * 100.).to_array(),
        ));
        let end = Vec3::from_array(crate::binary::source_position(
            (center + l4d2r_math::Vec3::X * 100.).to_array(),
        ));
        collision.sync_entities(&logic);
        assert_eq!(collision.trace(start, end, Vec3::ZERO).fraction, 1.);
        logic.world.ents[owner].alive = true;
        collision.sync_entities(&logic);
        assert!(collision.trace(start, end, Vec3::ZERO).fraction < 1.);
        logic.world.ents[owner].kind = Kind::Brush {
            enabled: false,
            solid: true,
        };
        collision.sync_entities(&logic);
        assert_eq!(collision.trace(start, end, Vec3::ZERO).fraction, 1.);
    }
    use super::*;
    #[test]
    fn prop_trace_ignores_only_self_and_wall_motion_clips_velocity() {
        use l4d2r_game_core::trace::{
            CONTENTS_SOLID, Collider, ColliderShape, MASK_SOLID, Obb, Tracer,
        };
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(bytes).unwrap());
        let mut tracer = Tracer::new(bsp);
        let v = l4d2r_math::Vec3::new;
        tracer.set_owned_colliders(vec![
            (
                100,
                Collider {
                    active: true,
                    shape: ColliderShape::Box {
                        mins: v(-5., -5., 40.),
                        maxs: v(5., 5., 60.),
                        contents: CONTENTS_SOLID,
                    },
                },
            ),
            (
                200,
                Collider {
                    active: true,
                    shape: ColliderShape::Box {
                        mins: v(64., -100., -10.),
                        maxs: v(68., 100., 200.),
                        contents: CONTENTS_SOLID,
                    },
                },
            ),
        ]);
        let hit = tracer.box_trace_without_owner(
            v(0., 0., 50.),
            v(100., 0., 50.),
            v(-1., -1., -1.),
            v(1., 1., 1.),
            MASK_SOLID,
            100,
        );
        assert!(!hit.start_solid && hit.fraction > 0.5 && hit.fraction < 0.7);
        assert!(
            tracer
                .box_trace(
                    v(0., 0., 50.),
                    v(100., 0., 50.),
                    v(-1., -1., -1.),
                    v(1., 1., 1.),
                    MASK_SOLID,
                    0,
                    v(0., 0., 0.)
                )
                .start_solid,
            "self filtering changed the shared collider state"
        );
        let mut stale = Obb::new(
            v(0., 0., 50.),
            v(5., 5., 5.),
            l4d2r_math::Mat3::IDENTITY,
            CONTENTS_SOLID,
        );
        stale.owner = 100;
        tracer.set_obbs(vec![stale]);
        tracer.set_owned_colliders(vec![(
            100,
            Collider {
                active: true,
                shape: ColliderShape::Box {
                    mins: v(95., -5., 45.),
                    maxs: v(105., 5., 55.),
                    contents: CONTENTS_SOLID,
                },
            },
        )]);
        assert_eq!(
            tracer
                .box_trace(
                    v(0., 0., 50.),
                    v(20., 0., 50.),
                    v(-1., -1., -1.),
                    v(1., 1., 1.),
                    MASK_SOLID,
                    0,
                    v(0., 0., 0.)
                )
                .fraction,
            1.,
            "moved prop left an old placed collision proxy"
        );
        let mut disabled = tracer.collider(0).unwrap();
        disabled.active = false;
        tracer.set_collider(0, disabled);
        assert_eq!(
            tracer
                .box_trace(
                    v(0., 0., 50.),
                    v(110., 0., 50.),
                    v(-1., -1., -1.),
                    v(1., 1., 1.),
                    MASK_SOLID,
                    0,
                    v(0., 0., 0.)
                )
                .fraction,
            1.,
            "disabled live collider must not revive its old proxy"
        );
        tracer.set_obbs(vec![]);
        tracer.set_colliders(vec![Collider {
            active: true,
            shape: ColliderShape::Box {
                mins: v(64., -100., -10.),
                maxs: v(68., 100., 200.),
                contents: CONTENTS_SOLID,
            },
        }]);
        let c = Collision {
            native: Some(std::sync::Arc::new(std::sync::Mutex::new(tracer))),
            ..Default::default()
        };
        let (position, velocity) = c.walk_velocity(
            Vec3::ZERO,
            Vec3::new(200. * UNIT, 0., 20. * UNIT),
            0.5,
            HEIGHT,
            false,
        );
        assert!(position.x < 49. * UNIT && position.x > 47. * UNIT);
        assert!(
            velocity.x.abs() < 0.0001 && (velocity.z - 20. * UNIT).abs() < 0.0001,
            "wall contact must remove inward velocity and preserve tangent motion"
        );
    }
    #[test]
    fn highway_bush_without_phy_does_not_create_a_phantom_trunk() {
        use l4d2r_game_core::propcol;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c2m1_highway.bsp");
        if !path.exists() {
            return;
        }
        let bytes = crate::bsp::load_bytes(&path, root).unwrap();
        let mut bsp = l4d2_formats::bsp::Bsp::parse(bytes).unwrap();
        let name = "models/props_foliage/urban_tree_base_bushes01_large.mdl";
        bsp.static_props
            .retain(|p| bsp.static_prop_names[p.model_idx as usize].eq_ignore_ascii_case(name));
        assert!(bsp.static_props.iter().any(|p| {
            p.origin
                .distance(l4d2r_math::Vec3::new(2460., 188., -1094.55))
                < 1.
        }));
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let mdl = assets.read(name).unwrap();
        let (lo, hi, _, _) = l4d2_formats::mdl::read_header_bounds(&mdl).unwrap();
        assert!(assets.read(&name.replace(".mdl", ".phy")).is_err());
        let mut bounds = |_: &str| {
            Some((
                l4d2r_math::Vec3::from_array(lo),
                l4d2r_math::Vec3::from_array(hi),
            ))
        };
        let mut phy = |_: &str| None;
        assert!(propcol::build_obbs(&bsp, &[], &mut bounds, &mut phy).is_empty());
        // Explicit box collision remains solid, even without a physics asset.
        for p in &mut bsp.static_props {
            p.solid = 2;
        }
        let boxes = propcol::build_obbs(&bsp, &[], &mut bounds, &mut phy);
        assert_eq!(boxes.len(), bsp.static_props.len());
        assert!(boxes.iter().all(|b| b.shape.is_none()));
    }
    fn box_world(min: Vec3, max: Vec3) -> Collision {
        let mut planes = vec![];
        for axis in 0..3 {
            let mut n = Vec3::ZERO;
            n[axis] = 1.;
            planes.push(Plane {
                normal: n,
                distance: max[axis],
            });
            planes.push(Plane {
                normal: -n,
                distance: -min[axis],
            });
        }
        Collision {
            brushes: vec![Brush { planes }],
            terrain: vec![],
            native: None,
            ..default()
        }
    }
    #[test]
    fn corpse_contacts_use_live_props_and_stop_when_the_prop_is_removed() {
        use l4d2r_game_core::trace::{CONTENTS_DEBRIS, Collider, ColliderShape, Tracer};
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(bytes).unwrap());
        let tracer = Tracer::new(bsp);
        tracer.set_colliders(vec![Collider {
            active: true,
            shape: ColliderShape::Box {
                mins: l4d2r_math::Vec3::new(-100., -100., 0.),
                maxs: l4d2r_math::Vec3::new(100., 100., 40.),
                contents: CONTENTS_DEBRIS,
            },
        }]);
        let c = Collision {
            native: Some(std::sync::Arc::new(std::sync::Mutex::new(tracer))),
            ..Default::default()
        };
        let start = Vec3::Y * 2.;
        let half = Vec3::splat(0.08);
        assert_eq!(c.physics_trace(start, Vec3::ZERO, half).fraction, 1.);
        let contact = c.corpse_trace(start, Vec3::ZERO, half);
        assert!(contact.fraction < 1. && contact.normal.y > 0.9);
        let embedded = Vec3::Y * 0.9;
        assert!(c.corpse_trace(embedded, embedded, half).start_solid);
        let recovered = c.recover_corpse_position(embedded, half, 0.6).unwrap();
        assert!(!c.corpse_trace(recovered, recovered, half).start_solid);
        c.native
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .set_colliders(vec![]);
        assert_eq!(c.corpse_trace(start, Vec3::ZERO, half).fraction, 1.);
    }
    #[test]
    fn rotated_door_has_no_solid_world_aabb_corners_and_debris_does_not_block_players() {
        use l4d2r_game_core::trace::{
            CONTENTS_DEBRIS, CONTENTS_SOLID, Collider, ColliderShape, MASK_PLAYERSOLID, MASK_SHOT,
            Tracer,
        };
        let mut bytes = vec![0; 1036];
        bytes[..4].copy_from_slice(b"VBSP");
        bytes[4..8].copy_from_slice(&21u32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(bytes).unwrap());
        let tracer = Tracer::new(bsp);
        let v = l4d2r_math::Vec3::new;
        let matrix = l4d2r_math::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4);
        let shape = ColliderShape::OrientedBox {
            mins: v(-4., 0., 0.),
            maxs: v(4., 55., 112.),
            matrix,
            contents: CONTENTS_SOLID,
        };
        tracer.set_colliders(vec![Collider {
            shape,
            active: true,
        }]);
        let trace = |start, end, extent: l4d2r_math::Vec3, mask| {
            tracer.box_trace(start, end, -extent, extent, mask, 0, l4d2r_math::Vec3::ZERO)
        };
        assert_eq!(
            trace(
                v(-34., -18., 40.),
                v(-34., -6., 40.),
                v(16., 16., 16.),
                MASK_PLAYERSOLID
            )
            .fraction,
            1.
        );
        assert!(trace(v(-50., 20., 40.), v(0., 20., 40.), v(0., 0., 0.), MASK_SHOT).fraction < 1.);
        // Two native convex pieces must preserve the opening between them, including after placement.
        let cube = |lo: l4d2r_math::Vec3, hi: l4d2r_math::Vec3| {
            let mut planes = vec![];
            let mut edges = vec![];
            for axis in 0..3 {
                let mut n = l4d2r_math::Vec3::ZERO;
                n[axis] = 1.;
                planes.push((n, hi[axis]));
                planes.push((-n, -lo[axis]));
                for a in [false, true] {
                    for b in [false, true] {
                        let mut p = lo;
                        p[(axis + 1) % 3] = if a {
                            hi[(axis + 1) % 3]
                        } else {
                            lo[(axis + 1) % 3]
                        };
                        p[(axis + 2) % 3] = if b {
                            hi[(axis + 2) % 3]
                        } else {
                            lo[(axis + 2) % 3]
                        };
                        let mut q = p;
                        q[axis] = hi[axis];
                        edges.push((p, q));
                    }
                }
            }
            l4d2_formats::phy::Convex {
                topology: None,
                triangle_headers: vec![],
                triangles: vec![],
                planes,
                edges,
                bevels: vec![],
                mins: lo,
                maxs: hi,
            }
        };
        let placement = l4d2r_math::Mat4::from_translation(v(100., 0., 0.)) * matrix;
        tracer.set_colliders(vec![Collider {
            shape: ColliderShape::Convexes {
                shape: std::sync::Arc::new(vec![
                    cube(v(-20., -5., 0.), v(-10., 5., 50.)),
                    cube(v(10., -5., 0.), v(20., 5., 50.)),
                ]),
                matrix: placement,
                contents: CONTENTS_SOLID,
            },
            active: true,
        }]);
        let placed_trace = |x| {
            trace(
                placement.transform_point3(v(x, -30., 25.)),
                placement.transform_point3(v(x, 30., 25.)),
                v(1., 1., 1.),
                MASK_PLAYERSOLID,
            )
        };
        assert_eq!(placed_trace(0.).fraction, 1.);
        assert!(placed_trace(15.).fraction < 1.);
        // Repeated world synchronization must retain the shape, while moving,
        // disabling and replacing a live prop must invalidate its placed cache.
        let original = tracer.collider(0).unwrap();
        let expected = placed_trace(15.).fraction;
        for _ in 0..3 {
            tracer.set_colliders(vec![original.clone()]);
            assert_eq!(placed_trace(15.).fraction, expected);
        }
        let mut moved = original.clone();
        if let ColliderShape::Convexes { matrix, .. } = &mut moved.shape {
            *matrix = l4d2r_math::Mat4::from_translation(v(500., 0., 0.)) * *matrix;
        }
        tracer.set_collider(0, moved.clone());
        assert_eq!(placed_trace(15.).fraction, 1.);
        let moved_start = placement.transform_point3(v(15., -30., 25.)) + v(500., 0., 0.);
        let moved_end = placement.transform_point3(v(15., 30., 25.)) + v(500., 0., 0.);
        assert!(
            (trace(moved_start, moved_end, v(1., 1., 1.), MASK_PLAYERSOLID).fraction - expected)
                .abs()
                < 1e-5
        );
        moved.active = false;
        tracer.set_collider(0, moved);
        assert_eq!(
            trace(moved_start, moved_end, v(1., 1., 1.), MASK_PLAYERSOLID).fraction,
            1.
        );
        let mut replaced = original;
        if let ColliderShape::Convexes { shape, .. } = &mut replaced.shape {
            *shape = std::sync::Arc::new(vec![cube(v(-20., -5., 0.), v(20., 5., 50.))]);
        }
        tracer.set_colliders(vec![replaced]);
        assert!(placed_trace(0.).fraction < 1.);
        tracer.set_colliders(vec![Collider {
            shape: ColliderShape::OrientedBox {
                mins: v(-10., -10., 0.),
                maxs: v(10., 10., 20.),
                matrix: l4d2r_math::Mat4::IDENTITY,
                contents: CONTENTS_DEBRIS,
            },
            active: true,
        }]);
        assert_eq!(
            trace(
                v(-30., 0., 10.),
                v(30., 0., 10.),
                v(1., 1., 1.),
                MASK_PLAYERSOLID
            )
            .fraction,
            1.
        );
        assert!(trace(v(-30., 0., 10.), v(30., 0., 10.), v(0., 0., 0.), MASK_SHOT).fraction < 1.);
    }
    #[test]
    fn snapped_brush_contact_keeps_blocking_gravity_over_repeated_steps() {
        let c = Collision::test_solid_floor();
        let mut feet = Vec3::Y * 0.04;
        for _ in 0..1200 {
            feet = c.snap_ground(feet, HEIGHT);
            assert!(
                c.grounded(feet, HEIGHT),
                "snapped hull lost ground at {feet:?}"
            );
            feet = c.walk(feet, Vec3::new(0.001, -0.003, 0.), HEIGHT, true);
            assert!(
                feet.y >= 0. && c.clear_hull(feet, HEIGHT),
                "contact tunneled into the floor: {feet:?}"
            );
        }
    }
    #[test]
    fn exact_floor_contact_and_seams_block_downward_motion() {
        let c = Collision {
            brushes: vec![],
            native: None,
            terrain: vec![
                [
                    Vec3::new(-10., 0., -10.),
                    Vec3::new(10., 0., -10.),
                    Vec3::new(10., 0., 10.),
                ],
                [
                    Vec3::new(-10., 0., -10.),
                    Vec3::new(10., 0., 10.),
                    Vec3::new(-10., 0., 10.),
                ],
            ],
            ..default()
        };
        let mut feet = Vec3::ZERO;
        for _ in 0..300 {
            feet = c.slide(feet, Vec3::new(0.01, -0.03, 0.005), HEIGHT);
            assert!(feet.y >= -0.001);
        }
        assert!(c.grounded(feet, HEIGHT));
        let spawn = c.supported_spawn(Vec3::NEG_Y * 0.01, HEIGHT).unwrap();
        assert!(spawn.y >= 0.);
    }
    #[test]
    fn displaced_roof_stops_a_survivor_hull() {
        let c = Collision {
            brushes: vec![],
            native: None,
            terrain: vec![[
                Vec3::new(-10., 0., -10.),
                Vec3::new(10., 0., -10.),
                Vec3::new(0., 0., 10.),
            ]],
            ..default()
        };
        let feet = c.slide(Vec3::Y, Vec3::NEG_Y * 3., HEIGHT);
        assert!(feet.y >= 0. && feet.y < 0.01);
        assert!(c.grounded(feet, HEIGHT));
    }
    #[test]
    fn swept_hull_blocks_thin_walls_and_slides() {
        let c = box_world(Vec3::new(1., -10., -10.), Vec3::new(1.05, 10., 10.));
        let p = c.slide(Vec3::ZERO, Vec3::new(5., 0., 2.), HEIGHT);
        assert!(p.x < 1. - RADIUS && p.x > 0.5);
        assert!((p.z - 2.).abs() < 0.01);
    }
    #[test]
    fn floor_support_and_standing_clearance() {
        let c = box_world(Vec3::splat(-10.), Vec3::new(10., 0., 10.));
        assert!(c.grounded(Vec3::Y * 0.002, HEIGHT));
        assert!(!c.grounded(Vec3::Y, HEIGHT));
        assert!(!c.clear_hull(Vec3::NEG_Y, HEIGHT));
    }
    #[test]
    fn steering_skirts_a_wall_and_rejects_unsupported_edges() {
        let mut c = box_world(Vec3::new(0.6, 0., -0.3), Vec3::new(0.8, 3., 0.3));
        c.terrain = Collision::test_floor().terrain;
        let direction = c.steer(Vec3::Y * 0.001, Vec3::X, HEIGHT);
        assert!(direction.x > 0. && direction.z.abs() > 0.4);
        let next = c.walk(Vec3::Y * 0.001, direction * 0.8, HEIGHT, true);
        assert!(c.clear_hull(next, HEIGHT));
        assert_eq!(
            Collision::default().steer(Vec3::ZERO, Vec3::X, HEIGHT),
            Vec3::ZERO
        );
    }
}
