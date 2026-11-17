//! Infected-player HUD geometry from installed Valve resources. Hunter
//! placement was checked against retail captures; other classes read their
//! own <class>health.res, tip_<class> portrait and pz_charge_<class> icon.
use crate::{
    game_state::Match,
    special_infected::{Hunter, MARKER_LAYER, SpecialClass},
};
use bevy::{
    camera::visibility::RenderLayers, prelude::*, render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct Progress {
    #[texture(0)]
    #[sampler(1)]
    image: Handle<Image>,
    #[uniform(2)]
    controls: Vec4,
    #[uniform(3)]
    tint: Vec4,
}
impl UiMaterial for Progress {
    fn fragment_shader() -> ShaderRef {
        "shaders/infected_hud.wgsl".into()
    }
}
#[derive(Component, Clone, Copy)]
enum Root {
    Live,
    Ghost,
    Aim,
}
#[derive(Component)]
struct HealthFill;
#[derive(Component)]
struct Number;
#[derive(Component)]
struct Crouch;
#[derive(Component)]
struct AbilityIcon;
#[derive(Component)]
struct Crosshair;
#[derive(Component)]
struct GhostReason;
#[derive(Component)]
struct Meter;
#[derive(Resource, Clone)]
struct Layout {
    health_anchor: f32,
    health_size: Vec2,
    health: [f32; 4],
    number: [f32; 4],
    backdrop: [f32; 4],
    crouch: [f32; 4],
    ability_anchor: Vec2,
    ability_size: Vec2,
    ability_icon: [f32; 4],
    crosshair_radius: f32,
    portrait_rect: Rect,
    backdrop_image: String,
}
fn rect(block: &crate::kv::Block) -> [f32; 4] {
    ["xpos", "ypos", "wide", "tall"].map(|k| block.values[k].parse().unwrap())
}
fn load_layout(assets: &mut crate::source_assets::SourceAssets, class: SpecialClass) -> Layout {
    let read = |assets: &mut crate::source_assets::SourceAssets, path: &str| {
        let text = String::from_utf8_lossy(&assets.read(path).expect("Installed HUD resource"))
            .into_owned();
        crate::kv::blocks(&text)
    };
    let hud = read(assets, "scripts/hudlayout.res");
    let hud = hud.children.values().next().unwrap();
    let h = &hud.children["hudzombiehealth"];
    let a = &hud.children["chudabilitytimer"];
    let anchor =
        |b: &crate::kv::Block, k: &str| b.values[k].trim_start_matches('r').parse::<f32>().unwrap();
    let hp = read(
        assets,
        &format!("resource/ui/hud/{}health.res", class.name()),
    );
    let hp = hp.children.values().next().unwrap();
    let ability = read(assets, "resource/ui/hud/abilitytimerhud.res");
    let ability = ability.children.values().next().unwrap();
    let icons = read(assets, "scripts/mod_textures.txt");
    let icon = &icons.children["sprites/640_hud"].children["texturedata"].children
        [&format!("tip_{}", class.name())];
    let n = |k: &str| icon.values[k].parse::<f32>().unwrap();
    let origin = Vec2::new(n("x"), n("y"));
    Layout {
        portrait_rect: Rect::from_corners(origin, origin + Vec2::new(n("width"), n("height"))),
        health_anchor: anchor(h, "xpos"),
        health_size: Vec2::new(anchor(h, "wide"), anchor(h, "tall")),
        health: rect(&hp.children["health"]),
        number: rect(&hp.children["healthnumber"]),
        backdrop: rect(&hp.children["backgroundimage"]),
        backdrop_image: hp.children["backgroundimage"]
            .values
            .get("image")
            .map_or("hud/pz_healthbar_250".into(), |i| i.to_ascii_lowercase()),
        crouch: rect(&hp.children["duckingicon"]),
        ability_anchor: Vec2::new(anchor(a, "xpos"), anchor(a, "ypos")),
        ability_size: Vec2::new(anchor(a, "wide"), anchor(a, "tall")),
        ability_icon: rect(&ability.children["abilityimage"]),
        crosshair_radius: anchor(&hud.children["hudcrosshair"], "ability_size"),
    }
}
fn node(r: [f32; 4]) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: vh(r[0] / 4.8),
        top: vh(r[1] / 4.8),
        width: vh(r[2] / 4.8),
        height: vh(r[3] / 4.8),
        ..default()
    }
}
fn anchored(anchor: Vec2, size: Vec2) -> Node {
    Node {
        position_type: PositionType::Absolute,
        right: vh((anchor.x - size.x) / 4.8),
        bottom: vh((anchor.y - size.y) / 4.8),
        width: vh(size.x / 4.8),
        height: vh(size.y / 4.8),
        ..default()
    }
}
fn image(
    commands: &mut Commands,
    parent: Entity,
    texture: Handle<Image>,
    r: [f32; 4],
    z: i32,
) -> Entity {
    commands
        .spawn((
            ImageNode::new(texture).with_mode(bevy::ui::widget::NodeImageMode::Stretch),
            node(r),
            ZIndex(z),
            ChildOf(parent),
        ))
        .id()
}
fn readiness(h: &Hunter, s: &Match) -> f32 {
    if h.ghost || s.dead {
        0.
    } else if let Some(progress) = h.ability_progress {
        progress.clamp(0., 1.)
    } else if h.pin.is_some() {
        0.
    } else if h.cooldown > 0. {
        (1. - h.cooldown).clamp(0., 1.)
    } else {
        h.charge.clamp(0., 1.)
    }
}
pub fn install(app: &mut App) {
    app.add_plugins(UiMaterialPlugin::<Progress>::default())
        .add_systems(Update, update.before(crate::game::controls));
}
fn update(
    mut commands: Commands,
    state: Res<Match>,
    hunter: Res<Hunter>,
    source: Res<crate::game_ui::HudSource>,
    frontend: Res<crate::game_menu::Frontend>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
    mut assets: (
        ResMut<Assets<Image>>,
        ResMut<Assets<Font>>,
        ResMut<Assets<Progress>>,
    ),
    camera: Query<Entity, With<crate::game::ViewCamera>>,
    mut layers: Query<&mut RenderLayers, With<crate::viewer::GameCamera>>,
    mut roots: Query<
        (&Root, &mut Node),
        (
            Without<HealthFill>,
            Without<Crouch>,
            Without<crate::game_ui::SurvivorHud>,
        ),
    >,
    mut survivors: Query<
        &mut Node,
        (
            With<crate::game_ui::SurvivorHud>,
            Without<Root>,
            Without<HealthFill>,
            Without<Crouch>,
        ),
    >,
    mut widgets: Query<
        (Option<&HealthFill>, Option<&Crouch>, &mut Node),
        (Without<Root>, Without<crate::game_ui::SurvivorHud>),
    >,
    mut pictures: Query<(Option<&AbilityIcon>, Option<&Crosshair>, &mut ImageNode)>,
    mut texts: Query<(
        Option<&Number>,
        Option<&GhostReason>,
        &mut Text,
        &mut TextColor,
    )>,
    mut rebuild: (
        Query<&MaterialNode<Progress>, With<Meter>>,
        Query<Entity, With<Root>>,
        Local<Option<SpecialClass>>,
    ),
    spawn_state: (
        Res<crate::collision::Collision>,
        Single<&crate::game::Body>,
        Res<crate::game_state::Squad>,
    ),
) {
    let (mut images, mut fonts, mut materials) = assets;
    let (meters, all_roots, built) = (&rebuild.0, &rebuild.1, &mut *rebuild.2);
    let visible = state.infected
        && frontend.page == crate::game_menu::Page::Game
        && !cinematic.as_ref().is_some_and(|c| c.active);
    for mut layer in &mut layers {
        // Keep the model-shadow light layer on the world camera.
        let shadows = crate::model_shadows::LAYER;
        layer.set_if_neq(if visible {
            RenderLayers::from_layers(&[0, 1, shadows, MARKER_LAYER])
        } else {
            RenderLayers::from_layers(&[0, 1, shadows])
        });
    }
    for mut n in &mut survivors {
        let d = if state.infected {
            Display::None
        } else {
            Display::Flex
        };
        if n.display != d {
            n.display = d;
        }
    }
    for (root, mut n) in &mut roots {
        let show = visible
            && match root {
                Root::Live => !hunter.ghost && !state.dead,
                Root::Ghost => hunter.ghost || state.dead,
                Root::Aim => !state.dead,
            };
        let d = if show { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
    let fraction = (state.health / state.special_class.health()).clamp(0., 1.);
    for (fill, duck, mut n) in &mut widgets {
        if fill.is_some() {
            let width = percent(fraction * 100.);
            if n.width != width {
                n.width = width;
            }
        }
        if duck.is_some() {
            let d = if spawn_state.1.crouched {
                Display::Flex
            } else {
                Display::None
            };
            if n.display != d {
                n.display = d;
            }
        }
    }
    let progress = readiness(&hunter, &state);
    let tint = if progress >= 0.999 {
        Color::WHITE
    } else {
        Color::srgb(127. / 255., 127. / 255., 127. / 255.)
    };
    for (icon, cross, mut img) in &mut pictures {
        if icon.is_some() || cross.is_some() {
            img.reborrow()
                .map_unchanged(|i| &mut i.color)
                .set_if_neq(tint);
        }
    }
    for handle in meters {
        if materials
            .get(&handle.0)
            .is_some_and(|m| m.controls.x != progress)
        {
            if let Some(mut material) = materials.get_mut(&handle.0) {
                material.controls.x = progress;
            }
        }
    }
    let ready = visible
        && hunter.ghost
        && crate::special_infected::can_spawn(&spawn_state.0, spawn_state.1.feet, &spawn_state.2);
    for (number, ghost, mut text, mut color) in &mut texts {
        let value = if number.is_some() {
            format!("{:.0}", state.health)
        } else if ghost.is_some() {
            if state.dead {
                "You are dead. Press R to return to ghost mode.".into()
            } else if ready {
                "Left click to spawn".into()
            } else if !spawn_state.0.infected_spawn_allowed(spawn_state.1.feet) {
                "You can't spawn inside a safe room".into()
            } else {
                "You can't spawn here".into()
            }
        } else {
            continue;
        };
        if text.0 != value {
            text.0 = value;
        }
        if ghost.is_some() {
            color.set_if_neq(TextColor(if ready && !state.dead {
                Color::WHITE
            } else {
                Color::srgb(1., 0., 0.)
            }));
        }
    }
    if visible && !roots.is_empty() && *built != Some(state.special_class) {
        // Role changed: rebuild the HUD from the new class resources.
        for entity in all_roots {
            commands.entity(entity).despawn();
        }
        *built = None;
        return;
    }
    if !visible || !roots.is_empty() {
        return;
    }
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let Ok(mut native) = crate::source_assets::SourceAssets::new(&source.0, vec![]) else {
        return;
    };
    let class = state.special_class;
    let layout = load_layout(&mut native, class);
    *built = Some(class);
    let load = |a: &mut crate::source_assets::SourceAssets, i: &mut Assets<Image>, name: &str| {
        crate::game_menu::image(a, i, &format!("materials/vgui/{name}.vtf"))
    };
    let backdrop = load(&mut native, &mut images, &layout.backdrop_image);
    let icon = load(
        &mut native,
        &mut images,
        &format!("hud/{}", class.ability_icon()),
    );
    let background = load(&mut native, &mut images, "hud/pz_charge_bg");
    let ring = load(&mut native, &mut images, "hud/pz_charge_meter");
    let aim = load(&mut native, &mut images, "hud/pz_charge_crosshair");
    let duck = load(&mut native, &mut images, "hud/crouch_infected");
    let portrait = load(&mut native, &mut images, "tipgraphic");
    let font = native
        .read("resource/stubble-bold.vfont")
        .and_then(|b| crate::vfont::decode(&b))
        .ok()
        .map(|b| crate::game_ui::shared_font(&mut *fonts, b))
        .unwrap_or_default();
    let text_font = |units: f32| TextFont {
        font: bevy::text::FontSource::Handle(font.clone()),
        font_size: FontSize::Vh(units / 4.8),
        ..default()
    };
    let hp = commands
        .spawn((
            anchored(
                Vec2::new(layout.health_anchor, layout.health_size.y),
                layout.health_size,
            ),
            UiTargetCamera(cam),
            Root::Live,
        ))
        .id();
    image(&mut commands, hp, backdrop, layout.backdrop, -2);
    commands.spawn((
        Text::new(format!("{:.0}", state.health)),
        text_font(14.5),
        TextColor(Color::WHITE),
        TextLayout::new(Justify::Right, LineBreak::NoWrap),
        Node {
            left: Val::Auto,
            right: vh((layout.health_size.x - layout.number[0] - layout.number[2]) / 4.8),
            width: Val::Auto,
            max_width: vh(layout.number[2] / 4.8),
            ..node(layout.number)
        },
        Number,
        ChildOf(hp),
        ZIndex(2),
    ));
    let mut frame = node(layout.health);
    frame.border = UiRect::all(vh(1. / 4.8));
    frame.overflow = Overflow::clip();
    let fill = commands
        .spawn((
            frame,
            BackgroundColor(Color::srgb(0., 0.1, 0.02)),
            BorderColor::all(Color::srgb(0.08, 0.35, 0.12)),
            ChildOf(hp),
            ZIndex(1),
        ))
        .id();
    let gradient = materials.add(Progress {
        image: ring.clone(),
        controls: Vec4::new(1., 1., 0., 0.),
        tint: Vec4::ONE,
    });
    commands.spawn((
        MaterialNode(gradient),
        Node {
            width: percent(fraction * 100.),
            height: percent(100.),
            ..default()
        },
        HealthFill,
        ChildOf(fill),
    ));
    let widget = image(&mut commands, hp, duck, layout.crouch, 2);
    commands.entity(widget).insert(Crouch);
    let ability = commands
        .spawn((
            anchored(layout.ability_anchor, layout.ability_size),
            UiTargetCamera(cam),
            Root::Live,
        ))
        .id();
    image(&mut commands, ability, background, [0., 0., 80., 80.], 0);
    let widget = image(&mut commands, ability, icon, layout.ability_icon, 1);
    commands.entity(widget).insert(AbilityIcon);
    let meter = materials.add(Progress {
        image: ring,
        controls: Vec4::new(progress, 0., 0., 0.),
        tint: Vec4::new(1., 0., 0., 1.),
    });
    commands.spawn((
        MaterialNode(meter),
        node(layout.ability_icon),
        Meter,
        ZIndex(2),
        ChildOf(ability),
    ));
    let center = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: percent(50.),
                width: px(0.),
                height: px(0.),
                ..default()
            },
            UiTargetCamera(cam),
            Root::Aim,
        ))
        .id();
    let diameter = layout.crosshair_radius * 2.;
    let widget = image(
        &mut commands,
        center,
        aim.clone(),
        [-diameter / 2., -diameter / 2., diameter, diameter],
        0,
    );
    commands.entity(widget).insert(Crosshair);
    // Retail capture: the charging/ready outer ring surrounds the base circle.
    let outer = diameter + 9.;
    let ring = materials.add(Progress {
        image: aim,
        controls: Vec4::new(progress, 0., 0., 0.),
        tint: Vec4::ONE,
    });
    commands.spawn((
        MaterialNode(ring),
        node([-outer / 2., -outer / 2., outer, outer]),
        Meter,
        ZIndex(1),
        ChildOf(center),
    ));
    let ghost = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50.),
                top: vh(277.5 / 4.8),
                margin: UiRect::left(vh(-160. / 4.8)),
                width: vh(320. / 4.8),
                height: vh(80. / 4.8),
                border: UiRect::all(vh(0.7 / 4.8)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.85)),
            BorderColor::all(Color::srgb(0.3, 0., 0.)),
            UiTargetCamera(cam),
            Root::Ghost,
        ))
        .id();
    let photo = image(
        &mut commands,
        ghost,
        portrait.clone(),
        // Fit the square artwork inside the frame's two 0.7-unit borders.
        [0., 0., 78.6, 78.6],
        1,
    );
    commands.entity(photo).insert(ImageNode {
        rect: Some(layout.portrait_rect),
        ..ImageNode::new(portrait).with_mode(bevy::ui::widget::NodeImageMode::Stretch)
    });
    commands.spawn((
        Text::new(class.label()),
        text_font(18.),
        TextColor(Color::WHITE),
        node([85., 5., 220., 20.]),
        ChildOf(ghost),
    ));
    commands.spawn((
        Text::new("Choose Spawn Location"),
        text_font(12.),
        TextColor(Color::srgb(0.8, 0.8, 0.8)),
        node([85., 28., 220., 16.]),
        ChildOf(ghost),
    ));
    commands.spawn((
        Text::new("You can't spawn here"),
        text_font(12.),
        TextColor(Color::srgb(1., 0., 0.)),
        node([85., 44., 220., 30.]),
        GhostReason,
        ChildOf(ghost),
    ));
    commands.insert_resource(layout);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_hunter_hud_uses_valve_geometry_and_resource_radius() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = crate::source_assets::SourceAssets::new(root, vec![]).unwrap();
        let l = load_layout(&mut assets, SpecialClass::Hunter);
        assert_eq!(l.backdrop_image, "hud/pz_healthbar_250");
        for class in SpecialClass::ALL {
            load_layout(&mut assets, class);
        }
        assert_eq!(l.health, [252., 69., 132., 13.]);
        assert_eq!(l.number, [335., 49., 50., 20.]);
        assert_eq!(l.backdrop, [250., 0., 200., 100.]);
        assert_eq!(l.ability_anchor, Vec2::new(72., 120.));
        assert_eq!(l.ability_icon, [10., 10., 60., 60.]);
        assert_eq!(l.crosshair_radius, 17.);
        assert_eq!(
            l.portrait_rect,
            Rect::from_corners(Vec2::new(832., 448.), Vec2::new(1024., 640.))
        );
        for scale in [1., 1.5, 2.25] {
            assert!(l.health[0] * scale + l.health[2] * scale <= l.health_size.x * scale);
        }
    }
    #[test]
    fn hunter_charge_suppression_and_ready_states_are_distinct() {
        let mut s = Match::new(&crate::game_state::tests_weapon());
        let mut h = Hunter::default();
        assert_eq!(readiness(&h, &s), 0.);
        h.charge = 0.5;
        assert_eq!(readiness(&h, &s), 0.5);
        h.charge = 1.;
        assert_eq!(readiness(&h, &s), 1.);
        h.ghost = true;
        assert_eq!(readiness(&h, &s), 0.);
        h.ghost = false;
        h.pin = Some(0);
        assert_eq!(readiness(&h, &s), 0.);
        h.pin = None;
        s.dead = true;
        assert_eq!(readiness(&h, &s), 0.);
    }
}
