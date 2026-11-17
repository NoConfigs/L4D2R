//! Scavenge round and match rules (server Director scavenge code:
//! 102858d0 can poured, 10286090 overtime, 10286c60 half end), and a
//! survivors-only half for `--scavenge` (no team swap or scoreboard yet;
//! KNOWN_GAPS "Scavenge mode").
#![allow(dead_code)]
use bevy::prelude::*;

/// `--scavenge` with `--game <map>`: load the map in Scavenge mode.
pub fn active() -> bool {
    static ARG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ARG.get_or_init(|| std::env::args().any(|a| a == "--scavenge"))
}
static HUD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// The round clock line for the fuel HUD while a Scavenge half runs.
pub fn hud_text() -> Option<String> {
    HUD.lock().ok().and_then(|h| h.clone())
}
fn clock_text(seconds: f32) -> String {
    let s = seconds.max(0.).ceil() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}
#[derive(Default)]
pub struct Game {
    round: Option<Round>,
    poured: u32,
    map: usize,
}
/// Survivors-only Scavenge half: the clock, pours from the map's gas can
/// progress, overtime while a can is carried, and the half's result.
pub fn round(
    mut game: Local<Game>,
    time: Res<Time>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    mut s: ResMut<crate::game_state::Match>,
    squad: Option<Res<crate::game_state::Squad>>,
    sounds: Option<ResMut<crate::game_audio::Queue>>,
) {
    if !active() {
        return;
    }
    let Some(mut logic) = logic else { return };
    let map = std::sync::Arc::as_ptr(&logic.world.bsp) as usize;
    let fire = |logic: &mut crate::l4d2r_cinematic::MapLogic, class: &str, output: &str| {
        let ids: Vec<usize> = logic.world.ents.iter().enumerate().filter(|(_, e)| e.alive && e.me.classname == class).map(|(i, _)| i).collect();
        for id in ids {
            logic.world.fire_output_pub(id, output);
        }
    };
    if game.map != map || game.round.is_none() {
        // The progress display may only turn on with the round; until then
        // the goal is unknown (the map script sets it).
        *game = Game { round: Some(Round::new(u32::MAX)), poured: 0, map };
        // Director FireScavengeMatchStart: info_gamemode OnScavengeMatchStart
        // (c1m4: the intro relay).
        fire(&mut logic, "info_gamemode", "OnScavengeMatchStart");
    }
    let (poured, goal) = logic.world.scavenge_progress().unwrap_or((game.poured, u32::MAX));
    if let Some(r) = game.round.as_mut() {
        if goal != u32::MAX {
            r.remaining = goal.saturating_sub(poured);
        }
    }
    if s.paused {
        return;
    }
    let carrying = s.carried.as_deref().is_some_and(|c| c.contains("gascan"))
        || squad.as_ref().is_some_and(|q| q.0.iter().any(|m| m.state.carried.as_deref().is_some_and(|c| c.contains("gascan"))));
    let mut events = vec![];
    let fresh = poured.saturating_sub(game.poured);
    game.poured = poured;
    let round = game.round.as_mut().unwrap();
    for _ in 0..fresh {
        events.extend(round.pour());
    }
    events.extend(round.tick(time.delta_secs(), carrying));
    let text = match round.phase {
        Phase::Setup { left } => format!("SCAVENGE  starts in {}", clock_text(left)),
        Phase::Playing => format!("SCAVENGE  {}", clock_text(round.clock)),
        Phase::Overtime { .. } => "SCAVENGE  OVERTIME".to_string(),
        Phase::Ended => format!("SCAVENGE  round over: {} cans in {}", round.half.cans, clock_text(round.half.time)),
    };
    if let Ok(mut h) = HUD.lock() {
        *h = Some(text);
    }
    let mut sounds = sounds;
    for &e in &events {
        // info_director outputs the map listens to.
        match e {
            Event::RoundStart => fire(&mut logic, "info_director", "OnScavengeRoundStart"),
            Event::OvertimeStart => {
                fire(&mut logic, "info_director", "OnScavengeTimerExpired");
                fire(&mut logic, "info_director", "OnScavengeOvertimeStart");
            }
            Event::HalfEnded if round.clock <= 0. => fire(&mut logic, "info_director", "OnScavengeTimerExpired"),
            _ => {}
        }
    }
    for e in events {
        println!("Scavenge: {e:?} (clock {:.1}, cans {})", round.clock, round.half.cans);
        let cue = match e {
            Event::RoundStart => Some("Event.ScavengeRoundStart"),
            Event::OvertimeStart => Some("Event.ScavengeOvertimeStart"),
            Event::OvertimeTimeAdded => Some("Event.ScavengeOvertimeTimeAdded"),
            Event::Scored => Some("Scavenge.point_scored"),
            Event::HalfEnded => Some("Event.ScavengeRoundEnd"),
        };
        if let (Some(cue), Some(q)) = (cue, sounds.as_mut()) {
            q.play(cue, None);
        }
        if e == Event::HalfEnded {
            s.hint = format!("Scavenge round over: {} cans", round.half.cans);
        }
    }
}
pub fn install(app: &mut App) {
    app.add_systems(Update, round);
}

/// Server cvar defaults.
pub const SETUP_TIME: f32 = 45.;
pub const INITIAL_TIME: f32 = 90.;
pub const SCORE_BONUS_TIME: f32 = 20.;
pub const OVERTIME_GRACE: f32 = 1.;
pub const RESTART_DELAY: f32 = 10.;
pub const RESTART_DELAY_TIED: f32 = 15.;
pub const MATCH_FINISHED_DELAY: f32 = 5.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Before the round: the survivors wait in the start area.
    Setup { left: f32 },
    Playing,
    /// The clock ran out while a can was being carried.
    Overtime { grace: f32 },
    /// The half is over.
    Ended,
}

/// One team's half: cans poured and the clock time it took.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Half {
    pub cans: u32,
    /// Seconds of play when the last can was poured.
    pub time: f32,
}

/// One half of a Scavenge round, from the survivors' side.
#[derive(Clone, Debug)]
pub struct Round {
    pub phase: Phase,
    /// Seconds left on the clock.
    pub clock: f32,
    /// Seconds played (for the time tie-break).
    pub elapsed: f32,
    pub half: Half,
    /// Cans still needed (the map goal).
    pub remaining: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    RoundStart,
    OvertimeStart,
    OvertimeTimeAdded,
    Scored,
    HalfEnded,
}

impl Round {
    pub fn new(goal: u32) -> Self {
        Self { phase: Phase::Setup { left: SETUP_TIME }, clock: INITIAL_TIME, elapsed: 0., half: Half::default(), remaining: goal }
    }
    /// ForceScavengeStart / leaving the start area ends setup early.
    pub fn start(&mut self) -> Option<Event> {
        matches!(self.phase, Phase::Setup { .. }).then(|| {
            self.phase = Phase::Playing;
            Event::RoundStart
        })
    }
    /// Advance by `dt`; `carrying` is whether any survivor holds a can.
    pub fn tick(&mut self, dt: f32, carrying: bool) -> Vec<Event> {
        let mut events = vec![];
        match self.phase {
            Phase::Setup { left } => {
                let left = left - dt;
                if left <= 0. {
                    self.phase = Phase::Playing;
                    events.push(Event::RoundStart);
                } else {
                    self.phase = Phase::Setup { left };
                }
            }
            Phase::Playing => {
                self.elapsed += dt;
                self.clock = (self.clock - dt).max(0.);
                if self.clock <= 0. {
                    if carrying {
                        self.phase = Phase::Overtime { grace: OVERTIME_GRACE };
                        events.push(Event::OvertimeStart);
                    } else {
                        self.phase = Phase::Ended;
                        events.push(Event::HalfEnded);
                    }
                }
            }
            Phase::Overtime { grace } => {
                self.elapsed += dt;
                // Overtime lasts while a can is carried, plus the grace.
                let grace = if carrying { OVERTIME_GRACE } else { grace - dt };
                if grace <= 0. {
                    self.phase = Phase::Ended;
                    events.push(Event::HalfEnded);
                } else {
                    self.phase = Phase::Overtime { grace };
                }
            }
            Phase::Ended => {}
        }
        events
    }
    /// A can poured into the generator (102858d0): one less needed, the
    /// clock gains the bonus (restarting it in overtime), and the half ends
    /// once the goal is met.
    pub fn pour(&mut self) -> Vec<Event> {
        if !matches!(self.phase, Phase::Playing | Phase::Overtime { .. }) {
            return vec![];
        }
        let mut events = vec![Event::Scored];
        self.half.cans += 1;
        self.half.time = self.elapsed;
        self.remaining = self.remaining.saturating_sub(1);
        if matches!(self.phase, Phase::Overtime { .. }) {
            self.phase = Phase::Playing;
            events.push(Event::OvertimeTimeAdded);
        }
        self.clock += SCORE_BONUS_TIME;
        if self.remaining == 0 {
            self.phase = Phase::Ended;
            events.push(Event::HalfEnded);
        }
        events
    }
}

/// The round winner after both halves (10286c60): more cans wins; equal
/// cans go to the faster team; equal cans and time is a tie. `first` is the
/// team that played survivors first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Winner {
    First,
    Second,
    Tie,
}
pub fn round_winner(first: Half, second: Half) -> Winner {
    if first.cans != second.cans {
        return if first.cans > second.cans { Winner::First } else { Winner::Second };
    }
    if first.time != second.time {
        return if first.time < second.time { Winner::First } else { Winner::Second };
    }
    Winner::Tie
}

/// Match state over rounds: a team wins the match with round_limit / 2 + 1
/// round wins (best of `round_limit`).
#[derive(Clone, Debug, Default)]
pub struct Match {
    pub wins: [u32; 2],
    pub played: u32,
}
impl Match {
    /// Record a round; returns the match winner (team index) when decided.
    pub fn record(&mut self, winner: Option<usize>, round_limit: u32) -> Option<usize> {
        self.played += 1;
        if let Some(w) = winner {
            self.wins[w] += 1;
        }
        let needed = round_limit / 2 + 1;
        (0..2).find(|&t| self.wins[t] >= needed)
    }
    /// Seconds before the next round starts.
    pub fn restart_delay(tied: bool) -> f32 {
        if tied { RESTART_DELAY_TIED } else { RESTART_DELAY }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_then_ninety_seconds_with_twenty_per_can() {
        let mut r = Round::new(8);
        assert!(r.tick(44., false).is_empty());
        assert_eq!(r.tick(1., false), vec![Event::RoundStart]);
        r.tick(30., false);
        assert!((r.clock - 60.).abs() < 1e-4);
        assert_eq!(r.pour(), vec![Event::Scored]);
        assert!((r.clock - 80.).abs() < 1e-4);
        assert_eq!(r.half, Half { cans: 1, time: 30. });
        assert_eq!(r.remaining, 7);
    }
    #[test]
    fn overtime_runs_while_a_can_is_carried_and_a_pour_restarts_the_clock() {
        let mut r = Round::new(8);
        r.start();
        assert_eq!(r.tick(90., true), vec![Event::OvertimeStart]);
        assert!(r.tick(5., true).is_empty());
        assert_eq!(r.pour(), vec![Event::Scored, Event::OvertimeTimeAdded]);
        assert_eq!(r.phase, Phase::Playing);
        assert!((r.clock - 20.).abs() < 1e-4);
        // Drop the can at zero: the grace second ends the half.
        assert_eq!(r.tick(20., true), vec![Event::OvertimeStart]);
        assert!(r.tick(0.5, false).is_empty());
        assert_eq!(r.tick(0.6, false), vec![Event::HalfEnded]);
    }
    #[test]
    fn time_out_without_a_can_or_meeting_the_goal_ends_the_half() {
        let mut r = Round::new(1);
        r.start();
        assert_eq!(r.pour(), vec![Event::Scored, Event::HalfEnded]);
        let mut r = Round::new(8);
        r.start();
        assert_eq!(r.tick(91., false), vec![Event::HalfEnded]);
    }
    #[test]
    fn cans_then_time_decide_the_round_and_majority_the_match() {
        let a = Half { cans: 6, time: 80. };
        assert_eq!(round_winner(a, Half { cans: 5, time: 10. }), Winner::First);
        assert_eq!(round_winner(a, Half { cans: 6, time: 70. }), Winner::Second);
        assert_eq!(round_winner(a, a), Winner::Tie);
        let mut m = Match::default();
        assert_eq!(m.record(Some(0), 5), None);
        assert_eq!(m.record(Some(1), 5), None);
        assert_eq!(m.record(None, 5), None);
        assert_eq!(m.record(Some(0), 5), None);
        assert_eq!(m.record(Some(0), 5), Some(0));
        assert_eq!(Match::restart_delay(true), 15.);
    }
}
