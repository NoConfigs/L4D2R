//! Shared medical inventory, timed use, teammate treatment and recovery.
use crate::{
    collision::{Collision, EYE},
    game::{Body, Input},
    game_state::{Match, Squad},
};
use bevy::prelude::*;

/// sb_temp_health_consider_factor: bots count temporary health at half.
const TEMP_HEALTH_FACTOR: f32 = 0.5;
/// sb_toughness_buffer.
const TOUGHNESS_BUFFER: f32 = 15.;
fn effective_health(m: &Match) -> f32 {
    m.health + m.temporary * TEMP_HEALTH_FACTOR
}
/// Retail FUN_1039cfb0 / SurvivorGivePillsToFriend: a bot with pills or
/// adrenaline hands them to a free friend without a pills-slot item whose
/// effective health is 30..50 and below the giver's plus sb_toughness_buffer.
pub fn wants_pills(giver: &Match, patient: &Match) -> bool {
    let health = effective_health(patient);
    (giver.pills || giver.adrenaline)
        && !patient.pills
        && !patient.adrenaline
        && !patient.dead
        && !patient.incap
        && !patient.pinned
        && !patient.infected
        && (30. ..50.).contains(&health)
        && health < effective_health(giver) + TOUGHNESS_BUFFER
}
/// Friend bot `me` should give its pills to: Some(None) for the player,
/// Some(Some(j)) for bot j; the lowest effective health first.
pub fn pill_patient(state: &Match, squad: &Squad, me: usize) -> Option<Option<usize>> {
    let giver = &squad.0[me].state;
    std::iter::once((None, state))
        .chain(squad.0.iter().enumerate().filter(|(j, _)| *j != me).map(|(j, m)| (Some(j), &m.state)))
        .filter(|(_, m)| wants_pills(giver, m))
        .min_by(|a, b| effective_health(a.1).total_cmp(&effective_health(b.1)))
        .map(|(who, _)| who)
}
fn hand_over(from: &mut Match, to: &mut Match) {
    if from.pills {
        from.pills = false;
        to.pills = true;
    } else if from.adrenaline {
        from.adrenaline = false;
        to.adrenaline = true;
    }
}
fn target(body: &Body, squad: &Squad, dead: bool, collision: &Collision) -> Option<usize> {
    let eye = body.feet + Vec3::Y * EYE;
    let forward = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(_, member)| {
            if dead {
                member.state.dead
            } else {
                !member.state.dead
                    && !member.state.incap
                    && (member.state.health < 100. || member.state.black_white)
            }
        })
        .filter_map(|(index, member)| {
            let center = member.position + Vec3::Y * if dead { 0.4 } else { 1. };
            let delta = center - eye;
            (delta.length() < 2.5
                && forward.dot(delta.normalize_or_zero()) > 0.55
                && collision.trace(eye, center, Vec3::ZERO).fraction > 0.99)
                .then_some((index, delta.length()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|p| p.0)
}
fn begin(state: &mut Match, kind: u8, target: Option<usize>, seconds: f32) {
    state.medical_kind = kind;
    state.medical_target = target;
    state.medical_damage_serial = state.damage_serial;
    state.action_total = seconds;
    state.action = seconds;
    state.item_use_serial = state.item_use_serial.wrapping_add(1);
}
pub fn update(
    time: Res<Time<Fixed>>,
    mut state: ResMut<Match>,
    mut input: ResMut<Input>,
    body: Single<&Body>,
    collision: Res<Collision>,
    mut squad: ResMut<Squad>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    rules: Res<crate::game_state::Rules>,
    library: Option<Res<crate::l4d2r_animation::AnimationLibrary>>,
    mut capture_started: Local<bool>,
) {
    if state.infected || state.paused || state.dead || cinematic.as_ref().is_some_and(|c| c.active)
    {
        return;
    }
    let dt = time.delta_secs();
    state.adrenaline_time = (state.adrenaline_time - dt).max(0.);
    if state.incap {
        return;
    }
    // Pinned or grabbed by a special: no medkit, pills, adrenaline or
    // defibrillator, and any use in progress is cancelled.
    if state.pinned {
        state.action = 0.;
        state.medical_kind = 0;
        state.medical_target = None;
        input.fire = false;
        return;
    }
    // `--capture-use <medkit|adrenaline|pills>`: hurt the player, give and
    // select the item a second into play, then use it (holding fire for the
    // first aid kit) for HUD/third-person captures.
    if let Some(item) = std::env::args()
        .skip_while(|a| a != "--capture-use")
        .nth(1)
        .filter(|_| state.elapsed > 1.)
    {
        if !*capture_started {
            *capture_started = true;
            state.health = 40.;
            match item.as_str() {
                "medkit" => {
                    state.medkit = true;
                    state.slot = 4;
                }
                "adrenaline" => {
                    state.adrenaline = true;
                    state.pills = false;
                    state.slot = 5;
                    input.fire = true;
                }
                _ => {
                    state.pills = true;
                    state.adrenaline = false;
                    state.slot = 5;
                    input.fire = true;
                }
            }
        }
        if item == "medkit" && state.medkit {
            input.fire_held = true;
        }
    }
    if matches!(state.medical_kind, 4 | 5) && state.action > 0. {
        state.action -= dt;
        if state.action <= 0. {
            if state.medical_kind == 4 {
                state.adrenaline = false;
                state.adrenaline_time = 15.;
                state.temporary = (state.temporary + 25.).min(100. - state.health);
            } else {
                state.pills = false;
                state.post_pills = 2.;
                state.temporary = (state.temporary + 50.).min(100. - state.health);
            }
            state.medical_kind = 0;
            state.slot = 2;
        }
        input.fire = false;
        return;
    }
    if state.slot == 5 && input.fire {
        let kind = if state.adrenaline {
            4
        } else if state.pills {
            5
        } else {
            return;
        };
        let duration = rules
            .weapons
            .get(state.held_class())
            .and_then(|w| {
                library.as_ref().map(|l| {
                    l.duration(
                        &w.view_model,
                        &[crate::l4d2r_animation::medical_activity(&w.class, false)],
                    )
                })
            })
            .unwrap_or(1.5);
        begin(&mut state, kind, None, duration);
        input.fire = false;
        input.fire_held = false;
        return;
    }
    if state.upgrade_pack.is_some() && state.slot == 4 {
        return;
    }
    if state.slot == 6 && state.medical_kind == 10 { return; }
    if state.slot != 4 {
        state.action = 0.;
        state.medical_kind = 0;
        return;
    }
    let secondary = input.shove;
    input.shove = false;
    if state.action <= 0. {
        let speed = if state.adrenaline_time > 0. { 0.5 } else { 1. };
        if state.defibrillator && input.fire_held {
            if let Some(index) = target(&body, &squad, true, &collision) {
                begin(&mut state, 3, Some(index), 3. * speed);
            } else {
                state.hint = "Aim at a dead teammate and hold left mouse to defibrillate".into();
            }
        } else if state.medkit && secondary {
            if let Some(index) = target(&body, &squad, false, &collision) {
                begin(&mut state, 2, Some(index), 5. * speed);
            }
        } else if state.medkit && input.fire_held && (state.health < 100. || state.black_white) {
            begin(&mut state, 1, None, 5. * speed);
        }
    }
    if state.action <= 0. {
        return;
    }
    let held = if state.medical_kind == 2 {
        input.shove_held
    } else {
        input.fire_held
    };
    let valid = state.medical_target.is_none_or(|index| {
        target(&body, &squad, state.medical_kind == 3, &collision) == Some(index)
    });
    if !held || !valid || state.medical_damage_serial != state.damage_serial {
        state.action = 0.;
        state.medical_kind = 0;
        return;
    }
    state.action -= dt;
    if state.action <= 0. {
        match state.medical_kind {
            1 => state.heal(),
            2 => {
                if let Some(member) = state.medical_target.and_then(|i| squad.0.get_mut(i)) {
                    member.state.receive_heal();
                    state.medkit = false;
                    state.slot = 2;
                }
            }
            3 => {
                if let Some(member) = state.medical_target.and_then(|i| squad.0.get_mut(i)) {
                    member.state.resuscitate();
                    member.revive_time = 0.;
                    member.vertical = 0.;
                    state.defibrillator = false;
                    state.slot = 2;
                }
            }
            _ => {}
        }
        state.medical_kind = 0;
        state.medical_target = None;
    }
}
pub fn bot_care(
    time: Res<Time<Fixed>>,
    mut state: ResMut<Match>,
    body: Single<&Body>,
    collision: Res<Collision>,
    mut squad: ResMut<Squad>,
    enemies: Query<(&Transform, &crate::game::Enemy)>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
) {
    let _timing = crate::performance::scope("survivor_items::bot_care");
    if state.infected
        || state.paused
        || crate::game_state::bots_stopped()
        || cinematic.as_ref().is_some_and(|c| c.active)
    {
        return;
    }
    let dt = time.delta_secs();
    for index in 0..squad.0.len() {
        let member = &squad.0[index];
        if member.state.dead || member.state.incap || crate::game_state::remote(index) {
            continue;
        }
        let danger = enemies
            .iter()
            .any(|(p, e)| e.health > 0. && p.translation.distance(member.position) < 4.);
        if matches!(member.state.medical_kind, 7 | 8) && member.state.action > 0. {
            let m = &mut squad.0[index].state;
            m.action -= dt;
            if m.action <= 0. {
                if m.medical_kind == 7 {
                    m.adrenaline = false;
                    m.adrenaline_time = 15.;
                    m.temporary = (m.temporary + 25.).min(100. - m.health);
                } else {
                    m.pills = false;
                    m.temporary = (m.temporary + 50.).min(100. - m.health);
                }
                m.medical_kind = 0;
                m.slot = 2;
            }
            continue;
        }
        if member.state.health + member.state.temporary < 40.
            && (member.state.pills || member.state.adrenaline)
            && (!member.state.medkit || danger)
        {
            let m = &mut squad.0[index].state;
            let kind = if m.adrenaline { 7 } else { 8 };
            m.slot = 5;
            begin(m, kind, None, 1.5);
            continue;
        }
        if danger {
            let m = &mut squad.0[index].state;
            m.action = 0.;
            m.medical_kind = 0;
            continue;
        }
        // Pass pills to a hurt friend in reach while nobody is in combat.
        if member.state.action <= 0. {
            let at = member.position;
            let reach = |p: Vec3| {
                p.distance(at) < 2.
                    && collision.trace(at + Vec3::Y, p + Vec3::Y * 0.4, Vec3::ZERO).fraction > 0.99
            };
            let calm = |p: Vec3| {
                !enemies.iter().any(|(t, e)| e.health > 0. && t.translation.distance(p) < 4.)
            };
            match pill_patient(&state, &squad, index) {
                Some(None) if reach(body.feet) && calm(body.feet) => {
                    hand_over(&mut squad.0[index].state, &mut state);
                    continue;
                }
                Some(Some(j)) if reach(squad.0[j].position) && calm(squad.0[j].position) => {
                    let mut giver = squad.0[index].state.clone();
                    hand_over(&mut giver, &mut squad.0[j].state);
                    squad.0[index].state.pills = giver.pills;
                    squad.0[index].state.adrenaline = giver.adrenaline;
                    continue;
                }
                _ => {}
            }
        }
        let member = &squad.0[index];
        if member.state.action <= 0. && !member.state.medkit && !member.state.defibrillator {
            continue;
        }
        if member.state.action <= 0. {
            let defib = member.state.defibrillator;
            let can_heal = member.state.medkit;
            let player_need = if defib {
                state.dead
            } else {
                can_heal && !state.dead && !state.incap && (state.health < 35. || state.black_white)
            };
            let player_clear = member.position.distance(body.feet) < 2.
                && collision
                    .trace(
                        member.position + Vec3::Y,
                        body.feet + Vec3::Y * 0.4,
                        Vec3::ZERO,
                    )
                    .fraction
                    > 0.99;
            let teammate = squad
                .0
                .iter()
                .enumerate()
                .find(|(j, m)| {
                    *j != index
                        && m.position.distance(member.position) < 2.
                        && collision
                            .trace(
                                member.position + Vec3::Y,
                                m.position + Vec3::Y * 0.4,
                                Vec3::ZERO,
                            )
                            .fraction
                            > 0.99
                        && if defib {
                            m.state.dead
                        } else {
                            can_heal
                                && !m.state.dead
                                && !m.state.incap
                                && (m.state.health < 35. || m.state.black_white)
                        }
                })
                .map(|(j, _)| j);
            let own = can_heal && (member.state.health < 35. || member.state.black_white);
            let m = &mut squad.0[index].state;
            if player_need && player_clear {
                begin(
                    m,
                    if defib { 6 } else { 5 },
                    None,
                    if defib { 3. } else { 5. },
                );
            } else if let Some(j) = teammate {
                begin(
                    m,
                    if defib { 3 } else { 2 },
                    Some(j),
                    if defib { 3. } else { 5. },
                );
            } else if own {
                begin(m, 1, None, 5.);
            }
        }
        let current = &squad.0[index];
        let valid = match current.state.medical_kind {
            2 | 3 => current
                .state
                .medical_target
                .and_then(|i| squad.0.get(i))
                .is_some_and(|m| {
                    m.position.distance(current.position) < 2.
                        && collision
                            .trace(
                                current.position + Vec3::Y,
                                m.position + Vec3::Y * 0.4,
                                Vec3::ZERO,
                            )
                            .fraction
                            > 0.99
                }),
            5 | 6 => {
                current.position.distance(body.feet) < 2.
                    && collision
                        .trace(
                            current.position + Vec3::Y,
                            body.feet + Vec3::Y * 0.4,
                            Vec3::ZERO,
                        )
                        .fraction
                        > 0.99
            }
            _ => true,
        };
        let m = &mut squad.0[index].state;
        if !valid {
            m.action = 0.;
            m.medical_kind = 0;
            continue;
        }
        if m.action <= 0. {
            continue;
        }
        m.action -= dt;
        if m.action > 0. {
            continue;
        }
        let kind = m.medical_kind;
        let target = m.medical_target;
        m.medical_kind = 0;
        match kind {
            1 => m.heal(),
            5 => {
                if !state.dead && !state.incap {
                    state.receive_heal();
                    m.medkit = false;
                }
            }
            6 => {
                if state.dead {
                    state.resuscitate();
                    m.defibrillator = false;
                }
            }
            2 | 3 => {
                if let Some(j) = target {
                    if kind == 2 && !squad.0[j].state.dead && !squad.0[j].state.incap {
                        squad.0[j].state.receive_heal();
                        squad.0[index].state.medkit = false;
                    } else if kind == 3 && squad.0[j].state.dead {
                        squad.0[j].state.resuscitate();
                        squad.0[j].vertical = 0.;
                        squad.0[index].state.defibrillator = false;
                    }
                }
            }
            _ => {}
        }
    }
}
pub fn rescue_goal(state: &Match, player: Vec3, squad: &Squad, index: usize) -> Vec3 {
    let m = &squad.0[index];
    // A teammate held by a special infected comes first: bots run in to
    // shove or shoot the attacker off (the player, or the nearest bot).
    if !state.infected && !state.dead && state.pinned {
        return player;
    }
    if let Some((_, b)) = squad
        .0
        .iter()
        .enumerate()
        .filter(|(i, b)| *i != index && !b.state.dead && b.state.pinned)
        .min_by(|a, b| {
            a.1.position
                .distance_squared(m.position)
                .total_cmp(&b.1.position.distance_squared(m.position))
        })
    {
        return b.position;
    }
    if state.incap
        || state.dead && m.state.defibrillator
        || m.state.medkit
            && !state.dead
            && !state.incap
            && (state.health < 35. || state.black_white)
    {
        return player;
    }
    squad
        .0
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            *i != index
                && (b.state.incap
                    || b.state.dead && m.state.defibrillator
                    || m.state.medkit
                        && !b.state.dead
                        && !b.state.incap
                        && (b.state.health < 35. || b.state.black_white))
        })
        .min_by(|a, b| {
            a.1.position
                .distance_squared(m.position)
                .total_cmp(&b.1.position.distance_squared(m.position))
        })
        .map(|(_, b)| b.position)
        // Lowest priority: walk over to give pills to a hurt friend.
        .or_else(|| pill_patient(state, squad, index).flatten().map(|j| squad.0[j].position))
        .unwrap_or(player)
}
pub fn black_white(
    state: Res<Match>,
    mut cameras: Query<
        &mut bevy::render::view::ColorGrading,
        Or<(
            With<crate::viewer::GameCamera>,
            With<crate::game::ViewCamera>,
        )>,
    >,
) {
    for mut grading in &mut cameras {
        let saturation = if state.black_white { 0. } else { 1. };
        if grading.global.post_saturation != saturation {
            grading.global.post_saturation = saturation;
        }
    }
}
#[cfg(test)]
mod pill_tests {
    use super::*;

    fn bot(health: f32, pills: bool) -> crate::game_state::Companion {
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.health = health;
        state.temporary = 0.;
        state.pills = pills;
        crate::game_state::Companion {
            state,
            position: Vec3::ZERO,
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::ZERO,
        }
    }

    #[test]
    fn bots_give_pills_to_the_most_hurt_friend_between_30_and_50() {
        let mut player = Match::new(&crate::game_state::tests_weapon());
        player.health = 45.;
        player.temporary = 0.;
        player.infected = false;
        let squad = Squad(vec![bot(100., true), bot(35., false), bot(20., false)]);
        // Bot 1 (35) is hurt worse than the player (45); bot 2 (20) is
        // below 30 and left for a medkit.
        assert_eq!(pill_patient(&player, &squad, 0), Some(Some(1)));
        // Temporary health counts at half: 25 + 30 * 0.5 = 40.
        let mut squad = Squad(vec![bot(100., true), bot(25., false)]);
        squad.0[1].state.temporary = 30.;
        player.health = 80.;
        assert_eq!(pill_patient(&player, &squad, 0), Some(Some(1)));
        // A friend who already has a pills-slot item, is pinned or incapped
        // gets nothing; neither does anyone from a bot without pills.
        squad.0[1].state.adrenaline = true;
        assert_eq!(pill_patient(&player, &squad, 0), None);
        squad.0[1].state.adrenaline = false;
        squad.0[1].state.pinned = true;
        assert_eq!(pill_patient(&player, &squad, 0), None);
        squad.0[1].state.pinned = false;
        squad.0[0].state.pills = false;
        assert_eq!(pill_patient(&player, &squad, 0), None);
        // The player, when hurt and the giver is healthier.
        squad.0[0].state.pills = true;
        squad.0[1].state.health = 90.;
        player.health = 40.;
        assert_eq!(pill_patient(&player, &squad, 0), Some(None));
        // Not when the giver is much worse off (sb_toughness_buffer 15).
        squad.0[0].state.health = 20.;
        assert_eq!(pill_patient(&player, &squad, 0), None);
    }

    #[test]
    fn handing_over_moves_the_pills_slot_item() {
        let (mut a, mut b) = (bot(100., true).state, bot(40., false).state);
        hand_over(&mut a, &mut b);
        assert!(!a.pills && b.pills);
        a.adrenaline = true;
        b.pills = false;
        hand_over(&mut a, &mut b);
        assert!(!a.adrenaline && b.adrenaline);
    }
}
