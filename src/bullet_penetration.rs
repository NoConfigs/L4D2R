//! Retail bullet penetration (server.dll).
//!
//! CTerrorPlayer::FireBullet (103c7470) gives every bullet the weapon's
//! PenetrationNumLayers, PenetrationPower, PenetrationMaxDistance and
//! CharacterPenetrationMaxDistance (weapon info 0xc48..0xc54, parsed at
//! 103f2fb0). The trace loop (103c6bf0) walks the hits along the ray in
//! order, and the per-hit handler (103c4b50) decides for each one:
//! - Every hit takes the bullet's damage (distance falloff only: the
//!   material damage modifier is computed but never used).
//! - A Tank (zombie class 8) stops the bullet.
//! - Beyond PenetrationMaxDistance (when non-zero) the bullet stops.
//! - A character within CharacterPenetrationMaxDistance (when non-zero) is
//!   passed through without using a layer: shotgun pellets within 300
//!   units, sniper rounds within 8192.
//! - Otherwise each hit uses a layer, and on the last layer the bullet
//!   stops. Before that a wall is passed only if its far side is found 12
//!   or 24 units in (103c45c0) and its thickness is at most
//!   PenetrationPower x the entry material's modifier (1024b680), doubled
//!   when both faces are the same wood or metal.
//! - The loop also ends once the exit point is past the weapon's range.
//! Characters are thinner than any PenetrationPower in the shipped
//! scripts (30 or 50), so a body never stops a bullet by its thickness.
use bevy::prelude::*;
use std::sync::RwLock;

use crate::collision::UNIT;

/// A weapon's penetration values, distances in rewrite units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Penetration {
    pub layers: u32,
    pub power: f32,
    pub max_distance: f32,
    pub character_distance: f32,
}
impl Penetration {
    /// From the weapon script keys (Source units).
    pub fn from_script(number: impl Fn(&str, f32) -> f32) -> Self {
        Self {
            layers: number("PenetrationNumLayers", 1.).max(0.) as u32,
            power: number("PenetrationPower", 0.).max(0.) * UNIT,
            max_distance: number("PenetrationMaxDistance", 0.).max(0.) * UNIT,
            character_distance: number("CharacterPenetrationMaxDistance", 0.).max(0.) * UNIT,
        }
    }
}

/// Penetration modifier of a game material (server 1024b680).
pub fn modifier(material: u8) -> f32 {
    match material {
        b'C' | b'P' => 0.4,
        b'D' | b'M' | b'V' => 0.5,
        b'T' => 0.65,
        _ => 1.,
    }
}

/// A world surface on the bullet's path.
#[derive(Clone, Copy, Debug)]
pub struct Wall {
    /// Distance from the muzzle to the entry face.
    pub entry: f32,
    pub normal: Vec3,
    /// Distance to the exit face, if one was found 12 or 24 units in.
    pub exit: Option<f32>,
    pub material: u8,
    pub exit_material: u8,
}

/// A character the ray touches.
#[derive(Clone, Copy, Debug)]
pub struct Target<T> {
    pub distance: f32,
    /// Zombie class 8: the bullet stops here.
    pub tank: bool,
    /// Armour that absorbs the hit (riot infected from the front).
    pub absorbs: bool,
    pub value: T,
}

/// What one bullet hit.
#[derive(Debug)]
pub struct Path<T> {
    /// Characters hit, nearest first, with their distances.
    pub hits: Vec<(f32, T)>,
    /// Wall faces the bullet struck or left: (distance, normal).
    pub marks: Vec<(f32, Vec3)>,
    /// Where the bullet stopped.
    pub end: f32,
}

const STEP: f32 = 12.;
const PROBE: f32 = 24.;

/// Walks one bullet: `targets` are every character on the ray (any order);
/// `wall(from)` finds the next world surface past distance `from`.
pub fn walk<T>(
    pen: Penetration,
    range: f32,
    mut targets: Vec<Target<T>>,
    mut wall: impl FnMut(f32) -> Option<Wall>,
) -> Path<T> {
    targets.retain(|t| t.distance <= range);
    targets.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let mut targets = targets.into_iter().peekable();
    let mut layers = pen.layers.max(1);
    let mut from = 0.;
    let mut path = Path { hits: Vec::new(), marks: Vec::new(), end: range };
    // Retail stops the multi-trace after 16 traces.
    for _ in 0..15 {
        let next = wall(from).filter(|w| w.entry < range);
        let limit = next.map_or(range, |w| w.entry);
        while let Some(t) = targets.next_if(|t| t.distance < limit) {
            let d = t.distance;
            let (tank, absorbs) = (t.tank, t.absorbs);
            path.hits.push((d, t.value));
            if tank || absorbs || (pen.max_distance > 0. && d > pen.max_distance) {
                path.end = d;
                return path;
            }
            if pen.character_distance > 0. && d <= pen.character_distance {
                continue;
            }
            // A body is thinner than any shipped PenetrationPower, but
            // upgraded rounds have none (103c7470) and stop in it.
            if layers <= 1 || pen.power <= 0. {
                path.end = d;
                return path;
            }
            layers -= 1;
        }
        let Some(w) = next else {
            return path;
        };
        path.marks.push((w.entry, w.normal));
        path.end = w.entry;
        if layers <= 1 || (pen.max_distance > 0. && w.entry > pen.max_distance) {
            return path;
        }
        let Some(exit) = w.exit else {
            return path;
        };
        let mut allowed = pen.power * modifier(w.material);
        if w.material == w.exit_material && matches!(w.material, b'W' | b'M') {
            allowed *= 2.;
        }
        if exit - w.entry > allowed || exit >= range {
            return path;
        }
        layers -= 1;
        path.marks.push((exit, -w.normal));
        // Skip any characters inside the wall.
        while targets.next_if(|t| t.distance < exit).is_some() {}
        from = exit + 0.01 * UNIT;
        path.end = range;
    }
    path
}

/// The next world surface on a ray, using the native shot tracer.
pub fn world_wall(
    collision: &crate::collision::Collision,
    origin: Vec3,
    ray: Vec3,
    range: f32,
) -> impl FnMut(f32) -> Option<Wall> + '_ {
    move |from| {
        let start = origin + ray * from;
        let t = collision.trace(start, origin + ray * range, Vec3::ZERO);
        if t.fraction >= 1. {
            return None;
        }
        let entry = from + t.fraction * (range - from);
        let hit = origin + ray * entry;
        let material = material_between(collision, hit - ray * UNIT, hit + ray * UNIT);
        let mut exit = None;
        let mut exit_material = 0;
        let mut step = STEP;
        while step <= PROBE {
            let probe = hit + ray * step * UNIT;
            let back = collision.trace(probe, hit, Vec3::ZERO);
            if !back.start_solid {
                let at = entry + step * UNIT * (1. - back.fraction);
                exit = Some(at);
                let face = origin + ray * at;
                exit_material = material_between(collision, face + ray * UNIT, face - ray * UNIT);
                break;
            }
            step += STEP;
        }
        Some(Wall { entry, normal: t.normal, exit, material, exit_material })
    }
}

fn material_between(collision: &crate::collision::Collision, a: Vec3, b: Vec3) -> u8 {
    let source = |p: Vec3| [p.x / UNIT, -p.z / UNIT, p.y / UNIT];
    collision
        .source_ray(source(a), source(b), l4d2r_game_core::trace::MASK_SHOT, true)
        .and_then(|(_, _, texinfo, _)| texinfo)
        .map_or(0, world_material)
}

/// Game material (surfaceproperties "gamematerial") of each surface name.
pub fn game_materials(assets: &mut crate::source_assets::SourceAssets, names: &[String]) -> Vec<u8> {
    let Ok(registry) = crate::ivp_surface_registry::Registry::load(assets) else {
        return vec![0; names.len()];
    };
    names
        .iter()
        .map(|name| {
            registry
                .get(name)
                .or_else(|| registry.get("default"))
                .map_or(0, |r| (r.words[19] & 0xff) as u8)
        })
        .collect()
}

static WORLD: RwLock<Vec<u8>> = RwLock::new(Vec::new());

/// Records the game material of each BSP texinfo for the loaded map.
pub fn set_world_materials(materials: Vec<u8>) {
    if let Ok(mut world) = WORLD.write() {
        *world = materials;
    }
}

fn world_material(texinfo: usize) -> u8 {
    WORLD.read().ok().and_then(|w| w.get(texinfo).copied()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rifle() -> Penetration {
        Penetration { layers: 2, power: 50. * UNIT, max_distance: 0., character_distance: 0. }
    }
    fn target(d: f32, id: u32) -> Target<u32> {
        Target { distance: d * UNIT, tank: false, absorbs: false, value: id }
    }
    fn no_wall(_: f32) -> Option<Wall> {
        None
    }
    fn ids(p: &Path<u32>) -> Vec<u32> {
        p.hits.iter().map(|h| h.1).collect()
    }

    #[test]
    fn rifle_passes_one_common_and_stops_in_the_second() {
        let p = walk(rifle(), 3000. * UNIT, vec![target(300., 3), target(100., 1), target(200., 2)], no_wall);
        assert_eq!(ids(&p), vec![1, 2]);
        assert!((p.end - 200. * UNIT).abs() < 1e-4);
    }

    #[test]
    fn sniper_passes_every_common_but_stops_at_a_tank() {
        let pen = Penetration { character_distance: 8192. * UNIT, ..rifle() };
        let mut t = vec![target(100., 1), target(200., 2), target(300., 3), target(400., 4)];
        let p = walk(pen, 8192. * UNIT, t.clone(), no_wall);
        assert_eq!(ids(&p), vec![1, 2, 3, 4]);
        t[1].tank = true;
        assert_eq!(ids(&walk(pen, 8192. * UNIT, t, no_wall)), vec![1, 2]);
    }

    #[test]
    fn shotgun_pellets_pass_commons_only_up_close() {
        let pen = Penetration { layers: 2, power: 30. * UNIT, max_distance: 500. * UNIT, character_distance: 300. * UNIT };
        let near = walk(pen, 3000. * UNIT, vec![target(50., 1), target(100., 2), target(150., 3)], no_wall);
        assert_eq!(ids(&near), vec![1, 2, 3]);
        let far = walk(pen, 3000. * UNIT, vec![target(600., 1), target(650., 2)], no_wall);
        assert_eq!(ids(&far), vec![1]);
        let middle = walk(pen, 3000. * UNIT, vec![target(350., 1), target(400., 2), target(450., 3)], no_wall);
        assert_eq!(ids(&middle), vec![1, 2]);
    }

    #[test]
    fn walls_pass_by_thickness_and_material() {
        let wall = |thick: f32, material: u8| {
            move |from: f32| {
                (from < 100. * UNIT).then_some(Wall {
                    entry: 100. * UNIT,
                    normal: Vec3::X,
                    exit: Some((100. + thick) * UNIT),
                    material,
                    exit_material: material,
                })
            }
        };
        let behind = || vec![target(200., 7)];
        // 16-unit concrete: 50 x 0.4 = 20.
        assert_eq!(ids(&walk(rifle(), 3000. * UNIT, behind(), wall(16., b'C'))), vec![7]);
        assert!(walk(rifle(), 3000. * UNIT, behind(), wall(22., b'C')).hits.is_empty());
        // Wood both sides doubles: 50 x 1 x 2.
        assert_eq!(ids(&walk(rifle(), 3000. * UNIT, behind(), wall(24., b'W'))), vec![7]);
        // A pistol (30) through 16-unit concrete (12) does not.
        let pistol = Penetration { power: 30. * UNIT, ..rifle() };
        assert!(walk(pistol, 2500. * UNIT, behind(), wall(16., b'C')).hits.is_empty());
        // No far side within 24 units: blocked.
        let thick = |from: f32| {
            (from < 100. * UNIT).then_some(Wall { entry: 100. * UNIT, normal: Vec3::X, exit: None, material: b'W', exit_material: 0 })
        };
        assert!(walk(rifle(), 3000. * UNIT, behind(), thick).hits.is_empty());
    }

    #[test]
    fn a_common_then_a_wall_uses_both_layers() {
        let wall = |from: f32| {
            (from < 150. * UNIT).then_some(Wall { entry: 150. * UNIT, normal: Vec3::X, exit: Some(152. * UNIT), material: b'W', exit_material: b'W' })
        };
        let p = walk(rifle(), 3000. * UNIT, vec![target(100., 1), target(200., 2)], wall);
        assert_eq!(ids(&p), vec![1]);
        assert!((p.end - 150. * UNIT).abs() < 1e-4);
    }

    #[test]
    fn modifiers_match_server() {
        assert_eq!(modifier(b'C'), 0.4);
        assert_eq!(modifier(b'D'), 0.5);
        assert_eq!(modifier(b'M'), 0.5);
        assert_eq!(modifier(b'P'), 0.4);
        assert_eq!(modifier(b'T'), 0.65);
        assert_eq!(modifier(b'V'), 0.5);
        assert_eq!(modifier(b'W'), 1.);
        assert_eq!(modifier(b'F'), 1.);
    }

    #[test]
    fn retail_scripts_and_surfaces_load() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let p = |assets: &mut _, class| crate::game_state::Weapon::load(assets, class).unwrap().penetration;
        assert_eq!(p(&mut assets, "weapon_rifle"), Penetration { layers: 2, power: 50. * UNIT, max_distance: 0., character_distance: 0. });
        assert_eq!(p(&mut assets, "weapon_pumpshotgun"), Penetration { layers: 2, power: 30. * UNIT, max_distance: 500. * UNIT, character_distance: 300. * UNIT });
        assert_eq!(p(&mut assets, "weapon_sniper_military").character_distance, 8192. * UNIT);
        let names = ["concrete", "wood", "metal", "tile", "dirt", "flesh", "plaster", "metalvent"].map(String::from);
        let m = game_materials(&mut assets, &names);
        assert_eq!(&m[..6], b"CWMTDF");
        eprintln!("plaster {} metalvent {}", m[6] as char, m[7] as char);
    }
}
