//! Multiplayer stage 2c: one game flow for everyone.
//!
//! - Map uses: every accepted +use on a door, button or finale radio
//!   (`EntityWorld::uses`, which also catches bots closing the checkpoint
//!   door and timed buttons) is sent to the other games, which replay it
//!   (`MapLogic::remote_use`). Entity indices match because every game loads
//!   the same map file.
//! - Mission lost: the host watches every survivor (its player, its bots and
//!   the remote players). When all are down or dead, everyone sees "The
//!   Survivors have been overwhelmed" and after `LOST_DELAY` the host
//!   restarts the chapter for all ("restart"). Online, a dead player waits
//!   for that (or for the next chapter) instead of restarting alone.
//! - Finale: the host's FinaleEscapeFinished ends the campaign for everyone
//!   ("won"); a guest's own map logic does not decide it.
use crate::multiplayer::Session;
use crate::net_play::NetPlay;
use bevy::prelude::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

/// Seconds from the last survivor going down to the shared restart.
/// Retail mission lost (server 10268ca0, case 1): a 7 s fade to black that
/// stays out (UTIL_ScreenFadeAll flags 0x1a), and the restart 7 s later.
const LOST_DELAY: f32 = 7.;
/// Starts retail's mission-lost fade on this game's screen.
fn fade_out(logic: &mut crate::l4d2r_cinematic::MapLogic) {
    let now = logic.world.time;
    logic.world.cine.fade = Some(l4d2r_game_core::entities::Fade {
        start: now,
        hold: 0.,
        duration: LOST_DELAY,
        color: [0.; 3],
        alpha: 255.,
        from: false,
        stay: true,
    });
}

static ONLINE: AtomicBool = AtomicBool::new(false);
static LOST: AtomicBool = AtomicBool::new(false);
static HOST_WON: AtomicBool = AtomicBool::new(false);
static RESTART: AtomicBool = AtomicBool::new(false);

/// A restart asked for by the player (console `restart`, the pause menu).
/// Online only the host may: it restarts everyone. False on a guest.
pub(crate) fn request_restart() -> bool {
    if crate::net_play::guest() {
        return false;
    }
    if crate::net_play::host() {
        RESTART.store(true, Ordering::Relaxed);
    }
    true
}

/// In a running online game (host with guests, or guest).
pub(crate) fn online() -> bool {
    ONLINE.load(Ordering::Relaxed)
}
/// Every survivor is down or dead (online).
pub(crate) fn lost() -> bool {
    LOST.load(Ordering::Relaxed)
}
/// Guest: the host's finale was won.
pub(crate) fn host_won() -> bool {
    HOST_WON.load(Ordering::Relaxed)
}

/// A use message: entity index and the opener (Source units).
pub(crate) fn use_json(id: usize, opener: Option<l4d2r_math::Vec3>) -> Value {
    json!({"k": "use", "e": id, "o": opener.map(|o| [o.x, o.y, o.z])})
}
pub(crate) fn parse_use(m: &Value) -> Option<(usize, Option<l4d2r_math::Vec3>)> {
    if m.get("k")?.as_str()? != "use" {
        return None;
    }
    let id = m.get("e")?.as_u64()? as usize;
    let opener = m.get("o").and_then(Value::as_array).and_then(|a| {
        Some(l4d2r_math::Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32))
    });
    Some((id, opener))
}

/// Whether every survivor is down or dead.
pub(crate) fn all_down(player: &crate::game_state::Match, squad: &crate::game_state::Squad) -> bool {
    let down = |m: &crate::game_state::Match| m.dead || m.incap;
    down(player) && squad.0.iter().all(|m| down(&m.state))
}

#[derive(Default)]
struct Clock {
    lost: f32,
    won_sent: bool,
    map: Option<std::path::PathBuf>,
}

#[allow(clippy::too_many_arguments)]
fn flow(
    mut net: ResMut<NetPlay>,
    session: Res<Session>,
    mut frontend: ResMut<crate::game_menu::Frontend>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    state: Option<ResMut<crate::game_state::Match>>,
    squad: Option<Res<crate::game_state::Squad>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
    time: Res<Time<Real>>,
    mut clock: Local<Clock>,
    mut fx: ResMut<NetFx>,
    mut round: ResMut<crate::survival::Round>,
    hints: Option<Res<crate::game_ui::NativeHints>>,
) {
    let host = crate::net_play::host();
    let guest = crate::net_play::guest();
    let on = (host || guest) && frontend.playing();
    ONLINE.store(on, Ordering::Relaxed);
    let map = settings.as_ref().map(|s| s.map.clone());
    if clock.map != map {
        // A new chapter: nothing lost or won yet.
        clock.map = map;
        clock.lost = 0.;
        clock.won_sent = false;
        HOST_WON.store(false, Ordering::Relaxed);
        LOST.store(false, Ordering::Relaxed);
    }
    let Some(mut logic) = logic else { return };
    let lost_text = hints.as_ref().map_or_else(
        || "The Survivors have been overwhelmed".to_string(),
        |h| h.caption("#L4D_Scenario_Survivors_Dead"),
    );
    if !on {
        logic.world.uses.clear();
        net.flow.clear();
        LOST.store(false, Ordering::Relaxed);
        // Solo: everyone down fades out and restarts the chapter, as retail.
        if let (Some(mut state), Some(squad)) = (state, squad) {
            // L4D2R_WIPE_AT=<seconds>: debug, the team counts as wiped then.
            let forced = std::env::var("L4D2R_WIPE_AT").ok().and_then(|v| v.parse::<f32>().ok())
                .is_some_and(|at| state.elapsed >= at);
            if frontend.playing() && !crate::survival::active() && !state.infected
                && (all_down(&state, &squad) || forced) && !state.paused
            {
                if clock.lost == 0. {
                    println!("Mission lost: every survivor is down; restarting in {LOST_DELAY} s");
                    fade_out(&mut logic);
                }
                clock.lost += time.delta_secs();
                state.hint = lost_text;
                if clock.lost >= LOST_DELAY {
                    clock.lost = 0.;
                    frontend.restart = true;
                }
            } else if clock.lost > 0. && !all_down(&state, &squad) {
                clock.lost = 0.;
            }
        }
        return;
    }
    // Our uses go to everyone else (the host relays a guest's).
    for (id, opener) in std::mem::take(&mut logic.world.uses) {
        net.outgoing.push(use_json(id, opener));
    }
    for (_, message) in std::mem::take(&mut net.flow) {
        match message.get("k").and_then(Value::as_str) {
            Some("use") => {
                if let (Some((id, opener)), Some(collision)) = (parse_use(&message), collision.as_ref()) {
                    logic.remote_use(id, opener, collision);
                }
            }
            Some("lost") if guest => {
                LOST.store(true, Ordering::Relaxed);
                fade_out(&mut logic);
            }
            Some("restart") if guest => {
                println!("Net flow: the host restarts the chapter");
                LOST.store(false, Ordering::Relaxed);
                frontend.restart = true;
            }
            Some("won") if guest => {
                println!("Net flow: the host's finale was won");
                HOST_WON.store(true, Ordering::Relaxed);
            }
            Some("spit" | "acid" | "rocks" | "gore" | "blood") if guest => fx.inbox.push(message),
            // The host's Survival round started / ended (survival.rs).
            Some("surv") if guest => {
                let f = |k: &str| message.get(k).and_then(Value::as_f64).map(|v| v as f32);
                crate::survival::remote_event(&mut round, time.elapsed_secs_f64(), f("s"), f("e"));
            }
            _ => {}
        }
    }
    if !host {
        return;
    }
    let _ = &session;
    if RESTART.swap(false, Ordering::Relaxed) {
        println!("Net flow: restarting the chapter for everyone");
        net.outgoing.push(json!({"k": "restart"}));
    }
    // Mission lost: everyone down or dead.
    if let (Some(mut state), Some(squad)) = (state, squad) {
        if all_down(&state, &squad) && !state.paused {
            if clock.lost == 0. {
                println!("Net flow: every survivor is down; restarting in {LOST_DELAY} s");
                net.outgoing.push(json!({"k": "lost"}));
                LOST.store(true, Ordering::Relaxed);
                fade_out(&mut logic);
            }
            clock.lost += time.delta_secs();
            state.hint = lost_text;
            if clock.lost >= LOST_DELAY {
                clock.lost = 0.;
                LOST.store(false, Ordering::Relaxed);
                net.outgoing.push(json!({"k": "restart"}));
                frontend.restart = true;
            }
        } else if clock.lost > 0. {
            clock.lost = 0.;
            LOST.store(false, Ordering::Relaxed);
        }
    }
    if logic.world.cine.finale_won && !clock.won_sent {
        clock.won_sent = true;
        net.outgoing.push(json!({"k": "won"}));
    }
}

/// Guest: the host's spit, acid and Tank rocks waiting for `effects`.
#[derive(Resource, Default)]
pub(crate) struct NetFx {
    inbox: Vec<Value>,
    rocks_sent: bool,
    rocks_clock: f64,
}

fn v3(v: Vec3) -> [f64; 3] {
    let r = |x: f32| (x as f64 * 100.).round() / 100.;
    [r(v.x), r(v.y), r(v.z)]
}
fn read_v3(m: &Value, key: &str) -> Option<Vec3> {
    let a = m.get(key)?.as_array()?;
    let v = Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32);
    v.is_finite().then_some(v)
}

/// Special-infected effects the host's AI makes, for guests to draw: Spitter
/// spit in flight and acid pools (guests' pools do no damage; the host's
/// game sends it), and Tank rocks held and flying (20 updates a second).
#[allow(clippy::too_many_arguments)]
fn effects(
    mut fx: ResMut<NetFx>,
    mut net: ResMut<NetPlay>,
    spit: Option<ResMut<crate::special_infected::spitter::Spit>>,
    rocks: Option<ResMut<crate::special_infected::tank::AiRocks>>,
    mut impacts: Option<ResMut<crate::bullet_impacts::Impacts>>,
    infected: Res<crate::net_infected::NetInfected>,
    time: Res<Time<Real>>,
) {
    let (Some(mut spit), Some(mut rocks)) = (spit, rocks) else { return };
    let host = crate::net_play::host() && online();
    let guest = crate::net_play::guest() && online();
    let launches = std::mem::take(&mut spit.net_launches);
    let pools = std::mem::take(&mut spit.net_pools);
    // Blood and wounds of the host's hits (anyone's, as the host's game
    // resolves every hit on its infected): guests show them too.
    let (blood, gore) = impacts
        .as_mut()
        .map(|i| (std::mem::take(&mut i.net_blood), std::mem::take(&mut i.net_gore)))
        .unwrap_or_default();
    if host {
        if !blood.is_empty() {
            let list: Vec<Value> = blood
                .iter()
                .map(|b| json!([v3(b.position), v3(b.direction), b.shotgun as u8]))
                .collect();
            net.outgoing.push(json!({"k": "blood", "l": list}));
        }
        let list: Vec<Value> = gore
            .iter()
            .filter_map(|h| {
                let id = infected.net_id(h.entity)?;
                Some(json!([
                    id,
                    v3(h.position),
                    v3(h.direction),
                    h.damage_force_source.map(v3),
                    h.box_index,
                    crate::net_infected::gore_cause_code(h.cause),
                    (h.distance * 100.).round() / 100.,
                    h.damage_type,
                    crate::infected_gore::pack_melee(h),
                    h.seed,
                ]))
            })
            .collect();
        if !list.is_empty() {
            net.outgoing.push(json!({"k": "gore", "l": list}));
        }
        for (p, v) in launches {
            net.outgoing.push(json!({"k": "spit", "p": v3(p), "v": v3(v)}));
        }
        for (p, n, flames) in pools {
            net.outgoing.push(json!({"k": "acid", "p": v3(p), "n": v3(n), "f": flames}));
        }
        let now = time.elapsed_secs_f64();
        let live: Vec<(u32, Vec3, Quat)> = rocks
            .rocks
            .iter()
            .map(|r| (r.id, r.position, r.rotation))
            .chain(rocks.held.iter().copied())
            .collect();
        if (!live.is_empty() || fx.rocks_sent) && now - fx.rocks_clock >= 0.05 {
            fx.rocks_clock = now;
            fx.rocks_sent = !live.is_empty();
            let list: Vec<Value> = live
                .iter()
                .map(|(id, p, q)| {
                    let r = |x: f32| (x as f64 * 1000.).round() / 1000.;
                    json!([id, r(p.x), r(p.y), r(p.z), r(q.x), r(q.y), r(q.z), r(q.w)])
                })
                .collect();
            net.outgoing.push(json!({"k": "rocks", "r": list}));
        }
        return;
    }
    if !guest {
        fx.inbox.clear();
        rocks.net.clear();
        return;
    }
    for m in std::mem::take(&mut fx.inbox) {
        if std::env::var_os("L4D2R_NET_DEBUG").is_some() && m.get("k").and_then(Value::as_str) != Some("rocks") {
            println!("Net flow: the host's {}", m);
        } else if std::env::var_os("L4D2R_NET_DEBUG").is_some()
            && rocks.net.is_empty()
            && m.get("r").and_then(Value::as_array).is_some_and(|r| !r.is_empty())
        {
            println!("Net flow: the host's Tank rock appears");
        }
        let entries = || m.get("l").and_then(Value::as_array).cloned().unwrap_or_default();
        let at = |e: &Value, i: usize| -> Option<Vec3> {
            let a = e.get(i)?.as_array()?;
            let v = Vec3::new(a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32);
            v.is_finite().then_some(v)
        };
        match m.get("k").and_then(Value::as_str) {
            Some("blood") => {
                if let Some(impacts) = impacts.as_mut() {
                    for e in entries().iter().take(16) {
                        if let (Some(position), Some(direction)) = (at(e, 0), at(e, 1)) {
                            let shotgun = e.get(2).and_then(Value::as_u64).unwrap_or(0) != 0;
                            impacts.blood.push(crate::bullet_impacts::BloodHit { position, direction, shotgun });
                        }
                    }
                }
            }
            Some("gore") => {
                if let Some(impacts) = impacts.as_mut() {
                    for e in entries().iter().take(64) {
                        let Some(entity) = e.get(0).and_then(Value::as_u64).and_then(|id| infected.puppet(id as u32)) else {
                            continue;
                        };
                        let (Some(position), Some(direction)) = (at(e, 1), at(e, 2)) else { continue };
                        let u = |i: usize| e.get(i).and_then(Value::as_u64).unwrap_or(0);
                        let mut hit = crate::infected_gore::Hit {
                            entity,
                            position,
                            direction,
                            damage_force_source: at(e, 3),
                            box_index: u(4) as usize,
                            cause: crate::net_infected::gore_cause(u(5).min(255) as u8),
                            distance: e.get(6).and_then(Value::as_f64).unwrap_or(1.) as f32,
                            damage_type: u(7) as u32,
                            sniper: false,
                            swing: None,
                            back: false,
                            decapitates: false,
                            seed: u(9),
                        };
                        crate::infected_gore::unpack_melee(&mut hit, u(8));
                        impacts.gore.push(hit);
                    }
                }
            }
            Some("spit") => {
                if let (Some(p), Some(v)) = (read_v3(&m, "p"), read_v3(&m, "v")) {
                    spit.add_net_projectile(p, v);
                }
            }
            Some("acid") => {
                if let (Some(p), Some(n)) = (read_v3(&m, "p"), read_v3(&m, "n")) {
                    let flames = m.get("f").and_then(Value::as_u64).unwrap_or(1) as usize;
                    spit.add_net_pool(p, n, flames);
                }
            }
            Some("rocks") => {
                rocks.net = m
                    .get("r")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(|r| {
                                let a = r.as_array()?;
                                let f = |i: usize| a.get(i).and_then(Value::as_f64).map(|v| v as f32);
                                let p = Vec3::new(f(1)?, f(2)?, f(3)?);
                                let q = Quat::from_xyzw(f(4)?, f(5)?, f(6)?, f(7)?).normalize();
                                (p.is_finite() && q.is_finite()).then_some((a.first()?.as_u64()? as u32, p, q))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }
}

/// Online, the death banner's "The Survivors have been overwhelmed" line
/// only shows once everyone is down.
fn overwhelmed_line(mut lines: Query<&mut Visibility, With<crate::game_ui::OverwhelmedText>>) {
    let show = !online() || lost();
    for mut v in &mut lines {
        v.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
    }
}

pub fn install(app: &mut App) {
    app.init_resource::<NetFx>()
        .add_systems(Update, ((flow, effects).chain().after(crate::net_play::exchange), overwhelmed_line));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_messages_round_trip() {
        let m = use_json(42, Some(l4d2r_math::Vec3::new(1., 2., 3.)));
        assert_eq!(parse_use(&m), Some((42, Some(l4d2r_math::Vec3::new(1., 2., 3.)))));
        assert_eq!(parse_use(&use_json(7, None)), Some((7, None)));
        assert_eq!(parse_use(&json!({"k": "p"})), None);
    }

    #[test]
    fn the_mission_is_lost_only_when_everyone_is_down() {
        let weapon = crate::game_state::tests_weapon();
        let mut player = crate::game_state::Match::new(&weapon);
        let bot = |incap: bool, dead: bool| {
            let mut state = crate::game_state::Match::new(&weapon);
            state.incap = incap;
            state.dead = dead;
            crate::game_state::Companion {
                state,
                position: Vec3::ZERO,
                spawn: Vec3::ZERO,
                model: String::new(),
                vertical: 0.,
                cooldown: 0.,
                revive_time: 0.,
                aim: Vec3::X,
            }
        };
        let squad = crate::game_state::Squad(vec![bot(true, false), bot(false, true)]);
        player.dead = true;
        assert!(all_down(&player, &squad));
        let standing = crate::game_state::Squad(vec![bot(true, false), bot(false, false)]);
        assert!(!all_down(&player, &standing));
    }
}
