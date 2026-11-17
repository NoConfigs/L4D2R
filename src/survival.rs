//! Survival mode: hold out as long as possible against endless waves.
//!
//! - Maps: each campaign's mission file lists its Survival maps ("modes"
//!   "survival"); choosing SURVIVAL in the main menu swaps them into the map
//!   picker. They load with the holdout entity patch (`_h_N.lmp`) and the
//!   OnSurvival outputs, and the survivors start at the GameMode "survival"
//!   info_survivor_positions.
//! - Start: nothing spawns until the round starts. The map's radio
//!   (survival_alarm_button -> relay_panicevent -> @director PanicEvent)
//!   starts it, as does the console `survival_start` (retail
//!   ForceSurvivalStart).
//! - Waves (`SurvivalDirector`, after the owned server.dll director
//!   survival think 1028ee00, with the retail cvar defaults recovered from
//!   server.dll): hordes every survival_horde_stage_interval (60 s, 2 s
//!   shorter each time, and at once after a lull); every
//!   survival_special_stage_interval (60 s) each special's limit and the
//!   special total rise (survival_*_limit_increase up to survival_max_*), and
//!   the special respawn interval (20 s) shortens by 1 s; Tank stages every
//!   survival_tank_stage_interval (80 s, 20 s shorter every other wave), each
//!   with more Tanks 10 s apart, then a lull (15 s, 15 s longer each time, up
//!   to 60 s) before the next wave.
//! - The round ends when every survivor is down or dead: the time and medal
//!   show, and the round restarts after survival_round_restart_delay (15 s).
//!   Medals: bronze 4:00, silver 7:00, gold 10:00.
//! - Online the host's game runs the director; the round's start and end are
//!   sent to guests (net_flow "surv").
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The SURVIVAL tile of the main menu's mode carousel (game_menu::MODES).
pub const MODE_INDEX: usize = 5;

/// Retail survival cvar defaults (server.dll ConVar registrations).
const HORDE_INTERVAL: f32 = 60.;
const HORDE_INTERVAL_DECAY: f32 = 2.;
const HORDE_INTERVAL_MIN: f32 = 20.;
const TANK_INTERVAL: f32 = 80.;
const TANK_INTERVAL_DECAY: f32 = 20.;
const TANK_INTERVAL_MIN: f32 = 30.;
const TANK_MULTIPLE_DELAY: f32 = 10.;
const SPECIAL_INTERVAL: f32 = 20.;
const SPECIAL_INTERVAL_DECAY: f32 = 1.;
const SPECIAL_INTERVAL_MIN: f32 = 5.;
const SPECIAL_STAGE_INTERVAL: f32 = 60.;
const LULL_TIME: f32 = 15.;
const LULL_INCREMENT: f32 = 15.;
const LULL_MAX: f32 = 60.;
pub const RESTART_DELAY: f32 = 15.;
/// Commons keep coming between hordes (retail survival never relaxes).
const TRICKLE_INTERVAL: (f32, f32) = (6., 10.);
/// Medal times (seconds): bronze, silver, gold.
pub const MEDALS: [f32; 3] = [240., 420., 600.];
/// Per class, in SpecialClass::MINIONS order (Hunter, Smoker, Boomer,
/// Spitter, Jockey, Charger): survival_*_limit_increase and survival_max_*.
const LIMIT_INCREASE: [u32; 6] = [1, 1, 0, 1, 1, 0];
const LIMIT_MAX: [u32; 6] = [3, 4, 2, 3, 2, 2];
const SPECIALS_START: u32 = 3;
const SPECIALS_MAX: u32 = 8;
/// Tanks per Tank stage (the stage table; 2 from stage 11 on).
fn tanks_for_stage(stage: u32) -> u32 {
    match stage {
        0..=2 => 1,
        3..=6 => 2,
        _ => 2 + (stage - 7) / 4,
    }
    .min(4)
}

static SURVIVAL: AtomicBool = AtomicBool::new(false);
/// The map being loaded/played is a Survival map.
pub fn active() -> bool {
    // `--survival` with `--game <map>`: a Survival map without the menu.
    static ARG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    SURVIVAL.load(Ordering::Relaxed) || *ARG.get_or_init(|| std::env::args().any(|a| a == "--survival"))
}
pub fn map_mode() -> crate::bsp::MapMode {
    if crate::scavenge_rules::active() {
        crate::bsp::MapMode::Scavenge
    } else if active() {
        crate::bsp::MapMode::Survival
    } else {
        crate::bsp::MapMode::Campaign
    }
}

/// What the Survival director wants this tick.
#[derive(Debug, Default, PartialEq)]
pub struct Wave {
    pub horde: bool,
    pub trickle: bool,
}

#[derive(Clone, Debug)]
pub struct SurvivalDirector {
    /// Director clock when the round started.
    pub started: Option<f32>,
    horde_at: f32,
    horde_interval: f32,
    trickle_at: f32,
    special_stage_at: f32,
    pub special_interval: f32,
    pub max_specials: u32,
    pub limits: [u32; 6],
    tank_at: f32,
    tank_interval: f32,
    tank_stage: u32,
    tanks_left: u32,
    wave: u32,
    lull_until: Option<f32>,
    lull_time: f32,
    rng: u64,
}
impl Default for SurvivalDirector {
    fn default() -> Self {
        Self {
            started: None,
            horde_at: 0.,
            horde_interval: HORDE_INTERVAL,
            trickle_at: 0.,
            special_stage_at: 0.,
            special_interval: SPECIAL_INTERVAL,
            max_specials: SPECIALS_START,
            limits: [1; 6],
            tank_at: 0.,
            tank_interval: TANK_INTERVAL,
            tank_stage: 0,
            tanks_left: 0,
            wave: 0,
            lull_until: None,
            lull_time: LULL_TIME,
            rng: 0x2545_f491_4f6c_dd1d,
        }
    }
}
impl SurvivalDirector {
    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        lo + (hi - lo) * ((self.rng >> 40) as f32 / (1u64 << 24) as f32)
    }
    pub fn start(&mut self, now: f32) {
        if self.started.is_some() {
            return;
        }
        println!("Survival: round started");
        self.started = Some(now);
        self.horde_at = now;
        self.trickle_at = now + TRICKLE_INTERVAL.0;
        self.special_stage_at = now + SPECIAL_STAGE_INTERVAL;
        self.tank_at = now + self.tank_interval;
    }
    pub fn running(&self) -> bool {
        self.started.is_some()
    }
    pub fn in_lull(&self, now: f32) -> bool {
        self.lull_until.is_some_and(|until| now < until)
    }
    /// One director tick: hordes, the common trickle and special stages.
    pub fn tick(&mut self, now: f32) -> Wave {
        let mut wave = Wave::default();
        if self.started.is_none() {
            return wave;
        }
        if let Some(until) = self.lull_until {
            if now < until {
                return wave;
            }
            // "[SURVIVAL]: Lull ended." -> a horde at once.
            self.lull_until = None;
            self.horde_at = now;
        }
        if now >= self.horde_at {
            wave.horde = true;
            self.horde_at = now + self.horde_interval;
            self.horde_interval = (self.horde_interval - HORDE_INTERVAL_DECAY).max(HORDE_INTERVAL_MIN);
        } else if now >= self.trickle_at {
            wave.trickle = true;
        }
        if now >= self.trickle_at {
            self.trickle_at = now + self.random(TRICKLE_INTERVAL.0, TRICKLE_INTERVAL.1);
        }
        if now >= self.special_stage_at {
            self.special_stage_at = now + SPECIAL_STAGE_INTERVAL;
            for (limit, (increase, max)) in self.limits.iter_mut().zip(LIMIT_INCREASE.iter().zip(LIMIT_MAX)) {
                *limit = (*limit + increase).min(max);
            }
            self.max_specials = (self.max_specials + 1).min(SPECIALS_MAX);
            self.special_interval = (self.special_interval - SPECIAL_INTERVAL_DECAY).max(SPECIAL_INTERVAL_MIN);
        }
        wave
    }
    /// A Tank is due now (several per stage, `TANK_MULTIPLE_DELAY` apart).
    pub fn tank_due(&mut self, now: f32) -> bool {
        if self.started.is_none() || self.in_lull(now) || now < self.tank_at {
            return false;
        }
        if self.tanks_left == 0 {
            self.tanks_left = tanks_for_stage(self.tank_stage);
            println!("Survival: Tank attack, {} this stage (stage {})", self.tanks_left, self.tank_stage);
        }
        self.tanks_left -= 1;
        if self.tanks_left > 0 {
            self.tank_at = now + TANK_MULTIPLE_DELAY;
            return true;
        }
        // Stage done: a lull, then the next wave.
        self.tank_stage += 1;
        self.wave += 1;
        let lull = self.lull_time;
        self.lull_until = Some(now + lull);
        self.lull_time = (self.lull_time + LULL_INCREMENT).min(LULL_MAX);
        if self.wave % 2 == 0 {
            self.tank_interval = (self.tank_interval - TANK_INTERVAL_DECAY).max(TANK_INTERVAL_MIN);
        }
        self.tank_at = now + lull + self.tank_interval;
        println!("Survival: lull of {lull:.0} s; next wave {}", self.wave);
        true
    }
}

/// Medal for a time: 0 none, 1 bronze, 2 silver, 3 gold.
pub fn medal(seconds: f32) -> usize {
    MEDALS.iter().filter(|m| seconds >= **m).count()
}
pub fn clock_text(seconds: f32) -> String {
    let s = seconds.max(0.);
    format!("{:02}:{:05.2}", (s / 60.) as u32, s % 60.)
}

/// The round as the HUD sees it (host or guest).
#[derive(Resource, Default)]
pub struct Round {
    /// Real-time seconds when the round started.
    pub started: Option<f64>,
    /// Final time once everyone is down.
    pub ended: Option<f32>,
    /// Real time of the end (the restart follows `RESTART_DELAY` later).
    ended_at: f64,
    /// This map's best time (saved), and the map.
    pub record: Option<f32>,
    pub map: String,
    /// Infected killed this round: common, Hunter, Smoker, Boomer, Charger,
    /// Jockey, Spitter, Tank (the end board's rows).
    pub kills: [u32; 8],
}
impl Round {
    pub fn elapsed(&self, now: f64) -> f32 {
        match (self.started, self.ended) {
            (_, Some(t)) => t,
            (Some(s), None) => (now - s) as f32,
            _ => 0.,
        }
    }
    fn reset(&mut self) {
        *self = Round { record: self.record, map: std::mem::take(&mut self.map), ..default() };
    }
    /// The round ended at `t`: a better time is saved as the map's record.
    fn finish(&mut self, t: f32, now: f64) {
        self.ended = Some(t);
        self.ended_at = now;
        if self.record.is_none_or(|r| t > r) && t > 0. && !self.map.is_empty() {
            self.record = Some(t);
            save_record(&self.map, t);
        }
    }
}

/// Best Survival times per map, beside the interface settings.
const RECORDS_FILE: &str = "runtime/survival_records.txt";
fn parse_records(text: &str) -> Vec<(String, f32)> {
    text.lines()
        .filter_map(|l| {
            let (map, t) = l.split_once('=')?;
            Some((map.trim().to_lowercase(), t.trim().parse::<f32>().ok().filter(|t| t.is_finite() && *t > 0.)?))
        })
        .collect()
}
fn load_record(map: &str) -> Option<f32> {
    let text = std::fs::read_to_string(RECORDS_FILE).ok()?;
    parse_records(&text).into_iter().find(|(m, _)| m == map).map(|(_, t)| t)
}
fn save_record(map: &str, time: f32) {
    let mut records = std::fs::read_to_string(RECORDS_FILE).map(|t| parse_records(&t)).unwrap_or_default();
    records.retain(|(m, _)| m != map);
    records.push((map.to_string(), time));
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let text: String = records.iter().map(|(m, t)| format!("{m}={t:.2}
")).collect();
    if let Some(dir) = std::path::Path::new(RECORDS_FILE).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(RECORDS_FILE, text) {
        Ok(()) => println!("Survival: new best time on {map}: {}", clock_text(time)),
        Err(e) => eprintln!("Survival: could not save the best time: {e}"),
    }
}

/// Guests: the host's round started (`elapsed` s ago) or ended at `time`.
pub(crate) fn remote_event(round: &mut Round, now: f64, started: Option<f32>, ended: Option<f32>) {
    if let Some(ago) = started {
        if round.started.is_none() {
            round.started = Some(now - ago as f64);
            round.ended = None;
            round.kills = [0; 8];
        }
    }
    if let Some(time) = ended {
        if round.ended.is_none() {
            round.finish(time, now);
        }
    }
}

/// The menu mode for a lobby leader's settings: SURVIVAL for a Survival
/// map, otherwise Campaign when we were on SURVIVAL.
pub(crate) fn lobby_mode(current: usize, survival: bool) -> usize {
    match (survival, current == MODE_INDEX) {
        (true, _) => MODE_INDEX,
        (false, true) => 0,
        (false, false) => current,
    }
}

/// Menu: the map picker lists Survival maps while SURVIVAL is chosen, and
/// the next map loads in Survival mode.
pub(crate) fn menu_mode(
    frontend: Res<crate::game_menu::Frontend>,
    catalog: Option<ResMut<crate::campaign_settings::Catalog>>,
    selection: Option<ResMut<crate::campaign_settings::Selection>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    mut lists: Local<Option<(Vec<crate::campaign_settings::Campaign>, Vec<crate::campaign_settings::Campaign>)>>,
    mut showing: Local<bool>,
) {
    if frontend.started {
        return;
    }
    let (Some(mut catalog), Some(source)) = (catalog, source) else { return };
    let want = frontend.mode == MODE_INDEX;
    SURVIVAL.store(want, Ordering::Relaxed);
    if want == *showing {
        return;
    }
    let (coop, survival) = lists.get_or_insert_with(|| {
        (catalog.campaigns.clone(), crate::campaign_settings::Catalog::survival(&source.0))
    });
    if want && survival.is_empty() {
        return;
    }
    *showing = want;
    catalog.campaigns = if want { survival.clone() } else { coop.clone() };
    if let Some(mut sel) = selection {
        sel.campaign = sel.campaign.min(catalog.campaigns.len().saturating_sub(1));
        let chapters = catalog.campaigns.get(sel.campaign).map_or(1, |c| c.chapters.len());
        sel.chapter = sel.chapter.min(chapters.saturating_sub(1));
    }
}

/// The map picker's title follows the mode.
fn picker_title(mut titles: Query<&mut Text, With<crate::game_menu::CampaignTitle>>) {
    let value = if active() { "PLAY SURVIVAL" } else { "PLAY CAMPAIGN" };
    for mut t in &mut titles {
        if t.0 != value {
            t.0 = value.into();
        }
    }
}

/// Round state, the end of a round and the restart (host and offline).
#[allow(clippy::too_many_arguments)]
fn round(
    mut round: ResMut<Round>,
    mut frontend: ResMut<crate::game_menu::Frontend>,
    mut director: Option<ResMut<crate::source_director::Director>>,
    state: Option<Res<crate::game_state::Match>>,
    squad: Option<Res<crate::game_state::Squad>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
    time: Res<Time<Real>>,
    mut net: ResMut<crate::net_play::NetPlay>,
) {
    let now = time.elapsed_secs_f64();
    if !active() || !frontend.playing() {
        if round.started.is_some() || !round.map.is_empty() {
            *round = Round::default();
        }
        return;
    }
    // The map's saved record (runtime/survival_records.txt).
    let map = settings.as_ref().map_or(String::new(), |s| s.map.file_stem().unwrap_or_default().to_string_lossy().to_lowercase());
    if round.map != map {
        *round = Round { record: load_record(&map), map, ..default() };
    }
    let guest = crate::net_play::guest();
    let host = crate::net_play::host();
    if FORCE_START.swap(false, Ordering::Relaxed) && !guest {
        if let Some(d) = director.as_mut() {
            let _ = d.force_survival_start();
        }
    }
    if !guest {
        let running = director.as_ref().and_then(|d| d.survival.as_ref()).is_some_and(|s| s.running());
        // Restarted: the director waits for the radio again.
        if !running && round.started.is_some() {
            round.reset();
        }
        if running && round.started.is_none() {
            round.started = Some(now);
            if host {
                net.outgoing.push(serde_json::json!({"k": "surv", "s": 0.}));
            }
        }
        // `--capture-survival-end <seconds>`: the round ends then (captures).
        static END: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
        let forced_end = END
            .get_or_init(|| {
                let args: Vec<String> = std::env::args().collect();
                args.windows(2).find(|w| w[0] == "--capture-survival-end").and_then(|w| w[1].parse().ok())
            })
            .is_some_and(|at| round.started.is_some() && round.elapsed(now) >= at);
        if let (Some(_), None, Some(state), Some(squad)) = (round.started, round.ended, state.as_ref(), squad.as_ref()) {
            if crate::net_flow::all_down(state, squad) || forced_end {
                let t = round.elapsed(now);
                round.finish(t, now);
                println!("Survival: round over at {} (medal {})", clock_text(t), medal(t));
                if host {
                    net.outgoing.push(serde_json::json!({"k": "surv", "e": t}));
                }
            }
        }
    }
    if round.ended.is_some() && now - round.ended_at >= RESTART_DELAY as f64 {
        // The host restarts everyone (net_flow "restart"); offline, here.
        if !guest && crate::net_flow::request_restart() {
            frontend.restart = true;
        }
        round.ended_at = f64::INFINITY;
        if guest {
            // The host's restart message resets the round; this is a fallback.
            round.reset();
        }
    }
}

/// Infected killed this round (the end-of-round board): an infected that
/// dies while the clock runs is counted once, by its model.
fn count_kills(
    mut round: ResMut<Round>,
    enemies: Query<(Entity, &crate::game::Enemy)>,
    mut seen: Local<std::collections::HashSet<Entity>>,
) {
    if round.started.is_none() || round.ended.is_some() {
        seen.clear();
        return;
    }
    for (entity, enemy) in &enemies {
        if enemy.health > 0. || !seen.insert(entity) {
            continue;
        }
        use crate::special_infected::SpecialClass;
        let slot = match SpecialClass::from_model(&enemy.model) {
            Some(SpecialClass::Hunter) => 1,
            Some(SpecialClass::Smoker) => 2,
            Some(SpecialClass::Boomer) => 3,
            Some(SpecialClass::Charger) => 4,
            Some(SpecialClass::Jockey) => 5,
            Some(SpecialClass::Spitter) => 6,
            Some(SpecialClass::Tank) => 7,
            None if enemy.model.contains("witch") => continue,
            None => 0,
        };
        round.kills[slot] += 1;
    }
    if seen.len() > 4096 {
        let alive: std::collections::HashSet<Entity> = enemies.iter().map(|(e, _)| e).collect();
        seen.retain(|e| alive.contains(e));
    }
}

/// The radial clock face (the shared radial shader of the infected HUD:
/// controls.x is the filled fraction, clockwise from twelve o'clock).
#[derive(Asset, TypePath, bevy::render::render_resource::AsBindGroup, Debug, Clone)]
struct ClockFace {
    #[texture(0)]
    #[sampler(1)]
    image: Handle<Image>,
    #[uniform(2)]
    controls: Vec4,
    #[uniform(3)]
    tint: Vec4,
}
impl UiMaterial for ClockFace {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "shaders/infected_hud.wgsl".into()
    }
}

#[derive(Component)]
enum Part {
    /// HudSurvivalTimer root.
    Timer,
    CurrentDigits,
    TargetDigits,
    Descriptor,
    GoalIcon,
    Awesome,
    /// FullscreenSurvivalModeScoreboard root.
    Board,
    FinalDigits,
    /// Survivor entry: name, best time, medal icon.
    EntryName(usize),
    EntryTime(usize),
    EntryMedal(usize),
    Kills(usize),
}

/// iconsheet2 cells (scripts/mod_textures.txt): bronze, silver, gold small
/// medals and the player-record icon, 32 x 32.
fn icon_rect(icon: usize) -> Rect {
    let x = [64., 96., 128., 192.][icon.min(3)];
    Rect::new(x, 64., x + 32., 96.)
}

/// The next goal at `t`: (time, icon, description). Medals are the
/// L4D_SurvivalTimer_Description_* standards; a saved record is "Your Best".
fn next_goal(t: f32, record: Option<f32>) -> (Option<(f32, usize, &'static str)>, f32) {
    let mut goals: Vec<(f32, usize, &'static str)> = vec![
        (MEDALS[0], 0, "Bronze Standard"),
        (MEDALS[1], 1, "Silver Standard"),
        (MEDALS[2], 2, "Gold Standard"),
    ];
    if let Some(r) = record.filter(|r| *r > 0.) {
        goals.push((r, 3, "Your Best"));
    }
    goals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let previous = goals.iter().map(|g| g.0).filter(|g| *g <= t).fold(0., f32::max);
    (goals.into_iter().find(|g| g.0 > t), previous)
}

/// HUD: retail HudSurvivalTimer (hudlayout.res: c-220, y -10, 440 x 100)
/// and, after the round, FullscreenSurvivalModeScoreboard.
#[allow(clippy::too_many_arguments)]
fn hud(
    mut commands: Commands,
    round: Res<Round>,
    frontend: Res<crate::game_menu::Frontend>,
    time: Res<Time<Real>>,
    camera: Option<Single<Entity, With<crate::game::ViewCamera>>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    state: Option<Res<crate::game_state::Match>>,
    squad: Option<Res<crate::game_state::Squad>>,
    settings: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut parts: Query<(&Part, Option<&mut Text>, Option<&mut Node>, Option<&mut ImageNode>, Option<&mut Visibility>)>,
    mut faces: Query<&MaterialNode<ClockFace>>,
    mut materials: ResMut<Assets<ClockFace>>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
) {
    let (Some(camera), Some(source)) = (camera, source) else { return };
    if parts.is_empty() {
        build(&mut commands, *camera, &source.0, &mut images, &mut fonts, &mut materials);
        return;
    }
    let show = active() && frontend.playing();
    let now = time.elapsed_secs_f64();
    let t = round.elapsed(now);
    let (goal, previous) = next_goal(t, round.record);
    let beaten = round.record.is_some_and(|r| r > 0. && t > r);
    let ended = show && round.ended.is_some();
    // Survivors for the board: the player first, then the squad.
    let me = crate::game_chat::local_name();
    let names: Vec<String> = std::iter::once(me)
        .chain(squad.iter().flat_map(|s| s.0.iter()).map(|m| {
            let survivor = crate::game_ui::kill_feed::display_name(&m.model);
            crate::net_play::player_of(survivor).unwrap_or_else(|| survivor.to_string())
        }))
        .take(4)
        .collect();
    let _ = (&state, &settings);
    let best = round.record.unwrap_or(0.).max(round.ended.unwrap_or(0.));
    for (part, text, node, image, visibility) in &mut parts {
        let set = |text: Option<Mut<Text>>, value: String| {
            if let Some(mut text) = text {
                if text.0 != value {
                    text.0 = value;
                }
            }
        };
        match part {
            Part::Timer => {
                if let Some(mut v) = visibility {
                    v.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
                }
            }
            Part::Board => {
                if let Some(mut v) = visibility {
                    v.set_if_neq(if ended { Visibility::Inherited } else { Visibility::Hidden });
                }
            }
            Part::CurrentDigits => set(text, clock_text(t)),
            Part::TargetDigits => set(text, clock_text(goal.map_or(previous, |g| g.0))),
            Part::Descriptor => set(text, goal.map_or(if beaten { "Your Best" } else { "Gold Standard" }, |g| g.2).into()),
            Part::GoalIcon => {
                if let Some(mut image) = image {
                    let icon = goal.map_or(if beaten { 3 } else { 2 }, |g| g.1);
                    let rect = Some(icon_rect(icon));
                    if image.rect != rect {
                        image.rect = rect;
                    }
                }
            }
            Part::Awesome => {
                // L4D_SurvivalTimer_Description_KeepGoing while beating the record.
                if let Some(mut v) = visibility {
                    v.set_if_neq(if beaten && round.ended.is_none() { Visibility::Inherited } else { Visibility::Hidden });
                }
            }
            Part::FinalDigits => set(text, clock_text(round.ended.unwrap_or(t))),
            Part::EntryName(i) => set(text, names.get(*i).cloned().unwrap_or_default()),
            Part::EntryTime(i) => set(text, if *i < names.len() { clock_text(best) } else { String::new() }),
            Part::EntryMedal(i) => {
                let m = medal(best);
                if let Some(mut node) = node {
                    let display = if *i < names.len() && m > 0 { Display::Flex } else { Display::None };
                    if node.display != display {
                        node.display = display;
                    }
                }
                if let Some(mut image) = image {
                    let rect = Some(icon_rect(m.saturating_sub(1)));
                    if image.rect != rect {
                        image.rect = rect;
                    }
                }
            }
            Part::Kills(i) => set(text, round.kills[*i].to_string()),
        }
    }
    // The clock face fills toward the next goal.
    let span = goal.map_or(1., |g| (g.0 - previous).max(1.));
    let fraction = if goal.is_some() { ((t - previous) / span).clamp(0., 1.) } else { 1. };
    for face in &mut faces {
        if let Some(mut m) = materials.get_mut(&face.0) {
            if (m.controls.x - fraction).abs() > 1e-4 {
                m.controls.x = fraction;
            }
        }
    }
}

/// Spawns the retail timer and end board on the HUD camera.
fn build(
    commands: &mut Commands,
    camera: Entity,
    root: &std::path::Path,
    images: &mut Assets<Image>,
    fonts: &mut Assets<Font>,
    materials: &mut Assets<ClockFace>,
) {
    let Ok(mut assets) = crate::source_assets::SourceAssets::new(root, vec![]) else { return };
    let mut image = |path: &str| -> Handle<Image> {
        match assets.read(&format!("materials/vgui/{path}.vtf")).and_then(|b| crate::vtf::decode(&b, 1024)) {
            Ok(t) => images.add(Image::new(
                bevy::render::render_resource::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                bevy::render::render_resource::TextureDimension::D2,
                t.rgba,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            )),
            Err(e) => {
                eprintln!("Survival HUD {path}: {e}");
                Handle::default()
            }
        }
    };
    let background = image("hud/survivaltimerbackground");
    let clock = image("hud/survivaltimerclock");
    let face = image("hud/survivaltimerclockface");
    let glow = image("hud/scalablepanel_bgmidgrey_glow");
    let icons = image("hud/iconsheet2");
    let red = image("hud/scalablepanel_bgblack_outlinered");
    let font = crate::source_assets::SourceAssets::new(root, vec![])
        .ok()
        .and_then(|mut a| a.read("resource/stubble-bold.vfont").ok())
        .and_then(|b| crate::vfont::decode(&b).ok())
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    // 480-line units.
    let u = |v: f32| vh(v / 4.8);
    let at = |x: f32, y: f32, w: f32, h: f32| Node {
        position_type: PositionType::Absolute,
        left: u(x),
        top: u(y),
        width: u(w),
        height: u(h),
        ..default()
    };
    let text_font = |size: f32| TextFont {
        font: bevy::text::FontSource::Handle(font.clone()),
        font_size: FontSize::Vh(size / 4.8),
        ..default()
    };
    let sliced = |handle: &Handle<Image>| ImageNode {
        image_mode: bevy::ui::widget::NodeImageMode::Sliced(bevy::sprite::TextureSlicer {
            border: bevy::sprite::BorderRect::all(16.),
            center_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
            sides_scale_mode: bevy::sprite::SliceScaleMode::Stretch,
            max_corner_scale: 0.5,
        }),
        ..ImageNode::new(handle.clone())
    };
    let white = Color::WHITE;
    let gray = Color::srgb_u8(145, 145, 145);
    // HudSurvivalTimer.
    let timer = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                margin: UiRect::left(u(-220.)),
                top: u(-10.),
                width: u(440.),
                height: u(100.),
                ..default()
            },
            Visibility::Hidden,
            UiTargetCamera(camera),
            GlobalZIndex(35),
            Part::Timer,
        ))
        .id();
    commands.spawn((ImageNode::new(background), at(0., 0., 440., 100.), ChildOf(timer)));
    commands.spawn((sliced(&glow), at(2., 29., 180., 42.), ChildOf(timer)));
    commands.spawn((sliced(&glow), at(180., 29., 257., 42.), ChildOf(timer)));
    commands.spawn((Text::new("00:00.00"), text_font(28.), TextColor(white), at(25., 36., 150., 28.), Part::CurrentDigits, ChildOf(timer)));
    commands.spawn((ImageNode::new(clock), at(172., 23., 58., 58.), ChildOf(timer)));
    let material = materials.add(ClockFace { image: face, controls: Vec4::ZERO, tint: Vec4::ONE });
    commands.spawn((MaterialNode(material), at(172., 23., 58., 58.), ChildOf(timer)));
    commands.spawn((
        ImageNode { rect: Some(icon_rect(0)), ..ImageNode::new(icons.clone()) },
        at(224., 52., 16., 16.),
        Part::GoalIcon,
        ChildOf(timer),
    ));
    commands.spawn((Text::new("00:00.00"), text_font(22.), TextColor(white), at(225., 33., 150., 24.), Part::TargetDigits, ChildOf(timer)));
    commands.spawn((Text::new(""), text_font(12.), TextColor(white), at(241., 54., 200., 12.), Part::Descriptor, ChildOf(timer)));
    commands.spawn((
        Text::new("New Team Record!"),
        text_font(12.),
        TextColor(white),
        TextLayout::justify(Justify::Center),
        at(120., 72., 200., 12.),
        Visibility::Hidden,
        Part::Awesome,
        ChildOf(timer),
    ));
    // FullscreenSurvivalModeScoreboard: a 0 0 0 230 band (y 115, 180 tall)
    // and SurvivalModeScoreboard at c-250.
    let board = commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: percent(100.), height: percent(100.), ..default() },
            Visibility::Hidden,
            UiTargetCamera(camera),
            GlobalZIndex(36),
            Part::Board,
        ))
        .id();
    commands.spawn((
        Node { position_type: PositionType::Absolute, left: px(0.), top: u(115.), width: percent(100.), height: u(180.), ..default() },
        BackgroundColor(Color::srgba(0., 0., 0., 230. / 255.)),
        ChildOf(board),
    ));
    let panel = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                margin: UiRect::left(u(-250.)),
                top: u(115.),
                width: u(500.),
                height: u(260.),
                ..default()
            },
            ChildOf(board),
        ))
        .id();
    commands.spawn((
        ImageNode { color: Color::srgba(1., 1., 1., 132. / 255.), ..sliced(&red) },
        at(0., -4., 320., 40.),
        ChildOf(panel),
    ));
    let label = |commands: &mut Commands, text: &str, x: f32, y: f32, w: f32, size: f32, color: Color, right: bool| {
        commands
            .spawn((
                Text::new(text),
                text_font(size),
                TextColor(color),
                TextLayout::justify(if right { Justify::Right } else { Justify::Left }),
                at(x, y, w, 24.),
                ChildOf(panel),
            ))
            .id()
    };
    // L4D_SurvivalScoreboard_* strings.
    label(commands, "Final Time:", 20., 5., 320., 20., white, false);
    let digits = label(commands, "00:00.00", 150., 4., 150., 20., white, true);
    commands.entity(digits).insert(Part::FinalDigits);
    label(commands, "Players", 25., 34., 200., 13., gray, false);
    label(commands, "Best Time", 160., 34., 125., 13., gray, true);
    for (i, y) in [58., 81., 104., 127.].into_iter().enumerate() {
        let name = label(commands, "", 25., y + 2., 200., 13., gray, false);
        commands.entity(name).insert(Part::EntryName(i));
        let time = label(commands, "", 165., y + 3., 125., 13., gray, true);
        commands.entity(time).insert(Part::EntryTime(i));
        commands.spawn((
            ImageNode { rect: Some(icon_rect(0)), ..ImageNode::new(icons.clone()) },
            at(291., y + 2., 18., 18.),
            Part::EntryMedal(i),
            ChildOf(panel),
        ));
    }
    label(commands, "Infected Killed", 340., 5., 170., 20., white, false);
    let rows = ["Common Infected:", "Hunters:", "Smokers:", "Boomers:", "Chargers:", "Jockeys:", "Spitters:", "Tanks:"];
    for (i, name) in rows.iter().enumerate() {
        let y = 25. + 18. * i as f32;
        label(commands, name, 340., y + 3., 125., 13., gray, false);
        let n = label(commands, "0", 466., y + 3., 110., 13., white, false);
        commands.entity(n).insert(Part::Kills(i));
    }
}

static FORCE_START: AtomicBool = AtomicBool::new(false);
/// Console `survival_start` (retail ForceSurvivalStart).
pub(crate) fn command(command: &str) -> Option<String> {
    if command.trim() != "survival_start" {
        return None;
    }
    if !active() {
        return Some("Not a Survival map".into());
    }
    FORCE_START.store(true, Ordering::Relaxed);
    Some("Survival round started".into())
}

pub fn install(app: &mut App) {
    app.add_plugins(UiMaterialPlugin::<ClockFace>::default());
    app.init_resource::<Round>().add_systems(
        Update,
        (
            menu_mode.after(crate::multiplayer::run).before(crate::game_menu::update),
            (round, count_kills, hud).chain(),
            picker_title,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_happens_before_the_radio() {
        let mut s = SurvivalDirector::default();
        assert_eq!(s.tick(100.), Wave::default());
        assert!(!s.tank_due(500.));
    }

    #[test]
    fn hordes_shorten_and_a_horde_follows_each_lull() {
        let mut s = SurvivalDirector::default();
        s.start(0.);
        assert!(s.tick(0.).horde, "the start brings a horde");
        assert!(!s.tick(30.).horde);
        assert!(s.tick(60.).horde);
        assert!(s.tick(60. + 58.).horde, "the interval shrinks by 2 s");
        // The Tank stage at 80 s ends with a lull; the horde comes after it.
        assert!(s.tank_due(200.));
        assert!(s.in_lull(201.));
        assert!(!s.tick(205.).horde);
        assert!(s.tick(216.).horde, "a horde when the 15 s lull ends");
    }

    #[test]
    fn special_limits_rise_each_stage_up_to_their_maximums() {
        let mut s = SurvivalDirector::default();
        s.start(0.);
        for minute in 1..=10 {
            s.tick(minute as f32 * 60.);
        }
        // Boomers and Chargers never rise (increase 0); the rest reach max.
        assert_eq!(s.limits, [3, 4, 1, 3, 2, 1]);
        assert_eq!(s.max_specials, SPECIALS_MAX);
        assert_eq!(s.special_interval, 10.);
    }

    #[test]
    fn later_tank_stages_bring_more_tanks_and_longer_lulls() {
        let mut s = SurvivalDirector::default();
        s.start(0.);
        let mut tanks = vec![];
        let mut lulls = vec![];
        let mut t = 0.;
        while tanks.len() < 12 && t < 5000. {
            t += 1.;
            s.tick(t);
            if s.tank_due(t) {
                tanks.push(t);
                if s.in_lull(t + 0.5) {
                    lulls.push(s.lull_until.unwrap() - t);
                }
            }
        }
        assert_eq!(tanks[0], 80.);
        assert_eq!(lulls[..4], [15., 30., 45., 60.]);
        assert!(tanks.windows(2).any(|w| w[1] - w[0] == TANK_MULTIPLE_DELAY), "several Tanks a stage later on");
    }

    #[test]
    fn the_next_goal_is_the_next_medal_or_your_best() {
        assert_eq!(next_goal(10., None).0.map(|g| g.2), Some("Bronze Standard"));
        assert_eq!(next_goal(300., None).0.map(|g| (g.0, g.2)), Some((420., "Silver Standard")));
        assert_eq!(next_goal(300., Some(330.)).0.map(|g| (g.0, g.2)), Some((330., "Your Best")));
        assert_eq!(next_goal(300., Some(330.)).1, 240.);
        assert_eq!(next_goal(700., None).0, None);
    }

    #[test]
    fn records_parse_per_map() {
        let r = parse_records("c1m4_atrium=312.50
C2M1_HIGHWAY=61
bad
x=-3
");
        assert_eq!(r, vec![("c1m4_atrium".into(), 312.5), ("c2m1_highway".into(), 61.)]);
    }

    #[test]
    fn medals_and_the_clock() {
        assert_eq!(medal(239.), 0);
        assert_eq!(medal(240.), 1);
        assert_eq!(medal(420.), 2);
        assert_eq!(medal(600.), 3);
        assert_eq!(clock_text(65.5), "01:05.50");
    }
}
