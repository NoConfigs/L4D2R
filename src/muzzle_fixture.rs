//! Opt-in render comparisons through the actual weapon combat/animation path.
use crate::game_state::{Gun, Match, Rules};
use bevy::prelude::*;
const WEAPONS: &[&str] = &[
    "pistol",
    "pistol_dual",
    "pistol_magnum",
    "smg",
    "smg_silenced",
    "smg_mp5",
    "rifle",
    "rifle_ak47",
    "rifle_desert",
    "rifle_sg552",
    "pumpshotgun",
    "shotgun_chrome",
    "autoshotgun",
    "shotgun_spas",
    "hunting_rifle",
    "sniper_military",
    "sniper_awp",
    "sniper_scout",
    "rifle_m60",
    "grenade_launcher",
];
#[derive(Resource)]
struct Suite {
    index: usize,
    serial: u64,
    shots: usize,
    pending: Vec<(u8, String)>,
}
pub fn install(app: &mut App) {
    if !std::env::args().any(|a| a == "--capture-muzzle-suite") {
        return;
    }
    std::fs::create_dir_all("local-research/parity/rewrite-muzzle-families")
        .expect("Muzzle capture output");
    app.insert_resource(Suite {
        index: usize::MAX,
        serial: 0,
        shots: 0,
        pending: vec![],
    })
    .add_systems(Update, stage.before(crate::game::combat))
    .add_systems(Update, capture.after(crate::game::combat));
}
fn stage(
    mut suite: ResMut<Suite>,
    mut state: ResMut<Match>,
    rules: Res<Rules>,
    warmup: Option<Res<crate::render_warmup::Warmup>>,
) {
    if warmup.is_some() {
        return;
    }
    let index = (state.elapsed.max(0.) / 2.) as usize;
    if index == suite.index || index >= WEAPONS.len() {
        return;
    }
    let class = format!("weapon_{}", WEAPONS[index]);
    let Some(weapon) = rules.weapons.get(&class) else {
        suite.index = index;
        eprintln!("Muzzle comparison unavailable: {class} is missing from the gameplay catalog");
        return;
    };
    let gun = Gun {
        class,
        ammo: weapon.clip,
        reserve: 100,
        laser: false,
        ammo_upgrade: None,
        upgraded_rounds: 0,
    };
    if weapon.infinite() {
        state.pistol = gun;
        state.slot = 2;
    } else {
        state.primary = Some(gun);
        state.slot = 1;
    }
    state.reload = 0.;
    // Let the actual draw sequence settle before the first comparison shot.
    state.cooldown = 1.2;
    state.action = 0.;
    state.shove = 0.;
    state.burst_left = 0;
    suite.index = index;
    suite.shots = 0;
    suite.serial = state.shot_serial;
    println!("Muzzle comparison: {}", WEAPONS[index]);
}
fn capture(
    mut commands: Commands,
    mut suite: ResMut<Suite>,
    state: Res<Match>,
    image: Option<Res<crate::viewer::CaptureImage>>,
) {
    let Some(image) = image else {
        return;
    };
    suite.pending.retain_mut(|(delay, path)| {
        if *delay > 0 {
            *delay -= 1;
            return true;
        }
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::image(
                image.image.clone(),
            ))
            .observe(bevy::render::view::screenshot::save_to_disk(path.clone()));
        false
    });
    if suite.index >= WEAPONS.len() || suite.serial == state.shot_serial {
        return;
    }
    if state.held_class() != format!("weapon_{}", WEAPONS[suite.index]) {
        return;
    }
    suite.serial = state.shot_serial;
    if suite.shots >= 3 {
        return;
    }
    let path = format!(
        "local-research/parity/rewrite-muzzle-families/{}-{}.png",
        WEAPONS[suite.index], suite.shots
    );
    suite.shots += 1;
    // The offscreen target/readback path can contain the preceding rendered
    // image. Keep both following samples instead of treating a pre-shot frame
    // as proof that no effect was rendered.
    suite.pending.push((0, path.clone()));
    suite.pending.push((1, path.replace(".png", "-next.png")));
}
