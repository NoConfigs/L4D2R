//! Throwable lifecycle over native models, PCF effects, sounds and collision.
//! Ballistics/contact response are local approximations, not IVP/VPhysics parity.
use crate::{
    collision::{Collision, EYE, UNIT},
    game::{Body, Enemy, Input},
    game_state::{Match, Rules},
    l4d2r_animation::AnimationLibrary,
    viewer::RenderModels,
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use l4d2r_game_core::{
    entities::{DamageKind, Ent, EntityWorld, Kind as EntityKind},
    mapents::MapEnt,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Molotov,
    Pipe,
    Bile,
}
impl Kind {
    pub fn parse(class: &str) -> Option<Self> {
        match class {
            "weapon_molotov" => Some(Self::Molotov),
            "weapon_pipe_bomb" => Some(Self::Pipe),
            "weapon_vomitjar" => Some(Self::Bile),
            _ => None,
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Molotov => 0,
            Self::Pipe => 1,
            Self::Bile => 2,
        }
    }
}
const FX: [&str; 3] = ["molotov_groundfire_low", "weapon_pipebomb", "vomit_jar"];
const POOL: usize = 8;
pub fn prepare(world: &mut EntityWorld) {
    for slot in 0..POOL {
        for (kind, fx) in FX.iter().enumerate() {
            let name = format!("__rewrite_throw_fx_{slot}_{kind}");
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
                        ("effect_name".into(), fx.to_string()),
                    ]),
                },
                conns: vec![],
                kind: EntityKind::Particle {
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
pub struct Projectile {
    pub kind: Kind,
    pub velocity: Vec3,
    pub age: f32,
    beep: f32,
}
struct Patch {
    kind: Kind,
    position: Vec3,
    left: f32,
    fx: usize,
}
#[derive(Resource, Default)]
pub struct Projectiles {
    patches: Vec<Patch>,
    pub(crate) biled: Vec<(Entity, Vec3, f32)>,
    pipes: Vec<Vec3>,
    epoch: f32,
    pub thrown: u32,
    pub impacts: u32,
}
impl Projectiles {
    pub fn attraction(
        &self,
        position: Vec3,
        self_id: Entity,
        ignore_pipe: bool,
        ignore_bile: bool,
    ) -> Option<(Vec3, Option<Entity>)> {
        self.pipes
            .iter()
            .filter(|_| !ignore_pipe)
            .copied()
            .map(|p| (p, None))
            .chain(
                self.patches
                    .iter()
                    .filter(|p| p.kind == Kind::Bile && !ignore_bile)
                    .map(|p| (p.position, None)),
            )
            .chain(
                self.biled
                    .iter()
                    .filter(|(id, _, _)| *id != self_id && !ignore_bile)
                    .map(|(id, p, _)| (*p, Some(*id))),
            )
            .filter(|(p, _)| p.distance(position) < 30.)
            .min_by(|a, b| {
                a.0.distance_squared(position)
                    .total_cmp(&b.0.distance_squared(position))
            })
    }
}
pub fn controls(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut state: ResMut<Match>,
    input: Res<Input>,
    body: Single<&Body>,
    rules: Res<Rules>,
    models: Res<RenderModels>,
    animations: Res<AnimationLibrary>,
    collision: Res<Collision>,
    mut data: ResMut<Projectiles>,
    mut sounds: ResMut<crate::game_audio::Queue>,
) {
    if state.infected || state.paused || state.dead || state.incap {
        return;
    }
    // A pinned survivor cannot throw; a pull in progress is dropped.
    if state.pinned {
        state.throw_stage = 0;
        state.throw_class = None;
        return;
    }
    if state.slot != 3 {
        state.throw_stage = 0;
        state.throw_class = None;
        return;
    }
    let auto = std::env::args().any(|a| a == "--lab-capture-throw");
    let held = if auto {
        state.elapsed > 0.7 && state.elapsed < 1.7
    } else {
        input.throw_held
    };
    let class = state.held_class().to_string();
    let Some(kind) = Kind::parse(&class) else {
        return;
    };
    let Some(weapon) = rules.weapons.get(&class) else {
        return;
    };
    if state.throw_stage == 0 && held {
        state.throw_stage = 1;
        state.throw_clock = animations.duration(&weapon.view_model, &["ACT_VM_PULLPIN"]);
    } else if state.throw_stage == 1 {
        state.throw_clock -= time.delta_secs();
        if state.throw_clock <= 0. {
            state.throw_stage = 2;
        }
    }
    if state.throw_stage == 2 && !held {
        if let Some(class) = state.throwable.take() {
            state.throw_class = Some(class);
            state.throw_stage = 3;
            state.throw_clock = animations.duration(&weapon.view_model, &["ACT_VM_THROW"]);
            let rotation = Quat::from_euler(EulerRot::YXZ, body.yaw, body.pitch, 0.);
            let forward = rotation * Vec3::NEG_Z;
            let eye = body.feet + Vec3::Y * EYE;
            let end = eye + forward * 0.4;
            let hit = collision.physics_trace(eye, end, Vec3::splat(0.055));
            let position = eye + (end - eye) * hit.fraction;
            let root = commands
                .spawn((
                    Transform::from_translation(position),
                    Visibility::Inherited,
                    Projectile {
                        kind,
                        velocity: forward * (750. * UNIT) + Vec3::Y * (200. * UNIT) + body.velocity,
                        age: 0.,
                        beep: 0.,
                    },
                ))
                .id();
            if let Some(handles) = models.0.get(&(weapon.model.clone(), 0, 0)) {
                for (mesh, material) in handles {
                    commands.spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        crate::viewer::DynamicModelLighting::default(),
                        Transform::default(),
                        RenderLayers::layer(1),
                        ChildOf(root),
                    ));
                }
            }
            if kind == Kind::Molotov {
                sounds.play("Molotov.Throw", None);
            }
            data.thrown += 1;
            println!("Throwable released: {kind:?}; count={}", data.thrown);
        }
    } else if state.throw_stage == 3 {
        state.throw_clock -= time.delta_secs();
        if state.throw_clock <= 0. {
            state.throw_stage = 0;
            state.throw_class = None;
            state.slot = 2;
        }
    }
}
/// Swept, bounded contact steps with Source-unit gravity. Restitution/friction
/// remain explicit provisional values until compared against native trajectories.
pub fn flight(
    collision: &Collision,
    position: &mut Vec3,
    velocity: &mut Vec3,
    dt: f32,
    bounce: bool,
) -> bool {
    let count = (dt / (1. / 120.)).ceil().max(1.) as usize;
    let step = dt / count as f32;
    let mut collided = false;
    for _ in 0..count.min(16) {
        let before = *velocity;
        velocity.y -= 800. * UNIT * step;
        let travel = (before + *velocity) * 0.5 * step;
        let trace = collision.physics_trace(*position, *position + travel, Vec3::splat(0.055));
        *position += travel * trace.fraction;
        if trace.start_solid || trace.fraction < 1. {
            collided = true;
            if !bounce {
                return true;
            }
            let speed = velocity.dot(trace.normal);
            if speed < 0. {
                *velocity -= trace.normal * speed * 1.45;
            }
            *position += trace.normal * 0.002;
            *velocity *= 0.8;
            if trace.normal.y > 0.6 && velocity.length() < 0.35 {
                *velocity = Vec3::ZERO;
            }
        }
    }
    collided
}
pub fn tick(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    mut state: ResMut<Match>,
    body: Single<&Body>,
    collision: Res<Collision>,
    mut data: ResMut<Projectiles>,
    mut logic: ResMut<crate::l4d2r_cinematic::MapLogic>,
    mut sounds: ResMut<crate::game_audio::Queue>,
    mut shots: Query<(Entity, &mut Transform, &mut Projectile), Without<Enemy>>,
    mut enemies: Query<(Entity, &Transform, &mut Enemy), Without<Projectile>>,
    mut impacts: Option<ResMut<crate::bullet_impacts::Impacts>>,
) {
    if state.paused || state.dead {
        return;
    }
    let dt = time.delta_secs();
    if state.elapsed < data.epoch {
        for (id, _, _) in &shots {
            commands.entity(id).despawn();
        }
        data.patches.clear();
        data.biled.clear();
        data.pipes.clear();
        data.thrown = 0;
        data.impacts = 0;
    }
    data.epoch = state.elapsed;
    data.pipes.clear();
    for patch in &mut data.patches {
        patch.left -= dt;
        if patch.left <= 0. {
            if let Some(e) = logic.world.ents.get_mut(patch.fx) {
                e.kind = EntityKind::Particle {
                    active: false,
                    started: 0.,
                };
            }
        }
    }
    data.patches.retain(|p| p.left > 0.);
    for b in &mut data.biled {
        b.2 -= dt;
    }
    data.biled.retain(|b| b.2 > 0.);
    for (entity, mut transform, mut shot) in &mut shots {
        shot.age += dt;
        let mut position = transform.translation;
        let mut velocity = shot.velocity;
        let touched = flight(
            &collision,
            &mut position,
            &mut velocity,
            dt,
            shot.kind == Kind::Pipe,
        );
        shot.velocity = velocity;
        transform.translation = position;
        transform.rotate_local_z(3. * dt);
        let victim = enemies
            .iter()
            .filter(|(_, _, e)| e.health > 0.)
            .find(|(_, t, _)| (t.translation + Vec3::Y).distance(position) < 0.65)
            .map(|(id, _, _)| id);
        let impact = if shot.kind == Kind::Pipe {
            shot.age >= 6.
        } else {
            touched || victim.is_some() || shot.age >= 12.
        };
        if shot.kind == Kind::Pipe && !impact {
            data.pipes.push(position);
            shot.beep -= dt;
            if shot.beep <= 0. {
                sounds.play("PipeBomb.TimerBeep", Some(position));
                shot.beep = if shot.age > 4.5 { 0.15 } else { 0.6 };
            }
        }
        if !impact {
            continue;
        }
        commands.entity(entity).despawn();
        data.impacts += 1;
        println!(
            "Throwable impact: {:?}; age={:.2}; impacts={}",
            shot.kind, shot.age, data.impacts
        );
        let fx = (0..POOL)
            .flat_map(|slot| {
                logic
                    .world
                    .find(&format!("__rewrite_throw_fx_{slot}_{}", shot.kind.index()))
            })
            .find(|id| !data.patches.iter().any(|p| p.fx == *id));
        if let Some(fx) = fx {
            let started = logic.world.time;
            let e = &mut logic.world.ents[fx];
            e.me.origin = l4d2r_math::Vec3::new(position.x, -position.z, position.y) / UNIT;
            e.kind = EntityKind::Particle {
                active: true,
                started,
            };
            data.patches.push(Patch {
                kind: shot.kind,
                position,
                left: match shot.kind {
                    Kind::Molotov => 15.,
                    Kind::Pipe => 2.,
                    Kind::Bile => 20.,
                },
                fx,
            });
        }
        sounds.play(
            match shot.kind {
                Kind::Molotov => "GlassBottle.ImpactHard",
                Kind::Pipe => "BaseGrenade.Explode",
                Kind::Bile => "CedaJar.Explode",
            },
            Some(position),
        );
        if shot.kind == Kind::Bile {
            if let Some(id) = victim {
                if enemies.get(id).is_ok_and(|(_, _, e)| {
                    crate::infected_population::Kind::model(&e.model)
                        != crate::infected_population::Kind::Jimmy
                }) {
                    data.biled.push((id, position, 20.));
                }
            }
        }
        if shot.kind == Kind::Pipe {
            for (id, t, mut enemy) in &mut enemies {
                let distance = t.translation.distance(position);
                if enemy.health > 0.
                    && distance < 8.9
                    && collision
                        .trace(
                            position + Vec3::Y * 0.15,
                            t.translation + Vec3::Y,
                            Vec3::ZERO,
                        )
                        .fraction
                        >= 0.99
                {
                    enemy.health -= 250. * (1. - distance / 8.9);
                    // Only the player throws (bots carry no grenades yet).
                    crate::game_ui::kill_feed::hit(id, crate::game_ui::kill_feed::Killer::Player);
                    enemy.reaction = 2;
                    enemy.attack = 0.2;
                    if enemy.health <= 0. {
                        enemy.record_death(
                            crate::infected_death::Cause::Explosion,
                            t,
                            t.translation - position,
                            1.,
                        );
                        enemy.corpse_time = 12.;
                        state.kills += 1;
                        if let Some(impacts) = impacts.as_mut() {
                            impacts.gore.push(crate::infected_gore::Hit {
                                damage_force_source: None,
                                entity: id, position: t.translation + Vec3::Y, direction: (t.translation - position).normalize_or_zero(),
                                box_index: 0, cause: crate::infected_death::Cause::Explosion, distance,
                                damage_type: crate::infected_gore::DMG_BLAST, sniper: false, swing: None,
                                back: false, decapitates: false, seed: id.to_bits(),
                            });
                        }
                    }
                }
            }
            let ids = logic.world.durability.keys().copied().collect::<Vec<_>>();
            for id in ids {
                if let Some((lo, hi)) = logic.bounds(id) {
                    let d = ((lo + hi) * 0.5).distance(position);
                    if d < 8.9 {
                        logic
                            .world
                            .damage_entity(id, 250. * (1. - d / 8.9), DamageKind::Blast);
                    }
                }
            }
            let d = body.feet.distance(position);
            if d < 8.9 {
                state.damage(50. * (1. - d / 8.9));
            }
        }
    }
    for patch in data.patches.iter().filter(|p| p.kind == Kind::Molotov) {
        for (id, t, mut enemy) in &mut enemies {
            if enemy.health > 0.
                && t.translation.distance(patch.position) < 3.5
                && collision
                    .trace(
                        patch.position + Vec3::Y * 0.1,
                        t.translation + Vec3::Y,
                        Vec3::ZERO,
                    )
                    .fraction
                    >= 0.99
            {
                if crate::infected_population::Kind::model(&enemy.model).fireproof() {
                    continue;
                }
                enemy.burning = 1.;
                if !crate::special_infected::tank::burns_by_duration(&enemy.model) {
                    enemy.health -= 80. * dt;
                }
                crate::game_ui::kill_feed::hit(id, crate::game_ui::kill_feed::Killer::Player);
                if enemy.health <= 0. {
                    enemy.record_death(
                        crate::infected_death::Cause::Fire,
                        t,
                        t.translation - patch.position,
                        1.,
                    );
                    enemy.corpse_time = 12.;
                    state.kills += 1;
                }
            }
        }
        if body.feet.distance(patch.position) < 3.5 {
            state.damage(10. * dt);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roadcrew_ignores_pipe_but_jimmy_ignores_all_bile_attraction() {
        let mut data = Projectiles::default();
        let actor = Entity::from_bits(1);
        data.pipes.push(Vec3::X);
        data.biled.push((Entity::from_bits(2), Vec3::X * 2., 20.));
        assert_eq!(
            data.attraction(Vec3::ZERO, actor, false, false).unwrap().0,
            Vec3::X
        );
        assert_eq!(
            data.attraction(Vec3::ZERO, actor, true, false).unwrap().0,
            Vec3::X * 2.
        );
        assert!(data.attraction(Vec3::ZERO, actor, true, true).is_none());
    }
    #[test]
    fn swept_throw_bounces_and_free_fall_is_step_rate_independent() {
        let c = Collision::test_floor();
        let mut p = Vec3::Y * 2.;
        let mut v = Vec3::new(2., -15., 0.);
        assert!(flight(&c, &mut p, &mut v, 0.2, true));
        assert!(p.y >= 0.05);
        assert!(v.y > 0.);
        let free = Collision::default();
        let mut a = Vec3::Y * 30.;
        let mut av = Vec3::X * 3.;
        let mut b = a;
        let mut bv = av;
        for _ in 0..60 {
            flight(&free, &mut a, &mut av, 1. / 60., true);
        }
        for _ in 0..120 {
            flight(&free, &mut b, &mut bv, 1. / 120., true);
        }
        assert!(a.distance(b) < 0.001);
        assert!(av.distance(bv) < 0.001);
    }
}
