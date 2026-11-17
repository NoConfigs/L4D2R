//! Actor-specific IK contacts. Studio targets are in Source model units;
//! traces and persistent foot targets are in renderer world coordinates.
use bevy::prelude::*;
use l4d2_formats::anim::{IkRule, Skeleton};
use l4d2r_math::Mat4 as XMat;
use std::collections::HashMap;
#[derive(Default)]
pub(crate) struct Context {
    contacts: HashMap<i32, Vec3>,
    last_position: Option<Vec3>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actor_space_roundtrip_and_ground_latch_support_loss() {
        let actor = Transform::from_translation(Vec3::new(3., 2., -4.))
            .with_rotation(Quat::from_rotation_y(0.7));
        let point = l4d2r_math::Vec3::new(20., -30., 12.);
        assert!(to_model(to_world(point, &actor), &actor).distance(point) < 0.0001);
        let root =
            std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets =
            std::cell::RefCell::new(crate::source_assets::SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        let chain = sk.ik_chains.iter().position(|c| c.name == "lfoot").unwrap();
        let tip = sk.ik_chains[chain].bones[2];
        let mut pose = sk.bind_pose();
        let mut actor = Transform::default();
        actor.translation.y = 0.1 - to_world(pose[tip].w_axis.truncate(), &actor).y;
        let rule = IkRule {
            index: 0,
            chain,
            kind: 3,
            bone: None,
            slot: 0,
            weight: 1.,
            target: XMat::IDENTITY,
            height: 8.,
            radius: 2.,
            floor: 0.,
            drop: 12.,
            top: 8.,
            attachment: String::new(),
        };
        let mut context = Context::default();
        context.apply(
            &sk,
            &mut pose,
            &[rule.clone()],
            &actor,
            &crate::collision::Collision::test_solid_floor(),
            true,
        );
        assert_eq!(context.contacts.len(), 1);
        assert!(pose.iter().all(|m| m.is_finite()));
        let foot = to_world(pose[tip].w_axis.truncate(), &actor);
        // The BSP trace deliberately retains a small surface clearance.
        assert!(foot.y.abs() < 0.004, "foot {foot:?}");
        let before = pose.clone();
        context.apply(
            &sk,
            &mut pose,
            &[rule],
            &actor,
            &crate::collision::Collision::default(),
            true,
        );
        assert!(context.contacts.is_empty());
        assert_eq!(pose, before);
    }
}
fn to_world(point: l4d2r_math::Vec3, actor: &Transform) -> Vec3 {
    actor.transform_point(Vec3::new(point.x, point.z, -point.y) * 0.0254)
}
fn to_model(point: Vec3, actor: &Transform) -> l4d2r_math::Vec3 {
    let p = actor.to_matrix().inverse().transform_point3(point) / 0.0254;
    l4d2r_math::Vec3::new(p.x, -p.z, p.y)
}
impl Context {
    pub(crate) fn apply(
        &mut self,
        sk: &Skeleton,
        pose: &mut [XMat],
        rules: &[IkRule],
        actor: &Transform,
        collision: &crate::collision::Collision,
        ground: bool,
    ) {
        if self
            .last_position
            .is_some_and(|p| p.distance(actor.translation) > 1.)
            || !ground
        {
            self.contacts.clear();
        }
        self.last_position = Some(actor.translation);
        self.contacts.retain(|slot, _| {
            rules
                .iter()
                .any(|r| r.slot == *slot && r.kind == 3 && r.weight > 0.01)
        });
        let mut results = std::collections::BTreeMap::new();
        for rule in rules {
            if matches!(rule.kind, 4 | 6) {
                self.contacts.remove(&rule.slot);
                if rule.kind == 6 {
                    continue;
                }
            }
            let Some(chain) = sk.ik_chains.get(rule.chain) else {
                continue;
            };
            let Some(&end) = pose.get(chain.bones[2]) else {
                continue;
            };
            let target = match rule.kind {
                4 => end,
                1 => {
                    rule.bone
                        .and_then(|b| pose.get(b))
                        .copied()
                        .unwrap_or(XMat::IDENTITY)
                        * rule.target
                }
                3 if ground => {
                    let foot = to_world(end.w_axis.truncate(), actor);
                    let candidate = self.contacts.get(&rule.slot).copied().unwrap_or(foot);
                    // Recheck cached support when doors/elevators move.
                    let up = rule.top.max(rule.height).clamp(4., 24.) * 0.0254;
                    let down = rule.drop.clamp(4., 32.) * 0.0254;
                    let start = candidate + Vec3::Y * up;
                    let finish = candidate - Vec3::Y * down;
                    let hit = collision.physics_trace(start, finish, Vec3::ZERO);
                    if hit.start_solid || hit.fraction >= 1. || hit.normal.y < 0.55 {
                        self.contacts.remove(&rule.slot);
                        continue;
                    }
                    let contact = start.lerp(finish, hit.fraction)
                        + Vec3::Y * rule.floor.clamp(0., 12.) * 0.0254;
                    if contact.distance(foot) > 24. * 0.0254 {
                        self.contacts.remove(&rule.slot);
                        continue;
                    }
                    if rule.weight >= 0.99 {
                        self.contacts.insert(rule.slot, contact);
                    }
                    let (_, rotation, _) = end.to_scale_rotation_translation();
                    XMat::from_rotation_translation(rotation, to_model(contact, actor))
                }
                // External targets need their owning entity's attachment,
                // rather than an invented target at model origin.
                _ => continue,
            };
            let accumulated: &mut XMat = results.entry(rule.chain).or_insert(end);
            let (_, q0, p0) = accumulated.to_scale_rotation_translation();
            let (_, q1, p1) = target.to_scale_rotation_translation();
            let weight = rule.weight.clamp(0., 1.);
            *accumulated =
                XMat::from_rotation_translation(q0.slerp(q1, weight), p0.lerp(p1, weight));
        }
        for (chain, target) in results {
            let tip = sk.ik_chains[chain].bones[2];
            // An untouched/released chain should not accumulate tiny solver
            // corrections merely because its descriptor reserves a slot.
            if !target.abs_diff_eq(pose[tip], 0.00001) {
                sk.solve_ik(pose, chain, target, 1.);
            }
        }
    }
}
