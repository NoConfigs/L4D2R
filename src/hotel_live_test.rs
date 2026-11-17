//! Opt-in visible checkpoint smoke test; never enabled by normal launchers.
use crate::{
    campaign_settings::SessionSettings,
    collision::{Collision, HEIGHT},
    game::Body,
    game_menu::{Frontend, Page},
    game_state::{Match, Squad},
    l4d2r_cinematic::MapLogic,
};
use bevy::prelude::*;
#[derive(Default)]
pub(crate) struct Tour {
    clock: f32,
    stage: Option<usize>,
    pressed: bool,
    opened: bool,
    closed: bool,
    done: bool,
}
pub(crate) fn update(
    time: Res<Time<Fixed>>,
    mut tour: Local<Tour>,
    mut state: ResMut<Match>,
    mut body: Single<&mut Body>,
    mut squad: ResMut<Squad>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut logic: ResMut<MapLogic>,
    collision: Res<Collision>,
    session: Res<SessionSettings>,
    frontend: Res<Frontend>,
) {
    if tour.done || frontend.page == Page::Loading {
        return;
    }
    if session.map.file_stem().is_none_or(|n| n != "c1m1_hotel") {
        println!("LIVE HOTEL TOUR: next chapter loaded in existing session");
        tour.done = true;
        keys.release(KeyCode::KeyW);
        return;
    }
    state.paused = false;
    state.health = 100.;
    state.dead = false;
    state.incap = false;
    if logic.active {
        return;
    }
    tour.clock += time.delta_secs();
    let t = tour.clock;
    let stage = if t < 3. {
        0
    } else if t < 8. {
        1
    } else if t < 12. {
        2
    } else if t < 44. {
        3
    } else if t < 50. {
        4
    } else if t < 55. {
        5
    } else {
        6
    };
    if tour.stage != Some(stage) {
        tour.stage = Some(stage);
        keys.release(KeyCode::KeyW);
        let (name, pos, yaw): (&str, [f32; 3], f32) = match stage {
            0 => ("roof", [556.143, 5504.9, 2848.], 180.),
            1 => ("first stairs", [248., 5660., 2848.], 90.),
            2 => ("room 708 hallway", [1830., 6280., 2464.], 90.),
            3 => ("elevator boarding", [2168., 5800., 2464.], 270.),
            4 => ("lower fire corridor", [-400., 5500., 1184.], 270.),
            5 => ("safe-room entrance", [1830., 4700., 1184.], 270.),
            _ => ("safe-room interior", [2110., 4472., 1184.], 270.),
        };
        let target = Vec3::from_array(crate::binary::source_position(pos));
        match collision.supported_spawn_near(target, HEIGHT) {
            Ok(p) => {
                body.feet = p;
                body.previous_feet = p;
                body.velocity = Vec3::ZERO;
                body.grounded = true;
                body.yaw = yaw.to_radians() - std::f32::consts::FRAC_PI_2;
                body.pitch = 0.;
                for (i, m) in squad.0.iter_mut().enumerate() {
                    m.position = p + Vec3::X * (i as f32 - 1.) * 0.45 + Vec3::Z * 0.45;
                    m.vertical = 0.;
                    m.state.resuscitate();
                    m.state.health = 100.;
                }
                println!("LIVE HOTEL TOUR: t={t:.2} {name} at {:?}", body.feet);
            }
            Err(e) => {
                println!("LIVE HOTEL TOUR: unsupported checkpoint {name}: {e}");
                tour.done = true;
                return;
            }
        }
        if stage == 5 {
            if let Some(&id) = logic.world.find("checkpoint_entrance").first() {
                logic.world.input(id, "Open", "", None, None);
            }
        }
    }
    if (3.0..4.2).contains(&t) {
        keys.press(KeyCode::KeyW);
    } else {
        keys.release(KeyCode::KeyW);
    }
    if stage == 3 && !tour.pressed && t > 13. {
        if let Some(&id) = logic.world.find("elevator_button").first() {
            if matches!(
                logic.world.ents[id].kind,
                l4d2r_game_core::entities::Kind::Button { locked: false, .. }
            ) {
                logic.world.use_ent(id, None);
                tour.pressed = true;
                println!("LIVE HOTEL TOUR: authored elevator button pressed");
            }
        }
    }
    if stage == 3 && t > 42. && !tour.opened {
        if let Some(&id) = logic.world.find("elevator_door_button1").first() {
            if matches!(
                logic.world.ents[id].kind,
                l4d2r_game_core::entities::Kind::Button { locked: false, .. }
            ) {
                logic.world.use_ent(id, None);
                tour.opened = true;
                println!(
                    "LIVE HOTEL TOUR: lower elevator button used; feet={:?}",
                    body.feet
                );
            }
        }
    }
    if stage == 6 && t > 56. && !tour.closed {
        if let Some(&id) = logic.world.find("checkpoint_entrance").first() {
            logic.world.input(id, "Close", "", None, None);
            tour.closed = true;
            println!("LIVE HOTEL TOUR: checkpoint door close requested");
        }
    }
    if t > 65. {
        println!(
            "LIVE HOTEL TOUR: checkpoint transition not completed: {}",
            state.hint
        );
        tour.done = true;
    }
}
