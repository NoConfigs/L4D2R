//! Bone-local studio boxes evaluated against the actual animated pose.
use bevy::prelude::*;
use l4d2_formats::anim::Skeleton;
use l4d2r_math::{Mat4 as XMat, Vec3 as XVec};
pub struct Boxes(Vec<(XMat, XVec, XVec, i32, usize)>);
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub distance: f32,
    pub group: i32,
    pub index: usize,
}
#[derive(Resource, Default)]
pub struct Audit {
    pub groups: Vec<i32>,
}
pub fn body_damage_factor(difficulty: crate::campaign_settings::Difficulty) -> f32 {
    // Recovered server.dll ConVar registrations, recorded in
    // local-research/recover-native-damage-defaults.py.
    use crate::campaign_settings::Difficulty::*;
    match difficulty {
        Easy => 2.,
        Normal => 1.,
        Advanced => 0.75,
        Expert => 0.5,
    }
}
impl Boxes {
    pub fn from_pose(sk: &Skeleton, pose: &[XMat], root: &Transform) -> Self {
        let root = XMat::from_cols_array(&root.to_matrix().to_cols_array());
        let source_to_render = XMat::from_cols(
            l4d2r_math::Vec4::new(0.0254, 0., 0., 0.),
            l4d2r_math::Vec4::new(0., 0., -0.0254, 0.),
            l4d2r_math::Vec4::new(0., 0.0254, 0., 0.),
            l4d2r_math::Vec4::W,
        );
        Self(
            sk.hitboxes
                .iter()
                .enumerate()
                .filter_map(|(index, b)| {
                    let matrix = root * source_to_render * *pose.get(b.bone)?;
                    let inverse = matrix.inverse();
                    inverse
                        .is_finite()
                        .then_some((inverse, b.min, b.max, b.group, index))
                })
                .collect(),
        )
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn trace(&self, origin: Vec3, direction: Vec3, limit: f32) -> Option<Hit> {
        self.0
            .iter()
            .filter_map(|(inverse, lo, hi, group, index)| {
                let p = inverse.transform_point3(XVec::from_array(origin.to_array()));
                let d = inverse.transform_vector3(XVec::from_array(direction.to_array()));
                // Leave the transformed direction unnormalised: the ray parameter
                // is still measured in world metres, regardless of bone/root scale.
                let distance = crate::ray_box(
                    Vec3::from_array(p.to_array()),
                    Vec3::from_array(d.to_array()),
                    Vec3::from_array(lo.to_array()),
                    Vec3::from_array(hi.to_array()),
                )?;
                (distance < limit).then_some(Hit {
                    distance,
                    group: *group,
                    index: *index,
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pose_hitboxes_follow_rotation_and_animation_and_reject_misses_and_occlusion() {
        use crate::source_assets::SourceAssets;
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let assets = std::cell::RefCell::new(SourceAssets::new(root, vec![]).unwrap());
        let sk = Skeleton::load_with("models/infected/common_male01.mdl", |p| {
            assets.borrow_mut().read(p).ok()
        })
        .unwrap();
        assert_eq!(sk.hitboxes.len(), 23);
        let seq = sk.find_sequence("ACT_TERROR_RUN_INTENSE").unwrap();
        let head = sk.hitboxes.iter().filter(|h| h.group == 1).last().unwrap();
        for clock in [0., 0.15, 0.35] {
            let pose = sk.pose_sequence_params(seq, clock, true, &[("move_x", 1.), ("move_y", 0.)]);
            for yaw in [0., 1.2] {
                let root = Transform::from_translation(Vec3::new(3., 2., -4.))
                    .with_rotation(Quat::from_rotation_y(yaw));
                let boxes = Boxes::from_pose(&sk, &pose, &root);
                let local = pose[head.bone].transform_point3((head.min + head.max) * 0.5);
                let center = root.transform_point(Vec3::from_array(
                    crate::binary::source_position(local.to_array()),
                ));
                let direction = root.rotation * Vec3::NEG_X;
                let origin = center - direction * 5.;
                let hit = boxes.trace(origin, direction, 10.).unwrap();
                assert_eq!(hit.group, 1);
                assert!(hit.distance > 4. && hit.distance < 5.1);
                assert!(boxes.trace(origin, direction, 1.).is_none());
                assert!(boxes.trace(origin + Vec3::Y * 3., direction, 10.).is_none());
            }
        }
    }
}
