//! Opt-in runtime observation for comparisons with the original game.
//! Values are observations; they are not automatically classified as parity.
use crate::{
    game::{Body, Enemy, HeldWeapon},
    game_state::{Match, Rules},
    l4d2r_animation::{AnimationLibrary, PlaybackState},
};
use bevy::prelude::*;
use std::{fs, path::PathBuf};
#[derive(Resource)]
pub(crate) struct MovementFixture {
    pub case: String,
    pub jumped: bool,
}
#[derive(Resource)]
struct Probe {
    path: PathBuf,
    rows: String,
    next: f32,
    finished: bool,
    door_enabled: bool,
    door: Option<usize>,
    door_opened: bool,
    door_closed: bool,
    door_rows: String,
}
pub fn install(app: &mut App) {
    let args = std::env::args().collect::<Vec<_>>();
    if let Some(case) = args
        .windows(2)
        .find(|a| a[0] == "--parity-movement")
        .map(|a| a[1].clone())
    {
        app.insert_resource(MovementFixture {
            case,
            jumped: false,
        });
    }
    let Some(path) = args
        .windows(2)
        .find(|a| a[0] == "--parity-output")
        .map(|a| PathBuf::from(&a[1]))
    else {
        return;
    };
    app.insert_resource(Probe {path,rows:"time,weapon,clip,reserve,reloading,reload_remaining,animation_label,animation_clock,animation_duration,source_x,source_y,source_z,common_alive,common_corpses,common_limit,pending\n".into(),next:0.,finished:false,
        door_enabled:args.iter().any(|a| a=="--parity-door"),door:None,door_opened:false,door_closed:false,door_rows:"time,yaw,state,speed,distance\n".into()})
        .add_systems(Update,record.after(crate::l4d2r_animation::update));
}
fn record(
    state: Res<Match>,
    rules: Res<Rules>,
    body: Single<&Body>,
    camera: Single<&Transform, With<crate::viewer::GameCamera>>,
    library: Res<AnimationLibrary>,
    playback: Res<PlaybackState>,
    held: Query<(Entity, &HeldWeapon)>,
    enemies: Query<&Enemy>,
    director: Res<crate::source_director::Director>,
    mut probe: ResMut<Probe>,
    warmup: Option<Res<crate::render_warmup::Warmup>>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
) {
    if probe.finished || state.paused || warmup.is_some() || state.elapsed + 1e-6 < probe.next {
        return;
    }
    probe.next = state.elapsed + 1. / 30.;
    if probe.door_enabled {
        use l4d2r_game_core::entities::{DoorState, Kind};
        if probe.door.is_none() {
            let pos = l4d2r_math::Vec3::new(1875.959961, 6364.080078, 2464.010010);
            probe.door = logic
                .world
                .ents
                .iter()
                .enumerate()
                .find(|(_, e)| {
                    e.me.classname == "prop_door_rotating" && e.me.origin.distance(pos) < 1.
                })
                .map(|(id, _)| id);
        }
        if let Some(id) = probe.door {
            if !probe.door_opened && state.elapsed >= 0.2 {
                logic.world.input(id, "Open", "", None, None);
                probe.door_opened = true;
            }
            if !probe.door_closed && state.elapsed >= 2. {
                logic.world.input(id, "Close", "", None, None);
                probe.door_closed = true;
            }
            let e = &logic.world.ents[id];
            if let Kind::Door(d) = &e.kind {
                let door_state = match d.state {
                    DoorState::Closed => 0,
                    DoorState::Opening => 1,
                    DoorState::Open => 2,
                    DoorState::Closing => 3,
                };
                probe.door_rows.push_str(&format!(
                    "{:.6},{:.6},{},{:.6},{:.6}\n",
                    state.elapsed,
                    e.me.angles.y + d.dist * d.frac,
                    door_state,
                    d.speed,
                    d.dist
                ));
            }
        }
    }
    let gun = state.gun();
    let weapon = rules.weapons.get(&gun.class).unwrap_or(&rules.pistol);
    let animation = held
        .iter()
        .find(|(_, w)| w.0 == gun.class)
        .and_then(|(id, _)| library.playback_info(&weapon.view_model, id, &playback));
    let (label, clock, duration) = animation.unwrap_or_else(|| ("unavailable".into(), 0., 0.));
    let pos = body.feet / 0.0254;
    if !probe.rows.contains("velocity_x") {
        probe.rows=probe.rows.replacen("common_limit,pending\n","common_limit,pending,velocity_x,velocity_y,velocity_z,eye_height,grounded,crouched,hull_height,camera_z\n",1);
    }
    probe.rows.push_str(&format!(
        "{:.6},{},{},{},{},{:.6},{},{:.6},{:.6},{:.4},{:.4},{:.4},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{},{},{:.4},{:.6}\n",
        state.elapsed,
        gun.class,
        gun.ammo,
        gun.reserve,
        u8::from(state.reload > 0.),
        state.reload,
        label,
        clock,
        duration,
        pos.x,
        -pos.z,
        pos.y,
        enemies.iter().filter(|e| e.health > 0.).count(),
        enemies.iter().filter(|e| e.health <= 0.).count(),
        director.limit(),
        director.pending,
        body.velocity.x/0.0254,-body.velocity.z/0.0254,body.velocity.y/0.0254,
        body.stance.eye()/0.0254,u8::from(body.grounded),u8::from(body.crouched),
        (if body.crouched {crate::collision::DUCK_HEIGHT} else {crate::collision::HEIGHT})/0.0254,
        camera.translation.y/0.0254
    ));
    let duration = if std::env::args().any(|a| a == "--capture-hotel-elevator") {
        32.
    } else {
        4.
    };
    if state.elapsed >= duration {
        if let Some(parent) = probe.path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Parity output directory: {e}");
            }
        }
        match fs::write(&probe.path, &probe.rows) {
            Ok(_) => println!("Parity trace: {}", probe.path.display()),
            Err(e) => eprintln!("Parity trace: {e}"),
        }
        if probe.door_enabled {
            let path = probe.path.with_extension("door.csv");
            if let Err(e) = fs::write(&path, &probe.door_rows) {
                eprintln!("Door parity trace: {e}");
            }
            if probe.door.is_none() {
                eprintln!("Door parity fixture not found in this map");
            }
        }
        probe.finished = true;
    }
}
