//! Uncommon sound pulses and screen-space mud using owned Source sound/PCF assets.
use crate::{
    game::Enemy, game_state::Match, infected_population::Kind, source_assets::SourceAssets,
};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use l4d2r_game_core::random::UniformRandomStream;
use std::collections::{HashMap, HashSet};
#[derive(Resource, Default)]
pub struct ClownSignals {
    pulses: Vec<(Entity, Vec3, f32)>,
    clock: f32,
    game_clock: f32,
}
impl ClownSignals {
    pub fn target(&self, position: Vec3) -> Option<Vec3> {
        self.pulses
            .iter()
            .filter(|(_, p, t)| {
                *t > self.clock && p.distance_squared(position) < (500. * 0.0254_f32).powi(2)
            })
            .min_by(|a, b| {
                a.1.distance_squared(position)
                    .total_cmp(&b.1.distance_squared(position))
            })
            .map(|x| x.1)
    }
}
#[derive(Default)]
pub struct Feet {
    positions: HashMap<Entity, (Vec3, f32, bool)>,
}
pub fn clown_steps(
    state: Res<Match>,
    time: Res<Time<Fixed>>,
    mut signals: ResMut<ClownSignals>,
    mut feet: Local<Feet>,
    enemies: Query<(Entity, &Transform, &Enemy)>,
    mut audio: ResMut<crate::game_audio::Queue>,
) {
    if state.paused {
        return;
    }
    if state.elapsed < signals.game_clock {
        signals.pulses.clear();
        feet.positions.clear();
    }
    signals.game_clock = state.elapsed;
    signals.clock += time.delta_secs();
    let living = enemies
        .iter()
        .filter(|(_, _, e)| e.health > 0. && Kind::model(&e.model) == Kind::Clown)
        .map(|(id, _, _)| id)
        .collect::<HashSet<_>>();
    feet.positions.retain(|id, _| living.contains(id));
    let now = signals.clock;
    signals
        .pulses
        .retain(|(id, _, t)| living.contains(id) && *t > now);
    for (id, t, e) in &enemies {
        if !living.contains(&id) {
            continue;
        }
        let entry = feet
            .positions
            .entry(id)
            .or_insert((t.translation, 0., false));
        let distance = (t.translation - entry.0).xz().length();
        entry.0 = t.translation;
        if e.movement_speed < 0.08 || distance > 3. {
            entry.1 = 0.;
            continue;
        }
        entry.1 += distance;
        if entry.1 >= 0.9 {
            entry.1 %= 0.9;
            entry.2 = !entry.2;
            audio.play(
                if entry.2 {
                    "Clown.Default.RunLeft"
                } else {
                    "Clown.Default.RunRight"
                },
                Some(t.translation),
            );
            signals.pulses.retain(|(who, _, _)| *who != id);
            signals.pulses.push((id, t.translation, now + 1.2));
        }
    }
}
#[derive(Resource)]
struct MudAssets {
    image: Handle<Image>,
    sequences: Vec<crate::source_pcf::SheetSequence>,
    size: Vec2,
    last: u64,
    rng: UniformRandomStream,
    life: f32,
    fade: f32,
}
#[derive(Component)]
struct MudSprite {
    age: f32,
    life: f32,
    fade: f32,
    sequence: usize,
    opacity: f32,
}
pub fn install(app: &mut App) {
    app.init_resource::<ClownSignals>()
        .add_systems(Update, (mud_setup, mud_update).chain());
}
fn mud_setup(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    existing: Option<Res<MudAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    if existing.is_some() {
        return;
    }
    let load=||->std::io::Result<(crate::vtf::Texture,Vec<crate::source_pcf::SheetSequence>,f32,f32)> {
  let mut assets=SourceAssets::new(&source.0,vec![])?;let elements=crate::source_pcf::parse(&assets.read("particles/screen_fx.pcf")?)?;
  let root=elements.iter().find(|e|e.name=="screen_mud_splatter_a" && e.kind=="DmeParticleSystemDefinition").ok_or_else(||std::io::Error::other("Missing native mud effect"))?;
  let material=crate::material::load(&mut assets,root.text("material"))?;
  let data=assets.read(&material.base.ok_or_else(||std::io::Error::other("Missing mud texture"))?)?;
  let life=root.references("initializers").into_iter().filter_map(|i|elements.get(i)).find(|e|e.text("functionName")=="Lifetime Random").map_or(4.,|e|e.number("lifetime_max",4.));
  let fade=root.references("operators").into_iter().filter_map(|i|elements.get(i)).find(|e|e.text("functionName")=="Alpha Fade Out Random").map_or(0.5,|e|e.number("fade out time max",0.5));
  Ok((crate::vtf::decode(&data,2048)?,crate::source_pcf::sheet(&data)?,life,fade))
 };
    match load() {
        Ok((t, sequences, life, fade)) => {
            let size = Vec2::new(t.width as f32, t.height as f32);
            let image = images.add(Image::new(
                Extent3d {
                    width: t.width,
                    height: t.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                t.rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            ));
            commands.insert_resource(MudAssets {
                image,
                sequences,
                size,
                last: 0,
                rng: UniformRandomStream::new(923),
                life,
                fade,
            });
        }
        Err(e) => eprintln!("Native mud screen load: {e}"),
    }
}
fn alpha(age: f32, life: f32, fade: f32) -> f32 {
    if age >= life {
        0.
    } else {
        ((life - age) / fade.max(0.001)).clamp(0., 1.)
    }
}
fn mud_update(
    mut commands: Commands,
    time: Res<Time>,
    mut state: ResMut<Match>,
    session: Res<crate::campaign_settings::SessionSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut assets: Option<ResMut<MudAssets>>,
    cameras: Query<Entity, With<crate::game::ViewCamera>>,
    mut sprites: Query<(Entity, &mut MudSprite, &mut ImageNode)>,
    mut captured: Local<bool>,
) {
    let Some(ref mut assets) = assets else {
        return;
    };
    if !state.paused
        && crate::test_lab::active(&session.map)
        && (keys.just_pressed(KeyCode::KeyM)
            || (!*captured
                && std::env::args().any(|a| a == "--capture-mud")
                && state.elapsed > 2.4))
    {
        state.mud_serial = state.mud_serial.wrapping_add(1);
        *captured = true;
    }
    for (id, mut s, mut image) in &mut sprites {
        if state.dead {
            commands.entity(id).despawn();
            continue;
        }
        if !state.paused {
            s.age += time.delta_secs();
        }
        if s.age >= s.life {
            commands.entity(id).despawn();
            continue;
        }
        image.color = image
            .color
            .with_alpha(s.opacity * alpha(s.age, s.life, s.fade));
        if let Some(seq) = assets.sequences.iter().find(|seq| seq.id == s.sequence) {
            let mut clock = s.age / s.life * seq.duration;
            let mut uv = seq.frames.last().map(|f| f.1);
            for (duration, frame) in &seq.frames {
                if clock < *duration {
                    uv = Some(*frame);
                    break;
                }
                clock -= *duration;
            }
            if let Some(uv) = uv {
                image.rect = Some(Rect::new(
                    uv[0] * assets.size.x,
                    uv[1] * assets.size.y,
                    uv[2] * assets.size.x,
                    uv[3] * assets.size.y,
                ));
            }
        }
    }
    if state.mud_serial == 0 {
        if assets.last != 0 {
            for (id, _, _) in &sprites {
                commands.entity(id).despawn();
            }
        }
        assets.last = 0;
        return;
    }
    if state.mud_serial == assets.last || state.paused || state.dead {
        return;
    }
    assets.last = state.mud_serial;
    let Some(camera) = cameras.iter().next() else {
        return;
    };
    let ids = sprites.iter().map(|(id, _, _)| id).collect::<Vec<_>>();
    for id in ids.iter().take(ids.len().saturating_sub(24)) {
        commands.entity(*id).despawn();
    }
    for _ in 0..12 {
        let x = assets.rng.random_float(-120., 120.);
        let y = assets.rng.random_float(-100., 100.);
        let radius = 48. + 16. * assets.rng.random_float(0., 1.).powi(2);
        let tint = assets.rng.random_float(0., 1.);
        let sequence = assets.rng.random_int(3, 27) as usize;
        let opacity = assets.rng.random_float(230. / 255., 1.);
        let mut image = ImageNode::new(assets.image.clone());
        image.color = Color::srgba(
            (33. + 15. * tint) / 255.,
            (21. + 9. * tint) / 255.,
            (5. + 3. * tint) / 255.,
            opacity,
        );
        if let Some(uv) = assets
            .sequences
            .iter()
            .find(|s| s.id == sequence)
            .and_then(|s| s.frames.first())
            .map(|f| f.1)
        {
            image.rect = Some(Rect::new(
                uv[0] * assets.size.x,
                uv[1] * assets.size.y,
                uv[2] * assets.size.x,
                uv[3] * assets.size.y,
            ));
        }
        let angle = assets.rng.random_float(0., 360.);
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: percent(50.),
                width: vh(radius * 2. / 4.8),
                height: vh(radius * 2. / 4.8),
                margin: UiRect {
                    left: vh((x - radius) / 4.8),
                    top: vh((y - radius) / 4.8),
                    ..default()
                },
                ..default()
            },
            image,
            UiTransform::from_rotation(Rot2::degrees(angle)),
            UiTargetCamera(camera),
            GlobalZIndex(-5),
            MudSprite {
                age: 0.,
                life: assets.life,
                fade: assets.fade,
                sequence,
                opacity,
            },
        ));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mud_lifetime_and_final_fade_clear_the_view() {
        assert_eq!(alpha(0., 4., 0.5), 1.);
        assert_eq!(alpha(3.75, 4., 0.5), 0.5);
        assert_eq!(alpha(4., 4., 0.5), 0.);
    }
    #[test]
    fn moving_clown_emits_native_steps_and_lure_disappears_when_dead() {
        let mut app = App::new();
        let mut state = Match::new(&crate::game_state::tests_weapon());
        state.paused = false;
        app.insert_resource(state)
            .init_resource::<ClownSignals>()
            .init_resource::<crate::game_audio::Queue>()
            .insert_resource(Time::<Fixed>::default())
            .add_systems(Update, clown_steps);
        let id = app
            .world_mut()
            .spawn((
                Transform::default(),
                Enemy {
                    model: "models/infected/common_male_clown.mdl".into(),
                    death_loot: None,
                    loot_dropped: false,
                    death: Default::default(),
                    movement_speed: 2.7,
                    reaction_serial: 0,
                    attack_serial: 0,
                    health: 50.,
                    attack: 0.,
                    reaction: 0,
                    corpse_time: 0.,
                    burning: 0.,
                    vertical: 0.,
                },
            ))
            .id();
        app.update();
        for i in 1..=5 {
            app.world_mut()
                .get_mut::<Transform>(id)
                .unwrap()
                .translation
                .x = i as f32 * 0.2;
            app.world_mut().resource_mut::<Match>().elapsed = i as f32 * 0.1;
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(std::time::Duration::from_secs_f32(0.1));
            app.update();
        }
        assert_eq!(
            app.world().resource::<crate::game_audio::Queue>().0.len(),
            1
        );
        assert_eq!(
            app.world().resource::<crate::game_audio::Queue>().0[0].name,
            "Clown.Default.RunLeft"
        );
        assert!(
            app.world()
                .resource::<ClownSignals>()
                .target(Vec3::ZERO)
                .is_some()
        );
        assert!(
            app.world()
                .resource::<ClownSignals>()
                .target(Vec3::X * 20.)
                .is_none()
        );
        app.world_mut().get_mut::<Enemy>(id).unwrap().health = 0.;
        app.update();
        assert!(
            app.world()
                .resource::<ClownSignals>()
                .target(Vec3::ZERO)
                .is_none()
        );
    }
    #[test]
    fn owned_mud_texture_atlas_and_clown_sound_waves_exist() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let registry = crate::source_sound::Registry::load(&mut assets).unwrap();
        for name in [
            "clown.default.runleft",
            "clown.default.runright",
            "weapon.hitclown",
        ] {
            for wave in &registry.definitions[name].waves {
                assert!(!assets.read(wave).unwrap().is_empty());
            }
        }
        let pcf =
            crate::source_pcf::parse(&assets.read("particles/screen_fx.pcf").unwrap()).unwrap();
        let def = pcf
            .iter()
            .find(|e| e.name == "screen_mud_splatter_a" && e.kind == "DmeParticleSystemDefinition")
            .unwrap();
        assert_eq!(def.number("max_particles", 0.), 6.);
        let mat = crate::material::load(&mut assets, def.text("material")).unwrap();
        let data = assets.read(&mat.base.unwrap()).unwrap();
        let sheet = crate::source_pcf::sheet(&data).unwrap();
        for id in 3..=27 {
            assert!(sheet.iter().any(|s| s.id == id && !s.frames.is_empty()));
        }
    }
}
