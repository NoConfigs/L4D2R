//! Authored finite pickup stock and visible dropped inventory.
use crate::{
    game::{Pickup, Taken},
    game_state::{Gun, Match, Rules, Squad},
    viewer::RenderModels,
};
use bevy::prelude::*;
#[derive(Component)]
pub struct Stock {
    pub initial: i32,
    pub remaining: i32,
}
#[derive(Component)]
pub struct Dropped(pub Option<Gun>);
#[derive(Component)]
pub(crate) struct DropHull(pub Vec3);
#[derive(Component, Clone, Copy)]
pub struct PickupPlacement(pub Vec3);
#[derive(Component)]
pub struct PickupOwner(pub Entity);
#[derive(Component)]
pub struct PickupMapEntity(pub usize);
/// An item sitting inside a map prop (a medical cabinet's pills or kits):
/// reach traces ignore that prop's own collision, which encloses the item.
#[derive(Component, Clone, Copy)]
pub struct PickupInside(pub usize);
pub fn pickup_active(
    binding: Option<&PickupMapEntity>,
    logic: Option<&crate::l4d2r_cinematic::MapLogic>,
) -> bool {
    binding.is_none_or(|b| logic.is_some_and(|l| l.world.ents.get(b.0).is_some_and(|e| e.alive)))
}
/// Long melee pickups are authored leaning against scenery. Preserve that
/// pose instead of dropping their pivot onto a different collision surface.
pub fn spawn_position(
    collision: &crate::collision::Collision,
    class: &str,
    origin: Vec3,
    lo: Vec3,
    hi: Vec3,
) -> Vec3 {
    if class.starts_with("weapon_melee_") {
        origin
    } else {
        supported_position(collision, origin, lo, hi).unwrap_or(origin)
    }
}
/// Place the posed model's lowest point on the support below the spawn.
pub fn supported_position(
    collision: &crate::collision::Collision,
    origin: Vec3,
    lo: Vec3,
    hi: Vec3,
) -> Option<Vec3> {
    if !lo.is_finite() || !hi.is_finite() {
        return None;
    }
    let center = (lo + hi) * 0.5;
    let start = origin + Vec3::new(center.x, lo.y.max(0.) + 0.1, center.z);
    let end = start - Vec3::Y * 4.;
    let hit = collision.trace(start, end, Vec3::ZERO);
    if hit.start_solid || hit.fraction >= 1. || hit.normal.y < 0.7 {
        return None;
    }
    let surface = start.y - 4. * hit.fraction;
    Some(Vec3::new(origin.x, surface - lo.y + 0.003, origin.z))
}
pub fn stock(count: i32) -> Stock {
    let count = if count < 0 { -1 } else { count.max(1) };
    Stock {
        initial: count,
        remaining: count,
    }
}
pub fn drop_item(
    commands: &mut Commands,
    models: Option<&RenderModels>,
    meshes: Option<&Assets<Mesh>>,
    rules: &Rules,
    collision: &crate::collision::Collision,
    class: &str,
    mut position: Vec3,
    gun: Option<Gun>,
) -> Entity {
    let rotation = if class == "weapon_first_aid_kit" {
        Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)
    } else {
        Quat::IDENTITY
    };
    let handles = rules
        .weapons
        .get(class)
        .and_then(|w| models.and_then(|m| m.0.get(&(w.model.clone(), 0, 0))));
    let mut center = Vec3::ZERO;
    let mut half = Vec3::splat(0.12);
    if let Some(meshes) = meshes {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (mesh, _) in handles.into_iter().flatten() {
            if let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = meshes
                .get(mesh)
                .and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
            {
                for p in positions {
                    let p = rotation * Vec3::from_array(*p);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        if let Some(supported) = supported_position(collision, position, lo, hi) {
            position = supported;
        } else if lo.is_finite() {
            position.y -= lo.y;
        }
        if lo.is_finite() && hi.is_finite() {
            center = (lo + hi) * 0.5;
            half = ((hi - lo) * 0.5).max(Vec3::splat(0.02));
        }
    }
    let root = commands
        .spawn((
            Pickup {
                class: class.into(),
                origin: position + center,
            },
            stock(1),
            Dropped(gun),
            DropHull(half),
            Name::new(format!("{class}_drop")),
            Transform::from_translation(position).with_rotation(rotation),
            Visibility::Inherited,
        ))
        .id();
    if let Some(handles) = rules
        .weapons
        .get(class)
        .and_then(|w| models.and_then(|m| m.0.get(&(w.model.clone(), 0, 0))))
    {
        for (mesh, material) in handles {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                crate::viewer::DynamicModelLighting::default(),
                ChildOf(root),
            ));
        }
    }
    root
}
/// An item placed exactly at an authored transform (no drop to the
/// ground), e.g. the pills on a medical cabinet's item attachments.
pub fn place_item(
    commands: &mut Commands,
    models: Option<&RenderModels>,
    meshes: Option<&Assets<Mesh>>,
    rules: &Rules,
    class: &str,
    placement: Transform,
) -> Entity {
    let handles = rules
        .weapons
        .get(class)
        .and_then(|w| models.and_then(|m| m.0.get(&(w.model.clone(), 0, 0))));
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for (mesh, _) in handles.into_iter().flatten() {
        if let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            meshes.and_then(|m| m.get(mesh)).and_then(|m| m.attribute(Mesh::ATTRIBUTE_POSITION))
        {
            for p in positions {
                let p = placement.rotation * Vec3::from_array(*p);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
    }
    let (center, half) = if lo.is_finite() && hi.is_finite() {
        ((lo + hi) * 0.5, ((hi - lo) * 0.5).max(Vec3::splat(0.02)))
    } else {
        (Vec3::ZERO, Vec3::splat(0.12))
    };
    let root = commands
        .spawn((
            Pickup {
                class: class.into(),
                origin: placement.translation + center,
            },
            stock(1),
            Dropped(None),
            DropHull(half),
            Name::new(format!("{class}_placed")),
            placement,
            Visibility::Inherited,
        ))
        .id();
    for (mesh, material) in handles.into_iter().flatten() {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            crate::viewer::DynamicModelLighting::default(),
            ChildOf(root),
        ));
    }
    root
}
pub fn consume(
    commands: &mut Commands,
    entity: Entity,
    class: &str,
    origin: Vec3,
    stocks: &mut Query<&mut Stock>,
    rendered: &Query<(Entity, &Name, &Transform, Option<&PickupOwner>), Without<Pickup>>,
) -> bool {
    let exhausted = if let Ok(mut count) = stocks.get_mut(entity) {
        if count.remaining < 0 {
            false
        } else {
            count.remaining = (count.remaining - 1).max(0);
            count.remaining == 0
        }
    } else {
        true
    };
    if exhausted {
        commands.entity(entity).insert((Taken, Visibility::Hidden));
        for (e, name, t, owner) in rendered {
            if owner.is_some_and(|owner| owner.0 == entity)
                || (t.translation.distance(origin) < 0.2
                    && (name.as_str().starts_with(&format!("{class}_spawn"))
                        || name.as_str().starts_with("weapon_spawn")
                        || name.as_str().starts_with("weapon_melee_spawn")
                        || name.as_str().starts_with("weapon_item_spawn")))
            {
                commands.entity(e).insert((Taken, Visibility::Hidden));
            }
        }
    }
    exhausted
}
pub fn bot_pickups(
    mut commands: Commands,
    state: Res<Match>,
    rules: Res<Rules>,
    mut squad: ResMut<Squad>,
    collision: Res<crate::collision::Collision>,
    pickups: Query<(Entity, &Pickup, Option<&PickupMapEntity>), Without<Taken>>,
    mut stocks: Query<&mut Stock>,
    rendered: Query<(Entity, &Name, &Transform, Option<&PickupOwner>), Without<Pickup>>,
    logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    (mut scavenge, inside): (Option<ResMut<scavenge::Scavenge>>, Query<&PickupInside>),
) {
    let _timing = crate::performance::scope("loot::bot_pickups");
    if state.infected || state.paused || crate::game_state::bots_stopped() {
        return;
    }
    let mut exhausted = std::collections::HashSet::new();
    for (index, member) in squad.0.iter_mut().enumerate() {
        if member.state.dead || member.state.incap || member.state.action > 0. || crate::game_state::remote(index) {
            continue;
        }
        // Retail bots only take the item they set out to scavenge, once it
        // is inside the 75-unit use range. Without the scavenge state (unit
        // fixtures) any wanted item within reach is taken.
        let target = scavenge.as_ref().map(|s| s.target(index));
        // Stuck on the way: retail collects the item where it is.
        let stuck = scavenge.as_ref().is_some_and(|s| s.stuck(index));
        let reach = if stuck {
            f32::INFINITY
        } else if target.is_some() {
            scavenge::USE_RANGE
        } else {
            2.2
        };
        let selected = pickups
            .iter()
            .filter(|(e, p, binding)| {
                target.is_none_or(|t| t == Some(*e))
                    && pickup_active(*binding, logic.as_deref())
                    && !exhausted.contains(e)
                    && stocks.get(*e).map_or(true, |s| s.remaining != 0)
                    && p.origin.distance(member.position) < reach
                    && (stuck || {
                        let (from, to) = (member.position + Vec3::Y, p.origin + Vec3::Y * 0.2);
                        match inside.get(*e) {
                            Ok(owner) => collision.prop_trace(owner.0, from, to, Vec3::ZERO),
                            Err(_) => collision.trace(from, to, Vec3::ZERO),
                        }
                        .fraction
                            > 0.99
                    })
                    && bot_wants(&member.state, &p.class, &rules)
            })
            .min_by(|a, b| {
                a.1.origin
                    .distance_squared(member.position)
                    .total_cmp(&b.1.origin.distance_squared(member.position))
            });
        if let Some((e, p, _)) = selected {
            if let Some(scavenge) = scavenge.as_mut() {
                scavenge.used(index);
            }
            match p.class.as_str() {
                "weapon_first_aid_kit" => member.state.medkit = true,
                "weapon_defibrillator" => member.state.defibrillator = true,
                "weapon_pain_pills" => member.state.pills = true,
                "weapon_adrenaline" => member.state.adrenaline = true,
                "weapon_ammo" => {
                    if let Some(g) = &mut member.state.primary {
                        g.reserve = rules
                            .weapons
                            .get(&g.class)
                            .map_or(g.reserve, |w| w.reserve_capacity());
                    }
                    continue;
                }
                _ => {
                    if let Some(w) = rules.weapons.get(&p.class) {
                        member.state.primary = Some(Gun {
                            laser: false,
                            ammo_upgrade: None,
                            upgraded_rounds: 0,
                            class: w.class.clone(),
                            ammo: w.clip,
                            reserve: w.reserve_capacity(),
                        });
                        member.state.slot = 1;
                        member.state.reload = 0.;
                        member.state.reload_phase = 0;
                    }
                }
            }
            if consume(&mut commands, e, &p.class, p.origin, &mut stocks, &rendered) {
                exhausted.insert(e);
            }
        }
    }
}
/// Items a survivor bot collects: a medkit or defibrillator when it has
/// neither, pills or adrenaline likewise, ammo when its reserve is short,
/// and a better-ranked primary.
pub(crate) fn bot_wants(state: &Match, class: &str, rules: &Rules) -> bool {
    match class {
        "weapon_first_aid_kit" | "weapon_defibrillator" => !state.medkit && !state.defibrillator,
        "weapon_pain_pills" | "weapon_adrenaline" => !state.pills && !state.adrenaline,
        "weapon_ammo" => state.primary.as_ref().is_some_and(|g| {
            rules
                .weapons
                .get(&g.class)
                .is_some_and(|w| g.reserve < w.reserve_capacity())
        }),
        _ => {
            rules
                .weapons
                .get(class)
                .is_some_and(|w| bot_primary_rank(&w.class).is_some())
                && bot_wants_primary(state, class)
        }
    }
}
fn bot_primary_rank(class: &str) -> Option<u8> {
    match class {
        "weapon_smg"
        | "weapon_smg_silenced"
        | "weapon_smg_mp5"
        | "weapon_pumpshotgun"
        | "weapon_shotgun_chrome" => Some(1),
        "weapon_rifle"
        | "weapon_rifle_ak47"
        | "weapon_rifle_desert"
        | "weapon_rifle_sg552"
        | "weapon_autoshotgun"
        | "weapon_shotgun_spas"
        | "weapon_hunting_rifle"
        | "weapon_sniper_military"
        | "weapon_sniper_scout"
        | "weapon_sniper_awp" => Some(2),
        _ => None,
    }
}
fn bot_wants_primary(state: &Match, class: &str) -> bool {
    let Some(rank) = bot_primary_rank(class) else {
        return false;
    };
    state
        .primary
        .as_ref()
        .is_none_or(|gun| rank > bot_primary_rank(&gun.class).unwrap_or(0))
}

/// Retail SurvivorCollectObject ("Scavenging something"), from server.dll
/// build a93ec916: the decision FUN_1039d480, candidate gathering
/// FUN_103a4ef0 / FUN_103a4c90, and the SurvivorUseObject base
/// (Update FUN_103b2830, use range FUN_103b25c0, use timer FUN_103b3280).
/// - The bot looks again every RandomFloat(0.5, 1) s, and only once it has
///   not been fighting for 2 s (FUN_10303160 >= 2.0).
/// - Candidates are items within a 420-unit box around the bot, at most
///   250 units above or 750 below it, with no infected within 250 units,
///   that the bot wants and no other bot is collecting. While the human
///   stands in a checkpoint only items in a checkpoint count. The closest
///   one is chosen, then rejected (no scavenging this time) unless the bot
///   stands in a checkpoint or the item's NAV area is at most 250 flow units
///   ahead of / 750 behind the bot's and the item is within
///   sb_max_scavenge_separation (750) of the human (FUN_1039d480).
/// - The bot walks to it, uses it inside 75 units and stands for 1 s.
/// - It gives up when the item is gone or taken, or the bot starts fighting
///   ("Situation changed - giving up").
/// - Before starting and then every RandomFloat(0, 1) s while walking, it
///   checks for a NAV path to the item (FUN_103b2660 -> FUN_1039a450) and
///   gives up when there is none ("giving up, object is UNREACHABLE"); the
///   item's area is the nearest within 200 units.
/// - Stuck on the way (NextBot StuckMonitor: still within 100 units of
///   the same spot after 100 / (0.1 * 220 + 0.1) s of trying to move), the
///   bot collects the item anyway and finishes (OnStuck 103ad4b0: "collected
///   %s because path to it failed!").
/// Not modelled: turning to face the item (dot 0.7) before using it.
pub mod scavenge {
    use super::*;
    use crate::collision::UNIT;
    use l4d2r_game_core::random::UniformRandomStream;

    pub const CHECK_INTERVAL: [f32; 2] = [0.5, 1.];
    pub const CALM: f32 = 2.;
    pub const SEARCH: f32 = 420. * UNIT;
    pub const ABOVE: f32 = 250. * UNIT;
    pub const BELOW: f32 = 750. * UNIT;
    pub const SEPARATION: f32 = 750. * UNIT;
    /// Flow window around the bot's own NAV area (Source units; server
    /// 1060dc0c / 105df604).
    pub const FLOW_AHEAD: f32 = 250.;
    pub const FLOW_BEHIND: f32 = 750.;
    pub const INFECTED_CLEARANCE: f32 = 250. * UNIT;
    pub const USE_RANGE: f32 = 75. * UNIT;
    pub const USE_TIME: f32 = 1.;
    /// NextBot STUCK_RADIUS (Source units) and the escape time at the
    /// survivor run speed: STUCK_RADIUS / (0.1 * 220 + 0.1).
    pub const STUCK_RADIUS: f32 = 100. * UNIT;
    pub const STUCK_TIME: f32 = 100. / (0.1 * 220. + 0.1);

    #[derive(Clone, Copy)]
    struct Bot {
        target: Option<(Entity, Vec3)>,
        check_in: f32,
        /// Seconds since the bot last had a threat in its sights.
        calm: f32,
        /// Standing after a use (the 1 s use timer).
        using: f32,
        /// Seconds to the next reachability check of the target.
        reach_in: f32,
        /// Stuck monitor: anchor position and seconds spent near it.
        anchor: Vec3,
        stuck_for: f32,
        /// Stuck: collect the target where it is (retail OnStuck).
        stuck: bool,
    }
    impl Default for Bot {
        fn default() -> Self {
            Self {
                target: None,
                check_in: 0.,
                calm: CALM,
                using: 0.,
                reach_in: 0.,
                anchor: Vec3::ZERO,
                stuck_for: 0.,
                stuck: false,
            }
        }
    }
    #[derive(Resource)]
    pub struct Scavenge {
        bots: Vec<Bot>,
        rng: UniformRandomStream,
    }
    impl Default for Scavenge {
        fn default() -> Self {
            Self {
                bots: vec![],
                rng: UniformRandomStream::new(0x5ca7),
            }
        }
    }
    impl Scavenge {
        fn bot(&mut self, index: usize) -> &mut Bot {
            if self.bots.len() <= index {
                self.bots.resize(index + 1, Bot::default());
            }
            &mut self.bots[index]
        }
        /// The item bot `index` is collecting.
        pub fn target(&self, index: usize) -> Option<Entity> {
            self.bots.get(index).and_then(|b| b.target).map(|(e, _)| e)
        }
        /// Where the bot is heading to collect, or where it stands while
        /// using an item.
        pub fn goal(&self, index: usize, position: Vec3) -> Option<Vec3> {
            let bot = self.bots.get(index)?;
            if bot.using > 0. {
                return Some(position);
            }
            bot.target.map(|(_, at)| at)
        }
        /// Bot `index` is stuck on the way to its item and collects it
        /// where it is.
        pub fn stuck(&self, index: usize) -> bool {
            self.bots.get(index).is_some_and(|b| b.stuck && b.target.is_some())
        }
        /// Seconds since bot `index` last had a threat in its sights.
        pub fn calm(&self, index: usize) -> f32 {
            self.bots.get(index).map_or(f32::INFINITY, |b| b.calm)
        }
        /// The bot has a threat in its sights this tick.
        pub fn fighting(&mut self, index: usize) {
            let bot = self.bot(index);
            bot.calm = 0.;
            bot.target = None;
        }
        /// The bot picked its item up.
        pub fn used(&mut self, index: usize) {
            let bot = self.bot(index);
            bot.target = None;
            bot.stuck = false;
            bot.using = USE_TIME;
        }
        fn forget(&mut self, index: usize) {
            let bot = self.bot(index);
            bot.target = None;
            bot.using = 0.;
        }
    }

    pub fn update(
        time: Res<Time<Fixed>>,
        state: Res<Match>,
        body: Single<&crate::game::Body>,
        rules: Res<Rules>,
        squad: Res<Squad>,
        mut scavenge: ResMut<Scavenge>,
        pickups: Query<(Entity, &Pickup, Option<&PickupMapEntity>), Without<Taken>>,
        stocks: Query<&Stock>,
        enemies: Query<(&Transform, &crate::game::Enemy)>,
        navigation: Option<Res<crate::l4d2r_navigation::Navigation>>,
        logic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
        director: Option<Res<crate::source_director::Director>>,
    ) {
        let _timing = crate::performance::scope("loot::scavenge");
        if state.infected || state.paused || crate::game_state::bots_stopped() {
            return;
        }
        let dt = time.delta_secs();
        let leader = (!state.dead).then_some(body.feet);
        let leader_in_checkpoint = leader
            .and_then(|l| navigation.as_ref().and_then(|n| n.checkpoint_at(l)))
            .unwrap_or(false);
        let infected: Vec<Vec3> = enemies
            .iter()
            .filter(|(_, e)| e.health > 0.)
            .map(|(t, _)| t.translation)
            .collect();
        let wanted = |state: &Match, e: Entity, p: &Pickup, binding: Option<&PickupMapEntity>| {
            pickup_active(binding, logic.as_deref())
                && stocks.get(e).map_or(true, |s| s.remaining != 0)
                && bot_wants(state, &p.class, &rules)
        };
        for (index, member) in squad.0.iter().enumerate() {
            if member.state.dead
                || member.state.incap
                || member.state.pinned
                || member.state.action > 0.
                || leader.is_none()
            {
                scavenge.forget(index);
                continue;
            }
            let bot = scavenge.bot(index);
            bot.calm += dt;
            bot.using = (bot.using - dt).max(0.);
            // Drop a target that is gone, taken or no longer wanted.
            if let Some((e, _)) = bot.target {
                if !pickups
                    .get(e)
                    .is_ok_and(|(e, p, binding)| wanted(&member.state, e, p, binding))
                {
                    bot.target = None;
                }
            }
            // NextBot stuck monitor while walking to the item.
            if bot.target.is_some() && bot.using <= 0. && !bot.stuck {
                if member.position.distance(bot.anchor) > STUCK_RADIUS {
                    bot.anchor = member.position;
                    bot.stuck_for = 0.;
                } else {
                    bot.stuck_for += dt;
                    if bot.stuck_for > STUCK_TIME {
                        bot.stuck = true;
                    }
                }
            }
            // Retail rechecks the path every RandomFloat(0, 1) s.
            if let Some((_, at)) = bot.target.filter(|_| !bot.stuck) {
                bot.reach_in -= dt;
                if bot.reach_in <= 0. {
                    let unreachable = navigation
                        .as_ref()
                        .and_then(|n| n.survivor_reachable(member.position, at))
                        == Some(false);
                    let next = scavenge.rng.random_float(0., 1.);
                    let bot = scavenge.bot(index);
                    bot.reach_in = next;
                    if unreachable {
                        bot.target = None;
                    }
                }
            }
            let bot = scavenge.bot(index);
            if bot.target.is_some() || bot.using > 0. || bot.calm < CALM {
                continue;
            }
            bot.check_in -= dt;
            if bot.check_in > 0. {
                continue;
            }
            let leader = leader.unwrap_or(member.position);
            let taken: Vec<Entity> = (0..squad.0.len())
                .filter(|i| *i != index)
                .filter_map(|i| scavenge.target(i))
                .collect();
            let choice = pickups
                .iter()
                .filter(|(e, p, binding)| {
                    let d = p.origin - member.position;
                    d.x.abs() <= SEARCH
                        && d.z.abs() <= SEARCH
                        && d.y <= ABOVE
                        && d.y >= -BELOW.min(SEARCH)
                        && !taken.contains(e)
                        && wanted(&member.state, *e, p, *binding)
                        && !infected
                            .iter()
                            .any(|i| i.distance(p.origin) <= INFECTED_CLEARANCE)
                        && (!leader_in_checkpoint
                            || navigation
                                .as_ref()
                                .is_none_or(|n| n.checkpoint_near(p.origin) == Some(true)))
                })
                .min_by(|a, b| {
                    a.1.origin
                        .distance_squared(member.position)
                        .total_cmp(&b.1.origin.distance_squared(member.position))
                })
                .map(|(e, p, _)| (e, p.origin));
            // The new behaviour's first update checks the path at once and
            // gives up on an unreachable item (it is picked again, and given
            // up again, next time: the bot keeps following meanwhile).
            let in_checkpoint = navigation
                .as_ref()
                .and_then(|n| n.checkpoint_at(member.position))
                .unwrap_or(false);
            let choice = choice
                .filter(|(_, at)| {
                    in_checkpoint || {
                        let flow = director.as_ref().and_then(|d| {
                            Some((d.flow_at(member.position)?, d.flow_near(*at, 200.)?))
                        });
                        flow.is_none_or(|(bot, item)| {
                            item <= bot + FLOW_AHEAD && item >= bot - FLOW_BEHIND
                        }) && at.distance(leader) <= SEPARATION
                    }
                })
                .filter(|(_, at)| {
                    navigation
                        .as_ref()
                        .and_then(|n| n.survivor_reachable(member.position, *at))
                        != Some(false)
                });
            let interval = scavenge.rng.random_float(CHECK_INTERVAL[0], CHECK_INTERVAL[1]);
            let next_reach = scavenge.rng.random_float(0., 1.);
            let bot = scavenge.bot(index);
            bot.check_in = interval;
            bot.target = choice;
            bot.reach_in = next_reach;
            bot.anchor = member.position;
            bot.stuck_for = 0.;
            bot.stuck = false;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn pistol_state() -> Match {
            Match::new(&crate::game_state::tests_weapon())
        }
        #[test]
        fn a_bot_waits_two_calm_seconds_and_drops_its_item_to_fight() {
            let mut s = Scavenge::default();
            s.bot(0).target = Some((Entity::PLACEHOLDER, Vec3::X));
            assert_eq!(s.goal(0, Vec3::ZERO), Some(Vec3::X));
            s.fighting(0);
            assert_eq!(s.target(0), None);
            assert_eq!(s.bots[0].calm, 0.);
            s.used(0);
            assert_eq!(s.goal(0, Vec3::Y), Some(Vec3::Y), "stands while using");
        }
        #[test]
        fn bots_want_what_retail_bots_collect() {
            let rules = Rules {
                pistol: crate::game_state::tests_weapon(),
                weapons: Default::default(),
            };
            let mut state = pistol_state();
            assert!(bot_wants(&state, "weapon_pain_pills", &rules));
            state.pills = true;
            assert!(!bot_wants(&state, "weapon_pain_pills", &rules));
            assert!(!bot_wants(&state, "weapon_adrenaline", &rules));
            assert!(bot_wants(&state, "weapon_first_aid_kit", &rules));
        }
        #[test]
        fn retail_scavenge_ranges() {
            assert!((SEARCH / UNIT - 420.).abs() < 1e-3);
            assert!((SEPARATION / UNIT - 750.).abs() < 1e-3);
            assert!((USE_RANGE / UNIT - 75.).abs() < 1e-3);
        }
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    #[test]
    fn bots_share_finite_weapon_stock_and_refill_without_consuming_ammo_piles() {
        let pistol = crate::game_state::tests_weapon();
        let mut rifle = pistol.clone();
        rifle.class = "weapon_rifle".into();
        rifle.clip = 50;
        let mut app = App::new();
        let mut state = Match::new(&pistol);
        state.paused = false;
        let member = || crate::game_state::Companion {
            state: Match::new(&pistol),
            position: Vec3::ZERO,
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 0.,
            aim: Vec3::NEG_Z,
        };
        app.insert_resource(state)
            .insert_resource(Rules {
                pistol: pistol.clone(),
                weapons: std::collections::HashMap::from([(rifle.class.clone(), rifle)]),
            })
            .insert_resource(Squad(vec![member(), member()]))
            .insert_resource(crate::collision::Collision::default())
            .add_systems(Update, bot_pickups);
        let pickup = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_rifle".into(),
                    origin: Vec3::ZERO,
                },
                Stock {
                    initial: 1,
                    remaining: 1,
                },
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .resource::<Squad>()
                .0
                .iter()
                .filter(|m| m.state.primary.is_some())
                .count(),
            1
        );
        assert_eq!(app.world().get::<Stock>(pickup).unwrap().remaining, 0);
        assert!(app.world().get::<Taken>(pickup).is_some());
        app.world_mut().resource_mut::<Squad>().0[0]
            .state
            .primary
            .as_mut()
            .unwrap()
            .reserve = 0;
        let ammo = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_ammo".into(),
                    origin: Vec3::ZERO,
                },
                Stock {
                    initial: -1,
                    remaining: -1,
                },
            ))
            .id();
        app.update();
        assert!(
            app.world().resource::<Squad>().0[0]
                .state
                .primary
                .as_ref()
                .unwrap()
                .reserve
                > 0
        );
        assert_eq!(app.world().get::<Stock>(ammo).unwrap().remaining, -1);
        assert!(app.world().get::<Taken>(ammo).is_none());
    }
    #[test]
    fn hotel_rooftop_axes_stay_above_the_authored_roof() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let mut collision = crate::collision::Collision::load(&path).unwrap();
        collision.enable_l4d2r(root, &logic).unwrap();
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let model = "models/weapons/melee/w_fireaxe.mdl";
        let mut draws = crate::studio::load(&mut assets, model, 0, 0).unwrap();
        let pose = crate::studio::pose::sample_l4d2r(&mut assets, model, &["idle"], 0.).unwrap();
        crate::studio::pose::apply(&mut assets, model, &mut draws, &pose, false).unwrap();
        let mut checked = 0;
        for e in &logic.world.ents {
            if e.me.classname != "weapon_melee_spawn"
                || e.me.kv("melee_weapon") != Some("fireaxe")
                || e.me.origin.z < 2840.
            {
                continue;
            }
            let transform = crate::l4d2r_cinematic::transform(logic.world.world_matrix(e.me.index));
            let origin = transform.translation;
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for p in draws.iter().flat_map(|d| &d.positions) {
                let p = transform.transform_point(Vec3::from_array(*p)) - origin;
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let placed = spawn_position(&collision, "weapon_melee_fireaxe", origin, lo, hi);
            println!(
                "Roof axe: original={origin:?} bottom={} placed={placed:?} bottom={}",
                origin.y + lo.y,
                placed.y + lo.y
            );
            assert!(
                placed.y + lo.y >= origin.y + lo.y - 0.02,
                "Authored roof axe was buried: {origin:?} -> {placed:?}"
            );
            assert!(
                (placed.y + lo.y - 2848. * crate::collision::UNIT).abs() < 0.02,
                "The complete posed axe must rest on the visible rooftop"
            );
            checked += 1;
        }
        assert_eq!(checked, 2);
    }
    #[test]
    fn positive_and_negative_pivots_settle_the_visible_geometry() {
        let floor = crate::collision::Collision::test_solid_floor();
        for bottom in [-0.3, 0.2] {
            let lo = Vec3::new(-0.1, bottom, -0.1);
            let hi = lo + Vec3::splat(0.2);
            let p = supported_position(&floor, Vec3::Y, lo, hi).unwrap();
            assert!((p.y + bottom - 0.004).abs() < 0.001);
            assert_eq!(p.x, 0.);
            assert_eq!(p.z, 0.);
        }
        assert!(
            supported_position(
                &crate::collision::Collision::default(),
                Vec3::Y,
                Vec3::ZERO,
                Vec3::ONE
            )
            .is_none()
        );
    }
    #[test]
    fn owned_pills_and_throwables_use_their_posed_rotated_bottom() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let floor = crate::collision::Collision::test_solid_floor();
        for class in [
            "weapon_pain_pills",
            "weapon_pipe_bomb",
            "weapon_molotov",
            "weapon_adrenaline",
        ] {
            let script = assets.read(&format!("scripts/{class}.txt")).unwrap();
            let path = crate::scene::pickup_model(&String::from_utf8_lossy(&script)).unwrap();
            let mut draws = crate::studio::load(&mut assets, &path, 0, 0).unwrap();
            let pose =
                crate::studio::pose::sample_l4d2r(&mut assets, &path, &["idle"], 0.).unwrap();
            crate::studio::pose::apply(&mut assets, &path, &mut draws, &pose, false).unwrap();
            for rotation in [Quat::IDENTITY, Quat::from_rotation_z(0.5)] {
                let mut lo = Vec3::splat(f32::INFINITY);
                let mut hi = Vec3::splat(f32::NEG_INFINITY);
                for p in draws.iter().flat_map(|d| &d.positions) {
                    let p = rotation * Vec3::from_array(*p);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
                let placed = supported_position(&floor, Vec3::Y, lo, hi).unwrap();
                assert!(
                    (placed.y + lo.y - 0.004).abs() < 0.001,
                    "{class} floated above the support"
                );
            }
        }
    }
    #[test]
    fn consuming_an_offset_pickup_hides_all_owner_draws() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        let pickup = app
            .world_mut()
            .spawn((
                Pickup {
                    class: "weapon_pain_pills".into(),
                    origin: Vec3::Y,
                },
                stock(1),
                Visibility::Inherited,
            ))
            .id();
        let draw = app
            .world_mut()
            .spawn((
                Name::new("weapon_item_spawn"),
                Transform::from_xyz(0., -0.5, 0.),
                PickupOwner(pickup),
                Visibility::Inherited,
            ))
            .id();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut stocks: Query<&mut Stock>,
                      rendered: Query<
                    (Entity, &Name, &Transform, Option<&PickupOwner>),
                    Without<Pickup>,
                >| {
                    assert!(consume(
                        &mut commands,
                        pickup,
                        "weapon_pain_pills",
                        Vec3::Y,
                        &mut stocks,
                        &rendered
                    ));
                },
            )
            .unwrap();
        assert!(app.world().get::<Taken>(draw).is_some());
        assert_eq!(
            *app.world().get::<Visibility>(draw).unwrap(),
            Visibility::Hidden
        );
    }
}
