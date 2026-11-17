//! Gun-owned laser/temporary-ammo upgrades and reusable floor packs.
use crate::game_state::{Gun, Match, Rules};
use bevy::prelude::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Incendiary,
    Explosive,
}
impl Kind {
    pub fn pack_class(self) -> &'static str {
        match self {
            Self::Incendiary => "weapon_upgradepack_incendiary",
            Self::Explosive => "weapon_upgradepack_explosive",
        }
    }
    pub fn floor_class(self) -> &'static str {
        match self {
            Self::Incendiary => "upgrade_ammo_incendiary",
            Self::Explosive => "upgrade_ammo_explosive",
        }
    }
    pub fn floor_model(self) -> &'static str {
        match self {
            Self::Incendiary => "models/props/terror/incendiary_ammo.mdl",
            Self::Explosive => "models/props/terror/exploding_ammo.mdl",
        }
    }
    fn fx(self) -> &'static str {
        match self {
            Self::Incendiary => "impact_incendiary_generic",
            Self::Explosive => "impact_explosive_ammo_small",
        }
    }
}
#[derive(Component, Default)]
pub struct Claims(pub [bool; 4]);
#[derive(Component)]
pub struct Burn(pub f32);
#[derive(Resource, Default)]
pub struct Effects {
    pub hits: Vec<(Kind, Vec3)>,
    clocks: Vec<(usize, f32)>,
    cursor: usize,
}
pub fn take_round(gun: &mut Gun) -> Option<Kind> {
    if gun.upgraded_rounds == 0 {
        gun.ammo_upgrade = None;
        return None;
    }
    let kind = gun.ammo_upgrade?;
    gun.upgraded_rounds -= 1;
    if gun.upgraded_rounds == 0 {
        gun.ammo_upgrade = None;
    }
    Some(kind)
}
pub fn claim(
    state: &mut Match,
    rules: &Rules,
    claims: &mut Claims,
    who: usize,
    kind: Kind,
) -> bool {
    if who >= 4 || claims.0[who] {
        return false;
    }
    let Some(gun) = state.primary.as_mut() else {
        return false;
    };
    let Some(weapon) = rules.weapons.get(&gun.class) else {
        return false;
    };
    if weapon.melee() {
        return false;
    }
    gun.ammo = weapon.clip;
    gun.upgraded_rounds = weapon.clip;
    gun.ammo_upgrade = Some(kind);
    claims.0[who] = true;
    state.reload = 0.;
    state.reload_phase = 0;
    state.burst_left = 0;
    true
}
pub fn prepare(world: &mut l4d2r_game_core::entities::EntityWorld) {
    use l4d2r_game_core::{
        entities::{Ent, Kind as K},
        mapents::MapEnt,
    };
    for kind in [Kind::Incendiary, Kind::Explosive] {
        for slot in 0..8 {
            let name = format!("__upgrade_{}_{slot}", kind.floor_class());
            if !world.find(&name).is_empty() {
                continue;
            }
            let index = world.ents.len();
            world.add_runtime_entity(Ent {
                me: MapEnt {
                    index,
                    classname: "info_particle_system".into(),
                    targetname: name.clone(),
                    origin: l4d2r_math::Vec3::ZERO,
                    angles: l4d2r_math::Vec3::ZERO,
                    mdl: None,
                    brush: None,
                    ent: l4d2_formats::bsp::Entity(vec![
                        ("classname".into(), "info_particle_system".into()),
                        ("targetname".into(), name),
                        ("effect_name".into(), kind.fx().into()),
                    ]),
                },
                conns: vec![],
                kind: K::Particle {
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
#[derive(Component)]
pub struct Beam;
#[derive(Component)]
pub struct Dot;
pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(1., 0.02, 0.01),
        emissive: LinearRgba::new(5., 0.05, 0.01, 1.),
        unlit: true,
        ..default()
    });
    commands.spawn((
        Beam,
        Mesh3d(meshes.add(Cylinder::new(0.0015, 1.))),
        MeshMaterial3d(material.clone()),
        Transform::default(),
        Visibility::Hidden,
        bevy::camera::visibility::NoFrustumCulling,
        bevy::light::NotShadowCaster,
    ));
    commands.spawn((
        Dot,
        Mesh3d(meshes.add(Sphere::new(0.015))),
        MeshMaterial3d(material),
        Transform::default(),
        Visibility::Hidden,
        bevy::light::NotShadowCaster,
    ));
    commands.insert_resource(Effects::default());
}
pub fn laser(
    state: Res<Match>,
    rules: Res<Rules>,
    frontend: Res<crate::game_menu::Frontend>,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    collision: Res<crate::collision::Collision>,
    library: Res<crate::l4d2r_animation::AnimationLibrary>,
    playback: Res<crate::l4d2r_animation::PlaybackState>,
    roots: Query<(Entity, &crate::game::HeldWeapon, &GlobalTransform)>,
    body: Single<&crate::game::Body>,
    cameras: (
        Single<(&GlobalTransform, &Projection), With<crate::viewer::GameCamera>>,
        Single<&Projection, With<crate::game::ViewCamera>>,
    ),
    mut beam: Query<(&mut Transform, &mut Visibility), (With<Beam>, Without<Dot>)>,
    mut dot: Query<(&mut Transform, &mut Visibility), (With<Dot>, Without<Beam>)>,
) {
    let enabled = frontend.playing()
        && !logic.active
        && !state.dead
        && !state.infected
        && state.slot == 1
        && state.gun().laser;
    let Some((mut bt, mut bv)) = beam.iter_mut().next() else {
        return;
    };
    let Some((mut dt, mut dv)) = dot.iter_mut().next() else {
        return;
    };
    *bv = Visibility::Hidden;
    *dv = Visibility::Hidden;
    if !enabled {
        return;
    }
    let w = rules
        .weapons
        .get(state.held_class())
        .unwrap_or(&rules.pistol);
    let Some((id, _, root)) = roots.iter().find(|(_, h, _)| h.0 == w.class) else {
        return;
    };
    let Some((mut start, _, _)) = library.muzzle(&w.view_model, id, root, &playback, false) else {
        return;
    };
    // The held model renders with a separate FOV. Match its screen-space
    // muzzle when rendering the world beam so it cannot float beside the gun.
    let (camera, world_projection) = *cameras.0;
    if let (Projection::Perspective(world), Projection::Perspective(view)) =
        (world_projection, &*cameras.1)
    {
        let mut local = camera.affine().inverse().transform_point3(start);
        let ratio = (world.fov * 0.5).tan() / (view.fov * 0.5).tan();
        local.x *= ratio;
        local.y *= ratio;
        start = camera.transform_point(local);
    }
    let eye = body.feet + Vec3::Y * body.stance.eye();
    let direction = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.) * Vec3::NEG_Z;
    let hit = collision.trace(eye, eye + direction * w.range, Vec3::ZERO);
    let end = eye + direction * w.range * hit.fraction;
    let delta = end - start;
    *bt = Transform::from_translation((start + end) * 0.5)
        .with_rotation(Quat::from_rotation_arc(Vec3::Y, delta.normalize_or_zero()))
        .with_scale(Vec3::new(1., delta.length(), 1.));
    *bv = Visibility::Inherited;
    if hit.fraction < 1. {
        dt.translation = end + hit.normal * 0.005;
        *dv = Visibility::Inherited;
    }
}
pub fn update(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut state: ResMut<Match>,
    mut input: ResMut<crate::game::Input>,
    body: Single<&crate::game::Body>,
    rules: Res<Rules>,
    collision: Res<crate::collision::Collision>,
    models: Option<Res<crate::viewer::RenderModels>>,
    meshes: Option<Res<Assets<Mesh>>>,
    mut squad: ResMut<crate::game_state::Squad>,
    mut packs: Query<(Entity, &crate::game::Pickup, &mut Claims), Without<crate::game::Taken>>,
    mut enemies: Query<(
        Entity,
        &Transform,
        &mut crate::game::Enemy,
        Option<&mut Burn>,
    )>,
    mut fx: ResMut<Effects>,
    mut logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
) {
    if state.paused || logic.as_ref().is_some_and(|l| l.active) {
        return;
    }
    let dt = time.delta_secs();
    for (id, t, mut enemy, burn) in &mut enemies {
        let Some(mut burn) = burn else {
            continue;
        };
        if enemy.health > 0. && burn.0 > 0. {
            if !crate::special_infected::tank::burns_by_duration(&enemy.model) {
                enemy.health -= 80. * dt;
            }
            enemy.burning = burn.0;
            if enemy.health <= 0. {
                enemy.record_death(crate::infected_death::Cause::Fire, t, Vec3::ZERO, 1.);
                enemy.corpse_time = 12.;
                state.kills += 1;
            }
        }
        burn.0 -= dt;
        if burn.0 <= 0. || enemy.health <= 0. {
            commands.entity(id).remove::<Burn>();
        }
    }
    for (entity, pickup, mut claims) in &mut packs {
        let kind = if pickup.class.ends_with("incendiary") {
            Kind::Incendiary
        } else {
            Kind::Explosive
        };
        for (i, member) in squad.0.iter_mut().enumerate().take(3) {
            if !member.state.dead
                && !member.state.incap
                && member.position.distance(pickup.origin) < 2.
                && collision
                    .sight_trace(member.position + Vec3::Y, pickup.origin + Vec3::Y * 0.12)
                    .fraction
                    > 0.98
            {
                claim(&mut member.state, &rules, &mut claims, i + 1, kind);
            }
        }
        if claims.0.iter().all(|v| *v) {
            commands
                .entity(entity)
                .insert((crate::game::Taken, Visibility::Hidden));
        }
    }
    if !state.dead && !state.incap && !state.infected && !state.pinned && state.slot == 4 {
        if let Some(kind) = state.upgrade_pack {
            state.hint = "Hold left mouse to deploy special ammunition".into();
            if input.fire
                || input.fire_held
                || std::env::args().any(|a| a == "--capture-deploy-upgrade")
            {
                input.fire = false;
                if state.medical_kind != 9 {
                    state.medical_kind = 9;
                    state.action = 1.9;
                    state.action_total = 1.9;
                    state.item_use_serial = state.item_use_serial.wrapping_add(1);
                }
                state.action -= dt;
                if state.action <= 0. {
                    let direction = Quat::from_rotation_y(body.yaw) * Vec3::NEG_Z;
                    let p = body.feet + direction * 0.6;
                    let entity = crate::loot::drop_item(
                        &mut commands,
                        models.as_deref(),
                        meshes.as_deref(),
                        &rules,
                        &collision,
                        kind.floor_class(),
                        p,
                        None,
                    );
                    commands.entity(entity).insert(Claims::default());
                    state.upgrade_pack = None;
                    state.medical_kind = 0;
                    state.slot = if state.primary.is_some() { 1 } else { 2 };
                }
            } else {
                state.action = 0.;
                state.medical_kind = 0;
            }
            if state.upgrade_pack.is_none() {
                input.fire_held = false;
            }
        }
    } else if state.medical_kind == 9 {
        state.action = 0.;
        state.medical_kind = 0;
    }
    let hits = std::mem::take(&mut fx.hits);
    for (kind, position) in hits.iter().filter(|(kind, _)| *kind == Kind::Explosive) {
        let _ = kind;
        for (_, t, mut enemy, _) in &mut enemies {
            let center = t.translation + Vec3::Y * 0.9;
            let distance = center.distance(*position);
            if enemy.health > 0.
                && distance < 1.5
                && collision
                    .sight_trace(*position + Vec3::Y * 0.03, center)
                    .fraction
                    >= 0.98
            {
                enemy.health -= 20. * (1. - distance / 1.5);
                enemy.reaction = 1;
                enemy.attack = 0.4;
                if enemy.health <= 0. {
                    enemy.record_death(
                        crate::infected_death::Cause::Explosion,
                        t,
                        center - *position,
                        1.,
                    );
                    enemy.corpse_time = 12.;
                    state.kills += 1;
                }
            }
        }
    }
    if let Some(logic) = logic.as_mut() {
        let now = logic.world.time;
        fx.clocks.retain(|(id, until)| {
            if now >= *until {
                if let Some(e) = logic.world.ents.get_mut(*id) {
                    e.kind = l4d2r_game_core::entities::Kind::Particle {
                        active: false,
                        started: now,
                    };
                }
                false
            } else {
                true
            }
        });
        for (kind, position) in hits.into_iter().take(32) {
            let slot = fx.cursor % 8;
            fx.cursor += 1;
            if let Some(&id) = logic
                .world
                .find(&format!("__upgrade_{}_{slot}", kind.floor_class()))
                .first()
            {
                logic.world.ents[id].me.origin =
                    l4d2r_math::Vec3::new(position.x, -position.z, position.y) / 0.0254;
                logic.world.ents[id].kind = l4d2r_game_core::entities::Kind::Particle {
                    active: true,
                    started: now,
                };
                fx.clocks.retain(|(old, _)| *old != id);
                fx.clocks.push((id, now + 0.1));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_keeps_pooled_upgrade_emitters_and_clears_their_activity() {
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let path = std::path::Path::new("maps/rewrite_lab.bsp");
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, path, true).unwrap();
        prepare(&mut logic.world);
        let name = "__upgrade_upgrade_ammo_explosive_0";
        let id = logic.world.find(name)[0];
        logic.world.ents[id].kind = l4d2r_game_core::entities::Kind::Particle {
            active: true,
            started: 1.,
        };
        logic.restart();
        assert_eq!(logic.world.find(name)[0], id);
        assert!(matches!(
            logic.world.ents[id].kind,
            l4d2r_game_core::entities::Kind::Particle { active: false, .. }
        ));
    }
    #[test]
    fn one_magazine_then_normal_rounds_and_laser_persists() {
        let w = crate::game_state::tests_weapon();
        let mut rules = Rules {
            pistol: w.clone(),
            weapons: Default::default(),
        };
        let mut primary = w.clone();
        primary.class = "weapon_rifle".into();
        primary.clip = 50;
        rules.weapons.insert(primary.class.clone(), primary);
        let mut state = Match::new(&w);
        state.primary = Some(Gun {
            class: "weapon_rifle".into(),
            ammo: 7,
            reserve: 200,
            laser: true,
            ammo_upgrade: None,
            upgraded_rounds: 0,
        });
        for kind in [Kind::Incendiary, Kind::Explosive] {
            let mut claims = Claims::default();
            assert!(claim(&mut state, &rules, &mut claims, 0, kind));
            assert!(!claim(&mut state, &rules, &mut claims, 0, kind));
            let gun = state.primary.as_mut().unwrap();
            for _ in 0..50 {
                assert_eq!(take_round(gun), Some(kind));
            }
            assert_eq!(take_round(gun), None);
            assert!(gun.laser);
            assert_eq!(gun.reserve, 200);
        }
    }
}
