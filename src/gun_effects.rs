//! Bounded weapon-script PCF muzzle emitters, driven by actual shot events.
use bevy::prelude::*;
use l4d2r_game_core::{
    entities::{Ent, Kind},
    mapents::MapEnt,
};
#[derive(Resource, Default)]
pub struct Shots {
    serial: u64,
    pending: Vec<(usize, f32)>,
    cursor: usize,
    anchors: std::collections::HashMap<usize, (String, bool)>,
}
pub fn setup(
    mut commands: Commands,
    rules: Res<crate::game_state::Rules>,
    state: Res<crate::game_state::Match>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
) {
    crate::upgrades::prepare(&mut logic.world);
    let mut names: Vec<_> = rules
        .weapons
        .values()
        .filter(|w| !w.muzzle_effect.is_empty())
        .map(|w| w.muzzle_effect.clone())
        .collect();
    names.sort();
    names.dedup();
    for fx in names {
        for slot in 0..2 {
            let name = format!("__rewrite_muzzle_fp_{fx}_{slot}");
            if !logic.world.find(&name).is_empty() {
                continue;
            }
            let index = logic.world.ents.len();
            logic.world.add_runtime_entity(Ent {
                me: MapEnt {
                    index,
                    classname: "info_particle_system".into(),
                    targetname: name.clone(),
                    origin: l4d2r_math::Vec3::ZERO,
                    angles: l4d2r_math::Vec3::ZERO,
                    mdl: None,
                    brush: None,
                    ent: l4d2_formats::bsp::Entity(vec![
                        ("classname".into(), "info_particle_system".into()),
                        ("targetname".into(), name),
                        ("effect_name".into(), fx.clone()),
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
    commands.insert_resource(Shots {
        serial: state.shot_serial,
        ..Default::default()
    });
}
pub fn update(
    mut shots: ResMut<Shots>,
    state: Res<crate::game_state::Match>,
    rules: Res<crate::game_state::Rules>,
    frontend: Res<crate::game_menu::Frontend>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    playback: Res<crate::l4d2r_animation::PlaybackState>,
    roots: Query<(Entity, &crate::game::HeldWeapon, &GlobalTransform)>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    effects: Res<crate::source_particles::Effects>,
) {
    let now = logic.world.time;
    let hide = !frontend.playing() || state.dead || state.infected || state.slot > 2;
    shots.pending.retain(|(id, until)| {
        if hide || now >= *until {
            if let Some(e) = logic.world.ents.get_mut(*id) {
                e.kind = Kind::Particle {
                    active: false,
                    started: now,
                };
            }
            false
        } else {
            true
        }
    });
    let pending: Vec<usize> = shots.pending.iter().map(|p| p.0).collect();
    shots.anchors.retain(|id, (class, second)| {
        if hide || (!pending.contains(id) && !effects.owner_has_particles(*id)) {
            return false;
        }
        if class.as_str() == state.held_class() {
            if let (Some(w), Some((entity, _, root))) = (
                rules.weapons.get(class),
                roots
                    .iter()
                    .find(|(_, held, _)| held.0.as_str() == class.as_str()),
            ) {
                if let Some((position, direction, up)) =
                    library.muzzle(&w.view_model, entity, root, &playback, *second)
                {
                    let e = &mut logic.world.ents[*id];
                    e.me.origin =
                        l4d2r_math::Vec3::new(position.x, -position.z, position.y) / 0.0254;
                    e.me.angles = muzzle_angles(direction, up);
                }
            }
        }
        true
    });
    if shots.serial == state.shot_serial {
        return;
    }
    shots.serial = state.shot_serial;
    if hide || state.paused {
        return;
    }
    let w = rules
        .weapons
        .get(state.held_class())
        .unwrap_or(&rules.pistol);
    if w.melee() || w.muzzle_effect.is_empty() {
        return;
    }
    let Some((entity, _, root)) = roots.iter().find(|(_, held, _)| held.0 == w.class) else {
        return;
    };
    let Some((position, direction, up)) = library.muzzle(
        &w.view_model,
        entity,
        root,
        &playback,
        w.class == "weapon_pistol_dual" && state.gun().ammo % 2 == 0,
    ) else {
        return;
    };
    let slot = shots.cursor % 2;
    shots.cursor += 1;
    if let Some(&id) = logic
        .world
        .find(&format!("__rewrite_muzzle_fp_{}_{slot}", w.muzzle_effect))
        .first()
    {
        let e = &mut logic.world.ents[id];
        e.me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / 0.0254;
        e.me.angles = muzzle_angles(direction, up);
        e.kind = Kind::Particle {
            active: true,
            started: now,
        };
        shots.pending.retain(|(old, _)| *old != id);
        shots.pending.push((id, now + 0.045));
        shots.anchors.insert(
            id,
            (
                w.class.clone(),
                w.class == "weapon_pistol_dual" && state.gun().ammo % 2 == 0,
            ),
        );
    }
}
fn muzzle_angles(direction: Vec3, up: Vec3) -> l4d2r_math::Vec3 {
    let d = l4d2r_math::Vec3::new(direction.x, -direction.z, direction.y).normalize_or_zero();
    let up = l4d2r_math::Vec3::new(up.x, -up.z, up.y).normalize_or_zero();
    let mut angles = l4d2r_math::Vec3::new(
        (-d.z).atan2(d.truncate().length()).to_degrees(),
        d.y.atan2(d.x).to_degrees(),
        0.,
    );
    let basis = l4d2r_game_core::mapents::angle_matrix(angles);
    angles.z = (-up.dot(basis.y_axis.truncate()))
        .atan2(up.dot(basis.z_axis.truncate()))
        .to_degrees();
    angles
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn muzzle_control_point_keeps_attachment_roll() {
        for degrees in [[-40., 120., 65.], [0., -90., -25.], [89., 160., 90.]] {
            let matrix =
                l4d2r_game_core::mapents::angle_matrix(l4d2r_math::Vec3::from_array(degrees));
            let to_render = |v: l4d2r_math::Vec3| Vec3::new(v.x, v.z, -v.y);
            let reconstructed = l4d2r_game_core::mapents::angle_matrix(muzzle_angles(
                to_render(matrix.x_axis.truncate()),
                to_render(matrix.z_axis.truncate()),
            ));
            assert!(matrix.x_axis.abs_diff_eq(reconstructed.x_axis, 1e-5));
            assert!(matrix.y_axis.abs_diff_eq(reconstructed.y_axis, 1e-5));
            assert!(matrix.z_axis.abs_diff_eq(reconstructed.z_axis, 1e-5));
        }
    }
}
