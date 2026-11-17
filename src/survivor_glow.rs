//! Campaign teammate silhouettes. Mask pass shares the existing animated meshes.
use bevy::{
    camera::{
        RenderTarget,
        visibility::{NoFrustumCulling, RenderLayers},
    },
    image::ImageSampler,
    mesh::skinning::SkinnedMesh,
    prelude::*,
    render::render_resource::{AsBindGroup, TextureFormat},
    shader::ShaderRef,
};
use std::collections::HashMap;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct GlowComposite {
    #[texture(0)]
    #[sampler(1)]
    mask: Handle<Image>,
    #[uniform(2)]
    effects: Vec4,
}
impl UiMaterial for GlowComposite {
    fn fragment_shader() -> ShaderRef {
        "shaders/survivor_glow.wgsl".into()
    }
}
#[derive(Component)]
struct GlowCamera;
#[derive(Component)]
struct GlowOverlay;
#[derive(Resource)]
pub(crate) struct GlowFixture;
#[derive(Component)]
struct GlowDraw {
    source: Entity,
    actor: Entity,
}
#[derive(Resource)]
struct GlowPass {
    image: Handle<Image>,
    materials: HashMap<Entity, Handle<StandardMaterial>>,
    fades: HashMap<Entity, f32>,
    composite: Handle<GlowComposite>,
    was_focused: bool,
    profile_next: f32,
}
pub(crate) fn reset(world: &mut World) {
    world.remove_resource::<GlowPass>();
}
pub(crate) fn verify_scene(world: &mut World) {
    assert!(world.contains_resource::<GlowPass>(), "Map lacks glow pass");
    assert_eq!(
        world
            .query_filtered::<Entity, With<GlowCamera>>()
            .iter(world)
            .count(),
        1,
        "Map must rebuild exactly one outline camera"
    );
    assert_eq!(
        world
            .query_filtered::<Entity, With<GlowOverlay>>()
            .iter(world)
            .count(),
        1,
        "Map must rebuild exactly one outline overlay"
    );
}
pub fn install(app: &mut App) {
    if std::env::args().any(|a| a == "--capture-survivor-glow") {
        app.insert_resource(GlowFixture);
    }
    app.init_resource::<crate::glow_settings::GlowSettings>()
        .add_plugins(UiMaterialPlugin::<GlowComposite>::default())
        .add_systems(Update, setup)
        .add_systems(
            PostUpdate,
            sync.after(crate::l4d2r_animation::update)
                .before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            sync_draw_transforms
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}
// Independent mask draws must not inherit PVS/hidden state from the world mesh.
// Copy current world transforms after propagation so skin and mask stay aligned.
fn sync_draw_transforms(
    sources: Query<&GlobalTransform, Without<GlowDraw>>,
    mut draws: Query<(&GlowDraw, &mut GlobalTransform)>,
) {
    for (draw, mut transform) in &mut draws {
        if let Ok(source) = sources.get(draw.source) {
            transform.set_if_neq(*source);
        }
    }
}
/// Phones render the glow mask at half resolution without MSAA. The mask is
/// blurred (cl_glow_blur_scale) before it is composited, so the outline looks
/// the same, but a full-screen 4x MSAA camera at the iPhone's native size
/// held ~100 MiB of colour/depth targets for an effect that is mostly empty.
fn mobile() -> bool {
    cfg!(any(target_os = "ios", target_os = "android"))
        || std::env::args().any(|a| a == "--profile-mobile-memory")
}
fn mask_size(size: UVec2) -> UVec2 {
    if mobile() {
        (size / 2).max(UVec2::ONE)
    } else {
        size
    }
}
fn setup(
    mut commands: Commands,
    pass: Option<Res<GlowPass>>,
    cameras: Query<(Entity, &Camera, &Projection), With<crate::viewer::GameCamera>>,
    hud: Query<Entity, With<crate::game::ViewCamera>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GlowComposite>>,
) {
    if pass.is_some() {
        return;
    }
    let (Some((_, camera, projection)), Some(hud)) = (cameras.iter().next(), hud.iter().next())
    else {
        return;
    };
    let size = mask_size(
        camera
            .physical_target_size()
            .filter(|s| s.x > 1 && s.y > 1)
            .unwrap_or(UVec2::new(1280, 720)),
    );
    // Full-resolution, antialiased silhouettes avoid magnified mask pixels.
    let mut mask = Image::new_target_texture(
        size.x.max(1),
        size.y.max(1),
        TextureFormat::Rgba8Unorm,
        None,
    );
    mask.sampler = ImageSampler::linear();
    let image = images.add(mask);
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -2,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(image.clone().into()),
        projection.clone(),
        Transform::default(),
        if mobile() { Msaa::Off } else { Msaa::Sample4 },
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        bevy::camera::Exposure { ev100: 0. },
        RenderLayers::layer(3),
        GlowCamera,
    ));
    let composite = materials.add(GlowComposite {
        mask: image.clone(),
        effects: Vec4::new(3., 1., 0., 0.),
    });
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: percent(100.),
            height: percent(100.),
            ..default()
        },
        MaterialNode(composite.clone()),
        UiTargetCamera(hud),
        GlobalZIndex(-10),
        GlowOverlay,
    ));
    commands.insert_resource(GlowPass {
        image,
        materials: HashMap::new(),
        fades: HashMap::new(),
        composite,
        was_focused: true,
        profile_next: 1.,
    });
}
#[cfg(test)]
fn fade(current: f32, hidden: bool, dt: f32) -> f32 {
    (current + if hidden { dt / 0.5 } else { -dt / 0.5 }).clamp(0., 1.)
}
fn survivor_color(
    settings: &crate::glow_settings::GlowSettings,
    state: &crate::game_state::Match,
) -> Option<Color> {
    if state.dead {
        None
    } else {
        // Retail client.dll 10257830: bile takes precedence, then
        // incapacitation/special holds. Ordinary hits and remaining health
        // do not change the survivor team's teammate colour.
        Some(settings.color(if state.vomited > 0. {
            "cl_glow_survivor_vomit"
        } else if state.incap || state.pinned {
            "cl_glow_survivor_hurt"
        } else {
            "cl_glow_survivor"
        }))
    }
}
fn survivor_view_color(
    settings: &crate::glow_settings::GlowSettings,
    state: &crate::game_state::Match,
    infected_view: bool,
    elapsed: f32,
) -> Option<Color> {
    if !infected_view {
        return survivor_color(settings, state);
    }
    if state.dead {
        return None;
    }
    let colorblind = settings.get("cl_colorblind", 0.) as i32 == 2;
    if state.vomited > 0. {
        return Some(settings.color(if colorblind {
            "cl_glow_ability_colorblind"
        } else {
            "cl_glow_ability"
        }));
    }
    // client.dll 1026ec00/1026ec80: 25% and survivor_limp_health=40%,
    // truncating the permanent + temporary sum before classification.
    let buffer = if settings.get("cl_glow_survivor_health_include_buffer", 1.) != 0. {
        state.temporary
    } else {
        0.
    };
    let health = (state.health as i32 as f32 + buffer) as i32;
    let tier = if state.incap || (state.black_white && health < 2) {
        "crit"
    } else if health < 25 {
        "low"
    } else if health < 40 {
        "med"
    } else {
        "high"
    };
    let prefix = format!(
        "cl_glow_survivor_health_{tier}{}",
        if colorblind { "_colorblind" } else { "" }
    );
    let color = settings.color(&prefix).to_srgba();
    // 10257830 applies this to the health colour, even when buffer inclusion
    // is disabled. Ability/bile colours return before the pulse.
    let pulse =
        if state.temporary > 0. && settings.get("cl_glow_survivor_health_bleed_pulse", 1.) != 0. {
            1. + (elapsed * settings.get("cl_glow_survivor_health_bleed_pulse_speed", 10.)).sin()
                * settings.get("cl_glow_survivor_health_bleed_pulse_amount", 0.3)
        } else {
            1.
        };
    Some(Color::srgb(
        color.red * pulse,
        color.green * pulse,
        color.blue * pulse,
    ))
}
fn pickup_glow_score(
    eye: Vec3,
    forward: Vec3,
    origin: Vec3,
    tolerance: f32,
    legacy: bool,
    sight_fraction: impl FnOnce() -> f32,
) -> Option<f32> {
    // Reach/aim are cheap and reject distant pickups before tracing across the
    // whole map. Legacy ordering is opt-in profiling only, for A/B evidence.
    let score = if legacy {
        None
    } else {
        Some(crate::game::pickup_aim_score(eye, forward, origin)?)
    };
    let direction = (origin + Vec3::Y * 0.12 - eye).normalize_or_zero();
    if forward.dot(direction) < tolerance || sight_fraction() < 0.98 {
        return None;
    }
    score.or_else(|| crate::game::pickup_aim_score(eye, forward, origin))
}
fn sync(
    mut commands: Commands,
    mut pass: Option<ResMut<GlowPass>>,
    time: Res<Time>,
    camera: Query<
        (&Camera, &Transform, &Projection),
        (With<crate::viewer::GameCamera>, Without<GlowCamera>),
    >,
    mut masks: Query<(&mut Transform, &mut Projection, &mut Camera), With<GlowCamera>>,
    actors: Query<(Entity, &crate::game::CompanionActor)>,
    sources: Query<
        (
            Entity,
            &Mesh3d,
            Option<&ChildOf>,
            Option<&SkinnedMesh>,
            Option<&crate::loot::PickupOwner>,
        ),
        Without<GlowDraw>,
    >,
    mut draws: Query<(
        Entity,
        &GlowDraw,
        &mut Mesh3d,
        &mut Visibility,
        Option<&SkinnedMesh>,
    )>,
    squad: Res<crate::game_state::Squad>,
    state: Res<crate::game_state::Match>,
    frontend: Res<crate::game_menu::Frontend>,
    collision: Res<crate::collision::Collision>,
    extra: (
        Res<crate::glow_settings::GlowSettings>,
        Query<
            (
                Entity,
                &crate::game::Pickup,
                Option<&crate::loot::Stock>,
                Option<&crate::loot::PickupMapEntity>,
            ),
            Without<crate::game::Taken>,
        >,
        Res<crate::l4d2r_cinematic::MapLogic>,
        Query<&Window>,
        Query<(Entity, &crate::l4d2r_cinematic::MapEntity)>,
        Option<Res<crate::render_warmup::Warmup>>,
    ),
    assets: (
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<GlowComposite>>,
        ResMut<Assets<Image>>,
    ),
    mut overlays: Query<&mut Visibility, (With<GlowOverlay>, Without<GlowDraw>)>,
    specials: Query<(Entity, &crate::special_infected::ai::AiSpecial, &crate::game::Enemy)>,
) {
    let _timing = crate::performance::scope("glow::sync");
    let (mut materials, mut composites, mut images) = assets;
    let (settings, pickups, logic, windows, map_draws, warming) = extra;
    let enabled = frontend.started && !logic.active;
    let Some(ref mut pass) = pass else {
        return;
    };
    let Some((camera, transform, projection)) = camera.iter().next() else {
        return;
    };
    let focused = windows.iter().next().is_none_or(|w| w.focused);
    let restored = focused && !pass.was_focused;
    pass.was_focused = focused;
    let mut resized = false;
    for (mut t, mut p, _) in &mut masks {
        t.set_if_neq(*transform);
        let unchanged = match (&*p, projection) {
            (Projection::Perspective(a), Projection::Perspective(b)) => {
                a.fov == b.fov
                    && a.aspect_ratio == b.aspect_ratio
                    && a.near == b.near
                    && a.far == b.far
                    && a.near_clip_plane == b.near_clip_plane
            }
            _ => false,
        };
        if !unchanged {
            *p = projection.clone();
        }
    }
    if let Some(size) = camera.physical_target_size().filter(|s| s.x > 1 && s.y > 1).map(mask_size) {
        if let Some(mut image) = images.get_mut(&pass.image) {
            let size = bevy::render::render_resource::Extent3d {
                width: size.x.max(1),
                height: size.y.max(1),
                depth_or_array_layers: 1,
            };
            if image.texture_descriptor.size != size {
                image.resize(size);
                resized = true;
            }
        }
    }
    let eye = transform.translation;
    if let Some(mut m) = composites.get_mut(&pass.composite) {
        let effects = Vec4::new(
            settings.get("cl_glow_blur_scale", 3.).clamp(0., 12.),
            settings.get("cl_glow_brightness", 1.).clamp(0., 5.),
            0.,
            0.,
        );
        // Image resize replaces the GPU texture: refresh its UI bind group.
        if resized || restored || m.effects != effects {
            m.effects = effects;
        }
    }
    let mut active = std::collections::HashSet::new();
    for (entity, actor) in &actors {
        if !enabled {
            continue;
        }
        let Some(member) = squad.0.get(actor.0) else {
            continue;
        };
        // Native sight fades are not health-colour thresholds (those are the
        // infected team's view). Multiple body samples avoid a single-ray pop.
        // The infected team sees survivor outlines at all times, in view or
        // behind walls; only teammates fade in when out of sight.
        let hidden = state.infected
            || [
            Vec3::Y * 0.8,
            Vec3::Y * 1.3,
            Vec3::Y * 1.65,
            Vec3::new(0.2, 1.2, 0.),
            Vec3::new(-0.2, 1.2, 0.),
        ]
        .iter()
        .all(|offset| {
            collision
                .sight_trace(eye, member.position + *offset)
                .fraction
                < 0.99
        });
        let alpha = pass.fades.entry(entity).or_insert(0.);
        if !state.paused {
            let duration = settings
                .get(
                    if hidden {
                        "cl_glow_los_fade_in_time"
                    } else {
                        "cl_glow_los_fade_out_time"
                    },
                    0.5,
                )
                .max(0.001);
            *alpha = (*alpha
                + if hidden {
                    time.delta_secs() / duration
                } else {
                    -time.delta_secs() / duration
                })
            .clamp(0., 1.);
        }
        // client 10257ce0: a pinned, hanging or incapacitated teammate
        // glows at full strength in or out of sight (in the hurt colour).
        let alpha = if !state.infected && (member.state.pinned || member.state.incap) {
            1.
        } else {
            *alpha
        };
        let tint = survivor_view_color(
            &settings,
            &member.state,
            state.infected,
            time.elapsed_secs(),
        );
        if tint.is_some() && alpha > 0.001 {
            active.insert(entity);
        }
        let handle = pass
            .materials
            .entry(entity)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::BLACK,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .clone();
        if let Some(mut m) = materials.get_mut(&handle) {
            let color = tint.unwrap_or(Color::NONE).with_alpha(alpha);
            if m.base_color != color {
                m.base_color = color;
            }
        }
    }
    // client 10257ce0/10257830: a survivor sees a Hunter pouncing, or a
    // Charger carrying or pummelling, a teammate other than themselves in
    // cl_glow_ability red at full strength. (The Jockey sets the same flag
    // with no strength, and the Smoker none, so neither shows.)
    for (entity, ai, enemy) in &specials {
        if !enabled || state.infected || state.dead || enemy.health <= 0. {
            continue;
        }
        if !attacker_glows(ai.class, ai.mode) {
            continue;
        }
        active.insert(entity);
        let tint = settings.color(
            if settings.get("cl_colorblind", 0.) as i32 == 2 {
                "cl_glow_ability_colorblind"
            } else {
                "cl_glow_ability"
            },
        );
        let handle = pass
            .materials
            .entry(entity)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: tint,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .clone();
        if let Some(mut m) = materials.get_mut(&handle) {
            if m.base_color != tint {
                m.base_color = tint;
            }
        }
    }
    let forward = transform.rotation * Vec3::NEG_Z;
    // Retail outlines the focused use target, rather than every pickup in
    // the nearby cone. Share the interaction aim score and origin offset.
    static PROFILE: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    let (audit, legacy) = *PROFILE.get_or_init(|| {
        let args = std::env::args().collect::<Vec<_>>();
        let audit = args.iter().any(|a| a == "--profile-frames");
        (
            audit,
            audit && args.iter().any(|a| a == "--profile-legacy-glow-scan"),
        )
    });
    let mut scanned = 0usize;
    let mut traces = 0usize;
    let focused = {
        let _timing = crate::performance::scope("glow::pickup_target");
        pickups
            .iter()
            .filter_map(|(entity, pickup, stock, binding)| {
                scanned += 1;
                if !enabled
                    || stock.is_some_and(|s| s.remaining == 0)
                    || !crate::loot::pickup_active(binding, Some(&logic))
                {
                    return None;
                }
                pickup_glow_score(
                    eye,
                    forward,
                    pickup.origin,
                    settings.get("glow_use_tolerance", 0.85).clamp(0., 1.),
                    legacy,
                    || {
                        traces += 1;
                        let target = pickup.origin + Vec3::Y * 0.12;
                        binding
                            .map_or_else(
                                || collision.sight_trace(eye, target),
                                |b| collision.prop_trace(b.0, eye, target, Vec3::ZERO),
                            )
                            .fraction
                    },
                )
                .map(|score| (entity, pickup, score))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
    };
    if audit && state.elapsed >= pass.profile_next {
        pass.profile_next = state.elapsed + 1.;
        println!(
            "Glow pickup scan: time={:.2} legacy={legacy} scanned={scanned} sight_traces={traces}",
            state.elapsed
        );
    }
    if let Some((entity, pickup, _)) = focused {
        active.insert(entity);
        let tint = settings.color(
            if state.black_white && pickup.class == "weapon_first_aid_kit" {
                "cl_glow_thirdstrike_item"
            } else {
                "cl_glow_item"
            },
        );
        let handle = pass
            .materials
            .entry(entity)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: tint,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .clone();
        if let Some(mut m) = materials.get_mut(&handle) {
            if m.base_color != tint {
                m.base_color = tint;
            }
        }
    }
    // Authored StartGlowing/StopGlowing state drives doors and event props.
    // Nearby available buttons also highlight their linked visible panel.
    let highlighted = if enabled && !state.infected && !state.dead {
        map_highlights(&logic, eye)
    } else { Default::default() };
    for (entity, binding) in &map_draws {
        if !highlighted.contains(&binding.0) {
            continue;
        }
        let Some(e) = logic.world.ents.get(binding.0) else {
            continue;
        };
        let tint =
            e.me.kv("glowcolor")
                .and_then(|v| {
                    let rgb: Vec<f32> = v
                        .split_whitespace()
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    (rgb.len() == 3 && rgb.iter().any(|v| *v > 0.))
                        .then(|| Color::srgb(rgb[0] / 255., rgb[1] / 255., rgb[2] / 255.))
                })
                .unwrap_or_else(|| settings.color("cl_glow_item"));
        active.insert(entity);
        let handle = pass
            .materials
            .entry(entity)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: tint,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .clone();
        if let Some(mut m) = materials.get_mut(&handle) {
            m.base_color = tint;
        }
    }
    let existing = draws
        .iter()
        .map(|(_, g, _, _, _)| (g.source, g.actor))
        .collect::<HashMap<_, _>>();
    for (source, mesh, parent, skin, owner) in &sources {
        let Some(actor) = active
            .contains(&source)
            .then_some(source)
            .or_else(|| owner.map(|o| o.0))
            .or_else(|| parent.map(|p| p.parent()))
        else {
            continue;
        };
        if !active.contains(&actor)
            || !pass.materials.contains_key(&actor)
            || existing.contains_key(&source)
        {
            continue;
        }
        let mut entity = commands.spawn((
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(pass.materials[&actor].clone()),
            Transform::default(),
            Visibility::Inherited,
            RenderLayers::layer(3),
            NoFrustumCulling,
            bevy::light::NotShadowCaster,
            GlowDraw { source, actor },
        ));
        if let Some(skin) = skin {
            entity.insert(skin.clone());
        }
    }
    for (entity, g, mut mesh, mut visible, current_skin) in &mut draws {
        let Ok((_, source, _, skin, _)) = sources.get(g.source) else {
            commands.entity(entity).despawn();
            continue;
        };
        if mesh.0 != source.0 {
            mesh.0 = source.0.clone();
        }
        if let Some(skin) = skin {
            if current_skin.is_none_or(|old| {
                old.inverse_bindposes != skin.inverse_bindposes || old.joints != skin.joints
            }) {
                commands.entity(entity).insert(skin.clone());
            }
        }
        let alive = active.contains(&g.actor);
        visible.set_if_neq(if alive {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    // The mask is a full-resolution 4x MSAA camera pass. Skip it (and hide
    // its composite, so no stale mask shows) while nothing is glowing; it
    // renders again on the same frame a glow starts.
    let glowing = mask_needed(enabled, &active);
    // Profiled call count = frames that render the mask.
    let _mask = glowing.then(|| crate::performance::scope("glow::mask_frames"));
    for mut v in &mut overlays {
        v.set_if_neq(if glowing {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    // Keep rendering during load warmup so the first glow does not allocate
    // the full-resolution MSAA targets mid-game (a measured 47 ms hitch).
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let legacy =
        *LEGACY.get_or_init(|| std::env::args().any(|a| a == "--profile-legacy-glow-camera"));
    let render_mask = glowing || warming.is_some() || legacy;
    for (_, _, mut camera) in &mut masks {
        if camera.is_active != render_mask {
            camera.is_active = render_mask;
        }
    }
}
/// Whether a special holding a survivor shows the red attacker glow to the
/// player (see `sync`); the player's own attacker does not.
fn attacker_glows(
    class: crate::special_infected::SpecialClass,
    mode: crate::special_infected::ai::Mode,
) -> bool {
    use crate::special_infected::{SpecialClass, ai::{Mode, Victim}};
    match (class, mode) {
        (SpecialClass::Hunter, Mode::Pin(victim)) => victim != Victim::Player,
        (SpecialClass::Charger, Mode::Pin(victim)) => victim != Victim::Player,
        (SpecialClass::Charger, Mode::Charge { carried: Some(victim), .. }) => victim != Victim::Player,
        _ => false,
    }
}
fn mask_needed(enabled: bool, active: &std::collections::HashSet<Entity>) -> bool {
    enabled && !active.is_empty()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn infected_health_glow_matches_retail_boundaries_and_viewer_team() {
        let settings = crate::glow_settings::GlowSettings::default();
        let mut state = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        for (health, tier) in [
            (100., "high"),
            (40., "high"),
            (39., "med"),
            (25., "med"),
            (24., "low"),
            (1., "low"),
        ] {
            state.health = health;
            assert_eq!(
                survivor_view_color(&settings, &state, true, 0.),
                Some(settings.color(&format!("cl_glow_survivor_health_{tier}"))),
                "{health}"
            );
            assert_eq!(
                survivor_view_color(&settings, &state, false, 0.),
                Some(settings.color("cl_glow_survivor"))
            );
        }
        state.black_white = true;
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_crit"))
        );
        state.health = 2.;
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_low"))
        );
        state.health = 100.;
        state.incap = true;
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_crit"))
        );
    }
    #[test]
    fn infected_glow_counts_buffer_and_uses_native_pulse_and_accessibility_palettes() {
        let mut settings = crate::glow_settings::GlowSettings::default();
        let mut state = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        state.health = 24.;
        state.temporary = 16.;
        let high = settings.color("cl_glow_survivor_health_high");
        assert_eq!(survivor_view_color(&settings, &state, true, 0.), Some(high));
        state.temporary = 15.9;
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_med"))
        );
        state.temporary = 16.;
        settings.command("cl_glow_survivor_health_include_buffer 0");
        let low = settings.color("cl_glow_survivor_health_low").to_srgba();
        let peak = survivor_view_color(&settings, &state, true, std::f32::consts::FRAC_PI_2 / 10.)
            .unwrap()
            .to_srgba();
        assert!((peak.red - low.red * 1.3).abs() < 1e-5);
        assert!((peak.green - low.green * 1.3).abs() < 1e-5);
        settings.command("cl_glow_survivor_health_bleed_pulse 0");
        assert_eq!(
            survivor_view_color(&settings, &state, true, 99.),
            Some(settings.color("cl_glow_survivor_health_low"))
        );
        settings.command("cl_colorblind 1");
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_low"))
        );
        settings.command("cl_colorblind 2");
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_survivor_health_low_colorblind"))
        );
        state.vomited = 5.;
        state.incap = true;
        assert_eq!(
            survivor_view_color(&settings, &state, true, 0.),
            Some(settings.color("cl_glow_ability_colorblind"))
        );
        assert_eq!(
            survivor_view_color(&settings, &state, false, 0.),
            Some(settings.color("cl_glow_survivor_vomit"))
        );
        state.dead = true;
        assert_eq!(survivor_view_color(&settings, &state, true, 0.), None);
    }
    #[test]
    fn red_attacker_glow_follows_retail_classes_and_skips_the_viewers_attacker() {
        use crate::special_infected::{SpecialClass, ai::{Mode, Victim}};
        assert!(attacker_glows(SpecialClass::Hunter, Mode::Pin(Victim::Bot(0))));
        assert!(!attacker_glows(SpecialClass::Hunter, Mode::Pin(Victim::Player)));
        assert!(attacker_glows(SpecialClass::Charger, Mode::Pin(Victim::Bot(1))));
        assert!(attacker_glows(
            SpecialClass::Charger,
            Mode::Charge { direction: Vec3::X, carried: Some(Victim::Bot(2)) }
        ));
        assert!(!attacker_glows(SpecialClass::Charger, Mode::Charge { direction: Vec3::X, carried: None }));
        // The Jockey's flag has no strength and the Smoker sets none.
        assert!(!attacker_glows(SpecialClass::Jockey, Mode::Pin(Victim::Bot(0))));
        assert!(!attacker_glows(
            SpecialClass::Smoker,
            Mode::Reel { victim: Victim::Bot(0), choking: true }
        ));
    }
    #[test]
    fn glow_mask_pass_runs_only_while_something_glows() {
        let mut active = std::collections::HashSet::new();
        assert!(!mask_needed(true, &active));
        active.insert(Entity::PLACEHOLDER);
        assert!(mask_needed(true, &active));
        // Menus/cinematics disable glows even if actors are tracked.
        assert!(!mask_needed(false, &active));
    }
    #[test]
    fn pickup_glow_rejects_distant_items_before_tracing_and_preserves_selection() {
        let eye = Vec3::Y;
        let forward = Vec3::NEG_Z;
        let far = Vec3::new(0., 0.88, -1000.);
        assert_eq!(
            pickup_glow_score(eye, forward, far, 0.85, false, || panic!(
                "distant item traced"
            )),
            None
        );
        let mut collision = crate::collision::Collision::test_solid_floor();
        collision.test_add_box(Vec3::new(-0.4, 0., -1.8), Vec3::new(0.4, 2., -1.7));
        let mut old_calls = 0;
        let mut new_calls = 0;
        let origins = (0..600)
            .map(|i| Vec3::new((i % 7) as f32 * 0.1, 0.88, -(i as f32 * 0.1 + 0.1)))
            .collect::<Vec<_>>();
        let mut old = Vec::new();
        let mut new = Vec::new();
        for (i, origin) in origins.into_iter().enumerate() {
            let trace = || collision.sight_trace(eye, origin + Vec3::Y * 0.12).fraction;
            let a = pickup_glow_score(eye, forward, origin, 0.85, true, || {
                old_calls += 1;
                trace()
            });
            let b = pickup_glow_score(eye, forward, origin, 0.85, false, || {
                new_calls += 1;
                trace()
            });
            assert_eq!(a, b);
            if let Some(score) = a {
                old.push((i, score));
            }
            if let Some(score) = b {
                new.push((i, score));
            }
        }
        assert!(!new.is_empty());
        assert_eq!(
            old.iter().max_by(|a, b| a.1.total_cmp(&b.1)),
            new.iter().max_by(|a, b| a.1.total_cmp(&b.1))
        );
        assert!(
            old_calls > 500 && new_calls < 25,
            "trace counts {old_calls}/{new_calls}"
        );
        assert_eq!(
            pickup_glow_score(eye, forward, Vec3::new(0., 0.88, -2.), 0.85, false, || 0.5),
            None
        );
    }
    #[test]
    fn mask_transform_tracks_hidden_source_without_inheriting_hidden_visibility() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let expected = GlobalTransform::from(Transform::from_xyz(3., 2., 1.));
        let source = world.spawn((expected, Visibility::Hidden)).id();
        let mask = world
            .spawn((
                GlobalTransform::IDENTITY,
                Visibility::Inherited,
                GlowDraw {
                    source,
                    actor: source,
                },
            ))
            .id();
        world.run_system_once(sync_draw_transforms).unwrap();
        assert_eq!(*world.get::<GlobalTransform>(mask).unwrap(), expected);
        assert!(world.get::<ChildOf>(mask).is_none());
        assert_eq!(
            *world.get::<Visibility>(source).unwrap(),
            Visibility::Hidden
        );
        assert_eq!(
            *world.get::<Visibility>(mask).unwrap(),
            Visibility::Inherited
        );
    }
    #[test]
    fn removed_map_glow_pass_rebuilds_in_the_same_world() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(Assets::<Image>::default());
        world.insert_resource(Assets::<GlowComposite>::default());
        world.spawn((
            Camera::default(),
            Projection::default(),
            crate::viewer::GameCamera,
        ));
        world.spawn(crate::game::ViewCamera);
        world.run_system_once(setup).unwrap();
        assert!(world.contains_resource::<GlowPass>());
        let cameras = world
            .query_filtered::<Entity, With<GlowCamera>>()
            .iter(&world)
            .collect::<Vec<_>>();
        let overlays = world
            .query_filtered::<Entity, With<GlowOverlay>>()
            .iter(&world)
            .collect::<Vec<_>>();
        for entity in cameras.into_iter().chain(overlays) {
            world.despawn(entity);
        }
        reset(&mut world);
        world.run_system_once(setup).unwrap();
        assert_eq!(
            world
                .query_filtered::<Entity, With<GlowCamera>>()
                .iter(&world)
                .count(),
            1
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<GlowOverlay>>()
                .iter(&world)
                .count(),
            1
        );
    }
    #[test]
    fn campaign_glow_uses_native_colours_and_half_second_sight_fade() {
        let settings = crate::glow_settings::GlowSettings::default();
        let mut state = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        assert_eq!(
            survivor_color(&settings, &state),
            Some(Color::srgb(0.3, 0.4, 1.))
        );
        state.incap = true;
        assert_eq!(
            survivor_color(&settings, &state),
            Some(Color::srgb(1., 0.4, 0.))
        );
        state.dead = true;
        assert_eq!(survivor_color(&settings, &state), None);
        assert_eq!(fade(0., true, 0.25), 0.5);
        assert_eq!(fade(0.5, true, 0.25), 1.);
        assert_eq!(fade(1., false, 0.5), 0.);
    }
    #[test]
    fn ordinary_damage_never_changes_teammate_glow_and_distress_uses_native_controls() {
        let mut settings = crate::glow_settings::GlowSettings::default();
        let mut state = crate::game_state::Match::new(&crate::game_state::tests_weapon());
        let normal = settings.color("cl_glow_survivor");
        for health in [99., 80., 40., 1.] {
            state.health = health;
            for flash in [1., 0.3, 0.] {
                state.damage_flash = flash;
                assert_eq!(survivor_color(&settings, &state), Some(normal));
            }
        }
        state.pinned = true;
        assert_eq!(
            survivor_color(&settings, &state),
            Some(settings.color("cl_glow_survivor_hurt"))
        );
        state.pinned = false;
        assert_eq!(survivor_color(&settings, &state), Some(normal));
        // Distinguish the configurable bile colour from the hurt colour.
        settings.command("cl_glow_survivor_vomit_g 0.8");
        state.incap = true;
        state.vomited = 5.;
        assert_eq!(
            survivor_color(&settings, &state),
            Some(settings.color("cl_glow_survivor_vomit"))
        );
        state.vomited = 0.;
        assert_eq!(
            survivor_color(&settings, &state),
            Some(settings.color("cl_glow_survivor_hurt"))
        );
        state.dead = true;
        assert_eq!(survivor_color(&settings, &state), None);
    }
}

fn map_highlights(logic: &crate::l4d2r_cinematic::MapLogic, eye: Vec3) -> std::collections::HashSet<usize> {
    let mut highlighted = std::collections::HashSet::new();
    {
        for (id, e) in logic.world.ents.iter().enumerate().filter(|(_, e)| e.alive) {
            if e.me.kv_i32("glowstate").unwrap_or(0) > 0 {
                let range = e.me.kv_f32("glowrange").unwrap_or(0.) * 0.0254;
                if range <= 0.
                    || logic
                        .bounds(id)
                        .is_some_and(|(lo, hi)| eye.distance(eye.clamp(lo, hi)) <= range)
                {
                    highlighted.insert(id);
                }
            }
            // Saferoom doors take the item use-glow while a survivor is in
            // use range (96 units), like nearby buttons.
            if e.me.classname == "prop_door_rotating_checkpoint"
                && matches!(e.kind, l4d2r_game_core::entities::Kind::Door(_))
                && logic
                    .bounds(id)
                    .is_some_and(|(lo, hi)| eye.distance(eye.clamp(lo, hi)) < 2.44)
            {
                highlighted.insert(id);
            }
            if matches!(
                e.kind,
                l4d2r_game_core::entities::Kind::Button {
                    locked: false,
                    pressed: false,
                    ..
                }
            ) && logic
                .bounds(id)
                .is_some_and(|(lo, hi)| eye.distance(eye.clamp(lo, hi)) < 2.44)
            {
                highlighted.insert(id);
                // Source buttons name the visible lock/panel in their glow key.
                if let Some(target) = e.me.kv("glow") {
                    highlighted.extend(logic.world.find(target).iter().copied());
                }
                for c in &e.conns {
                    if c.input.eq_ignore_ascii_case("skin")
                        || c.input.eq_ignore_ascii_case("stopglowing")
                    {
                        highlighted.extend(logic.world.find(&c.target).iter().copied());
                    }
                }
            }
        }
    }
    highlighted
}

#[cfg(test)]
mod whitaker_glow_tests {
    use super::*;
    #[test]
    fn installed_whitaker_button_highlights_authored_lock_only_nearby() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut logic=crate::l4d2r_cinematic::MapLogic::load(root,&root.join("left4dead2/maps/c1m2_streets.bsp"),true).unwrap();
        let button=logic.world.find("gunshop_door_button")[0];
        let lock=logic.world.find("gunshop_door_lock")[0];
        let (lo,hi)=logic.bounds(button).unwrap();
        let eye=(lo+hi)*0.5;
        assert!(map_highlights(&logic,eye).contains(&lock));
        assert!(!map_highlights(&logic,eye+Vec3::splat(10.)).contains(&lock));
        logic.world.use_ent(button,None); logic.world.tick(0.1);
        assert!(!map_highlights(&logic,eye).contains(&lock),"Used button must not stay highlighted");
    }
}
