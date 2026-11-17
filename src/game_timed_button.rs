//! func_button_timed: hold use on the button for its `use_time` (c9m2's
//! generator switch, c7m3's generators, c14m1, c13m1, c7m1). OnPressed when
//! the hold starts, OnUnPressed when it is let go early, OnTimeUp when the
//! time is up; `auto_disable` 1 disables the button afterwards.
use bevy::prelude::*;
use l4d2r_game_core::entities::Kind;

/// Survivor use range (96 units).
const USE_RANGE: f32 = 96. * crate::collision::UNIT;

#[derive(Default)]
pub(crate) struct Hold {
    target: Option<usize>,
    since: f32,
}

/// The enabled func_button_timed under the view ray within use range.
pub(crate) fn aimed(
    logic: &crate::l4d2r_cinematic::MapLogic,
    collision: &crate::collision::Collision,
    eye: Vec3,
    forward: Vec3,
) -> Option<usize> {
    let wall = collision.trace(eye, eye + forward * USE_RANGE, Vec3::ZERO).fraction * USE_RANGE;
    logic
        .world
        .ents
        .iter()
        .enumerate()
        .filter(|(_, e)| e.alive && matches!(e.kind, Kind::TimedButton { disabled: false }))
        .filter_map(|(id, _)| {
            let (lo, hi) = logic.bounds(id)?;
            let d = crate::ray_box(eye, forward, lo - Vec3::splat(0.05), hi + Vec3::splat(0.05))?;
            (d <= USE_RANGE && d <= wall + 0.3).then_some((id, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Advance a hold by `held` this tick; returns true when it completed.
pub(crate) fn step(
    hold: &mut Hold,
    world: &mut l4d2r_game_core::entities::EntityWorld,
    target: Option<usize>,
    held: bool,
    now: f32,
) -> Option<(String, f32)> {
    let wanted = target.filter(|_| held);
    if hold.target != wanted {
        if let Some(old) = hold.target.take() {
            world.fire_output_pub(old, "OnUnPressed");
        }
        if let Some(id) = wanted {
            hold.target = Some(id);
            hold.since = now;
            world.fire_output_pub(id, "OnPressed");
        }
    }
    let id = hold.target?;
    let me = &world.ents[id].me;
    let total = me.kv_f32("use_time").unwrap_or(5.).max(0.);
    // use_string is usually a localisation token (#L4D_INSTRUCTOR_...),
    // which has no text table here yet.
    let title = me
        .kv("use_string")
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .unwrap_or("Using")
        .to_uppercase();
    let progress = if total <= 0. { 1. } else { ((now - hold.since) / total).clamp(0., 1.) };
    if progress >= 1. {
        let disable = me.kv_i32("auto_disable").unwrap_or(1) != 0;
        hold.target = None;
        world.fire_output_pub(id, "OnTimeUp");
        if disable {
            world.input(id, "Disable", "", None, None);
        }
        return None;
    }
    Some((title, progress))
}

pub(crate) fn update(
    keys: Res<ButtonInput<KeyCode>>,
    mut s: ResMut<crate::game_state::Match>,
    body: Single<&crate::game::Body>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Res<crate::collision::Collision>,
    mut hold: Local<Hold>,
) {
    let Some(mut logic) = logic else { return };
    let able = !s.paused && !s.dead && !s.incap && !s.infected && !s.pinned;
    let eye = body.feet + Vec3::Y * body.stance.eye();
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let target = able.then(|| aimed(&logic, &collision, eye, forward)).flatten();
    let now = logic.world.time;
    s.hold_bar = step(&mut hold, &mut logic.world, target, able && keys.pressed(KeyCode::KeyE), now);
    if target.is_some() && s.hold_bar.is_none() && s.hint.is_empty() {
        s.hint = "Hold E to use".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c9m2_generator_switch_opens_the_lift_after_its_hold_time() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let Some(path) = ["left4dead2", "left4dead2_dlc3", "update"]
            .iter()
            .map(|d| root.join(d).join("maps/c9m2_lots.bsp"))
            .find(|p| p.is_file())
        else {
            return;
        };
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        // The generator breaks mid-finale and the switch is spawned then.
        logic.world.queue_input("generatorbutton_template", "ForceSpawn", "", 0., None);
        for _ in 0..6 {
            logic.world.tick(1. / 60.);
        }
        let switch = logic.world.find("generator_switch")[0];
        assert!(logic.world.ents[switch].alive, "the switch spawns from its template");
        assert!(matches!(logic.world.ents[switch].kind, Kind::TimedButton { .. }));
        let total = logic.world.ents[switch].me.kv_f32("use_time").unwrap_or(5.);
        let mut hold = Hold::default();
        // Let go half way: nothing opens.
        let mut t = 0.;
        while t < total * 0.5 {
            step(&mut hold, &mut logic.world, Some(switch), true, t);
            t += 0.1;
        }
        step(&mut hold, &mut logic.world, Some(switch), false, t);
        assert!(logic.world.ents[switch].alive);
        // A full hold fires OnTimeUp (the switch is killed, the lift opens).
        let start = t;
        let mut bar = None;
        while t < start + total + 0.2 {
            // Stop at completion (a held key would start a new hold).
            if !logic.world.ents[switch].alive { break; }
            let r = step(&mut hold, &mut logic.world, Some(switch), true, t);
            let done = r.is_none();
            bar = r.or(bar);
            t += 0.1;
            if done {
                break;
            }
        }
        for _ in 0..30 {
            logic.world.tick(1. / 60.);
        }
        assert!(bar.is_some_and(|(_, p)| p > 0.9), "the hold shows progress");
        assert!(!logic.world.ents[switch].alive, "OnTimeUp ran (generator_switch Kill)");
    }
}
