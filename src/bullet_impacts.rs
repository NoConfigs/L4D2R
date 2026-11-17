//! Bounded native bullet-hole and infected-blood effects.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use l4d2r_game_core::{
    entities::{Ent, EntityWorld, Kind},
    mapents::MapEnt,
};
const BLOOD_FX: [&str; 2] = [
    "blood_impact_infected_01",
    "blood_impact_infected_01_shotgun",
];
const BLOOD_POOL: usize = 8;
#[derive(Clone, Copy)]
pub struct BloodHit {
    pub position: Vec3,
    pub direction: Vec3,
    pub shotgun: bool,
}
#[derive(Resource, Default)]
pub struct Impacts {
    pub hits: Vec<(Vec3, Vec3)>,
    pub blood: Vec<BloodHit>,
    pub gore: Vec<crate::infected_gore::Hit>,
    live: std::collections::VecDeque<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    blood_material: Option<Handle<StandardMaterial>>,
    blood_clock: Vec<(usize, f32)>,
    cursor: usize,
    pub bullet_marks: u64,
    pub blood_bursts: u64,
    pub blood_marks: u64,
    /// Multiplayer host: blood and gore hits as they were used, for guests
    /// (net_infected sends them).
    pub net_blood: Vec<BloodHit>,
    pub net_gore: Vec<crate::infected_gore::Hit>,
}
pub fn prepare(world: &mut EntityWorld) {
    for slot in 0..BLOOD_POOL {
        for (kind, fx) in BLOOD_FX.iter().enumerate() {
            let name = format!("__rewrite_blood_{slot}_{kind}");
            if !world.find(&name).is_empty() {
                continue;
            }
            world.add_runtime_entity(Ent {
                me: MapEnt {
                    index: world.ents.len(),
                    classname: "info_particle_system".into(),
                    targetname: name.clone(),
                    origin: l4d2r_math::Vec3::ZERO,
                    angles: l4d2r_math::Vec3::ZERO,
                    mdl: None,
                    brush: None,
                    ent: l4d2_formats::bsp::Entity(vec![
                        ("classname".into(), "info_particle_system".into()),
                        ("targetname".into(), name),
                        ("effect_name".into(), fx.to_string()),
                    ]),
                },
                conns: vec![],
                kind: Kind::Particle {
                    active: false,
                    started: 0.,
                },
                alive: true,
                templated: false,
                parent: None,
            });
        }
    }
}
pub fn setup(
    mut commands: Commands,
    source: Res<crate::game_ui::HudSource>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut data = Impacts::default();
    if let Ok(mut assets) = crate::source_assets::SourceAssets::new(&source.0, vec![]) {
        let mut load = |path: &str| -> Option<Handle<StandardMaterial>> {
            let t = crate::vtf::decode(&assets.read(path).ok()?, 512).ok()?;
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
            Some(materials.add(StandardMaterial {
                base_color_texture: Some(image),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                ..default()
            }))
        };
        data.material = load("materials/decals/concrete/shot1.vtf");
        data.blood_material = load("materials/decals/flesh/blood1.vtf");
    }
    let mesh = meshes.add(Plane3d::default().mesh().size(1., 1.));
    // Let map render warmup prepare decal variants before the first shot.
    // These zero-sized draws stay hidden during normal play.
    for material in [data.material.as_ref(), data.blood_material.as_ref()]
        .into_iter()
        .flatten()
    {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_scale(Vec3::ZERO),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
        ));
    }
    data.mesh = Some(mesh);
    commands.insert_resource(data);
}
fn mark(
    commands: &mut Commands,
    data: &mut Impacts,
    position: Vec3,
    normal: Vec3,
    size: f32,
    blood: bool,
) {
    if !position.is_finite() || !normal.is_finite() || normal.length_squared() < 0.5 {
        return;
    }
    // Low Violence: no blood decals (client blood effects 100f1940/1022f380).
    if blood && crate::low_violence::on() {
        return;
    }
    let (Some(mesh), Some(material)) = (
        data.mesh.clone(),
        if blood {
            data.blood_material.clone()
        } else {
            data.material.clone()
        },
    ) else {
        return;
    };
    let entity = commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(position + normal * 0.003)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, normal.normalize()))
                .with_scale(Vec3::splat(size)),
            bevy::light::NotShadowCaster,
        ))
        .id();
    data.live.push_back(entity);
    if data.live.len() > 256 {
        if let Some(old) = data.live.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
    if blood {
        data.blood_marks += 1;
    } else {
        data.bullet_marks += 1;
    }
}
pub fn update(
    mut commands: Commands,
    mut impacts: ResMut<Impacts>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    collision: Res<crate::collision::Collision>,
    state: Res<crate::game_state::Match>,
    frontend: Res<crate::game_menu::Frontend>,
) {
    if !frontend.playing() || state.paused {
        return;
    }
    let now = logic.world.time;
    impacts.blood_clock.retain(|(id, until)| {
        if now >= *until {
            if let Some(e) = logic.world.ents.get_mut(*id) {
                if let Kind::Particle { active, .. } = &mut e.kind {
                    *active = false;
                }
            }
            false
        } else {
            true
        }
    });
    // L4D2R_DECAL_LOG: debug, bullet holes placed per frame.
    if !impacts.hits.is_empty() && std::env::var_os("L4D2R_DECAL_LOG").is_some() {
        println!("Bullet holes: {} at {:?}", impacts.hits.len(), impacts.hits.first().map(|h| h.0));
    }
    for (position, normal) in std::mem::take(&mut impacts.hits).into_iter().take(32) {
        mark(&mut commands, &mut impacts, position, normal, 0.12, false);
    }
    if crate::net_play::host() {
        let copies: Vec<BloodHit> = impacts.blood.iter().take(16).cloned().collect();
        impacts.net_blood.extend(copies);
    }
    // Low Violence: no blood sprays on hits.
    if crate::low_violence::on() {
        impacts.blood.clear();
    }
    for hit in std::mem::take(&mut impacts.blood).into_iter().take(16) {
        if !hit.position.is_finite() || !hit.direction.is_finite() {
            continue;
        }
        let direction = hit.direction.normalize_or_zero();
        let slot = impacts.cursor % BLOOD_POOL;
        impacts.cursor += 1;
        if let Some(&id) = logic
            .world
            .find(&format!("__rewrite_blood_{slot}_{}", hit.shotgun as usize))
            .first()
        {
            let e = &mut logic.world.ents[id];
            e.me.origin =
                l4d2r_math::Vec3::new(hit.position.x, -hit.position.z, hit.position.y) / 0.0254;
            let source = l4d2r_math::Vec3::new(direction.x, -direction.z, direction.y);
            e.me.angles = l4d2r_math::Vec3::new(
                (-source.z).atan2(source.truncate().length()).to_degrees(),
                source.y.atan2(source.x).to_degrees(),
                0.,
            );
            e.kind = Kind::Particle {
                active: true,
                started: now,
            };
            impacts.blood_clock.retain(|(old, _)| *old != id);
            impacts.blood_clock.push((id, now + 0.2));
            impacts.blood_bursts += 1;
        }
        let start = hit.position + direction * 0.03;
        let end = start + direction * 3.;
        let trace = collision.trace(start, end, Vec3::ZERO);
        if trace.fraction < 1. && trace.normal.length_squared() > 0.5 {
            let position = start + (end - start) * trace.fraction;
            let moving = logic.world.ents.iter().enumerate().any(|(i, e)| {
                (e.me.brush.is_some_and(|model| model > 0) || e.me.classname.starts_with("prop_"))
                    && logic.bounds(i).is_some_and(|(lo, hi)| {
                        (position.cmpge(lo - Vec3::splat(0.02))
                            & position.cmple(hi + Vec3::splat(0.02)))
                        .all()
                    })
            });
            if !moving {
                mark(
                    &mut commands,
                    &mut impacts,
                    position,
                    trace.normal,
                    if hit.shotgun { 0.6 } else { 0.4 },
                    true,
                );
            }
        }
    }
}
