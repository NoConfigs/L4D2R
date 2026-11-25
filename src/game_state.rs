use crate::{kv, source_assets::SourceAssets};
use bevy::prelude::*;
use std::{collections::HashMap, io};
#[derive(Clone)]
pub struct Weapon {
    pub pellets: u8,
    /// Retail bullet penetration (bullet_penetration).
    pub penetration: crate::bullet_penetration::Penetration,
    pub range_modifier: f32,
    pub scatter: Vec2,
    pub accuracy: crate::ballistics::Accuracy,
    pub muzzle_effect: String,
    pub class: String,
    pub clip: u32,
    pub damage: f32,
    pub cycle: f32,
    pub range: f32,
    pub speed: f32,
    pub punch: f32,
    pub model: String,
    pub player_model: String,
    pub reload_times: [f32; 3],
    pub reload_empty_time: f32,
    pub damage_flags: u32,
    /// Melee script "decapitates" (decapitate wounds, 10405d90).
    pub decapitates: bool,
    pub view_model: String,
    pub muzzle_forward: [f32; 3],
    pub fire_sound: String,
    pub melee_hit_sound: String,
    pub melee_world_sound: String,
    pub icon: String,
    pub icon_rect: Rect,
    /// Melee script "primaryattacks", played in order (see `melee`).
    pub melee_attacks: Vec<MeleeAttack>,
}
/// One melee script attack (scripts/melee/*.txt "primaryattacks").
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeleeAttack {
    /// Swing directions: N S E W NE SE SW NW = 0..7 (server melee info).
    pub start: u8,
    pub end: u8,
    pub duration: f32,
    /// The sweep runs between these times after the swing starts.
    pub start_time: f32,
    pub end_time: f32,
    pub activity: String,
}
fn melee_direction(name: &str) -> u8 {
    match name.to_ascii_uppercase().as_str() {
        "N" => 0,
        "S" => 1,
        "E" => 2,
        "W" => 3,
        "NE" => 4,
        "SE" => 5,
        "SW" => 6,
        "NW" => 7,
        _ => 0,
    }
}
/// The attacks in one melee script block ("primaryattacks").
pub fn melee_attacks(tokens: &[String], block: &str) -> Vec<MeleeAttack> {
    let Some(start) = tokens.iter().position(|t| t.eq_ignore_ascii_case(block)) else {
        return vec![];
    };
    let mut i = start + 1;
    if tokens.get(i).map(String::as_str) != Some("{") {
        return vec![];
    }
    i += 1;
    let mut out = vec![];
    while i < tokens.len() && tokens[i] != "}" {
        if tokens.get(i + 1).map(String::as_str) != Some("{") {
            break;
        }
        i += 2;
        let mut attack = MeleeAttack::default();
        while i + 1 < tokens.len() && tokens[i] != "}" {
            let value = &tokens[i + 1];
            let number = || value.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.);
            match tokens[i].to_ascii_lowercase().as_str() {
                "startdir" => attack.start = melee_direction(value),
                "enddir" => attack.end = melee_direction(value),
                "duration" => attack.duration = number(),
                "starttime" => attack.start_time = number(),
                "endtime" => attack.end_time = number(),
                "activity" => attack.activity = value.clone(),
                _ => {}
            }
            i += 2;
        }
        i += 1;
        out.push(attack);
    }
    out
}
impl Weapon {
    pub fn load(assets: &mut SourceAssets, class: &str) -> io::Result<Self> {
        let melee = class.strip_prefix("weapon_melee_");
        let script = melee
            .map(|name| format!("scripts/melee/{name}.txt"))
            .unwrap_or_else(|| {
                format!(
                    "scripts/{}.txt",
                    if class == "weapon_pistol_dual" {
                        "weapon_pistol"
                    } else {
                        class
                    }
                )
            });
        let bytes = assets.read(&script)?;
        let tokens = kv::tokens(&String::from_utf8_lossy(&bytes));
        let get = |key: &str| {
            tokens
                .windows(2)
                .find(|p| p[0].eq_ignore_ascii_case(key))
                .map(|p| p[1].as_str())
        };
        let number = |key: &str, default: f32| {
            get(key)
                .and_then(|s| s.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let sprite = tokens
            .iter()
            .position(|s| {
                s == if melee.is_some() {
                    "sprite_active"
                } else if class == "weapon_pistol_dual" {
                    "weapon_dual"
                } else {
                    "weapon"
                }
            })
            .filter(|i| tokens.get(i + 1).is_some_and(|s| s == "{"))
            .map(|i| {
                let end = tokens[i + 2..]
                    .iter()
                    .position(|s| s == "}")
                    .map(|n| i + 2 + n)
                    .unwrap_or(tokens.len());
                &tokens[i + 2..end]
            })
            .unwrap_or(&[]);
        let sprite_value = |key: &str| {
            sprite
                .windows(2)
                .find(|p| p[0].eq_ignore_ascii_case(key))
                .map(|p| p[1].as_str())
        };
        let sprite_number = |key: &str, default: f32| {
            sprite_value(key)
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(default)
        };
        let origin = Vec2::new(sprite_number("x", 0.), sprite_number("y", 0.));
        let model = crate::scene::pickup_model(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|| "models/w_models/weapons/w_pistol_b.mdl".into());
        let muzzle_forward = crate::studio::muzzle_forward(assets, &model).unwrap_or_else(|e| {
            eprintln!("Weapon axis {class}: {e}; using Source -Y fallback");
            [0., 0., 1.]
        });
        let mut weapon = Self {
            penetration: crate::bullet_penetration::Penetration::from_script(&number),
            pellets: number("Bullets", 1.).clamp(1., 32.) as u8,
            range_modifier: number("RangeModifier", 1.).clamp(0., 1.),
            // Retail shotgun scripts name these PelletScatterPitch/Yaw.
            scatter: Vec2::new(
                number("PelletScatterPitch", number("ScatterPitch", 0.)),
                number("PelletScatterYaw", number("ScatterYaw", 0.)),
            ),
            accuracy: crate::ballistics::Accuracy {
                duck: number("MinDuckingSpread", 0.),
                stand: number("MinStandingSpread", 0.),
                air: number("MinInAirSpread", 0.),
                movement: number("MaxMovementSpread", 0.),
                per_shot: number("SpreadPerShot", 0.),
                maximum: number("MaxSpread", 0.),
                decay: number("SpreadDecay", 0.),
            },
            muzzle_effect: get("MuzzleFlashEffect_1stPerson").unwrap_or("").into(),
            class: class.into(),
            clip: number("clip_size", 15.).clamp(1., 200.) as u32,
            damage: number("Damage", 36.),
            cycle: number(
                if melee.is_some() {
                    "refire_delay"
                } else {
                    "CycleTime"
                },
                0.175,
            )
            .max(0.01),
            // Melee reach is the server's melee_range (70), not a script key.
            range: number("Range", if melee.is_some() { 70. } else { 2500. })
                * crate::collision::UNIT,
            speed: number("MaxPlayerSpeed", 250.) * crate::collision::UNIT,
            punch: number("VerticalPunch", if melee.is_some() { 0. } else { 2. }),
            model,
            reload_times: [1.7, 0.5, 0.5],
            reload_empty_time: 0.,
            // Native GetInt: atoi, so "128 | 4" reads 128.
            damage_flags: get("damage_flags").map_or(2, |v| {
                let digits: String = v.trim().chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            }),
            decapitates: number("decapitates", 0.) != 0.,
            player_model: get("playermodel")
                .unwrap_or("")
                .replace('\\', "/")
                .to_lowercase(),
            view_model: get("viewmodel").unwrap_or("").to_lowercase(),
            muzzle_forward,
            fire_sound: get("single_shot")
                .or_else(|| get("melee_miss"))
                .unwrap_or("Pistol.Fire")
                .into(),
            melee_hit_sound: get("melee_hit").unwrap_or("").into(),
            melee_world_sound: get("melee_hit_world").unwrap_or("").into(),
            icon: format!(
                "materials/{}.vtf",
                sprite_value("file").unwrap_or("vgui/hud/icon_pistol")
            ),
            icon_rect: Rect::from_corners(
                origin,
                origin + Vec2::new(sprite_number("width", 64.), sprite_number("height", 64.)),
            ),
            melee_attacks: if melee.is_some() {
                melee_attacks(&tokens, "primaryattacks")
            } else {
                vec![]
            },
        };
        if class == "weapon_pistol_dual" {
            weapon.clip = 30;
            weapon.cycle = 0.075;
            weapon.view_model = "models/v_models/v_dual_pistola.mdl".into();
            weapon.model = "models/w_models/weapons/w_pistol_a_dual.mdl".into();
            weapon.player_model = weapon.model.clone();
            weapon.muzzle_forward =
                crate::studio::muzzle_forward(assets, &weapon.model).unwrap_or([0., 0., 1.]);
        }
        Ok(weapon)
    }
    pub fn melee(&self) -> bool {
        self.class.starts_with("weapon_melee_")
    }
    pub fn shell_reload(&self) -> bool {
        self.class.contains("shotgun")
    }
    pub fn measured_shell_reload(&self) -> bool {
        matches!(
            self.class.as_str(),
            "weapon_pumpshotgun"
                | "weapon_shotgun_chrome"
                | "weapon_autoshotgun"
                | "weapon_shotgun_spas"
        )
    }
    pub fn calibrate_reload_times(&mut self, authored: [f32; 3]) {
        // Live original m_reload{Start,Insert,End}Duration properties after
        // reload starts. These gameplay timers differ from raw clip lengths.
        self.reload_times = match self.class.as_str() {
            "weapon_pistol" => [1.766667, 0.5, 0.5],
            "weapon_pistol_magnum" => [1.733333, 0.5, 0.5],
            "weapon_pistol_dual" => [2.4, 0.5, 0.5],
            "weapon_pumpshotgun" | "weapon_shotgun_chrome" => [0.475, 0.5, 0.6],
            "weapon_autoshotgun" => [0.633333, 0.38, 0.675],
            "weapon_shotgun_spas" => [0.475, 0.375, 0.7],
            _ => authored,
        };
    }
    pub fn throwable(&self) -> bool {
        matches!(
            self.class.as_str(),
            "weapon_molotov" | "weapon_pipe_bomb" | "weapon_vomitjar"
        )
    }
    pub fn infinite(&self) -> bool {
        matches!(
            self.class.as_str(),
            "weapon_pistol" | "weapon_pistol_dual" | "weapon_pistol_magnum"
        )
    }
    pub fn automatic(&self) -> bool {
        if self.class == "weapon_rifle_desert" {
            return false;
        }
        self.class.contains("smg")
            || self.class.contains("rifle")
            || self.class == "weapon_autoshotgun"
            || self.class == "weapon_shotgun_spas"
    }
    pub fn shot_cycle(&self, remaining: u32, incap: bool) -> f32 {
        if incap {
            return 0.3;
        }
        if self.class == "weapon_pistol_dual" && remaining == 0 {
            0.2
        } else {
            self.cycle
        }
    }
    pub fn damage_at(&self, distance: f32) -> f32 {
        self.damage
            * self
                .range_modifier
                .powf((distance / crate::collision::UNIT / 500.).max(0.))
    }
    pub fn medical(&self) -> bool {
        matches!(
            self.class.as_str(),
            "weapon_first_aid_kit"
                | "weapon_pain_pills"
                | "weapon_adrenaline"
                | "weapon_defibrillator"
        )
    }
    /// Constructor defaults recovered from the installed server.dll.
    pub fn reserve_capacity(&self) -> u32 {
        match self.class.as_str() {
            "weapon_smg" | "weapon_smg_silenced" | "weapon_smg_mp5" => 650,
            "weapon_pumpshotgun" | "weapon_shotgun_chrome" => 72,
            "weapon_autoshotgun" | "weapon_shotgun_spas" => 90,
            "weapon_hunting_rifle" => 150,
            "weapon_sniper_military" | "weapon_sniper_awp" | "weapon_sniper_scout" => 180,
            "weapon_grenade_launcher" => 30,
            class if class.starts_with("weapon_rifle") => 360,
            _ => 0,
        }
    }
}
#[derive(Resource)]
pub struct Rules {
    pub weapons: HashMap<String, Weapon>,
    pub pistol: Weapon,
}
pub struct Companion {
    pub state: Match,
    pub position: Vec3,
    pub spawn: Vec3,
    pub model: String,
    pub vertical: f32,
    pub cooldown: f32,
    pub revive_time: f32,
    pub aim: Vec3,
}
#[derive(Resource)]
pub struct Squad(pub Vec<Companion>);
/// Retail `sb_stop`: survivor bots stand still and take no actions (no
/// movement, attacks, rescues, revives, pickups or healing). Pinned or
/// carried bots are still moved by their attacker. Console-only test aid.
#[cfg(not(test))]
static BOTS_STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(not(test))]
pub fn bots_stopped() -> bool {
    BOTS_STOPPED.load(std::sync::atomic::Ordering::Relaxed)
}
/// Retail `director_stop` / `director_start`: the director spawns nothing
/// (no commons, specials, witches or tanks) while stopped; map logic and
/// scripts keep running. EDIT WORLD stops it while editing.
static DIRECTOR_STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn director_stopped() -> bool {
    DIRECTOR_STOPPED.load(std::sync::atomic::Ordering::Relaxed)
}
pub fn set_director_stopped(stopped: bool) {
    DIRECTOR_STOPPED.store(stopped, std::sync::atomic::Ordering::Relaxed);
}
/// `bot_setpos <survivor> x y z [yaw]` requests (Source units/degrees),
/// applied by the bot update: matched parity captures need bots on exact
/// spots.
/// Retail `kick <survivor>`: queued names, applied by the bot loop.
/// `l4d2r_damage <name|hammerid> [amount]`: debug bullet damage to an entity
/// (fires its OnTakeDamage like a shot; for testing testscripts).
pub static DAMAGE_REQUESTS: std::sync::Mutex<Vec<(String, f32)>> = std::sync::Mutex::new(Vec::new());
pub static KICKS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// Squad slots removed by `kick` (bit per index). A kicked bot is dead to
/// every system and is not drawn, shown on the HUD or defibrillated; a
/// restart or the next chapter brings the team back (retail repopulates).
static KICKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub fn kicked(index: usize) -> bool {
    index < 32 && KICKED.load(std::sync::atomic::Ordering::Relaxed) & (1 << index) != 0
}
pub fn set_kicked(index: usize) {
    if index < 32 {
        KICKED.fetch_or(1 << index, std::sync::atomic::Ordering::Relaxed);
    }
}
pub fn clear_kicked() {
    KICKED.store(0, std::sync::atomic::Ordering::Relaxed);
}
pub static BOT_PLACEMENTS: std::sync::Mutex<Vec<(String, [f32; 3], f32)>> = std::sync::Mutex::new(Vec::new());
#[cfg(not(test))]
pub fn set_bots_stopped(stopped: bool) {
    BOTS_STOPPED.store(stopped, std::sync::atomic::Ordering::Relaxed);
}
// Tests run in parallel threads; keep each test's sb_stop to its own thread.
#[cfg(test)]
thread_local! {
    static BOTS_STOPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
pub fn bots_stopped() -> bool {
    BOTS_STOPPED.with(std::cell::Cell::get)
}
#[cfg(test)]
pub fn set_bots_stopped(stopped: bool) {
    BOTS_STOPPED.with(|s| s.set(stopped));
}
/// Squad members played by other people over the network (bit per squad
/// index; multiplayer). Their bot AI is off: movement, aim and state come
/// from the network (net_play.rs).
static REMOTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub fn remote(index: usize) -> bool {
    index < 8 && REMOTE.load(std::sync::atomic::Ordering::Relaxed) & (1 << index) != 0
}
pub fn set_remote(mask: u8) {
    REMOTE.store(mask, std::sync::atomic::Ordering::Relaxed);
}
#[derive(Clone)]
pub struct Gun {
    pub laser: bool,
    pub ammo_upgrade: Option<crate::upgrades::Kind>,
    pub upgraded_rounds: u32,
    pub class: String,
    pub ammo: u32,
    pub reserve: u32,
}
#[derive(Resource, Clone)]
pub struct Match {
    /// Transient bot NAV-ladder presentation; cleared every AI tick/map carry.
    pub ladder_pose: Option<crate::ai_ladder::Pose>,
    pub infected: bool,
    /// Playable role while `infected`; see special_infected::SpecialClass.
    pub special_class: crate::special_infected::SpecialClass,
    pub pinned: bool,
    /// Which special infected holds this survivor while `pinned`.
    pub pinned_by: Option<crate::special_infected::SpecialClass>,
    /// A pinned survivor may still shoot (a Smoker drag before the choke;
    /// retail tongue_victim_accuracy_penalty applies to that fire).
    pub pin_allows_fire: bool,
    /// Shared infected-player phase: 0 idle, 2 ability in progress,
    /// 3 pin/ride/pummel, 4 claw, 5 ghost. Classes may use 1 and 6..
    pub special_phase: u8,
    /// Set by bullet/club damage sources before `damage`; the infected-player
    /// voice uses `<Class>.PainShort` for these (retail DMG_BULLET|DMG_CLUB).
    pub damage_short: bool,
    pub adrenaline: bool,
    pub defibrillator: bool,
    pub adrenaline_time: f32,
    pub revives: u8,
    pub black_white: bool,
    pub damage_serial: u64,
    pub medical_target: Option<usize>,
    /// Squad member the player is reviving right now (holding use on them).
    /// Bots reviving someone near the player must not show the player's bar.
    pub reviving: Option<usize>,
    pub medical_kind: u8,
    pub medical_damage_serial: u64,
    pub item_use_serial: u64,
    pub burst_left: u8,
    /// Melee attack playing (index into `Weapon::melee_attacks`), the next
    /// one, and when the last swing started (server 103eaf30 combo).
    pub melee_attack: u8,
    pub melee_next: u8,
    pub melee_last: f32,
    pub shot_serial: u64,
    pub accuracy_bloom: f32,
    /// Retail m_vecPunchAngle pitch in degrees (positive kicks the view up).
    /// View only: shots still follow the eye angles.
    pub view_punch: f32,
    pub throwable: Option<String>,
    pub throw_class: Option<String>,
    pub throw_stage: u8,
    pub throw_clock: f32,
    pub shove_serial: u64,
    pub reload_phase: u8,
    pub reload_total: f32,
    pub reload_empty: bool,
    pub health: f32,
    pub temporary: f32,
    pub incap: bool,
    pub incap_health: f32,
    pub dead: bool,
    pub paused: bool,
    pub primary: Option<Gun>,
    pub pistol: Gun,
    pub slot: u8,
    pub carried: Option<String>,
    pub upgrade_pack: Option<crate::upgrades::Kind>,
    pub medkit: bool,
    pub pills: bool,
    pub cooldown: f32,
    pub reload: f32,
    pub action: f32,
    pub action_total: f32,
    /// A held map use (func_button_timed): title and progress 0..1.
    pub hold_bar: Option<(String, f32)>,
    pub shove: f32,
    pub kills: u32,
    pub elapsed: f32,
    pub hint: String,
    pub damage_flash: f32,
    pub mud_serial: u64,
    /// Seconds left as Boomer "IT" (vomited/biled); see special_infected::boomer.
    pub vomited: f32,
    /// Seconds left of the client's post-process pills timer (2 s from
    /// pills_used) and healed timer (3 s from heal_success): engine_post.
    pub post_pills: f32,
    pub post_healed: f32,
    /// Server 0x2ef0 (10349470): set while the player holds walk or crouch,
    /// cleared once they move faster than z_walk_speed (85). Bots following
    /// within sb_separation_range walk while it is set (1038ed10).
    pub walk_flag: bool,
    /// Seconds left of a survivor stagger and its horizontal push direction.
    pub stagger: f32,
    pub stagger_dir: Vec3,
    /// Hanging from a ledge (also `incap`); see ledge_hang.
    pub ledge: Option<crate::ledge_hang::Hang>,
    /// A ledge save's spot on top, for the movement tick to move the player to.
    pub ledge_climb: Option<Vec3>,
}
impl Match {
    pub fn new(pistol: &Weapon) -> Self {
        Self {
            ladder_pose: None,
            infected: false,
            special_class: Default::default(),
            pinned: false,
            pinned_by: None,
            pin_allows_fire: false,
            special_phase: 0,
            damage_short: false,
            adrenaline: false,
            defibrillator: false,
            adrenaline_time: 0.,
            revives: 0,
            black_white: false,
            damage_serial: 0,
            medical_target: None,
            reviving: None,
            medical_kind: 0,
            medical_damage_serial: 0,
            item_use_serial: 0,
            burst_left: 0,
            melee_attack: 0,
            melee_next: 0,
            melee_last: f32::NEG_INFINITY,
            shot_serial: 0,
            accuracy_bloom: 0.,
            view_punch: 0.,
            throwable: None,
            throw_class: None,
            throw_stage: 0,
            throw_clock: 0.,
            shove_serial: 0,
            reload_phase: 0,
            reload_total: 0.,
            reload_empty: false,
            health: 100.,
            temporary: 0.,
            incap: false,
            incap_health: 300.,
            dead: false,
            paused: true,
            primary: None,
            pistol: Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: pistol.class.clone(),
                ammo: pistol.clip,
                reserve: 0,
            },
            slot: 2,
            carried: None,
            upgrade_pack: None,
            medkit: false,
            pills: false,
            cooldown: 0.,
            reload: 0.,
            action: 0.,
            action_total: 0.,
            hold_bar: None,
            shove: 0.,
            kills: 0,
            elapsed: 0.,
            hint: String::new(),
            damage_flash: 0.,
            mud_serial: 0,
            vomited: 0.,
            post_pills: 0.,
            post_healed: 0.,
            walk_flag: false,
            stagger: 0.,
            stagger_dir: Vec3::ZERO,
            ledge: None,
            ledge_climb: None,
        }
    }
    pub fn owns_slot(&self, slot: u8) -> bool {
        match slot {
            1 => self.primary.is_some(),
            2 => true,
            3 => self.throwable.is_some(),
            4 => self.medkit || self.defibrillator || self.upgrade_pack.is_some(),
            5 => self.pills || self.adrenaline,
            _ => false,
        }
    }
    pub fn select_inventory_slot(&mut self, slot: u8) -> bool {
        // A pinned survivor cannot change weapons (any special's grab).
        if self.paused || self.dead || self.incap || self.pinned || !self.owns_slot(slot) {
            return false;
        }
        self.slot = slot;
        self.burst_left = 0;
        self.reload = 0.;
        self.action = 0.;
        true
    }
    pub fn cycle_inventory(&mut self, direction: i32) -> bool {
        if direction == 0 || self.paused || self.dead || self.incap || self.pinned {
            return false;
        }
        for step in 1..=5 {
            let slot = ((self.slot as i32 - 1 + direction.signum() * step).rem_euclid(5) + 1) as u8;
            if self.owns_slot(slot) {
                return slot != self.slot && self.select_inventory_slot(slot);
            }
        }
        false
    }
    pub fn held_class(&self) -> &str {
        if self.infected {
            return self.special_class.weapon();
        }
        match self.slot {
            6 => self.carried.as_deref().unwrap_or("weapon_pistol"),
            3 => self
                .throw_class
                .as_deref()
                .or(self.throwable.as_deref())
                .unwrap_or("weapon_pistol"),
            4 => {
                if let Some(kind) = self.upgrade_pack {
                    return kind.pack_class();
                }
                if self.defibrillator {
                    "weapon_defibrillator"
                } else {
                    "weapon_first_aid_kit"
                }
            }
            5 => {
                if self.adrenaline {
                    "weapon_adrenaline"
                } else {
                    "weapon_pain_pills"
                }
            }
            _ => &self.gun().class,
        }
    }
    /// Player Tank in a rock throw (special_phase 2 wind-up, 6 swing). Retail
    /// leaves first person for the whole throw animation, showing the Tank
    /// pick up and hurl the rock in third person.
    pub fn tank_throwing(&self) -> bool {
        self.infected
            && !self.dead
            && self.special_class == crate::special_infected::SpecialClass::Tank
            && matches!(self.special_phase, 2 | 6)
    }
    /// Alive infected player in an active pin that uses the orbit camera.
    pub fn infected_third_person(&self) -> bool {
        self.infected
            && !self.dead
            && self.special_phase == 3
            && self.special_class.third_person_pin()
    }
    /// Survivor player held by a special infected: the view leaves first
    /// person for an orbit camera on the pin (Hunter pounce, Smoker tongue,
    /// Jockey ride, Charger carry/pummel).
    pub fn survivor_third_person(&self) -> bool {
        !self.infected
            && !self.dead
            && (self.pinned || self.backpack_action() || self.reviving_teammate() || self.ledge.is_some())
    }
    /// Holding use on a downed teammate: retail roots the reviver and shows
    /// them in the third-person action view until the revive ends.
    pub fn reviving_teammate(&self) -> bool {
        !self.infected && !self.dead && !self.incap && !self.pinned && self.reviving.is_some()
    }
    /// Timed backpack-item use that holds the survivor in place and switches
    /// to the third-person view in retail: first aid on yourself (1) or a
    /// teammate (2), the defibrillator (3), or deploying an upgrade pack.
    /// Pills (5) and adrenaline (4) are quick first-person uses.
    pub fn backpack_action(&self) -> bool {
        !self.infected
            && !self.incap
            && self.action > 0.
            && (matches!(self.medical_kind, 1..=3)
                || self.medical_kind == 0 && self.upgrade_pack.is_some() && self.slot == 4)
    }
    /// First-person claws/weapon are hidden: third-person pins, a pinned
    /// survivor's third-person view, and the retail Jockey ride, which stays
    /// first person without a viewmodel.
    pub fn infected_arms_hidden(&self) -> bool {
        self.tank_throwing()
            || self.infected_third_person()
            || self.survivor_third_person()
            // Skate mod: the third-person board view.
            || !self.infected && !self.dead && !self.pinned && crate::skate_mod::riding()
            || self.infected
                && !self.dead
                && self.special_phase == 3
                && self.special_class == crate::special_infected::SpecialClass::Jockey
    }
    pub fn gun(&self) -> &Gun {
        if self.slot == 1 {
            self.primary.as_ref().unwrap_or(&self.pistol)
        } else {
            &self.pistol
        }
    }
    pub fn gun_mut(&mut self) -> &mut Gun {
        if self.slot == 1 {
            self.primary.as_mut().unwrap_or(&mut self.pistol)
        } else {
            &mut self.pistol
        }
    }
    pub fn damage(&mut self, amount: f32) {
        if self.dead || !amount.is_finite() || amount <= 0. {
            return;
        }
        if self.infected && self.special_phase == 5 {
            return;
        }
        self.damage_serial = self.damage_serial.wrapping_add(1);
        if self.infected {
            self.health = (self.health - amount).max(0.);
            self.dead = self.health <= 0.;
            self.incap = false;
            return;
        }
        self.action = 0.;
        self.medical_kind = 0;
        self.damage_flash = 0.4;
        if self.incap {
            self.incap_health = (self.incap_health - amount).max(0.);
            // A hanging survivor lets go instead (ledge_hang::hold).
            self.dead = self.incap_health <= 0. && self.ledge.is_none();
            return;
        }
        let temporary = self.temporary.min(amount);
        self.temporary -= temporary;
        self.health = (self.health - (amount - temporary)).max(0.);
        if self.health <= 0. && self.temporary <= 0. {
            self.dead = self.revives >= 2;
            self.incap = !self.dead;
            self.incap_health = 300.;
            self.slot = 2;
            self.reload = 0.;
            self.action = 0.;
            self.burst_left = 0;
        }
    }
    pub fn pickup_secondary(&mut self, gun: Gun) -> bool {
        if gun.class == "weapon_pistol" && self.pistol.class == "weapon_pistol_dual" {
            return false;
        }
        self.pistol = if gun.class == "weapon_pistol" && self.pistol.class == "weapon_pistol" {
            Gun {
                laser: false,
                ammo_upgrade: None,
                upgraded_rounds: 0,
                class: "weapon_pistol_dual".into(),
                ammo: self.pistol.ammo.saturating_add(gun.ammo).min(30),
                reserve: 0,
            }
        } else {
            gun
        };
        self.slot = 2;
        self.reload = 0.;
        self.reload_phase = 0;
        true
    }
    pub fn begin_reload(&mut self, weapon: &Weapon) {
        self.reload_empty = self.gun().ammo == 0;
        // Retail rifle/SMG probes: magazine rounds return to reserve at reload
        // start, leaving clip=0 until refill. Shell reloads retain their rounds.
        if !weapon.infinite() && !weapon.shell_reload() && !weapon.melee() {
            let gun = self.gun_mut();
            gun.reserve = gun.reserve.saturating_add(gun.ammo);
            gun.ammo = 0;
        }
        self.reload_phase = if weapon.shell_reload() { 1 } else { 0 };
        self.reload = if weapon.class == "weapon_pistol_dual" && self.gun().ammo == 0 {
            2.6
        } else if self.reload_empty
            && matches!(
                weapon.class.as_str(),
                "weapon_pistol" | "weapon_pistol_magnum"
            )
        {
            2.1
        } else if self.reload_empty && !weapon.shell_reload() && weapon.reload_empty_time > 0. {
            weapon.reload_empty_time
        } else {
            weapon.reload_times[0]
        };
        self.reload_total = self.reload;
    }
    pub fn reload_complete(&mut self, weapon: &Weapon) {
        let gun = self.gun_mut();
        let missing = weapon.clip.saturating_sub(gun.ammo);
        let added = if weapon.infinite() {
            missing
        } else {
            missing.min(gun.reserve)
        };
        gun.ammo += added;
        if !weapon.infinite() {
            gun.reserve -= added;
        }
        self.reload = 0.;
    }
    pub fn advance_reload(&mut self, weapon: &Weapon, dt: f32) {
        if self.reload <= 0. {
            return;
        }
        self.reload -= dt;
        if self.reload > dt * 0.001 {
            return;
        }
        if !weapon.shell_reload() || self.reload_phase == 0 {
            self.reload_complete(weapon);
            return;
        }
        if self.reload_phase == 3 {
            self.reload_phase = 0;
            self.reload = 0.;
            return;
        }
        if self.reload_phase == 2 || weapon.measured_shell_reload() {
            let gun = self.gun_mut();
            if gun.reserve > 0 && gun.ammo < weapon.clip {
                gun.ammo += 1;
                gun.reserve -= 1;
            }
        }
        if self.gun().ammo >= weapon.clip || self.gun().reserve == 0 {
            self.reload_phase = 3;
            self.reload = weapon.reload_times[2];
        } else {
            self.reload_phase = 2;
            self.reload = weapon.reload_times[1];
        }
        self.reload_total = self.reload;
    }
    pub fn revive(&mut self) {
        if self.ledge.is_some() {
            self.ledge_saved();
            return;
        }
        self.revives = self.revives.saturating_add(1);
        self.black_white = self.revives >= 2;
        self.incap = false;
        self.incap_health = 300.;
        self.health = 1.;
        self.temporary = 29.;
    }
    pub fn heal(&mut self) {
        self.receive_heal();
        self.medkit = false;
        self.slot = 2;
    }
    pub fn receive_heal(&mut self) {
        self.post_healed = 3.;
        self.revives = 0;
        self.black_white = false;
        self.health += (100. - self.health) * 0.8;
        self.temporary = 0.;
    }
    pub fn resuscitate(&mut self) {
        self.dead = false;
        self.incap = false;
        self.health = 50.;
        self.temporary = 0.;
        self.revives = 0;
        self.black_white = false;
        self.incap_health = 300.;
        self.action = 0.;
        self.medical_kind = 0;
        self.reload = 0.;
        self.slot = 2;
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn reviving_a_teammate_roots_the_player_in_the_third_person_view() {
        let mut state = Match::new(&pistol());
        assert!(!state.reviving_teammate() && !state.survivor_third_person());
        state.reviving = Some(1);
        assert!(state.reviving_teammate() && state.survivor_third_person());
        assert!(state.infected_arms_hidden(), "no first-person weapon while reviving");
        state.incap = true;
        assert!(!state.reviving_teammate(), "a downed player cannot revive");
    }
    #[test]
    fn empty_reload_selection_survives_magazine_removal_and_restarts_correctly() {
        let mut weapon = pistol();
        weapon.class = "weapon_rifle".into();
        weapon.reload_empty_time = 3.;
        let mut state = Match::new(&weapon);
        state.pistol.ammo = 8;
        state.pistol.reserve = 50;
        state.begin_reload(&weapon);
        assert_eq!(state.gun().ammo, 0);
        assert!(!state.reload_empty);
        assert_eq!(state.reload_total, weapon.reload_times[0]);
        state.begin_reload(&weapon);
        assert!(state.reload_empty);
        assert_eq!(state.reload_total, 3.);
        weapon.class = "weapon_pistol".into();
        weapon.calibrate_reload_times([1.666667, 0., 0.]);
        state.pistol.ammo = 0;
        state.begin_reload(&weapon);
        assert_eq!(state.reload_total, 2.1);
        state.pistol.ammo = 8;
        state.begin_reload(&weapon);
        assert!(!state.reload_empty);
        assert!((state.reload_total - 1.766667).abs() < 1e-5);
    }
    #[test]
    fn inventory_cycle_wraps_skips_empty_slots_and_keeps_alternative_items() {
        let mut s = Match::new(&pistol());
        s.paused = false;
        s.primary = Some(s.pistol.clone());
        s.slot = 1;
        s.throwable = Some("weapon_pipe_bomb".into());
        s.defibrillator = true;
        s.adrenaline = true;
        s.pistol.class = "weapon_pistol_dual".into();
        for expected in [2, 3, 4, 5, 1] {
            assert!(s.cycle_inventory(1));
            assert_eq!(s.slot, expected);
        }
        for expected in [5, 4, 3, 2, 1] {
            assert!(s.cycle_inventory(-1));
            assert_eq!(s.slot, expected);
        }
        s.primary = None;
        s.throwable = None;
        s.defibrillator = false;
        s.adrenaline = false;
        s.slot = 2;
        s.reload = 1.;
        assert!(!s.cycle_inventory(1));
        assert_eq!((s.slot, s.reload), (2, 1.));
        s.pills = true;
        assert!(s.cycle_inventory(-1));
        assert_eq!((s.slot, s.reload), (5, 0.));
        for blocked in 0..3 {
            s.paused = blocked == 0;
            s.dead = blocked == 1;
            s.incap = blocked == 2;
            assert!(!s.cycle_inventory(1));
            assert_eq!(s.slot, 5);
        }
    }
    #[test]
    fn measured_pump_reload_inserts_on_timer_boundaries_and_finishes_after_closing() {
        let mut weapon = pistol();
        weapon.class = "weapon_pumpshotgun".into();
        weapon.clip = 8;
        weapon.calibrate_reload_times([0.483333, 0.666667, 0.6]);
        let mut s = Match::new(&pistol());
        s.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: weapon.class.clone(),
            ammo: 4,
            reserve: 50,
        });
        s.slot = 1;
        s.begin_reload(&weapon);
        for shell in 1..=4 {
            let ticks = if shell == 1 { 29 } else { 30 };
            for _ in 0..ticks - 1 {
                s.advance_reload(&weapon, 1. / 60.);
            }
            assert_eq!(s.gun().ammo, 3 + shell);
            s.advance_reload(&weapon, 1. / 60.);
            assert_eq!((s.gun().ammo, s.gun().reserve), (4 + shell, 50 - shell));
            assert!(s.reload > 0.);
        }
        assert_eq!(s.reload_phase, 3);
        for _ in 0..36 {
            s.advance_reload(&weapon, 1. / 60.);
        }
        assert_eq!(s.reload, 0.);
        // A nearly empty reserve must stop after the available shell and close.
        s.gun_mut().ammo = 4;
        s.gun_mut().reserve = 1;
        s.begin_reload(&weapon);
        for _ in 0..29 {
            s.advance_reload(&weapon, 1. / 60.);
        }
        assert_eq!((s.gun().ammo, s.gun().reserve, s.reload_phase), (5, 0, 3));
    }
    #[test]
    fn retail_magazine_reload_transfers_rounds_and_keeps_total_ammo_on_completion_or_cancel() {
        let mut weapon = pistol();
        weapon.class = "weapon_rifle".into();
        weapon.clip = 50;
        weapon.reload_times[0] = 2.2;
        let mut s = Match::new(&pistol());
        s.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: weapon.class.clone(),
            ammo: 46,
            reserve: 275,
        });
        s.slot = 1;
        s.begin_reload(&weapon);
        assert_eq!((s.gun().ammo, s.gun().reserve), (0, 321));
        s.reload = 0.;
        assert_eq!(s.gun().ammo + s.gun().reserve, 321);
        s.begin_reload(&weapon);
        s.reload_complete(&weapon);
        assert_eq!((s.gun().ammo, s.gun().reserve), (50, 271));
        weapon.class = "weapon_pumpshotgun".into();
        weapon.clip = 8;
        s.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: weapon.class.clone(),
            ammo: 4,
            reserve: 50,
        });
        s.begin_reload(&weapon);
        assert_eq!((s.gun().ammo, s.gun().reserve), (4, 50));
        assert_eq!(s.reload_phase, 1);
    }
    pub(crate) fn pistol() -> Weapon {
        Weapon {
            penetration: Default::default(),
            pellets: 1,
            range_modifier: 1.,
            scatter: Vec2::ZERO,
            accuracy: Default::default(),
            muzzle_effect: String::new(),
            class: "weapon_pistol".into(),
            clip: 15,
            damage: 36.,
            cycle: 0.175,
            range: 63.5,
            speed: 6.35,
            punch: 2.,
            model: String::new(),
            view_model: String::new(),
            reload_times: [1.7, 0.5, 0.5],
            reload_empty_time: 0.,
            damage_flags: 2,
            decapitates: false,
            player_model: String::new(),
            muzzle_forward: [0., 0., 1.],
            fire_sound: "Pistol.Fire".into(),
            melee_hit_sound: String::new(),
            melee_world_sound: String::new(),
            melee_attacks: vec![],
            icon: String::new(),
            icon_rect: Rect::default(),
        }
    }
    #[test]
    fn third_strike_is_fatal_and_healing_resets_the_counter() {
        let mut s = Match::new(&pistol());
        for _ in 0..2 {
            s.damage(200.);
            assert!(s.incap && !s.dead);
            s.revive();
        }
        assert!(s.black_white);
        s.damage(200.);
        assert!(s.dead && !s.incap);
        s.resuscitate();
        assert!(!s.dead && !s.black_white);
        assert_eq!(s.health, 50.);
        s.damage(f32::NAN);
        assert_eq!(s.health, 50.);
    }
    #[test]
    fn recovered_ammo_defaults_and_range_attenuation_are_used() {
        let mut w = pistol();
        w.class = "weapon_rifle".into();
        assert_eq!(w.reserve_capacity(), 360);
        w.class = "weapon_shotgun_spas".into();
        assert_eq!(w.reserve_capacity(), 90);
        w.class = "weapon_hunting_rifle".into();
        assert_eq!(w.reserve_capacity(), 150);
        w.range_modifier = 0.5;
        assert!((w.damage_at(500. * crate::collision::UNIT) - 18.).abs() < 0.001);
    }
    #[test]
    fn infinite_pistol_reload_and_finite_primary_reserve() {
        let w = pistol();
        let mut s = Match::new(&w);
        s.pistol.ammo = 2;
        s.reload_complete(&w);
        assert_eq!(s.pistol.ammo, 15);
        assert_eq!(s.pistol.reserve, 0);
        let mut primary = w.clone();
        primary.class = "weapon_smg".into();
        primary.clip = 50;
        s.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: primary.class.clone(),
            ammo: 45,
            reserve: 3,
        });
        s.slot = 1;
        s.reload_complete(&primary);
        assert_eq!(s.gun().ammo, 48);
        assert_eq!(s.gun().reserve, 0);
    }
    #[test]
    fn damage_incap_revive_and_healing_change_live_hud_state() {
        let mut s = Match::new(&pistol());
        s.temporary = 10.;
        s.damage(15.);
        assert_eq!(s.health, 95.);
        s.damage(100.);
        assert!(s.incap && !s.dead);
        s.revive();
        assert_eq!(s.health + s.temporary, 30.);
        s.medkit = true;
        s.heal();
        assert!((s.health - 80.2).abs() < 0.01);
        assert!(!s.medkit);
        s.damage(100.);
        s.damage(301.);
        assert!(s.dead);
    }
}

#[cfg(test)]
pub(crate) fn tests_weapon() -> Weapon {
    tests::pistol()
}

#[cfg(test)]
mod dual_tests {
    use super::*;
    #[test]
    fn second_pistol_preserves_rounds_upgrades_secondary_and_keeps_primary() {
        let mut state = Match::new(&tests_weapon());
        state.pistol.ammo = 7;
        state.primary = Some(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: "weapon_rifle".into(),
            ammo: 46,
            reserve: 275,
        });
        state.slot = 1;
        assert!(state.pickup_secondary(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: "weapon_pistol".into(),
            ammo: 15,
            reserve: 0
        }));
        assert_eq!(state.pistol.class, "weapon_pistol_dual");
        assert_eq!(state.pistol.ammo, 22);
        assert_eq!(state.slot, 2);
        assert_eq!(state.primary.as_ref().unwrap().ammo, 46);
        assert!(!state.pickup_secondary(Gun {
            laser: false,
            ammo_upgrade: None,
            upgraded_rounds: 0,
            class: "weapon_pistol".into(),
            ammo: 15,
            reserve: 0
        }));
        assert_eq!(state.pistol.ammo, 22);
        let mut dual = tests_weapon();
        dual.class = "weapon_pistol_dual".into();
        dual.clip = 30;
        dual.cycle = 0.075;
        dual.calibrate_reload_times([1., 1., 1.]);
        state.begin_reload(&dual);
        state.advance_reload(&dual, 2.39);
        assert_eq!(state.pistol.ammo, 22);
        state.advance_reload(&dual, 0.01);
        assert_eq!(state.pistol.ammo, 30);
        state.pistol.ammo = 0;
        state.begin_reload(&dual);
        assert_eq!(state.reload_total, 2.6);
        state.advance_reload(&dual, 2.59);
        assert_eq!(state.pistol.ammo, 0);
        state.advance_reload(&dual, 0.01);
        assert_eq!(state.pistol.ammo, 30);
        assert_eq!(dual.shot_cycle(29, false), 0.075);
        assert_eq!(dual.shot_cycle(0, false), 0.2);
        assert_eq!(dual.shot_cycle(29, true), 0.3);
    }
    #[test]
    fn owned_melee_icons_and_impact_sounds_are_valid() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let sounds = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "fireaxe",
            "crowbar",
            "frying_pan",
            "machete",
            "katana",
            "baseball_bat",
            "cricket_bat",
            "electric_guitar",
            "tonfa",
            "knife",
            "golfclub",
            "pitchfork",
            "shovel",
        ] {
            let weapon = Weapon::load(&mut assets, &format!("weapon_melee_{name}")).unwrap();
            let image = crate::vtf::decode(&assets.read(&weapon.icon).unwrap(), 1024).unwrap();
            assert!(weapon.icon_rect.min.cmpge(Vec2::ZERO).all(), "{name}");
            assert!(
                weapon
                    .icon_rect
                    .max
                    .cmple(Vec2::new(image.width as f32, image.height as f32))
                    .all(),
                "{name}"
            );
            for sound in [&weapon.melee_hit_sound, &weapon.melee_world_sound] {
                let definition = sounds
                    .definitions
                    .get(&sound.to_lowercase())
                    .unwrap_or_else(|| panic!("{name}: missing {sound}"));
                assert!(!definition.waves.is_empty(), "{name}: {sound}");
            }
            // Retail primary attacks: a swing window inside the animation.
            // (The knife has one: its stab is commented out in the script.)
            assert!(!weapon.melee_attacks.is_empty(), "{name}: {:?}", weapon.melee_attacks);
            for attack in &weapon.melee_attacks {
                assert!(
                    0. <= attack.start_time && attack.start_time < attack.end_time && attack.end_time <= attack.duration,
                    "{name}: {attack:?}"
                );
                assert!(attack.activity.starts_with("ACT_VM_"), "{name}: {attack:?}");
            }
            assert!((weapon.range / crate::collision::UNIT - 70.).abs() < 1e-3);
        }
        let axe = Weapon::load(&mut assets, "weapon_melee_fireaxe").unwrap();
        assert_eq!(
            axe.melee_attacks[1],
            MeleeAttack {
                start: 3,
                end: 2,
                duration: 1.23,
                start_time: 0.1,
                end_time: 0.33,
                activity: "ACT_VM_HITLEFT".into(),
            }
        );
    }
    #[test]
    fn owned_dual_models_and_remapped_actions_are_available() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = std::cell::RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let dual = Weapon::load(&mut assets.borrow_mut(), "weapon_pistol_dual").unwrap();
        let single = Weapon::load(&mut assets.borrow_mut(), "weapon_pistol").unwrap();
        assert_eq!(single.icon_rect, Rect::new(64., 0., 128., 64.));
        assert_eq!(dual.icon_rect, Rect::new(0., 0., 64., 64.));
        assert_eq!(dual.icon, single.icon);
        assert_eq!(dual.clip, 30);
        assert!(dual.infinite());
        let skeleton = l4d2_formats::anim::Skeleton::load_with(&dual.view_model, |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let report = skeleton
            .sequences()
            .iter()
            .map(|s| format!("{}\t{}\t{}\n", s.label, s.activity_name, s.activity))
            .collect::<String>();
        std::fs::write("local-research/parity/dual-pistol-sequences.tsv", report).unwrap();
        for action in [
            "ACT_VM_PRIMARYATTACK",
            "ACT_VM_RELOAD",
            "ACT_VM_IDLE",
            "ACT_VM_MELEE",
        ] {
            assert!(
                skeleton
                    .sequences()
                    .iter()
                    .any(|s| s.activity_name.eq_ignore_ascii_case(action)),
                "missing native dual action {action}"
            );
        }
        for (ammo, label) in [
            (29, "fire_right"),
            (28, "fire_left"),
            (1, "fire_last_right"),
            (0, "fire_last_left"),
        ] {
            let seq = crate::l4d2r_animation::fire_sequence(&skeleton, &dual.class, ammo).unwrap();
            assert_eq!(skeleton.sequences()[seq].label, label);
            let pose = skeleton.pose_retargeted_view_sequence(seq, 0.05, false);
            assert!(pose.iter().all(|m| m.is_finite()));
        }
    }
}
