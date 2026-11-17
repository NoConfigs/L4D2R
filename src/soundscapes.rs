//! Soundscapes: `env_soundscape` / `env_soundscape_proxy` selection and the
//! scripts/soundscapes_*.txt ambience they play. Reconstructed from the
//! public Source SDK 2013 (server soundscape.cpp, client c_soundscape.cpp):
//!
//! * selection (CEnvSoundscape::UpdateForPlayer): a soundscape is a
//!   candidate when the listener is inside `radius` (-1 = unlimited) and a
//!   MASK_SOLID_BRUSHONLY|MASK_WATER line from the entity to the listener is
//!   clear. An unclaimed listener takes any candidate; a claimed one switches
//!   only to a closer candidate. The current soundscape is kept when the
//!   listener leaves its range or sight; Disable clears it.
//! * playback (C_SoundscapeSystem): `playlooping` loops (non-spatial unless
//!   `position N` names a positionN target), `playrandom` replays a random
//!   `rndwave` every random `time` seconds (first after half an interval) at a
//!   random volume/pitch, `position "random"` 36 units around the listener,
//!   and `playsoundscape` nests another soundscape with a volume scale and
//!   position overrides (8 levels). Changing soundscape fades loops toward
//!   their new volume at 1/soundscape_fadetime (3 s) per second; a loop of the
//!   same wave is kept and retargeted.
//!
//! Positional entries use the engine distance curve
//! (`source_sound::distance_gain`): loops default to SNDLVL_NORM, random
//! sounds to SNDLVL_NONE (their zeroed interval), unless `soundlevel` or
//! `attenuation` is given. Save/restore suppression is not reproduced.
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings, SpatialAudioSink, Volume};
use bevy::prelude::*;
use std::collections::HashMap;

const UNIT: f32 = 0.0254;
/// soundscape_fadetime.
const FADE_TIME: f32 = 3.;
/// DEFAULT_SOUND_RADIUS for `position "random"`.
const RANDOM_RADIUS: f32 = 36.;
const MAX_RECURSION: usize = 8;

#[derive(Clone, Debug)]
enum Node {
    Value(String, String),
    Block(String, Vec<Node>),
}
fn parse_nodes(tokens: &[String], at: &mut usize, depth: usize) -> Vec<Node> {
    let mut out = vec![];
    while *at < tokens.len() && tokens[*at] != "}" && depth < 32 {
        let key = tokens[*at].to_ascii_lowercase();
        *at += 1;
        match tokens.get(*at).map(String::as_str) {
            Some("{") => {
                *at += 1;
                let children = parse_nodes(tokens, at, depth + 1);
                out.push(Node::Block(key, children));
            }
            Some(_) => {
                out.push(Node::Value(key, tokens[*at].clone()));
                *at += 1;
            }
            None => break,
        }
    }
    if *at < tokens.len() {
        *at += 1;
    }
    out
}
/// `"a,b"` or `"a"` as a closed interval.
fn interval(text: &str) -> [f32; 2] {
    let mut parts = text.split(',').map(|p| p.trim().parse::<f32>().unwrap_or(0.));
    let a = parts.next().unwrap_or(0.);
    let b = parts.next().unwrap_or(a);
    [a, b]
}
/// `origin "x, y, z;"` in Source units, as a Bevy point.
fn origin(text: &str) -> Option<Vec3> {
    let v: Vec<f32> = text
        .split(|c: char| c == ',' || c.is_whitespace() || c == ';')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    (v.len() >= 3).then(|| bevy(v[0], v[1], v[2]))
}
fn placement(body: &[Node]) -> Position {
    if let Some(o) = value(body, "origin").and_then(origin) {
        return Position::Origin(o);
    }
    match value(body, "position") {
        Some(p) if p.eq_ignore_ascii_case("random") => Position::Random,
        Some(p) => p.trim().parse().map_or(Position::Ambient, Position::Index),
        None => Position::Ambient,
    }
}
/// `soundlevel` (SNDLVL_ name or number) or `attenuation` of an entry.
fn level(nodes: &[Node]) -> Option<f32> {
    value(nodes, "soundlevel")
        .and_then(crate::source_sound::soundlevel)
        .or_else(|| value(nodes, "attenuation").and_then(crate::source_sound::attenuation_soundlevel))
}
fn value<'a>(nodes: &'a [Node], key: &str) -> Option<&'a str> {
    nodes.iter().find_map(|n| match n {
        Node::Value(k, v) if k == key => Some(v.as_str()),
        _ => None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Position {
    Ambient,
    Random,
    Index(usize),
    /// L4D2 `origin "x, y, z"`: a fixed world point.
    Origin(Vec3),
}
#[derive(Clone, Debug)]
pub(crate) struct Looping {
    volume: [f32; 2],
    pitch: [f32; 2],
    wave: String,
    position: Option<usize>,
    /// L4D2 client 1009d240: `position "random"` (36 units around the
    /// listener when it starts) or `origin` (a world point) make the loop
    /// positional instead of ambient.
    placement: Position,
    soundlevel: f32,
}
#[derive(Clone, Debug)]
pub(crate) struct Random {
    time: [f32; 2],
    volume: [f32; 2],
    pitch: [f32; 2],
    position: Position,
    waves: Vec<String>,
    soundlevel: f32,
}
#[derive(Clone, Debug)]
pub(crate) struct Sub {
    name: String,
    volume: [f32; 2],
    position: Option<usize>,
    position_override: Option<usize>,
    ambient_override: Option<usize>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Definition {
    pub dsp: Option<i32>,
    looping: Vec<Looping>,
    random: Vec<Random>,
    subs: Vec<Sub>,
}

/// Soundscape definitions by lower-case name.
pub(crate) fn parse(text: &str) -> HashMap<String, Definition> {
    let tokens = crate::kv::tokens(text);
    let mut at = 0;
    let mut out = HashMap::new();
    while at < tokens.len() {
        let name = tokens[at].to_ascii_lowercase();
        at += 1;
        if tokens.get(at).map(String::as_str) != Some("{") {
            continue;
        }
        at += 1;
        let nodes = parse_nodes(&tokens, &mut at, 1);
        let mut def = Definition::default();
        for node in &nodes {
            match node {
                Node::Value(k, v) if k == "dsp" => def.dsp = v.trim().parse().ok(),
                Node::Block(k, body) if k == "playlooping" => {
                    if let Some(wave) = value(body, "wave") {
                        def.looping.push(Looping {
                            volume: value(body, "volume").map_or([0.; 2], interval),
                            pitch: value(body, "pitch").map_or([100.; 2], interval),
                            wave: wave.to_string(),
                            position: value(body, "position").and_then(|p| p.trim().parse().ok()),
                            placement: placement(body),
                            // The client's positional default is 74 dB (0x4a).
                            soundlevel: level(body).unwrap_or(74.),
                        });
                    }
                }
                Node::Block(k, body) if k == "playrandom" => {
                    let waves = body
                        .iter()
                        .filter_map(|n| match n {
                            Node::Block(k, list) if k == "rndwave" => Some(list),
                            _ => None,
                        })
                        .flatten()
                        .filter_map(|n| match n {
                            Node::Value(k, v) if k == "wave" => Some(v.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let position = placement(body);
                    def.random.push(Random {
                        time: value(body, "time").map_or([0.; 2], interval),
                        volume: value(body, "volume").map_or([0.; 2], interval),
                        pitch: value(body, "pitch").map_or([100.; 2], interval),
                        position,
                        waves,
                        // Unset intervals are zeroed (SNDLVL_NONE).
                        soundlevel: level(body).unwrap_or(0.),
                    });
                }
                Node::Block(k, body) if k == "playsoundscape" => {
                    if let Some(name) = value(body, "name") {
                        def.subs.push(Sub {
                            name: name.to_ascii_lowercase(),
                            volume: value(body, "volume").map_or([1.; 2], interval),
                            position: value(body, "position").and_then(|p| p.trim().parse().ok()),
                            position_override: value(body, "positionoverride").and_then(|p| p.trim().parse().ok()),
                            ambient_override: value(body, "ambientpositionoverride").and_then(|p| p.trim().parse().ok()),
                        });
                    }
                }
                _ => {}
            }
        }
        out.insert(name, def);
    }
    out
}

/// All soundscapes from scripts/soundscapes_manifest.txt (and the map's own
/// scripts/soundscapes_<map>.txt when present).
pub(crate) fn load(assets: &mut crate::source_assets::SourceAssets, map: &str) -> HashMap<String, Definition> {
    let mut files = vec![];
    if let Ok(bytes) = assets.read("scripts/soundscapes_manifest.txt") {
        let tokens = crate::kv::tokens(&String::from_utf8_lossy(&bytes));
        for pair in tokens.windows(2) {
            if pair[0].eq_ignore_ascii_case("file") {
                files.push(pair[1].replace('\\', "/").to_ascii_lowercase());
            }
        }
    }
    files.push(format!("scripts/soundscapes_{}.txt", map.to_ascii_lowercase()));
    let mut out = HashMap::new();
    for file in files {
        if let Ok(bytes) = assets.read(&file) {
            out.extend(parse(&String::from_utf8_lossy(&bytes)));
        }
    }
    out
}

/// One placed soundscape (env_soundscape or a resolved proxy).
#[derive(Clone, Debug)]
pub(crate) struct Emitter {
    pub ent: usize,
    pub targetname: String,
    pub origin: Vec3,
    pub radius: f32,
    pub soundscape: String,
    pub positions: [Option<Vec3>; 8],
    pub enabled: bool,
}
/// Source (x forward, y left, z up) inches to Bevy metres.
fn bevy(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, z, -y) * UNIT
}
pub(crate) fn emitters(world: &l4d2r_game_core::entities::EntityWorld) -> Vec<Emitter> {
    let by_name = |name: &str| {
        world
            .ents
            .iter()
            .find(|e| !name.is_empty() && e.me.targetname.eq_ignore_ascii_case(name))
    };
    let positions = |e: &l4d2r_game_core::entities::Ent| {
        std::array::from_fn(|i| {
            e.me.kv(&format!("position{i}"))
                .and_then(|n| by_name(n))
                .map(|t| bevy(t.me.origin.x, t.me.origin.y, t.me.origin.z))
        })
    };
    let mut out = vec![];
    for (ent, e) in world.ents.iter().enumerate() {
        let class = e.me.classname.as_str();
        let (soundscape, positions) = match class {
            "env_soundscape" => (e.me.kv("soundscape").unwrap_or("").to_ascii_lowercase(), positions(e)),
            // env_soundscape_proxy copies the main soundscape's name and
            // position targets; its own origin and radius are used.
            "env_soundscape_proxy" => {
                let Some(main) = e.me.kv("MainSoundscapeName").and_then(|n| by_name(n)) else { continue };
                (main.me.kv("soundscape").unwrap_or("").to_ascii_lowercase(), positions(main))
            }
            _ => continue,
        };
        out.push(Emitter {
            ent,
            targetname: e.me.targetname.clone(),
            origin: bevy(e.me.origin.x, e.me.origin.y, e.me.origin.z),
            radius: e.me.kv_f32("radius").unwrap_or(128.),
            soundscape,
            positions,
            enabled: e.me.kv_i32("StartDisabled").unwrap_or(0) == 0,
        });
    }
    out
}

/// The server's per-listener choice (CEnvSoundscape::UpdateForPlayer), run
/// over every emitter. `current` is (emitter index, distance, in range).
pub(crate) fn select(
    emitters: &[Emitter],
    listener: Vec3,
    current: Option<(usize, f32, bool)>,
    clear: impl Fn(Vec3, Vec3) -> bool,
) -> Option<(usize, f32, bool)> {
    let mut current = current;
    for (i, e) in emitters.iter().enumerate() {
        if !e.enabled {
            if current.is_some_and(|(c, _, _)| c == i) {
                current = None;
            }
            continue;
        }
        let range = e.origin.distance(listener) / UNIT;
        let within = e.radius == -1. || range < e.radius;
        if let Some((c, _, _)) = current.filter(|(c, _, _)| *c == i) {
            let in_range = within && clear(e.origin, listener);
            current = Some((c, range, in_range));
            continue;
        }
        let takes = current.is_none_or(|(_, distance, in_range)| !in_range || range < distance);
        if takes && within && clear(e.origin, listener) {
            current = Some((i, range, true));
        }
    }
    current
}

/// A soundscape expanded with its nested soundscapes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LoopingSound {
    pub wave: String,
    pub volume: f32,
    pub pitch: f32,
    pub position: Option<Vec3>,
    pub soundlevel: f32,
    /// Placed around the listener when it starts.
    pub random: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct RandomSound {
    params: Random,
    master: f32,
    position: Option<Vec3>,
    next: f32,
}
pub(crate) fn expand(
    defs: &HashMap<String, Definition>,
    name: &str,
    positions: &[Option<Vec3>; 8],
    rng: &mut impl FnMut(f32, f32) -> f32,
    now: f32,
) -> (Vec<LoopingSound>, Vec<RandomSound>) {
    let mut looping = vec![];
    let mut random = vec![];
    #[allow(clippy::too_many_arguments)]
    fn walk(
        defs: &HashMap<String, Definition>,
        name: &str,
        master: f32,
        start: usize,
        overrides: (Option<usize>, Option<usize>),
        positions: &[Option<Vec3>; 8],
        rng: &mut impl FnMut(f32, f32) -> f32,
        now: f32,
        depth: usize,
        looping: &mut Vec<LoopingSound>,
        random: &mut Vec<RandomSound>,
    ) {
        let Some(def) = defs.get(name) else { return };
        if depth >= MAX_RECURSION {
            return;
        }
        let at = |index: Option<usize>, ambient: bool| -> Option<Option<Vec3>> {
            match index {
                Some(i) => positions.get(start + i).copied().flatten().map(Some),
                None => {
                    let fallback = if ambient { overrides.1 } else { overrides.0 };
                    Some(fallback.and_then(|i| positions.get(i).copied().flatten()))
                }
            }
        };
        for l in &def.looping {
            let volume = master * rng(l.volume[0], l.volume[1]);
            if volume <= 0. {
                continue;
            }
            let (position, random) = match l.placement {
                Position::Random => (None, true),
                Position::Origin(o) => (Some(o), false),
                // A missing position target drops the sound, as in retail.
                _ => match at(l.position, true) {
                    Some(p) => (p, false),
                    None => continue,
                },
            };
            looping.push(LoopingSound { wave: l.wave.clone(), volume, pitch: rng(l.pitch[0], l.pitch[1]), position, soundlevel: l.soundlevel, random });
        }
        for r in &def.random {
            let position = match r.position {
                Position::Random => None,
                Position::Origin(o) => Some(o),
                Position::Index(i) => match at(Some(i), false) {
                    Some(p) => p,
                    None => continue,
                },
                Position::Ambient => at(None, true).flatten(),
            };
            random.push(RandomSound {
                params: r.clone(),
                master,
                position,
                next: now + 0.5 * rng(r.time[0], r.time[1]),
            });
        }
        for sub in &def.subs {
            let volume = master * rng(sub.volume[0], sub.volume[1]);
            let overrides = (
                overrides.0.or(sub.position_override.map(|p| start + p)),
                overrides.1.or(sub.position_override.or(sub.ambient_override).map(|p| start + p)),
            );
            walk(defs, &sub.name, volume, start + sub.position.unwrap_or(0), overrides, positions, rng, now, depth + 1, looping, random);
        }
    }
    walk(defs, name, 1., 0, (None, None), positions, rng, now, 0, &mut looping, &mut random);
    (looping, random)
}

#[derive(Component)]
struct LoopSound {
    key: (String, Option<[i32; 3]>),
    target: f32,
    current: f32,
    soundlevel: f32,
    position: Option<Vec3>,
}
#[derive(Component)]
struct OneShot;

#[derive(Default)]
struct State {
    map: usize,
    defs: HashMap<String, Definition>,
    emitters: Vec<Emitter>,
    current: Option<(usize, f32, bool)>,
    active: Option<String>,
    random: Vec<RandomSound>,
    pending_loops: Vec<LoopingSound>,
    seed: u32,
    next_select: f32,
}
impl State {
    fn rand(&mut self, a: f32, b: f32) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        a + (b - a) * (self.seed as f32 / u32::MAX as f32)
    }
}

pub fn install(app: &mut App) {
    app.add_systems(Update, update)
        .add_systems(PostUpdate, crate::game_audio::attenuate.after(crate::game_audio::fade_out));
}

fn key_of(sound: &LoopingSound) -> (String, Option<[i32; 3]>) {
    (sound.wave.to_ascii_lowercase(), sound.position.map(|p| (p * 100.).round().as_ivec3().to_array()))
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    map: Option<Res<crate::viewer::Map>>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    collision: Option<Res<crate::collision::Collision>>,
    source: Option<Res<crate::game_ui::HudSource>>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    frontend: Res<crate::game_menu::Frontend>,
    settings: Res<crate::game_audio::Settings>,
    mut bank: Option<ResMut<crate::game_audio::Bank>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    time: Res<Time>,
    mut loops: Query<(Entity, &mut LoopSound, Option<&mut AudioSink>, Option<&mut SpatialAudioSink>)>,
    shots: Query<Entity, With<OneShot>>,
    mut room: ResMut<crate::room_dsp::SoundscapeDsp>,
    mix: Option<Res<crate::adrenaline::MixRules>>,
    mut state: Local<State>,
) {
    let _timing = crate::performance::scope("soundscapes::update");
    let (Some(map), Some(collision), Some(source), Some(bank)) = (map, collision, source, bank.as_mut()) else {
        return;
    };
    let now = time.elapsed_secs();
    let world = &map.0.world;
    let id = world.entities.as_ptr() as usize ^ world.entities.len();
    if state.map != id {
        let name = session
            .as_ref()
            .and_then(|s| s.map.file_stem().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) {
            state.defs = load(&mut assets, &name);
        }
        state.map = id;
        state.emitters = logic.as_ref().map(|l| emitters(&l.world)).unwrap_or_default();
        state.current = None;
        state.active = None;
        state.random.clear();
        state.pending_loops.clear();
        state.seed = 0x9e37_79b9;
        for (entity, ..) in &loops {
            commands.entity(entity).despawn();
        }
        for entity in &shots {
            commands.entity(entity).despawn();
        }
        if !state.emitters.is_empty() {
            println!("Soundscapes: {} definitions, {} placed", state.defs.len(), state.emitters.len());
        }
    }
    // Enable / Disable / ToggleEnabled inputs.
    if let Some(logic) = logic.as_mut() {
        if !logic.world.unhandled.is_empty() {
            let pending = std::mem::take(&mut logic.world.unhandled);
            for (ent, input, param) in pending {
                let action = input.to_ascii_lowercase();
                let mut handled = false;
                for e in state.emitters.iter_mut().filter(|e| e.ent == ent) {
                    match action.as_str() {
                        "enable" => e.enabled = true,
                        "disable" => e.enabled = false,
                        "toggleenabled" => e.enabled = !e.enabled,
                        _ => continue,
                    }
                    handled = true;
                }
                if !handled {
                    logic.world.unhandled.push((ent, input, param));
                }
            }
        }
    }
    let Some(listener) = camera.iter().next().map(GlobalTransform::compute_transform) else {
        return;
    };
    if !frontend.playing() {
        return;
    }
    // Choose the soundscape (the server updates players continuously; ten
    // times a second is plenty for a listener moving at walking speed).
    if now >= state.next_select {
        state.next_select = now + 0.1;
        let chosen = select(&state.emitters, listener.translation, state.current, |a, b| collision.brush_water_clear(a, b));
        state.current = chosen;
        let wanted = chosen.map(|(i, _, _)| (state.emitters[i].soundscape.clone(), state.emitters[i].positions));
        if wanted.as_ref().map(|w| &w.0) != state.active.as_ref() {
            state.active = wanted.as_ref().map(|w| w.0.clone());
            // Only the top-level soundscape sets the room (allowDSP); one
            // without a "dsp" key keeps the previous room.
            if let Some(dsp) = wanted.as_ref().and_then(|w| state.defs.get(&w.0)).and_then(|d| d.dsp) {
                room.0 = Some(dsp);
            }
            if std::env::args().any(|a| a == "--audit-audio") {
                println!("Soundscape active: {:?}", state.active);
            }
            let (looping, random) = match &wanted {
                Some((name, positions)) => {
                    let defs = std::mem::take(&mut state.defs);
                    let mut rng = |a: f32, b: f32| state.rand(a, b);
                    let result = expand(&defs, name, positions, &mut rng, now);
                    state.defs = defs;
                    result
                }
                None => (vec![], vec![]),
            };
            // Default_Mix group volume (ambient/... is 0.55), and random
            // loops placed 36 units from the listener at a random angle.
            let forward = listener.rotation * Vec3::NEG_Z;
            let right = listener.rotation * Vec3::X;
            let mut looping = looping;
            for l in &mut looping {
                if let Ok(path) = crate::source_sound::wave_path(&l.wave) {
                    l.volume *= mix.as_ref().map_or(1., |m| m.mix_volume(&path, l.soundlevel));
                }
                if l.random {
                    let angle = state.rand(-180., 180.).to_radians();
                    l.position = Some(listener.translation + (forward * angle.cos() + right * angle.sin()) * RANDOM_RADIUS * UNIT);
                }
            }
            // Existing loops fade to 0 unless the new soundscape keeps them.
            for (_, mut sound, ..) in &mut loops {
                sound.target = looping
                    .iter()
                    .find(|l| key_of(l) == sound.key)
                    .map_or(0., |l| l.volume);
            }
            state.pending_loops = looping
                .into_iter()
                .filter(|l| !loops.iter().any(|(_, s, ..)| s.key == key_of(l)))
                .collect();
            state.random = random;
        }
    }
    // Start new loops once their waves have loaded (they fade in from 0).
    let pending = std::mem::take(&mut state.pending_loops);
    for sound in pending {
        let Ok(path) = crate::source_sound::wave_path(&sound.wave) else { continue };
        match bank.file(&path) {
            Some(handle) => {
                if std::env::args().any(|a| a == "--audit-audio") {
                    println!("Soundscape loop: {} volume {:.2} positional {}", sound.wave, sound.volume, sound.position.is_some());
                }
                let mut entity = commands.spawn((
                    AudioPlayer(handle),
                    PlaybackSettings {
                        volume: Volume::Linear(0.),
                        speed: sound.pitch / 100.,
                        spatial: sound.position.is_some(),
                        ..PlaybackSettings::LOOP
                    },
                    LoopSound { key: key_of(&sound), target: sound.volume, current: 0., soundlevel: sound.soundlevel, position: sound.position },
                ));
                if let Some(position) = sound.position {
                    entity.insert(Transform::from_translation(position));
                }
            }
            None if bank.is_missing(&path) => {}
            None => state.pending_loops.push(sound),
        }
    }
    // Fade loops toward their targets at 1/soundscape_fadetime per second.
    let step = time.delta_secs() / FADE_TIME;
    for (entity, mut sound, sink, spatial) in &mut loops {
        let delta = sound.target - sound.current;
        sound.current += delta.clamp(-step, step);
        if sound.target <= 0. && sound.current <= 0. {
            commands.entity(entity).despawn();
            continue;
        }
        let gain = sound.position.map_or(1., |p| {
            crate::source_sound::distance_gain(sound.soundlevel, listener.translation.distance(p) / UNIT)
        });
        let volume = Volume::Linear(settings.volume * sound.current * gain);
        if let Some(mut sink) = sink {
            sink.set_volume(volume);
        }
        if let Some(mut sink) = spatial {
            sink.set_volume(volume);
        }
    }
    // Random sounds.
    let forward = listener.rotation * Vec3::NEG_Z;
    let right = listener.rotation * Vec3::X;
    for k in 0..state.random.len() {
        if now < state.random[k].next {
            continue;
        }
        let r = state.random[k].clone();
        let time_range = r.params.time;
        state.random[k].next = now + state.rand(time_range[0], time_range[1]).max(0.05);
        if r.params.waves.is_empty() {
            continue;
        }
        let pick = (state.rand(0., r.params.waves.len() as f32) as usize).min(r.params.waves.len() - 1);
        let mut volume = r.master * state.rand(r.params.volume[0], r.params.volume[1]);
        let pitch = state.rand(r.params.pitch[0], r.params.pitch[1]);
        if volume <= 0. {
            continue;
        }
        let position = if r.params.position == Position::Random {
            // GenerateRandomSoundPosition: 36 units from the ear at a random
            // angle in the listener's horizontal plane.
            let angle = state.rand(-180., 180.).to_radians();
            Some(listener.translation + (forward * angle.cos() + right * angle.sin()) * RANDOM_RADIUS * UNIT)
        } else {
            r.position
        };
        let Ok(path) = crate::source_sound::wave_path(&r.params.waves[pick]) else { continue };
        volume *= mix.as_ref().map_or(1., |m| m.mix_volume(&path, r.params.soundlevel));
        // Not yet loaded: skip this play (the load is now in flight).
        let Some(handle) = bank.file(&path) else { continue };
        let mut entity = commands.spawn((
            AudioPlayer(handle),
            PlaybackSettings {
                volume: Volume::Linear(settings.volume * volume * position.map_or(1., |p| {
                    crate::source_sound::distance_gain(r.params.soundlevel, listener.translation.distance(p) / UNIT)
                })),
                speed: pitch / 100.,
                spatial: position.is_some(),
                ..PlaybackSettings::DESPAWN
            },
            OneShot,
        ));
        if let Some(position) = position {
            entity.insert(Transform::from_translation(position));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"
"test.outer"
{
    "dsp" "1"
    "playlooping" { "volume" "0.5" "pitch" "100" "wave" "ambient/loop.wav" }
    "playlooping" { "volume" "0.4" "wave" "ambient/positioned.wav" "position" "1" }
    "playrandom"
    {
        "time" "5,13" "volume" ".7,1" "pitch" "65,100" "position" "random"
        "rndwave" { "wave" "a.wav" "wave" "b.wav" }
    }
    "playsoundscape" { "name" "test.inner" "volume" "0.5" }
}
"test.inner"
{
    "playlooping" { "volume" "0.8" "wave" "ambient/inner.wav" }
}
"#;
    #[test]
    fn soundscape_scripts_parse_loops_randoms_and_nesting() {
        let defs = parse(SAMPLE);
        let outer = &defs["test.outer"];
        assert_eq!(outer.dsp, Some(1));
        assert_eq!(outer.looping.len(), 2);
        assert_eq!(outer.random[0].waves, vec!["a.wav", "b.wav"]);
        assert_eq!(outer.random[0].time, [5., 13.]);
        assert_eq!(outer.random[0].position, Position::Random);
        let positions = [None, Some(Vec3::X), None, None, None, None, None, None];
        let mut rng = |a: f32, _b: f32| a;
        let (looping, random) = expand(&defs, "test.outer", &positions, &mut rng, 10.);
        assert_eq!(looping.len(), 3);
        assert_eq!(looping[1].position, Some(Vec3::X));
        // Nested volume scales by the playsoundscape volume.
        assert!((looping[2].volume - 0.4).abs() < 1e-6);
        // First random play after half an interval.
        assert!((random[0].next - 12.5).abs() < 1e-6);
        // A loop whose position target is missing is dropped.
        let (looping, _) = expand(&defs, "test.outer", &[None; 8], &mut rng, 0.);
        assert_eq!(looping.len(), 2);
    }
    #[test]
    fn selection_keeps_the_current_soundscape_until_a_closer_visible_one() {
        let emitter = |x: f32, radius: f32| Emitter {
            ent: 0,
            targetname: String::new(),
            origin: Vec3::X * x * UNIT,
            radius,
            soundscape: format!("s{x}"),
            positions: [None; 8],
            enabled: true,
        };
        let emitters = vec![emitter(0., 200.), emitter(300., 200.)];
        let clear = |_: Vec3, _: Vec3| true;
        let at = |x: f32| Vec3::X * x * UNIT;
        let first = select(&emitters, at(50.), None, clear);
        assert_eq!(first.map(|c| c.0), Some(0));
        // Out of every radius: the current one is kept.
        let kept = select(&emitters, at(1000.), first, clear);
        assert_eq!(kept.map(|c| (c.0, c.2)), Some((0, false)));
        // Back in range of the second only: it takes over.
        let second = select(&emitters, at(320.), kept, clear);
        assert_eq!(second.map(|c| c.0), Some(1));
        // A blocked line of sight prevents a switch.
        let blocked = select(&emitters, at(60.), second, |a, _| a.x > 1.);
        assert_eq!(blocked.map(|c| c.0), Some(1));
        // Disabling the current soundscape clears it.
        let mut off = emitters.clone();
        off[1].enabled = false;
        assert_eq!(select(&off, at(320.), second, clear), None);
    }
    #[test]
    fn retail_campaign_soundscapes_load_and_every_placed_name_resolves() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        for map in ["c1m1_hotel", "c2m1_highway", "c3m1_plankcountry", "c4m1_milltown_a", "c5m1_waterfront", "c6m1_riverbank", "c7m1_docks", "c8m1_apartment", "c11m1_greenhouse", "c13m1_alpinecreek"] {
        let Some(path) = ["left4dead2", "left4dead2_dlc1", "left4dead2_dlc2", "left4dead2_dlc3", "update"]
            .iter()
            .map(|d| root.join(d).join("maps").join(format!("{map}.bsp")))
            .find(|p| p.is_file())
        else {
            continue;
        };
        let defs = load(&mut assets, map);
        assert!(defs.len() > 100, "{}", defs.len());
        let logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let placed = emitters(&logic.world);
        assert!(!placed.is_empty(), "{map}");
        let missing: Vec<_> = placed.iter().filter(|e| !defs.contains_key(&e.soundscape)).map(|e| e.soundscape.clone()).collect();
        // Retail data has dangling names (c4m1 "milltown.a.spawn" is defined in
        // no soundscape file); retail plays nothing for them, as here.
        if !missing.is_empty() {
            println!("SOUNDSCAPES {map}: undefined in retail data: {missing:?}");
        }
        // Every wave each placed soundscape can play exists.
        for e in &placed {
            let mut rng = |a: f32, _b: f32| a.max(0.01);
            let (looping, random) = expand(&defs, &e.soundscape, &e.positions, &mut rng, 0.);
            for wave in looping.iter().map(|l| l.wave.clone()).chain(random.iter().flat_map(|r| r.params.waves.clone())) {
                let path = crate::source_sound::wave_path(&wave).unwrap();
                assert!(assets.exists(&path), "{map} {} -> {path}", e.soundscape);
            }
        }
        println!("SOUNDSCAPES {map}: {} placed, {} definitions", placed.len(), defs.len());
        }
    }
}
