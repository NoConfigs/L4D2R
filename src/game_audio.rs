//! Lazy playback from the owned installation; no retail audio is bundled.
use crate::{
    game_ui::HudSource,
    source_assets::SourceAssets,
    source_sound::{Definition, Registry},
};
use bevy::{
    audio::{SpatialListener, Volume},
    prelude::*,
};
use std::collections::{HashMap, HashSet, VecDeque};
#[derive(Resource, Default)]
pub struct Queue(pub VecDeque<Request>, pub Vec<usize>);
pub struct Request {
    pub name: String,
    pub position: Option<Vec3>,
    pub looping: bool,
    pub gain: f32,
    pub map_entity: Option<usize>,
    /// Overrides the soundscript sound level (ambient_generic radius).
    pub soundlevel: Option<f32>,
}
/// A positional sound whose volume follows the engine's distance curve.
#[derive(Component)]
pub struct Attenuated {
    pub soundlevel: f32,
    /// Occlusion gain and the time it is next re-traced.
    pub occlusion: f32,
    pub next_check: f32,
}
impl Attenuated {
    pub fn new(soundlevel: f32) -> Self {
        Self { soundlevel, occlusion: 1., next_check: 0. }
    }
}
/// snd_obscured_gain_dB.
const OBSCURED_GAIN_DB: f32 = -2.7;
/// engine.dll 10010da0: a sound whose centre is hidden from the listener
/// (world and props, mask 0x4003, fraction < 0.99) traces four points around
/// it, on the two diagonals of the plane facing the listener at half and full
/// radius ((soundlevel - 60) * 2.7 + 24 units, 1000c170); every blocked point
/// after the first drops snd_obscured_gain_dB.
pub fn occlusion(listener: [f32; 3], source: [f32; 3], soundlevel: f32, mut blocked: impl FnMut([f32; 3], [f32; 3]) -> bool) -> f32 {
    if !blocked(listener, source) {
        return 1.;
    }
    let l = Vec3::from_array(listener);
    let s = Vec3::from_array(source);
    let dir = (s - l).normalize_or_zero();
    let right = dir.cross(Vec3::Z).normalize_or(Vec3::X);
    let mut up = right.cross(dir);
    if l.z + 120. < s.z {
        up.z = -up.z;
    }
    let radius = (soundlevel - 60.) * 216. * 0.0125 + 24.;
    let c = (up - right).normalize_or_zero();
    let d = (right + up).normalize_or_zero();
    let mut hidden = 0;
    let mut gain = 1.;
    for point in [s + c * radius * 0.5, s + d * radius * 0.5, s + c * radius, s + d * radius] {
        if blocked(listener, point.to_array()) {
            hidden += 1;
            if hidden > 1 {
                gain *= 10f32.powf(OBSCURED_GAIN_DB / 20.);
            }
        }
    }
    gain
}
/// Source units from the listener to `position`.
fn source_distance(listener: Option<Vec3>, position: Vec3) -> f32 {
    listener.map_or(0., |l| l.distance(position) / 0.0254)
}
/// Rodio's spatial sink applies inverse-square falloff beyond one scaled
/// unit. A tiny scale keeps every map position inside that unit, leaving
/// panning only, so the engine curve (`Attenuated`) owns distance gain.
pub const SPATIAL_SCALE: f32 = 1e-4;
/// Applies the engine distance gain to positional sounds every frame
/// (after adrenaline mixing and fades have set their base volume).
pub fn attenuate(
    adrenaline: Option<Res<crate::adrenaline::Adrenaline>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    collision: Option<Res<crate::collision::Collision>>,
    time: Res<Time>,
    mut sounds: Query<(Entity, &mut Attenuated, &GlobalTransform, &crate::adrenaline::Mixed, &mut bevy::audio::SpatialAudioSink)>,
) {
    use bevy::audio::AudioSinkPlayback;
    let listener = camera.iter().next().map(GlobalTransform::translation);
    let w = adrenaline.as_ref().map_or(0., |a| a.weight);
    let now = time.elapsed_secs();
    let source = |p: Vec3| [p.x / 0.0254, -p.z / 0.0254, p.y / 0.0254];
    for (entity, mut a, transform, mixed, mut sink) in &mut sounds {
        // Re-trace every 0.1 s, staggered so the traces spread over frames.
        if let (Some(collision), Some(l)) = (collision.as_ref(), listener) {
            if now >= a.next_check {
                a.next_check = now + 0.1 + (entity.index_u32() % 7) as f32 * 0.005;
                let soundlevel = a.soundlevel;
                a.occlusion = occlusion(source(l), source(transform.translation()), soundlevel, |from, to| {
                    collision
                        .source_ray(from, to, 0x4003, true)
                        .is_some_and(|(fraction, _, _, solid)| solid || fraction < 0.99)
                });
            }
        }
        let gain = crate::source_sound::distance_gain(a.soundlevel, source_distance(listener, transform.translation())) * a.occlusion;
        sink.set_volume(Volume::Linear(mixed.base * crate::adrenaline::mix_gain(mixed.layer, w) * gain));
    }
}
impl Queue {
    pub fn play(&mut self, name: impl Into<String>, position: Option<Vec3>) {
        if self.0.len() < 64 {
            self.0.push_back(Request {
                name: name.into(),
                position,
                looping: false,
                gain: 1.,
                map_entity: None,
                soundlevel: None,
            });
        }
    }
}
#[derive(Resource)]
pub struct Bank {
    loader: std::sync::mpsc::Sender<String>,
    loaded: std::sync::Mutex<std::sync::mpsc::Receiver<(String, std::io::Result<Vec<u8>>)>>,
    loading: HashSet<String>,
    pending: Vec<(Request, String, Definition, f64)>,
    registry: Registry,
    cache: HashMap<String, Handle<AudioSource>>,
    missing: HashSet<String>,
    chooser: crate::source_sound::Chooser,
    budget: CacheBudget,
}
/// Loaded sound files kept for reuse. Every sound ever played stayed loaded
/// for the whole map; phones cap the bytes and drop the least recently
/// used (a dropped sound reloads from disk the next time it plays).
#[derive(Default)]
struct CacheBudget {
    sizes: HashMap<String, usize>,
    last_use: HashMap<String, u64>,
    bytes: usize,
    clock: u64,
}
impl CacheBudget {
    fn limit() -> usize {
        if cfg!(any(target_os = "ios", target_os = "android"))
            || std::env::args().any(|a| a == "--profile-mobile-memory")
        {
            48 << 20
        } else {
            usize::MAX
        }
    }
    fn touch(&mut self, path: &str) {
        self.clock += 1;
        if let Some(t) = self.last_use.get_mut(path) {
            *t = self.clock;
        }
    }
    fn insert(&mut self, path: &str, size: usize) {
        self.clock += 1;
        if let Some(old) = self.sizes.insert(path.to_string(), size) {
            self.bytes -= old;
        }
        self.bytes += size;
        self.last_use.insert(path.to_string(), self.clock);
    }
    /// Paths to drop, oldest use first, until under three quarters of `limit`.
    fn evict(&mut self, limit: usize) -> Vec<String> {
        if self.bytes <= limit {
            return vec![];
        }
        let mut order: Vec<(u64, String)> = self.last_use.iter().map(|(p, t)| (*t, p.clone())).collect();
        order.sort();
        let mut out = vec![];
        for (_, path) in order {
            if self.bytes <= limit / 4 * 3 {
                break;
            }
            self.bytes -= self.sizes.remove(&path).unwrap_or(0);
            self.last_use.remove(&path);
            out.push(path);
        }
        out
    }
}
impl Bank {
    /// The loaded handle for a sound file (`sound/...`), starting an
    /// asynchronous load the first time; `None` until it has loaded.
    pub fn file(&mut self, path: &str) -> Option<Handle<AudioSource>> {
        if let Some(handle) = self.cache.get(path) {
            let handle = handle.clone();
            self.budget.touch(path);
            return Some(handle);
        }
        if !self.missing.contains(path) && self.loading.insert(path.to_string()) {
            if self.loader.send(path.to_string()).is_err() {
                self.missing.insert(path.to_string());
            }
        }
        None
    }
    pub fn is_missing(&self, path: &str) -> bool {
        self.missing.contains(path)
    }
}
#[derive(Resource)]
pub struct Settings {
    pub volume: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self { volume: 0.7 }
    }
}
#[derive(Component)]
pub struct MenuMusic;
#[derive(Component)]
pub struct MapSound(pub usize);
/// MapSound ids at or above this are gameplay cues (music, voices, acid,
/// Witch), not map entity indices; map logic must not stop them.
pub const GAMEPLAY_SOUND_IDS: usize = 0x0100_0000;
/// A playing one-shot: its wave and start time (same-sound limit).
#[derive(Component)]
pub struct Voice(pub String, pub f64);
/// Engine snd_max_same_sounds / snd_max_same_weapon_sounds defaults.
pub const MAX_SAME_SOUNDS: usize = 4;
pub const MAX_SAME_WEAPON_SOUNDS: usize = 3;
#[derive(Component)]
pub struct FadeOut {
    remaining: f32,
    duration: f32,
    base: Option<f32>,
}
impl FadeOut {
    pub fn new(seconds: f32) -> Self {
        Self { remaining: seconds, duration: seconds.max(0.001), base: None }
    }
}
/// Run after the normal mix so fading owned cues cannot have their volume
/// restored by the adrenaline layer. Pending loads are cancelled separately.
pub fn fade_out(
    mut commands: Commands, time: Res<Time>,
    adrenaline: Option<Res<crate::adrenaline::Adrenaline>>,
    mut sounds: Query<(Entity, &mut FadeOut, Option<&mut crate::adrenaline::Mixed>,
        Option<&mut AudioSink>, Option<&mut bevy::audio::SpatialAudioSink>)>,
) {
    use bevy::audio::AudioSinkPlayback;
    for (entity, mut fade, mut mixed, mut sink, mut spatial) in &mut sounds {
        fade.remaining = (fade.remaining - time.delta_secs()).max(0.);
        if fade.remaining == 0. { commands.entity(entity).despawn(); continue; }
        let base = *fade.base.get_or_insert_with(|| mixed.as_ref().map(|m|m.base)
            .or_else(||sink.as_ref().map(|s|s.volume().to_linear()))
            .or_else(||spatial.as_ref().map(|s|s.volume().to_linear())).unwrap_or(1.));
        let gain = base * fade.remaining / fade.duration;
        let layer = mixed.as_ref().and_then(|m|m.layer);
        if let Some(mixed) = mixed.as_mut() { mixed.base = gain; }
        let volume = Volume::Linear(gain * crate::adrenaline::mix_gain(layer,
            adrenaline.as_ref().map_or(0., |a|a.weight)));
        if let Some(sink) = sink.as_mut() { sink.set_volume(volume); }
        if let Some(sink) = spatial.as_mut() { sink.set_volume(volume); }
    }
}
pub fn music_volume(settings: Res<Settings>, mut music: Query<&mut AudioSink, With<MenuMusic>>) {
    for mut sink in &mut music {
        sink.set_volume(Volume::Linear(settings.volume));
    }
}
pub fn setup(
    mut commands: Commands,
    source: Res<HudSource>,
    camera: Single<Entity, With<crate::viewer::GameCamera>>,
) {
    let mut assets = SourceAssets::new(&source.0, vec![]).expect("Sound mounts");
    let registry = Registry::load(&mut assets).expect("Native sound manifest");
    println!(
        "Native sound registry: {} definitions from {} scripts (lazy loading)",
        registry.definitions.len(),
        registry.files
    );
    let (loader, requests) = std::sync::mpsc::channel::<String>();
    let (results, loaded) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("native-audio-loader".into())
        .spawn(move || {
            while let Ok(path) = requests.recv() {
                let bytes = assets.read(&path);
                if results.send((path, bytes)).is_err() {
                    break;
                }
            }
        })
        .expect("Native audio worker");
    // Queue intro audio while the menu is open, so first-play disk IO cannot miss its cue.
    let mut loading = HashSet::new();
    if let Some(sound) = registry.definitions.get("c1.intro.chopperleave") {
        for path in &sound.waves {
            if loader.send(path.clone()).is_ok() {
                loading.insert(path.clone());
            }
        }
    }
    commands.insert_resource(Bank {
        loader,
        loaded: std::sync::Mutex::new(loaded),
        loading,
        pending: vec![],
        registry,
        cache: HashMap::new(),
        missing: HashSet::new(),
        chooser: Default::default(),
        budget: Default::default(),
    });
    commands.entity(*camera).insert(SpatialListener::new(0.2));
    commands.insert_resource(bevy::audio::DefaultSpatialScale(bevy::audio::SpatialScale::new(SPATIAL_SCALE)));
}
pub fn playback(
    mut commands: Commands,
    mut bank: ResMut<Bank>,
    mut queue: ResMut<Queue>,
    mut sources: ResMut<Assets<AudioSource>>,
    settings: Res<Settings>,
    time: Res<Time>,
    frontend: Res<crate::game_menu::Frontend>,
    mix: Option<Res<crate::adrenaline::MixRules>>,
    camera: Query<&GlobalTransform, With<crate::viewer::GameCamera>>,
    room: Option<Res<crate::room_dsp::Current>>,
    mut rooms: Option<ResMut<Assets<crate::room_dsp::RoomAudio>>>,
    extra: (Option<Res<crate::room_dsp::Bus>>, Query<(Entity, &Voice)>),
) {
    let (bus, voices) = extra;
    // snd_max_same_sounds: voices per wave started this frame or playing.
    let mut same: HashMap<String, Vec<(f64, Entity)>> = HashMap::new();
    for (entity, voice) in &voices {
        same.entry(voice.0.clone()).or_default().push((voice.1, entity));
    }
    let listener = camera.iter().next().map(GlobalTransform::translation);
    let _timing = crate::performance::scope("game_audio::playback");
    let cancelled = std::mem::take(&mut queue.1);
    bank.pending
        .retain(|(request, _, _, _)| !request.map_entity.is_some_and(|id| cancelled.contains(&id)));
    let ready = bank.loaded.lock().unwrap().try_iter().collect::<Vec<_>>();
    for (path, bytes) in ready {
        bank.loading.remove(&path);
        match bytes {
            Ok(bytes) if !path.ends_with(".wav") || pcm_wave(&bytes) => {
                bank.budget.insert(&path, bytes.len());
                let handle = sources.add(AudioSource {
                    bytes: bytes.into(),
                });
                bank.cache.insert(path.clone(), handle);
                for old in bank.budget.evict(CacheBudget::limit()) {
                    // Playing sounds keep their own handle and finish.
                    bank.cache.remove(&old);
                }
                if audit_enabled() {
                    println!("Native sound loaded asynchronously: {path}");
                }
            }
            _ => {
                bank.missing.insert(path);
            }
        }
    }
    while let Some(request) = queue.0.pop_front() {
        if request.map_entity.is_some_and(|id| cancelled.contains(&id))
            || (frontend.playing() && request.looping && request.map_entity.is_none())
        {
            continue;
        }
        let key = request.name.to_lowercase();
        // Source resolves `$gender` from the speaker's model; `#female`
        // selects the female variant (e.g. Boomette), otherwise male.
        let (key, gender) = match key.strip_suffix("#female") {
            Some(key) => (key.to_string(), "female"),
            None => (key, "male"),
        };
        let definition = bank
            .registry
            .definitions
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Definition {
                waves: vec![crate::source_sound::wave_path(&key).unwrap_or(key.clone())],
                volume: 1.,
                pitch: 1.,
                volume_range: [1.;2],
                pitch_range: [1.;2],
                delay: 0.,
                global: false,
                soundlevel: 75.,
                dry: false,
            });
        if definition.waves.is_empty() {
            continue;
        }
        let Some((path,definition))=bank.chooser.choose(&key,&definition) else {continue};
        let path=path.replace("$gender",gender);
        let due=time.elapsed_secs_f64()+definition.delay as f64;
        // Owned loops (music, ambients) are never dropped by the cap: their
        // owners request them once.
        if bank.pending.len() < 128 || request.looping && request.map_entity.is_some() {bank.pending.push((request,path,definition,due));}
    }
    let pending = std::mem::take(&mut bank.pending);
    for (mut request, path, definition, due) in pending {
        if bank.missing.contains(&path) {
            continue;
        }
        let Some(handle) = bank.cache.get(&path).cloned() else {
            if bank.loading.insert(path.clone()) {
                if bank.loader.send(path.clone()).is_err() {
                    bank.missing.insert(path.clone());
                }
            }
            bank.pending.push((request, path, definition, due));
            continue;
        };
        if time.elapsed_secs_f64()<due {bank.pending.push((request,path,definition,due));continue;}
        if definition.global {request.position=None;}
        let soundlevel = request.soundlevel.unwrap_or(definition.soundlevel);
        // Default_Mix group volume (menu music owns its own sink volume).
        let mixer = if request.looping && request.map_entity.is_none() {
            1.
        } else {
            mix.as_ref().map_or(1., |m| m.mix_volume(&path, soundlevel))
        };
        // Source clamps a channel at full scale: boosting groups (animation
        // 3.0, LOUD 3.0) raise quiet sounds but never past 1.
        let mixer = mixer.min(1. / (definition.volume * request.gain).max(1e-6));
        if soundlevel <= 0. {request.position=None;}
        // Start at the attenuated volume so the first mixed buffer is not loud.
        let gain = request.position.map_or(1., |p| crate::source_sound::distance_gain(soundlevel, source_distance(listener, p)));
        let looping = request.looping
            && (request.map_entity.is_none()
                || !path.ends_with(".wav")
                || sources
                    .get(&handle)
                    .is_some_and(|audio| wave_has_loop(&audio.bytes)));
        let settings_for = |looping: bool| PlaybackSettings {
            volume: Volume::Linear(settings.volume * definition.volume * request.gain * mixer * gain),
            speed: definition.pitch,
            spatial: request.position.is_some(),
            ..if looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN }
        };
        // Positional one-shots start through the current room effect.
        let reverb = room
            .as_ref()
            .and_then(|r| r.preset.clone())
            .filter(|_| !looping && !definition.dry && request.position.is_some())
            .zip(sources.get(&handle).cloned());
        // Retail engine snd_max_same_sounds (4) / snd_max_same_weapon_sounds
        // (3): a new copy of a wave stops the oldest playing one.
        if !looping {
            let limit = if path.contains("weapons/") { MAX_SAME_WEAPON_SOUNDS } else { MAX_SAME_SOUNDS };
            let playing = same.entry(path.clone()).or_default();
            playing.sort_by(|a, b| a.0.total_cmp(&b.0));
            while playing.len() >= limit {
                let (_, oldest) = playing.remove(0);
                commands.entity(oldest).despawn();
            }
        }
        let mut entity = if let (Some((preset, source)), Some(rooms), Some(bus)) = (reverb, rooms.as_mut(), bus.as_ref()) {
            let mix = crate::room_dsp::mix(&preset, source_distance(listener, request.position.unwrap()), soundlevel);
            let send = settings.volume * definition.volume * request.gain * mixer * gain;
            commands.spawn((AudioPlayer(rooms.add(crate::room_dsp::RoomAudio { source, mix, send, bus: bus.0.clone() })), settings_for(false)))
        } else {
            commands.spawn((AudioPlayer(handle), settings_for(looping)))
        };
        if !looping {
            let started = time.elapsed_secs_f64();
            entity.insert(Voice(path.clone(), started));
            same.entry(path.clone()).or_default().push((started, entity.id()));
        }
        // Retail mix group for the adrenalineLayer (menu music is excluded:
        // music_volume owns its sink).
        if !(request.looping && request.map_entity.is_none()) {
            entity.insert(crate::adrenaline::Mixed {
                base: settings.volume * definition.volume * request.gain * mixer,
                layer: mix.as_ref().and_then(|m| m.layer_volume(&path)),
            });
        }
        if audit_enabled() {
            println!("Audio start {:.3} {} -> {} dsp={} pos={:?} map={:?}", time.elapsed_secs_f64(), request.name, path,
                room.as_ref().and_then(|r| r.preset.as_ref()).is_some() && !looping && !definition.dry && request.position.is_some(),
                request.position, request.map_entity);
        }
        if audit_enabled() && request.name.eq_ignore_ascii_case("megamobincoming") {
            println!("Native horde alert started (new wave)");
        }
        if let Some(id) = request.map_entity {
            entity.insert(MapSound(id));
            if audit_enabled() {
                println!(
                    "Native map sound started: {} spatial={} volume={} looping={}",
                    request.name,
                    request.position.is_some(),
                    settings.volume * definition.volume * request.gain * mixer,
                    looping
                );
            }
        } else if request.looping {
            entity.insert(MenuMusic);
        }
        if let Some(position) = request.position {
            entity.insert((Transform::from_translation(position), Attenuated::new(soundlevel)));
        }
    }
}
pub(crate) fn audit_enabled() -> bool {
    static AUDIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AUDIT.get_or_init(|| std::env::args().any(|a| a == "--audit-audio"))
}
pub fn wave_has_loop(bytes: &[u8]) -> bool {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return false;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let Some(end) = (at + 8).checked_add(size) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        let data = &bytes[at + 8..end];
        if &bytes[at..at + 4] == b"cue " && size >= 4 {
            let n = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
            if n > 0 && n <= (size - 4) / 24 {
                return true;
            }
        }
        if &bytes[at..at + 4] == b"smpl"
            && size >= 36
            && u32::from_le_bytes(data[28..32].try_into().unwrap()) > 0
        {
            return true;
        }
        let Some(next) = end.checked_add(size % 2) else {
            return false;
        };
        at = next;
    }
    false
}
#[cfg(test)]
mod loop_tests {
    use super::*;
    #[test]
    fn owned_music_fades_once_over_one_second_without_compounding_mix_gain() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.insert_resource(Time::<()>::default());
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.25));
        let entity = app.world_mut().spawn((FadeOut::new(1.),
            crate::adrenaline::Mixed {base: 0.8, layer: Some(0.5)})).id();
        for expected in [0.6_f32, 0.4, 0.2] {
            app.world_mut().run_system_once(fade_out).unwrap();
            assert!((app.world().get::<crate::adrenaline::Mixed>(entity).unwrap().base - expected).abs() < 0.0001);
        }
        app.world_mut().run_system_once(fade_out).unwrap();
        assert!(app.world().get_entity(entity).is_err());
    }
    #[test]
    fn owned_spitter_sizzle_waves_retain_loop_markers_and_burn_is_global() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.is_dir() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let registry = Registry::load(&mut assets).unwrap();
        for path in &registry.definitions["spitterzombie.acid"].waves {
            let bytes = assets.read(path).unwrap();
            assert!(pcm_wave(&bytes) && wave_has_loop(&bytes), "{path}");
        }
        let burn = &registry.definitions["event.spitterburn"];
        assert!(burn.global);
        assert!(burn.waves.iter().all(|w|w.ends_with("enzymicide.wav")));
    }
    #[test]
    fn delayed_sound_waits_once_and_cancelled_pending_audio_cannot_start() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app=App::new();let (loader,_)=std::sync::mpsc::channel();let (_,loaded)=std::sync::mpsc::channel();
        let registry=Registry{definitions:crate::source_sound::parse(r#"Test {wave a.wav delay_msec 250 soundlevel SNDLVL_NONE pitch "95,105"}"#),files:1};
        let mut sources=Assets::<AudioSource>::default();let handle=sources.add(AudioSource{bytes:Vec::<u8>::new().into()});
        app.insert_resource(Bank{loader,loaded:std::sync::Mutex::new(loaded),loading:HashSet::new(),pending:vec![],registry,cache:HashMap::from([("sound/a.wav".into(),handle)]),missing:HashSet::new(),chooser:Default::default(),budget:Default::default()});
        app.insert_resource(sources).insert_resource(Settings::default()).insert_resource(Time::<()>::default()).init_resource::<Queue>();
        app.insert_resource(crate::game_menu::Frontend{page:crate::game_menu::Page::Game,mode:0,restart:false,started:true});
        app.world_mut().resource_mut::<Queue>().play("Test",Some(Vec3::X));
        app.world_mut().run_system_once(playback).unwrap();
        assert_eq!(app.world_mut().query::<&AudioPlayer>().iter(app.world()).count(),0);
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.25));
        app.world_mut().run_system_once(playback).unwrap();
        let settings=app.world_mut().query::<&PlaybackSettings>().single(app.world()).unwrap();
        assert!(!settings.spatial,"SNDLVL_NONE remains global even with an emitter position");assert!((0.95..=1.05).contains(&settings.speed));
        app.world_mut().resource_mut::<Queue>().0.push_back(Request{name:"Test".into(),position:None,looping:false,gain:1.,map_entity:Some(7),soundlevel:None});
        app.world_mut().run_system_once(playback).unwrap();
        app.world_mut().resource_mut::<Queue>().1.push(7);
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(1.));
        app.world_mut().run_system_once(playback).unwrap();
        assert_eq!(app.world_mut().query::<&AudioPlayer>().iter(app.world()).count(),1);
    }
    #[test]
    fn a_fifth_copy_of_a_sound_stops_the_oldest_and_weapons_keep_three() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app=App::new();let (loader,_)=std::sync::mpsc::channel();let (_,loaded)=std::sync::mpsc::channel();
        let registry=Registry{definitions:crate::source_sound::parse(r#"Groan {wave a.wav} Shot {wave weapons/b.wav}"#),files:1};
        let mut sources=Assets::<AudioSource>::default();
        let a=sources.add(AudioSource{bytes:Vec::<u8>::new().into()});let b=sources.add(AudioSource{bytes:Vec::<u8>::new().into()});
        app.insert_resource(Bank{loader,loaded:std::sync::Mutex::new(loaded),loading:HashSet::new(),pending:vec![],registry,cache:HashMap::from([("sound/a.wav".into(),a),("sound/weapons/b.wav".into(),b)]),missing:HashSet::new(),chooser:Default::default(),budget:Default::default()});
        app.insert_resource(sources).insert_resource(Settings::default()).insert_resource(Time::<()>::default()).init_resource::<Queue>();
        app.insert_resource(crate::game_menu::Frontend{page:crate::game_menu::Page::Game,mode:0,restart:false,started:true});
        for step in 0..6 {
            app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.1));
            app.world_mut().resource_mut::<Queue>().play("Groan",None);
            app.world_mut().resource_mut::<Queue>().play("Shot",None);
            app.world_mut().run_system_once(playback).unwrap();
            let _ = step;
        }
        let mut voices: Vec<(String, f64)> = app.world_mut().query::<&Voice>().iter(app.world()).map(|v| (v.0.clone(), v.1)).collect();
        voices.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(voices.iter().filter(|v| v.0 == "sound/a.wav").count(), MAX_SAME_SOUNDS);
        assert_eq!(voices.iter().filter(|v| v.0 == "sound/weapons/b.wav").count(), MAX_SAME_WEAPON_SOUNDS);
        // The newest copies survive.
        assert!(voices.iter().filter(|v| v.0 == "sound/a.wav").all(|v| v.1 > 0.25));
    }
    #[test]
    fn retail_elevator_power_off_is_one_shot_and_movement_has_loop_points() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let registry = Registry::load(&mut assets).unwrap();
        for (name, expected) in [("elevator.poweroff", false), ("elevator.squeek", true)] {
            let def = &registry.definitions[name];
            for path in &def.waves {
                let bytes = assets.read(path).unwrap();
                assert_eq!(wave_has_loop(&bytes), expected, "{name}: {path}");
            }
        }
    }
}
pub fn pcm_wave(bytes: &[u8]) -> bool {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return false;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let Some(end) = (at + 8).checked_add(n) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if &bytes[at..at + 4] == b"fmt " {
            return n >= 16 && matches!(u16::from_le_bytes([bytes[at + 8], bytes[at + 9]]), 1 | 3);
        }
        let Some(next) = end.checked_add(n % 2) else {
            return false;
        };
        at = next;
    }
    false
}
pub fn verify(root: &std::path::Path) -> std::io::Result<()> {
    let mut assets = SourceAssets::new(root, vec![])?;
    let registry = Registry::load(&mut assets)?;
    for name in [
        "pistol.fire",
        "menu.select",
        "menu.scroll",
        "player.pickupweapon",
        "pistol.clipout",
        "pistol.clipin",
        "painpills.use",
    ] {
        let definition = registry
            .definitions
            .get(name)
            .ok_or_else(|| crate::binary::invalid(format!("Missing sound definition {name}")))?;
        for wave in &definition.waves {
            let b = assets.read(wave)?;
            if !pcm_wave(&b) {
                return Err(crate::binary::invalid(format!("Unsupported WAV {wave}")));
            }
        }
        println!(
            "Native sound verified: {name} ({} variants)",
            definition.waves.len()
        );
    }
    let music = assets.read("sound/music/l4d2/l4d2_c1.wav")?;
    if !pcm_wave(&music) {
        return Err(crate::binary::invalid("Unsupported menu music codec"));
    }
    println!(
        "Native audio verification passed: {} sound definitions / {} manifest scripts; menu music {} bytes",
        registry.definitions.len(),
        registry.files,
        music.len()
    );
    let paths = registry
        .definitions
        .values()
        .flat_map(|d| d.waves.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let missing = paths
        .iter()
        .filter(|p| !p.contains('$') && !assets.exists(p))
        .cloned()
        .collect::<Vec<_>>();
    let templates = paths
        .iter()
        .filter(|p| p.contains('$'))
        .cloned()
        .collect::<Vec<_>>();
    std::fs::create_dir_all("local-research")?;
    std::fs::write("local-research/audio-missing.txt", missing.join("\n"))?;
    std::fs::write("local-research/audio-templates.txt", templates.join("\n"))?;
    println!(
        "Sound library availability: {}/{} referenced wave paths mounted; {} character templates; {} missing literal paths listed in local-research/audio-missing.txt",
        paths.len() - missing.len() - templates.len(),
        paths.len(),
        templates.len(),
        missing.len()
    );
    Ok(())
}
#[cfg(test)]
mod occlusion_tests {
    use super::*;
    #[test]
    fn occlusion_drops_obscured_db_per_hidden_edge_after_the_first() {
        let step = 10f32.powf(OBSCURED_GAIN_DB / 20.);
        // Clear line of sight: no loss.
        assert_eq!(occlusion([0., 0., 64.], [500., 0., 64.], 75., |_, _| false), 1.);
        // A full wall at x = 250 hides the centre and all four edge points.
        let wall = |a: [f32; 3], b: [f32; 3]| (a[0] - 250.) * (b[0] - 250.) < 0.;
        let full = occlusion([0., 0., 64.], [500., 0., 64.], 75., wall);
        assert!((full - step.powi(3)).abs() < 1e-5, "{full}");
        // A narrow pillar hides the centre only: edges are visible.
        let pillar = |a: [f32; 3], b: [f32; 3]| {
            let t = (250. - a[0]) / (b[0] - a[0]);
            let y = a[1] + (b[1] - a[1]) * t;
            let z = a[2] + (b[2] - a[2]) * t;
            (0. ..=1.).contains(&t) && y.abs() < 4. && (z - 64.).abs() < 4.
        };
        assert_eq!(occlusion([0., 0., 64.], [500., 0., 64.], 75., pillar), 1.);
    }
}
#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn sound_cache_drops_the_least_recently_used_over_budget() {
        let mut b = CacheBudget::default();
        b.insert("a", 40);
        b.insert("b", 40);
        b.touch("a");
        b.insert("c", 40);
        // 120 bytes over a 100-byte budget: drop to <= 75, oldest use first.
        let dropped = b.evict(100);
        assert_eq!(dropped, vec!["b".to_string(), "a".to_string()]);
        assert_eq!(b.bytes, 40);
        assert!(b.evict(100).is_empty());
    }
}
