//! Teammate labels on desktop and mobile. TeamName shares the HUD's live player/bot naming.
use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component)]
struct OverheadName(Entity);
#[derive(Resource)]
struct NameFont(Handle<Font>);

pub fn install(app: &mut App) {
    app.add_systems(Startup, load_font)
        .add_systems(PostUpdate, update
            .after(bevy::transform::TransformSystems::Propagate)
            .after(bevy::camera::CameraUpdateSystems)
            .before(bevy::ui::UiSystems::Prepare));
}

fn load_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    commands.insert_resource(NameFont(crate::game_chat::chat_font(&mut fonts)));
}

fn screen_position(camera: &Camera, transform: &GlobalTransform, head: Vec3) -> Option<Vec2> {
    let point = camera.world_to_viewport(transform, head).ok()?;
    let size = camera.logical_viewport_size()?;
    (point.is_finite() && point.x >= 0. && point.y >= 0. && point.x <= size.x && point.y <= size.y)
        .then_some(point)
}

fn update(world: &mut World) {
    let camera = world.query_filtered::<(Entity, &Camera, &GlobalTransform), With<crate::viewer::GameCamera>>()
        .iter(world).next().map(|(e, c, t)| (e, c.clone(), *t));
    let Some((_, camera, view)) = camera else { return };
    let target = world.query_filtered::<Entity, With<crate::ui_presentation::UiPresentationCamera>>()
        .iter(world).next().or_else(|| world.query_filtered::<Entity, With<crate::game::ViewCamera>>()
            .iter(world).next());
    let Some(target) = target else { return };
    let ui_scale = world.get_resource::<UiScale>().map_or(1., |s| s.0.max(0.01));
    let active = world.get_resource::<crate::game_menu::Frontend>().is_some_and(|f| f.playing())
        && !world.contains_resource::<crate::render_warmup::Warmup>()
        && !world.get_resource::<crate::game_state::Match>().is_some_and(|s| s.infected)
        && !world.get_resource::<crate::l4d2r_cinematic::MapLogic>().is_some_and(|l| l.active);
    let actors = world.query::<(Entity, &crate::game::CompanionActor, &GlobalTransform)>()
        .iter(world).map(|(e, a, t)| (e, a.0, *t)).collect::<Vec<_>>();
    let alive: HashSet<_> = actors.iter().map(|a| a.0).collect();
    let labels = world.query::<(Entity, &OverheadName)>().iter(world)
        .map(|(e, n)| (e, n.0)).collect::<Vec<_>>();
    for (label, actor) in &labels {
        if !alive.contains(actor) { world.despawn(*label); }
    }
    let Some(font) = world.get_resource::<NameFont>().map(|f| f.0.clone()) else { return };
    for (actor, index, transform) in actors {
        let Some(member) = world.get_resource::<crate::game_state::Squad>().and_then(|s| s.0.get(index)) else { continue };
        let character = crate::game_ui::kill_feed::display_name(&member.model);
        let name = crate::net_play::player_of(character).unwrap_or_else(|| character.to_string());
        let eyes = world.get_resource::<crate::l4d2r_animation::AnimationLibrary>()
            .zip(world.get_resource::<crate::l4d2r_animation::PlaybackState>())
            .and_then(|(library, playback)| library.gore_attachment(&member.model, actor, playback, "eyes"))
            .unwrap_or(Vec3::Y * if member.state.incap { 0.6 } else { crate::collision::EYE });
        let eye = transform.transform_point(eyes);
        let head = eye + Vec3::Y * 0.18;
        let visible = active && !member.state.dead
            && world.get_resource::<crate::collision::Collision>()
                .is_none_or(|c| { let ray = c.sight_trace(view.translation(), eye); !ray.start_solid && ray.fraction >= 0.99 });
        let position = visible.then(|| screen_position(&camera, &view, head)).flatten().map(|p| p / ui_scale);
        let label = labels.iter().find(|(_, e)| *e == actor).map(|(e, _)| *e).unwrap_or_else(|| {
            world.spawn((
                OverheadName(actor),
                Node { position_type: PositionType::Absolute, width: px(320.), height: px(32.),
                    justify_content: JustifyContent::Center, align_items: AlignItems::FlexEnd, ..default() },
                UiTargetCamera(target), GlobalZIndex(15), Pickable::IGNORE,
                Name::new("Survivor overhead name"),
            )).with_children(|root| {
                root.spawn((
                    crate::game_ui::TeamName(index), Text::new(name),
                    TextFont { font: bevy::text::FontSource::Handle(font.clone()), font_size: FontSize::Vh(2.), ..default() },
                    TextColor(Color::srgb(0.38, 0.68, 1.)),
                    TextShadow { offset: Vec2::ONE, color: Color::BLACK.with_alpha(0.9) },
                    TextLayout::no_wrap(), Pickable::IGNORE,
                ));
            }).id()
        });
        world.entity_mut(label).insert((UiTargetCamera(target),
            if position.is_some() { Visibility::Inherited } else { Visibility::Hidden }));
        if let Some(point) = position {
            let mut node = world.get_mut::<Node>(label).unwrap();
            node.left = px(point.x - 160.);
            node.top = px(point.y - 32.);
        }
    }
}

