//! Named L4D2 game state on top of the decoded entity world: who is playing,
//! where everyone is, health / incapacitation / pins, held weapons, the
//! infected. This is what the rewrite's guest mode consumes.

use crate::entities::{PlayerInfo, StringTables, Value, World};

/// Player-shaped server classes (survivors, survivor bots, special infected).
pub const PLAYER_CLASSES: &[&str] =
    &["CTerrorPlayer", "SurvivorBot", "Boomer", "Charger", "Hunter", "Jockey", "Smoker", "Spitter", "Tank"];

pub const TEAM_SURVIVORS: i64 = 2;
pub const TEAM_INFECTED: i64 = 3;

/// L4D2 survivor characters (m_survivorCharacter).
pub fn survivor_name(character: i64) -> &'static str {
    match character {
        0 => "Nick",
        1 => "Rochelle",
        2 => "Coach",
        3 => "Ellis",
        4 => "Bill",
        5 => "Zoey",
        6 => "Francis",
        7 => "Louis",
        _ => "?",
    }
}

/// Networked entity handle -> entity index (11 index bits; all-ones = none).
pub fn handle_index(h: i64) -> Option<u32> {
    let h = h as u32;
    if h & 0x1F_FFFF == 0x1F_FFFF {
        return None;
    }
    let idx = h & 0x7FF;
    (idx != 0x7FF).then_some(idx)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub entity: u32,
    pub class: String,
    pub name: String,
    pub xuid: u64,
    pub team: i64,
    pub character: i64,
    pub zombie_class: i64,
    pub health: i64,
    pub max_health: i64,
    pub temp_health: f32,
    pub incapacitated: bool,
    pub hanging: bool,
    pub alive: bool,
    pub ghost: bool,
    pub origin: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub active_weapon: Option<String>,
    /// Special infected currently holding this survivor.
    pub pinned_by: Option<u32>,
    /// Model path (modelprecache), when the dictionary is loaded.
    pub model: Option<String>,
    /// Weapon classes this player owns (weapon entities whose m_hOwner is them).
    pub weapons: Vec<String>,
    /// The same weapons with their entity indexes.
    pub weapon_ids: Vec<(u32, String)>,
    /// Bots: entity of the idle human waiting to take this bot back (0 = none).
    pub idle_human: i64,
    /// Playing sequence index on the model.
    pub sequence: i64,
    /// userinfo user id (game events name players by it).
    pub user_id: i32,
    /// Crouched (m_fFlags FL_DUCKING).
    pub ducked: bool,
    /// Exact origin when the server sends it (only for our own player).
    pub precise_origin: Option<[f32; 3]>,
    /// Observer mode and target entity (spectators).
    pub observer_mode: i64,
    pub observer_target: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Infected {
    pub entity: u32,
    pub class: String,
    pub origin: [f32; 3],
    pub model: Option<String>,
    /// Facing (Source yaw, degrees).
    pub yaw: f32,
    pub burning: bool,
    /// Playing sequence index on the model.
    pub sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameState {
    pub tick: u32,
    pub players: Vec<Player>,
    pub infected: Vec<Infected>,
    pub doors: Vec<Door>,
}

/// A prop door (prop_door_rotating, checkpoint doors).
#[derive(Debug, Clone, PartialEq)]
pub struct Door {
    pub entity: u32,
    pub class: String,
    pub origin: [f32; 3],
    /// Source DoorState: 0 closed, 1 opening, 2 open, 3 closing, 4 ajar.
    pub state: i64,
}

fn int(w: &World, id: u32, name: &str) -> Option<i64> {
    w.prop(id, name).and_then(Value::as_int)
}
fn float(w: &World, id: u32, name: &str) -> Option<f32> {
    w.prop(id, name).and_then(Value::as_float)
}

fn model(w: &World, tables: &StringTables, id: u32) -> Option<String> {
    let i = int(w, id, "m_nModelIndex")?;
    tables.string("modelprecache", i as usize).map(str::to_string)
}

/// Player records from the `userinfo` table (entry n = entity n + 1).
pub fn player_infos(tables: &StringTables) -> Vec<Option<PlayerInfo>> {
    tables
        .get("userinfo")
        .map(|t| t.entries.iter().map(|(_, d)| d.as_deref().and_then(PlayerInfo::parse)).collect())
        .unwrap_or_default()
}

pub fn game_state(w: &World, tables: &StringTables) -> GameState {
    let infos = player_infos(tables);
    let mut ids: Vec<u32> = w.entities.keys().copied().collect();
    ids.sort_unstable();
    let mut gs = GameState { tick: w.tick, ..Default::default() };
    for id in ids {
        let Some(class) = w.class_name(id) else { continue };
        if PLAYER_CLASSES.contains(&class) {
            let info = infos.get(id as usize - 1).cloned().flatten();
            let weapon = int(w, id, "m_hActiveWeapon").and_then(handle_index).and_then(|i| w.class_name(i)).map(str::to_string);
            let pinned_by = ["m_pounceAttacker", "m_tongueOwner", "m_pummelAttacker", "m_jockeyAttacker", "m_carryAttacker"]
                .iter()
                .find_map(|n| int(w, id, n).and_then(handle_index));
            gs.players.push(Player {
                entity: id,
                class: class.to_string(),
                name: info.as_ref().map(|i| i.name.clone()).unwrap_or_default(),
                xuid: info.as_ref().map(|i| i.xuid).unwrap_or(0),
                user_id: info.as_ref().map(|i| i.user_id).unwrap_or(-1),
                ducked: int(w, id, "m_fFlags").unwrap_or(0) & 2 != 0,
                precise_origin: w.precise_origin(id),
                team: int(w, id, "m_iTeamNum").unwrap_or(0),
                character: int(w, id, "m_survivorCharacter").unwrap_or(-1),
                zombie_class: int(w, id, "m_zombieClass").unwrap_or(0),
                health: int(w, id, "m_iHealth").unwrap_or(0),
                max_health: int(w, id, "m_iMaxHealth").unwrap_or(0),
                temp_health: float(w, id, "m_healthBuffer").unwrap_or(0.0),
                incapacitated: int(w, id, "m_isIncapacitated").unwrap_or(0) != 0,
                hanging: int(w, id, "m_isHangingFromLedge").unwrap_or(0) != 0,
                alive: int(w, id, "m_lifeState").unwrap_or(0) == 0,
                idle_human: int(w, id, "m_humanSpectatorEntIndex").unwrap_or(0),
                observer_mode: int(w, id, "m_iObserverMode").unwrap_or(0),
                sequence: int(w, id, "m_nSequence").unwrap_or(-1),
                observer_target: int(w, id, "m_hObserverTarget").and_then(|h| handle_index(h)),
                ghost: int(w, id, "m_isGhost").unwrap_or(0) != 0,
                origin: w.origin(id).unwrap_or_default(),
                pitch: float(w, id, "m_angEyeAngles[0]").unwrap_or(0.0),
                yaw: float(w, id, "m_angEyeAngles[1]").unwrap_or(0.0),
                active_weapon: weapon,
                pinned_by,
                model: model(w, tables, id),
                weapons: Vec::new(),
                weapon_ids: Vec::new(),
            });
        } else if class.contains("Door") && w.prop(id, "m_eDoorState").is_some() {
            gs.doors.push(Door { entity: id, class: class.to_string(), origin: w.origin(id).unwrap_or_default(),
                state: int(w, id, "m_eDoorState").unwrap_or(0) });
        } else if class == "Infected" || class == "Witch" {
            gs.infected.push(Infected { entity: id, class: class.to_string(), origin: w.origin(id).unwrap_or_default(),
                model: model(w, tables, id),
                yaw: float(w, id, "m_angRotation[1]").unwrap_or(0.0),
                burning: int(w, id, "m_bIsBurning").unwrap_or(0) != 0,
                sequence: int(w, id, "m_nSequence").unwrap_or(-1) });
        }
    }
    // Owned weapons: weapon entities point at their owner.
    let mut owned: Vec<(u32, u32, String)> = Vec::new();
    for (&id, _) in &w.entities {
        let Some(class) = w.class_name(id) else { continue };
        if let Some(owner) = int(w, id, "m_hOwner").and_then(handle_index) {
            // Weapons only: view models are owned by the player too.
            if owner != 0 && !class.contains("ViewModel") {
                owned.push((owner, id, class.to_string()));
            }
        }
    }
    owned.sort();
    for p in &mut gs.players {
        p.weapon_ids = owned.iter().filter(|(o, _, _)| *o == p.entity).map(|(_, id, c)| (*id, c.clone())).collect();
        p.weapons = p.weapon_ids.iter().map(|(_, c)| c.clone()).collect();
    }
    gs
}

/// Rewrite / retail weapon name ("weapon_smg_silenced") for a 360 server class ("CSMG_Silenced").
pub fn weapon_name(class: &str) -> String {
    let special = match class {
        "CAssaultRifle" => Some("weapon_rifle"),
        "CSubMachinegun" => Some("weapon_smg"),
        "CSniperRifle" => Some("weapon_hunting_rifle"),
        "CMagnumPistol" => Some("weapon_pistol_magnum"),
        "CTerrorMeleeWeapon" => Some("weapon_melee"),
        "CFirstAidKit" => Some("weapon_first_aid_kit"),
        "CPainPills" => Some("weapon_pain_pills"),
        "CPipeBomb" => Some("weapon_pipe_bomb"),
        "CVomitJar" => Some("weapon_vomitjar"),
        "CGrenadeLauncher" => Some("weapon_grenade_launcher"),
        "CItemUpgradePackIncendiary" => Some("weapon_upgradepack_incendiary"),
        "CItemUpgradePackExplosive" => Some("weapon_upgradepack_explosive"),
        _ => None,
    };
    if let Some(s) = special {
        return s.to_string();
    }
    let base = class.strip_prefix('C').unwrap_or(class);
    format!("weapon_{}", base.to_ascii_lowercase())
}

/// Weapon slot (0 primary, 1 secondary, 2 throwable, 3 kit, 4 pills) of a weapon name.
pub fn weapon_slot(name: &str) -> u8 {
    match name {
        "weapon_pistol" | "weapon_pistol_magnum" | "weapon_melee" | "weapon_chainsaw" => 1,
        "weapon_molotov" | "weapon_pipe_bomb" | "weapon_vomitjar" => 2,
        "weapon_first_aid_kit" | "weapon_defibrillator" | "weapon_upgradepack_incendiary" | "weapon_upgradepack_explosive" => 3,
        "weapon_pain_pills" | "weapon_adrenaline" => 4,
        _ => 0,
    }
}
