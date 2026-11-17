//! Per-instance common-infected shader data recovered from the owned client.
//!
//! C_Infected's constructor (client 103167e0) draws skin 0..7, clothing
//! 8..15, then the blood-pattern quadrant 0..3. Client 103176c0 preserves
//! all three on the ragdoll. These are palette indices, not MDL skin families
//! or bodygroup values. Do not randomize them again when a wound is applied.
use bevy::prelude::*;
use l4d2r_game_core::random::UniformRandomStream;

#[derive(Resource)]
struct AppearanceStream {
    rng: UniformRandomStream,
    fixture: Option<Appearance>,
}

pub fn install(app: &mut App) {
    app.insert_resource(AppearanceStream {
        // The recovered client uses a global stream shared by unrelated effects;
        // its seed/interleaving are not synchronized with this rewrite. An owned
        // local stream preserves the native distribution without changing the
        // director's gameplay RNG or rerolling appearance on every frame.
        rng: UniformRandomStream::new(0x4107),
        fixture: Appearance::capture_override(&std::env::args().collect::<Vec<_>>()),
    }).add_systems(PostUpdate, (assign, update_burn).chain()
        .before(crate::viewer::ModelLightingSystems::Write));
}

fn assign(
    mut commands: Commands,
    mut stream: ResMut<AppearanceStream>,
    enemies: Query<(Entity, &crate::game::Enemy), Without<Appearance>>,
) {
    for (entity, enemy) in &enemies {
        if enemy.model.starts_with("models/infected/common_")
            || enemy.model == "models/infected/common_male01.mdl" {
            let appearance = stream.fixture.unwrap_or_else(|| Appearance::random(&mut stream.rng));
            commands.entity(entity).insert(appearance);
        }
    }
}

#[derive(Component)]
struct BurnStarted(f32);

fn update_burn(
    mut commands: Commands,
    state: Option<Res<crate::game_state::Match>>,
    mut actors: Query<(Entity, &crate::game::Enemy, &mut Appearance, Option<&BurnStarted>)>,
) {
    let Some(state) = state else { return; };
    if !state.elapsed.is_finite() { return; }
    for (entity, enemy, mut appearance, started) in &mut actors {
        let start = match started {
            Some(start) => start.0,
            None if enemy.burning > 0. => {
                commands.entity(entity).insert(BurnStarted(state.elapsed));
                state.elapsed
            },
            None => continue,
        };
        // Native 103162f0 starts the IntervalTimer only when burning and its
        // timestamp is invalid. 103147f0 then uses elapsed time independently
        // of the current flame flag, and 103176c0 carries elapsed into the
        // ragdoll. Do not restart on refreshed damage or clear when extinguished.
        let updated = (*appearance).with_burn_elapsed(state.elapsed - start, 10., 0.6);
        if appearance.burn != updated.burn { appearance.burn = updated.burn; }
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub skin: u8,
    pub cloth: u8,
    pub sheet: u8,
    /// Lower 16 bits of the native packed render modulation; not a float tint.
    pub burn: u16,
}

impl Appearance {
    /// Uses the native inclusive ranges and draw order. A supplied stream makes
    /// replay deterministic; retail's global stream interleaving is not known.
    pub fn random(rng: &mut UniformRandomStream) -> Self {
        Self {
            skin: rng.random_int(0, 7) as u8,
            cloth: rng.random_int(8, 15) as u8,
            sheet: rng.random_int(0, 3) as u8,
            burn: 0,
        }
    }

    /// Explicit fixture rows, retaining the retail skin/clothing partitions.
    pub fn from_rows(skin: u8, cloth: u8, sheet: u8) -> Option<Self> {
        (skin < 8 && (8..16).contains(&cloth) && sheet < 4)
            .then_some(Self { skin, cloth, sheet, burn: 0 })
    }

    /// `--capture-infected-appearance "skin cloth sheet"` is a test fixture,
    /// never a source of production appearance randomization.
    pub fn capture_override(args: &[String]) -> Option<Self> {
        let value = &args.windows(2)
            .find(|a| a[0] == "--capture-infected-appearance")?[1];
        let rows = value.split(|c: char| c.is_whitespace() || c == ',')
            .filter(|v| !v.is_empty()).map(str::parse::<u8>)
            .collect::<Result<Vec<_>, _>>().ok()?;
        if rows.len() != 3 { return None; }
        Self::from_rows(rows[0], rows[1], rows[2])
    }

    /// shaderapidx9 100207b0 case 0x13: quarter-sheet offsets and palette
    /// row centers. The material's actual palette height is required.
    pub fn lookup(self, palette_height: f32) -> Vec4 {
        let height = if palette_height.is_finite() && palette_height > 0. {
            palette_height
        } else { 16. };
        Vec4::new((self.sheet / 2) as f32 * 0.5,
            (self.sheet % 2) as f32 * 0.5,
            (self.skin as f32 + 0.5) / height,
            (self.cloth as f32 + 0.5) / height)
    }

    /// client 103147f0: RGB render modulation carries indices and an integer
    /// packed into a numeric float. It is not an IEEE bit reinterpretation.
    pub fn packed_modulation(self) -> [f32; 3] {
        [self.skin as f32, self.cloth as f32,
            ((self.sheet as u32) * 65536 + self.burn as u32) as f32]
    }

    /// Native burning ramp (client 103147f0, also ragdoll 10263900).
    /// Defaults are duration 10.0 and strength 0.6; callers supply current
    /// cvar values. Unburned entities retain zero instead of an elapsed ramp.
    pub fn with_burn_elapsed(mut self, elapsed: f32, duration: f32, strength: f32) -> Self {
        let progress = if elapsed.is_finite() && duration.is_finite() && duration > 0. {
            (elapsed / duration).clamp(0., 1.)
        } else { 0. };
        self.burn = if strength.is_finite() {
            (65535. * strength * progress).clamp(0., 65535.) as u16
        } else { 0 };
        self
    }
}

/// Geometry indices are independent of palette appearance. In particular, the
/// legacy common's 288 MDL skin families are not its 8 skin palette gradients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryVariation {
    pub skin_family: u32,
    pub body: u32,
}

#[derive(Clone, Debug)]
pub struct Bodygroup {
    pub name: String,
    pub divisor: u32,
    pub models: Vec<String>,
}

impl Bodygroup {
    /// server 10032c30/10032d00: reject blank, dismembered ('D'), and -1
    /// sentinel names. _atol of the name after its first byte is intentional;
    /// modern "dmx/..." names parse as zero and remain eligible.
    pub fn eligible(&self) -> Vec<usize> {
        self.models.iter().enumerate().filter_map(|(index, name)| {
            let bytes = name.as_bytes();
            if bytes.is_empty() || bytes[0] == b'D' { return None; }
            let tail = &bytes[1..];
            let mut at = 0;
            while tail.get(at).is_some_and(u8::is_ascii_whitespace) { at += 1; }
            let negative = tail.get(at) == Some(&b'-');
            if negative || tail.get(at) == Some(&b'+') { at += 1; }
            let mut number = 0u64;
            while let Some(&digit) = tail.get(at).filter(|d| d.is_ascii_digit()) {
                number = number.saturating_mul(10).saturating_add((digit - b'0') as u64);
                at += 1;
            }
            (!(negative && number == 1)).then_some(index)
        }).collect()
    }
}

#[derive(Clone, Debug)]
pub struct GeometryPolicy {
    pub skin_families: u32,
    pub groups: Vec<Bodygroup>,
}

impl GeometryPolicy {
    pub fn from_mdl(bytes: &[u8]) -> std::io::Result<Self> {
        use crate::binary::*;
        if range(bytes, 0, 4)? != b"IDST" { return Err(invalid("Not a studio model")); }
        let skin_families = count(bytes, 224, 4096)? as u32;
        let count_groups = count(bytes, 232, 256)?;
        let base = count(bytes, 236, bytes.len())?;
        let mut groups = Vec::with_capacity(count_groups);
        for index in 0..count_groups {
            let group = base + index * 16;
            range(bytes, group, 16)?;
            let name = cstring(bytes, relative(bytes, group, 0)?)?;
            let count_models = count(bytes, group + 4, 256)?;
            let divisor = count(bytes, group + 8, 1_000_000)? as u32;
            let models_base = relative(bytes, group, 12)?;
            let mut models = Vec::with_capacity(count_models);
            for model in 0..count_models {
                let name = range(bytes, models_base + model * 148, 64)?;
                let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
                models.push(String::from_utf8_lossy(&name[..end]).into_owned());
            }
            groups.push(Bodygroup { name, divisor, models });
        }
        Ok(Self { skin_families, groups })
    }

    /// Owned server 10402450 / 100387f0 / 1002dc20. Flags correspond to
    /// z_randomskins, z_randombodygroups, z_infected_damage_cutouts (all default
    /// 1). Returns indices only: caller must preload/render the matching variant.
    pub fn random(
        &self,
        rng: &mut UniformRandomStream,
        mut initial: GeometryVariation,
        random_skins: bool,
        random_bodies: bool,
        damage_cutouts: bool,
    ) -> GeometryVariation {
        if random_skins && self.skin_families > 0 {
            if damage_cutouts {
                initial.skin_family = rng.random_int(0, ((self.skin_families - 1) / 4) as i32) as u32 * 4;
            } else {
                // The installed spawn branch consumes this draw but does not
                // store its return value (10402782..10402793). Preserve that.
                rng.random_int(0, (self.skin_families - 1) as i32);
            }
        }
        if random_bodies {
            for requested in ["Head", "UpperBody", "LowerBody"] {
                let Some(group) = self.groups.iter().find(|g| g.name.eq_ignore_ascii_case(requested)) else {
                    // 100387f0 initializes the minimum to 10000 even when no
                    // matching bodygroup exists. This apparently wasted draw
                    // affects subsequent shared-stream choices.
                    rng.random_int(1, 10000);
                    continue;
                };
                let eligible = group.eligible();
                if eligible.is_empty() { continue; }
                let selected = eligible[(rng.random_int(1, eligible.len() as i32) - 1) as usize] as u32;
                if group.divisor == 0 || group.models.is_empty() { continue; }
                let old = initial.body / group.divisor % group.models.len() as u32;
                let value = i64::from(initial.body)
                    + (i64::from(selected) - i64::from(old)) * i64::from(group.divisor);
                if let Ok(body) = u32::try_from(value) { initial.body = body; }
            }
        }
        initial
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_draws_native_ranges_in_native_order() {
        let mut actual = UniformRandomStream::new(481);
        let mut oracle = actual.clone();
        let mut seen = [0u16; 3];
        for _ in 0..4096 {
            let a = Appearance::random(&mut actual);
            assert_eq!(a.skin as i32, oracle.random_int(0, 7));
            assert_eq!(a.cloth as i32, oracle.random_int(8, 15));
            assert_eq!(a.sheet as i32, oracle.random_int(0, 3));
            seen[0] |= 1 << a.skin;
            seen[1] |= 1 << (a.cloth - 8);
            seen[2] |= 1 << a.sheet;
        }
        assert_eq!(seen, [255, 255, 15]);
    }

    #[test]
    fn shader_packing_preserves_every_burn_value_and_all_quadrants() {
        for sheet in 0..4 {
            for burn in 0..=u16::MAX {
                let a = Appearance { skin: 7, cloth: 15, sheet, burn };
                let packed = a.packed_modulation()[2] as u32;
                assert_eq!(packed >> 16, sheet as u32);
                assert_eq!(packed & 65535, burn as u32);
            }
            let a = Appearance::from_rows(7, 15, sheet).unwrap();
            assert_eq!(a.lookup(16.), Vec4::new((sheet / 2) as f32 * 0.5,
                (sheet % 2) as f32 * 0.5, 7.5 / 16., 15.5 / 16.));
        }
    }

    #[test]
    fn burning_uses_truncation_and_native_defaults() {
        let a = Appearance::from_rows(0, 8, 3).unwrap();
        assert_eq!(a.with_burn_elapsed(5., 10., 0.6).burn, 19660);
        assert_eq!(a.with_burn_elapsed(10., 10., 0.6).burn, 39321);
        assert_eq!(a.with_burn_elapsed(30., 10., 0.6).burn, 39321);
        assert_eq!(a.with_burn_elapsed(-1., 10., 0.6).burn, 0);
        assert_eq!(a.with_burn_elapsed(f32::NAN, 10., 0.6).burn, 0);
        assert_eq!(a.with_burn_elapsed(5., 0., 0.6).burn, 0);
    }

    #[test]
    fn fixture_override_is_explicit_and_rejects_wrong_partitions() {
        let args = |s: &str| vec!["rewrite".into(), "--capture-infected-appearance".into(), s.into()];
        assert_eq!(Appearance::capture_override(&args("7 15 3")), Appearance::from_rows(7, 15, 3));
        assert!(Appearance::capture_override(&args("7 7 3")).is_none());
        assert!(Appearance::capture_override(&args("7 15 4")).is_none());
        assert!(Appearance::capture_override(&args("7 15 3 0")).is_none());
    }

    fn enemy() -> crate::game::Enemy {
        crate::game::Enemy {
            model: "models/infected/common_male01.mdl".into(),
            death_loot: None, loot_dropped: false, death: Default::default(),
            movement_speed: 0., reaction_serial: 0, attack_serial: 0,
            health: 50., attack: 0., reaction: 0, corpse_time: 0., burning: 0., vertical: 0.,
        }
    }

    #[test]
    fn assignment_is_once_per_entity_and_retains_appearance_through_death() {
        let mut app = App::new();
        install(&mut app);
        let actor = app.world_mut().spawn(enemy()).id();
        app.world_mut().run_schedule(PostUpdate);
        let original = *app.world().get::<Appearance>(actor).unwrap();
        app.world_mut().get_mut::<crate::game::Enemy>(actor).unwrap().health = 0.;
        for _ in 0..3 { app.world_mut().run_schedule(PostUpdate); }
        assert_eq!(app.world().get::<Appearance>(actor), Some(&original));
        let next = app.world_mut().spawn(enemy()).id();
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<Appearance>(next).is_some());
        assert_eq!(app.world().get::<Appearance>(actor), Some(&original));
    }

    #[test]
    fn assignment_preserves_explicit_actor_data_and_excludes_special_infected() {
        let mut app = App::new();
        install(&mut app);
        let fixture = Appearance::from_rows(6, 12, 2).unwrap();
        let actor = app.world_mut().spawn((enemy(), fixture)).id();
        let mut hunter = enemy();
        hunter.model = "models/infected/hunter.mdl".into();
        let special = app.world_mut().spawn(hunter).id();
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<Appearance>(actor), Some(&fixture));
        assert!(app.world().get::<Appearance>(special).is_none());
    }

    #[test]
    fn native_burn_start_survives_flame_refresh_extinguishing_and_death() {
        let mut app = App::new();
        install(&mut app);
        let mut state = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        state.paused = false;
        state.elapsed = 2.;
        app.insert_resource(state);
        let actor = app.world_mut().spawn(enemy()).id();
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<BurnStarted>(actor).is_none());
        assert_eq!(app.world().get::<Appearance>(actor).unwrap().burn, 0);
        app.world_mut().get_mut::<crate::game::Enemy>(actor).unwrap().burning = 4.;
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<BurnStarted>(actor).unwrap().0, 2.);
        app.world_mut().resource_mut::<crate::game_state::Match>().elapsed = 7.;
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<Appearance>(actor).unwrap().burn, 19660);
        // A paused match keeps its simulation clock fixed, regardless of frames.
        app.world_mut().resource_mut::<crate::game_state::Match>().paused = true;
        for _ in 0..4 { app.world_mut().run_schedule(PostUpdate); }
        assert_eq!(app.world().get::<Appearance>(actor).unwrap().burn, 19660);
        {
            let mut enemy = app.world_mut().get_mut::<crate::game::Enemy>(actor).unwrap();
            enemy.burning = 0.;
            enemy.health = 0.;
        }
        app.world_mut().resource_mut::<crate::game_state::Match>().paused = false;
        app.world_mut().resource_mut::<crate::game_state::Match>().elapsed = 12.;
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(app.world().get::<Appearance>(actor).unwrap().burn, 39321);
        assert_eq!(app.world().get::<BurnStarted>(actor).unwrap().0, 2.);
    }

    #[test]
    fn native_bodygroup_eligibility_excludes_dismembered_and_sentinel_models() {
        let group = Bodygroup { name: "Head".into(), divisor: 1,
            models: ["", "D01_Head", "H-1", "H01_Head", "dmx/cim_head.dmx", "H-01_bad", "H0", "d01"]
                .map(str::to_string).to_vec() };
        assert_eq!(group.eligible(), [3, 4, 6, 7]);
    }

    #[test]
    fn native_geometry_draws_head_before_upper_body_and_preserves_other_groups() {
        let policy = GeometryPolicy { skin_families: 8, groups: vec![
            Bodygroup { name: "UpperBody".into(), divisor: 4,
                models: vec!["U01".into(), "D01".into(), "U02".into()] },
            Bodygroup { name: "head".into(), divisor: 1,
                models: vec!["H01".into(), "D01".into(), "H02".into(), "H-1".into()] },
            Bodygroup { name: "LowerBody".into(), divisor: 12, models: vec!["".into()] },
        ] };
        let mut rng = UniformRandomStream::new(93);
        let mut oracle = rng.clone();
        for _ in 0..128 {
            let actual = policy.random(&mut rng, GeometryVariation { skin_family: 0, body: 12 }, true, true, true);
            let skin = oracle.random_int(0, 1) as u32 * 4;
            let head = [0, 2][(oracle.random_int(1, 2) - 1) as usize];
            let upper = [0, 2][(oracle.random_int(1, 2) - 1) as usize];
            assert_eq!(actual, GeometryVariation { skin_family: skin, body: 12 + head + upper * 4 });
        }
        assert_eq!(rng.random_int(0, 10000), oracle.random_int(0, 10000));
    }

    #[test]
    fn native_missing_group_and_disabled_cutout_draws_are_retained() {
        let policy = GeometryPolicy { skin_families: 8, groups: vec![] };
        let initial = GeometryVariation { skin_family: 3, body: 17 };
        let mut rng = UniformRandomStream::new(6);
        let mut oracle = rng.clone();
        assert_eq!(policy.random(&mut rng, initial, true, true, false), initial);
        oracle.random_int(0, 7);
        for _ in 0..3 { oracle.random_int(1, 10000); }
        assert_eq!(rng.random_int(0, 100000), oracle.random_int(0, 100000));
        let mut unchanged = rng.clone();
        assert_eq!(policy.random(&mut rng, initial, false, false, true), initial);
        assert_eq!(rng.random_int(0, 100000), unchanged.random_int(0, 100000));
    }

    #[test]
    fn installed_common_bodygroup_metadata_matches_native_policy() {
        let root = std::env::var("L4D2_GAME_DIR")
            .unwrap_or_else(|_| r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2".into());
        let mut assets = crate::source_assets::SourceAssets::new(std::path::Path::new(&root), vec![]).unwrap();
        for (path, families, counts) in [
            ("models/infected/common_male01.mdl", 288, [2, 2, 1]),
            ("models/infected/common_male_tshirt_cargos.mdl", 8, [7, 5, 0]),
            ("models/infected/common_male_ceda.mdl", 8, [6, 1, 0]),
        ] {
            let policy = GeometryPolicy::from_mdl(&assets.read(path).unwrap()).unwrap();
            assert_eq!(policy.skin_families, families, "{path}");
            for (name, expected) in ["Head", "UpperBody", "LowerBody"].into_iter().zip(counts) {
                assert_eq!(policy.groups.iter().find(|g| g.name.eq_ignore_ascii_case(name)).unwrap().eligible().len(), expected, "{path}: {name}");
            }
            let mut rng = UniformRandomStream::new(42);
            for _ in 0..128 {
                let variant = policy.random(&mut rng, GeometryVariation::default(), true, true, true);
                assert!(variant.skin_family < policy.skin_families);
                assert_eq!(variant.skin_family % 4, 0);
                for group in &policy.groups {
                    if group.divisor == 0 || group.eligible().is_empty() { continue; }
                    let index = variant.body / group.divisor % group.models.len() as u32;
                    assert!(group.eligible().contains(&(index as usize)), "{path}: {}", group.name);
                }
            }
        }
    }
}
