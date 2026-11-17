//! iPhone UI gets its own simple SDR pass, independent of the HDR weapon view.
use bevy::{
    camera::{CameraOutputMode, ClearColorConfig, RenderTarget, visibility::RenderLayers},
    prelude::*,
};

#[derive(Component)]
pub(crate) struct UiPresentationCamera {
    source: Entity,
}

pub(crate) fn install(app: &mut App) {
    app.add_systems(Update, crate::game_menu::touch_carousel.before(crate::game_menu::update));
    if std::env::args().any(|a| a == "--capture-disable-viewmodel-camera") {
        app.add_systems(
            PostUpdate,
            disable_foreground.before(bevy::camera::CameraUpdateSystems),
        );
    }
    if !cfg!(target_os = "ios") && !std::env::args().any(|a| a == "--ui-safe-pass") {
        return;
    }
    app.add_systems(
        PostUpdate,
        route_ui
            .before(bevy::camera::CameraUpdateSystems)
            .before(bevy::ui::UiSystems::Prepare),
    );
}

fn route_ui(world: &mut World) {
    let foreground = world
        .query_filtered::<(Entity, Option<&RenderTarget>), With<crate::game::ViewCamera>>()
        .iter(world)
        .next()
        .map(|(e, t)| (e, t.cloned()));
    let Some((source, target)) = foreground else {
        return;
    };
    let existing = world
        .query::<(Entity, &UiPresentationCamera)>()
        .iter(world)
        .map(|(e, c)| (e, c.source))
        .collect::<Vec<_>>();
    let camera = if let Some((e, _)) = existing.iter().find(|(_, s)| *s == source) {
        *e
    } else {
        let mut e = world.spawn((
            Camera2d,
            Msaa::Off,
            IsDefaultUiCamera,
            Camera {
                order: 10, // Above world/weapon passes, below the loading poster (20).
                clear_color: ClearColorConfig::Custom(Color::NONE),
                output_mode: CameraOutputMode::Write {
                    blend_state: Some(
                        bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                    ),
                    clear_color: ClearColorConfig::None,
                },
                ..default()
            },
            RenderLayers::none(),
            UiPresentationCamera { source },
            Name::new("iPhone SDR menu/HUD"),
        ));
        if let Some(target) = target.clone() {
            e.insert(target);
        }
        let id = e.id();
        eprintln!("UI presentation: independent SDR camera {id:?}, foreground {source:?}");
        id
    };
    let stale: Vec<_> = existing
        .iter()
        .filter(|(e, _)| *e != camera)
        .map(|(e, _)| *e)
        .collect();
    for mut ui in world.query::<&mut UiTargetCamera>().iter_mut(world) {
        if ui.0 == source || stale.contains(&ui.0) {
            ui.0 = camera;
        }
    }
    for old in stale {
        world.despawn(old);
    }
    // Reassert only the UI pass, never turn a disabled weapon camera back on.
    if !world.get::<Camera>(camera).unwrap().is_active {
        world.get_mut::<Camera>(camera).unwrap().is_active = true;
    }
}

fn disable_foreground(mut views: Query<&mut Camera, With<crate::game::ViewCamera>>) {
    for mut view in &mut views {
        view.is_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hud_survives_a_disabled_hdr_foreground_and_loading_keeps_its_own_target() {
        let mut world = World::new();
        let view = world
            .spawn((
                Camera3d::default(),
                bevy::camera::Hdr,
                Camera {
                    is_active: false,
                    ..default()
                },
                crate::game::ViewCamera,
            ))
            .id();
        let loading = world.spawn(Camera2d).id();
        let hud = world.spawn(UiTargetCamera(view)).id();
        let poster = world.spawn(UiTargetCamera(loading)).id();
        route_ui(&mut world);
        let ui = world.get::<UiTargetCamera>(hud).unwrap().0;
        assert_ne!(ui, view);
        assert!(world.get::<Camera>(ui).unwrap().is_active);
        assert!(!world.get::<Camera>(view).unwrap().is_active);
        assert!(world.get::<bevy::camera::Hdr>(ui).is_none());
        assert!(
            world
                .get::<bevy::post_process::auto_exposure::AutoExposure>(ui)
                .is_none()
        );
        assert_eq!(*world.get::<Msaa>(ui).unwrap(), Msaa::Off);
        assert_eq!(world.get::<UiTargetCamera>(poster).unwrap().0, loading);
        route_ui(&mut world);
        assert_eq!(
            world.query::<&UiPresentationCamera>().iter(&world).count(),
            1
        );
    }
    #[test]
    fn map_camera_replacement_rebinds_ui_without_leaving_extra_cameras() {
        let mut world = World::new();
        let old = world
            .spawn((Camera3d::default(), crate::game::ViewCamera))
            .id();
        let root = world.spawn(UiTargetCamera(old)).id();
        route_ui(&mut world);
        let old_ui = world.get::<UiTargetCamera>(root).unwrap().0;
        world.despawn(old);
        let new = world
            .spawn((Camera3d::default(), crate::game::ViewCamera))
            .id();
        route_ui(&mut world);
        let ui = world.get::<UiTargetCamera>(root).unwrap().0;
        assert_ne!(old_ui, ui);
        assert_eq!(world.get::<UiPresentationCamera>(ui).unwrap().source, new);
        assert!(world.get_entity(old_ui).is_err());
        assert_eq!(
            world.query::<&UiPresentationCamera>().iter(&world).count(),
            1
        );
    }
}
