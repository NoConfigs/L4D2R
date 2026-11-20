//! Survivor bot flashlights. Retail bots switch theirs on once they leave
//! the starting checkpoint (see `l4d2r_animation::BotFlashlights`); this
//! draws them with the same shadowless spot light as the player's.
use crate::{game_state::Squad, l4d2r_animation::BotFlashlights};
use bevy::prelude::*;

#[derive(Component)]
pub struct BotFlashlight(pub usize);

/// Retail draws other survivors' flashlights from the held weapon:
/// roughly chest height (m) and this far in front of the body.
/// Flashlight beam. Peak brightness from the c1m1 stairwell corridor
/// (local-research/parity/flash-1030, about 150/255). The cone from a
/// matched live retail shot on c1m2's store floor (19 Nov, setpos -6960
/// -2400 392, setang 30 108 0): full brightness to about 10 degrees off the
/// aim, fading through 14-17 and gone by about 22 (retail's flashlight
/// texture over its 53-degree r_flashlightfov); 3..16 lit only half of it.
pub const LUMENS: f32 = 200.;
pub const INNER_ANGLE: f32 = 6.;
pub const OUTER_ANGLE: f32 = 22.;
const GUN_HEIGHT: f32 = 1.35;
const GUN_FORWARD: f32 = 0.4;

pub fn update(
    mut commands: Commands,
    squad: Option<Res<Squad>>,
    on: Option<Res<BotFlashlights>>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    mut lights: Query<(Entity, &BotFlashlight, &mut SpotLight, &mut Transform)>,
) {
    let (Some(squad), Some(on)) = (squad, on) else {
        return;
    };
    let playing = frontend.is_none_or(|f| f.page == crate::game_menu::Page::Game);
    let mut seen = vec![false; squad.0.len()];
    for (entity, light, mut spot, mut transform) in &mut lights {
        let Some(member) = squad.0.get(light.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        seen[light.0] = true;
        let lit = playing && on.0.get(light.0).copied().unwrap_or(false) && !member.state.dead;
        let intensity = if lit { LUMENS } else { 0. };
        if spot.intensity != intensity {
            spot.intensity = intensity;
        }
        if lit {
            let aim = member.aim.try_normalize().unwrap_or(Vec3::X);
            let flat = (aim * Vec3::new(1., 0., 1.)).try_normalize().unwrap_or(Vec3::X);
            let gun = member.position + Vec3::Y * GUN_HEIGHT + flat * GUN_FORWARD;
            let wanted = Transform::from_translation(gun).looking_to(aim, Vec3::Y);
            if *transform != wanted {
                *transform = wanted;
            }
        }
    }
    for (index, seen) in seen.into_iter().enumerate() {
        if !seen {
            commands.spawn((
                SpotLight {
                    intensity: 0.,
                    range: 25.,
                    radius: 0.02,
                    inner_angle: INNER_ANGLE.to_radians(),
                    outer_angle: OUTER_ANGLE.to_radians(),
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::default(),
                Visibility::Visible,
                bevy::camera::visibility::RenderLayers::from_layers(&[0, 1]),
                BotFlashlight(index),
            ));
        }
    }
}
