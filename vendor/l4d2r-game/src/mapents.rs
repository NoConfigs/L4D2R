//! Typed view over the BSP entity lump: which entities are brush models, props, item spawns, and how brush
//! entities are placed. (Gameplay behaviour lives in `entities.rs`; this is the data layer both the renderer and the sim use.)

use glam::{Mat4, Vec3};
use l4d2_formats::bsp::{Bsp, Entity};

#[derive(Clone, Debug)]
pub struct MapEnt {
    pub index: usize,
    pub classname: String,
    pub targetname: String,
    pub origin: Vec3,
    pub angles: Vec3,
    /// `model` key when it names a `.mdl`.
    pub mdl: Option<String>,
    /// Brush-model index when `model` is `*N`.
    pub brush: Option<usize>,
    pub ent: Entity,
}

impl MapEnt {
    pub fn kv(&self, k: &str) -> Option<&str> {
        self.ent.get(k)
    }
    pub fn kv_i32(&self, k: &str) -> Option<i32> {
        self.kv(k).and_then(|v| v.trim().parse().ok())
    }
    pub fn kv_f32(&self, k: &str) -> Option<f32> {
        self.kv(k).and_then(|v| v.trim().parse().ok())
    }
    pub fn spawnflags(&self) -> u32 {
        self.kv_i32("spawnflags").unwrap_or(0) as u32
    }
    /// `rendermode 10` = don't draw.
    pub fn draws(&self) -> bool {
        self.kv_i32("rendermode") != Some(10)
    }
}

pub fn collect(bsp: &Bsp) -> Vec<MapEnt> {
    bsp.entities
        .iter()
        .enumerate()
        .map(|(index, e)| {
            let model = e.get("model").unwrap_or("");
            MapEnt {
                index,
                classname: e.classname().to_string(),
                targetname: e.get("targetname").unwrap_or("").to_string(),
                origin: e.origin(),
                angles: e.angles(),
                mdl: model
                    .to_ascii_lowercase()
                    .ends_with(".mdl")
                    .then(|| model.replace('\\', "/")),
                brush: model.strip_prefix('*').and_then(|n| n.parse().ok()),
                ent: e.clone(),
            }
        })
        .collect()
}

/// Brush-entity classes whose geometry is drawn.
pub fn is_visible_brush_class(c: &str) -> bool {
    matches!(
        c,
        "func_brush"
            | "func_breakable"
            | "func_breakable_surf"
            | "func_door"
            | "func_door_rotating"
            | "func_movelinear"
            | "func_rotating"
            | "func_button"
            | "func_rot_button"
            | "func_tracktrain"
            | "func_wall"
            | "func_wall_toggle"
            | "func_illusionary"
            | "func_physbox"
            | "func_conveyor"
            | "func_lod"
            | "func_water_analog"
    )
}

/// Prop-like entities drawn with a studio model.
pub fn is_prop_class(c: &str) -> bool {
    matches!(
        c,
        "prop_physics"
            | "prop_physics_override"
            | "prop_physics_multiplayer"
            | "prop_dynamic"
            | "prop_dynamic_override"
            | "prop_door_rotating"
            | "prop_door_rotating_checkpoint"
            | "prop_wall_breakable"
            | "prop_car_alarm"
            | "prop_car_glass"
            | "prop_health_cabinet"
            | "prop_minigun"
            | "prop_mounted_machine_gun"
            | "prop_fuel_barrel"
    )
}

/// Source `QAngle` (pitch, yaw, roll degrees) -> rotation: `Rz(yaw) * Ry(pitch) * Rx(roll)`.
pub fn angle_matrix(angles: Vec3) -> Mat4 {
    Mat4::from_rotation_z(angles.y.to_radians())
        * Mat4::from_rotation_y(angles.x.to_radians())
        * Mat4::from_rotation_x(angles.z.to_radians())
}

pub fn entity_matrix(origin: Vec3, angles: Vec3) -> Mat4 {
    Mat4::from_translation(origin) * angle_matrix(angles)
}

/// The brush entities that get a draw list (initially hidden ones are toggled by `startdisabled`/I/O).
pub fn visible_brush_ents(ents: &[MapEnt]) -> Vec<&MapEnt> {
    ents.iter()
        .filter(|e| e.brush.is_some() && is_visible_brush_class(&e.classname) && e.draws())
        .collect()
}

/// A prop-like entity placed in the world (renderer-neutral).
#[derive(Clone, Debug)]
pub struct PropInstance {
    /// Index of the map entity this prop came from.
    pub ent: usize,
    pub model: String,
    pub origin: Vec3,
    pub angles: Vec3,
    pub scale: f32,
    pub fade_max: f32,
}

/// Prop entities that should be drawn (skips `rendermode 10` and `startdisabled`).
pub fn prop_instances(ents: &[MapEnt]) -> Vec<PropInstance> {
    ents.iter()
        .filter(|e| {
            is_prop_class(&e.classname) && e.draws() && e.kv_i32("startdisabled") != Some(1)
        })
        .filter_map(|e| {
            Some(PropInstance {
                ent: e.index,
                model: e.mdl.clone()?,
                origin: e.origin,
                angles: e.angles,
                scale: e.kv_f32("modelscale").unwrap_or(1.0),
                fade_max: e.kv_f32("fademaxdist").unwrap_or(0.0),
            })
        })
        .collect()
}

/// Where a freshly loaded map starts the survivors. First-campaign maps carry `info_survivor_position`; later maps have none
/// (the team arrives through the previous map's changelevel), so use the *arrival* `info_landmark` (the one this map's own
/// `info_changelevel` doesn't name), snapped to the nearest nav area centre when a mesh is available.
/// UNVERIFIED: the original positions the team relative to the previous map's landmark; this is the direct-load stand-in.
pub fn start_position(bsp: &l4d2_formats::bsp::Bsp, map: &str, nav: &[(Vec3, u32)]) -> Vec3 {
    // Chapter 1 of a campaign (`cNm1_*`) really starts at info_survivor_position; later chapters use the arrival landmark / player start.
    let first_chapter = map
        .split_once('m')
        .is_some_and(|(_, r)| r.starts_with("1_"));
    if first_chapter {
        if let Some(e) = bsp
            .entities
            .iter()
            .find(|e| e.classname() == "info_survivor_position")
        {
            return e.origin();
        }
    }
    // The exit changelevel is the one that names a destination map; the other (no `map`) is the arrival side.
    let exit = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "info_changelevel" && !e.get("map").unwrap_or("").is_empty())
        .and_then(|e| e.get("landmark"))
        .unwrap_or("")
        .to_ascii_lowercase();
    let lm = bsp
        .entities
        .iter()
        .filter(|e| e.classname() == "info_landmark")
        .find(|e| e.get("targetname").unwrap_or("").to_ascii_lowercase() != exit)
        .map(|e| e.origin());
    let snap = |p: Vec3, only: &dyn Fn(u32) -> bool| {
        nav.iter()
            .filter(|(_, a)| only(*a))
            .min_by(|a, b| a.0.distance_squared(p).total_cmp(&b.0.distance_squared(p)))
            .map(|(c, _)| *c + Vec3::Z * 4.0)
    };
    if let Some(p) = lm {
        // Saferoom (CHECKPOINT 0x800) nav area nearest the arrival landmark; any area if the mesh has none.
        return snap(p, &|a| a & 0x800 != 0)
            .or_else(|| snap(p, &|_| true))
            .unwrap_or(p);
    }
    if let Some(e) = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "info_player_start")
    {
        return e.origin();
    }
    if let Some(e) = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "info_survivor_position")
    {
        return e.origin();
    }
    // No landmark and no start entity: a saferoom area (CHECKPOINT 0x800 + 0x80 start bit, falling back to either) nearest a saferoom door.
    let door = bsp
        .entities
        .iter()
        .find(|e| e.classname() == "prop_door_rotating_checkpoint")
        .map(|e| e.origin())
        .unwrap_or(Vec3::ZERO);
    for mask in [0x880u32, 0x800, 0x80] {
        if let Some(c) = snap(door, &|a| a & mask == mask) {
            return c;
        }
    }
    snap(door, &|_| true).unwrap_or(door)
}
