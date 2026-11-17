//! Native common-infected death selection (server 10415d80). Damage context
//! is captured on the lethal hit and remains fixed throughout playback.
//!
//! Retail, measured on c1m1_hotel (13 November) by killing commons from the
//! script and with real katana and frying-pan swings: blasts and melee kills
//! go limp at once (melee asks for ACT_SLICING_DEATH_* / bludgeon deaths the
//! common models mostly lack, so SetActivity fails into a ragdoll); bullets
//! play a weighted ACT_TERROR_DIE_FROM_STAND, HeadshotFront/HeadshotBack
//! included, with no special headshot choice.
use bevy::prelude::Vec3;
use l4d2_formats::anim::Skeleton;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum Cause {
    #[default]
    Bullet,
    Shotgun,
    Slash,
    Blunt,
    Fire,
    Explosion,
}
#[derive(Clone, Copy, Default, Debug)]
pub struct Context {
    pub cause: Cause,
    pub head: bool,
    pub running: bool,
    pub incoming: Vec3,
    /// A wound with DeathThroes "none" (limbs, halves): no death animation.
    pub ragdoll: bool,
    /// The killer was more than 500 units away: no directional shotgun
    /// death (10415d80 tests the attacker within 500).
    pub far: bool,
}
/// Shotgun deaths need the push within 45 degrees of an axis (0.707).
const AXIS: f32 = 0.707;
pub fn activity(c: Context) -> &'static str {
    // `incoming` is the push in model space: +X forward, +Z right.
    let forward = c.incoming.x;
    let right = c.incoming.z;
    match c.cause {
        _ if c.ragdoll => "ACT_DIERAGDOLL",
        Cause::Fire | Cause::Explosion | Cause::Slash | Cause::Blunt => "ACT_DIERAGDOLL",
        Cause::Shotgun if c.far => if c.running { "ACT_TERROR_DIE_WHILE_RUNNING" } else { "ACT_TERROR_DIE_FROM_STAND" },
        Cause::Shotgun if -forward > AXIS => "ACT_TERROR_DIE_BACKWARD_FROM_SHOTGUN",
        Cause::Shotgun if forward > AXIS => "ACT_TERROR_DIE_FORWARD_FROM_SHOTGUN",
        Cause::Shotgun if -right > AXIS => "ACT_TERROR_DIE_LEFTWARD_FROM_SHOTGUN",
        Cause::Shotgun if right > AXIS => "ACT_TERROR_DIE_RIGHTWARD_FROM_SHOTGUN",
        _ if c.running => "ACT_TERROR_DIE_WHILE_RUNNING",
        _ => "ACT_TERROR_DIE_FROM_STAND",
    }
}
/// Death goes limp at once from the current pose.
pub fn immediate_ragdoll(c: Context) -> bool {
    activity(c) == "ACT_DIERAGDOLL"
}
pub fn select(sk: &Skeleton, context: Context, seed: u64) -> Option<usize> {
    let desired = activity(context);
    let candidates = |name: &str| {
        sk.sequences()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.activity_name.eq_ignore_ascii_case(name))
            .map(|(i, s)| (i, s.activity_weight.unsigned_abs().max(1) as u64))
            .collect::<Vec<_>>()
    };
    let mut options = candidates(desired);
    if options.is_empty() {
        options = candidates("ACT_TERROR_DIE_FROM_STAND");
    }
    // Special infected models only author ACT_DIERAGDOLL: go limp at once.
    if options.is_empty() {
        options = candidates("ACT_DIERAGDOLL");
    }
    let total: u64 = options.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let mut value = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        % total;
    for (i, w) in options {
        if value < w {
            return Some(i);
        }
        value -= w;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_death_rules() {
        let c = |cause, incoming: Vec3| Context { cause, incoming, ..Default::default() };
        for cause in [Cause::Explosion, Cause::Fire, Cause::Slash, Cause::Blunt] {
            assert_eq!(activity(c(cause, Vec3::X)), "ACT_DIERAGDOLL");
        }
        assert_eq!(activity(c(Cause::Shotgun, Vec3::NEG_X)), "ACT_TERROR_DIE_BACKWARD_FROM_SHOTGUN");
        assert_eq!(activity(c(Cause::Shotgun, Vec3::X)), "ACT_TERROR_DIE_FORWARD_FROM_SHOTGUN");
        assert_eq!(activity(c(Cause::Shotgun, Vec3::NEG_Z)), "ACT_TERROR_DIE_LEFTWARD_FROM_SHOTGUN");
        assert_eq!(activity(c(Cause::Shotgun, Vec3::Z)), "ACT_TERROR_DIE_RIGHTWARD_FROM_SHOTGUN");
        // Between axes there is no directional shotgun death.
        assert_eq!(activity(c(Cause::Shotgun, Vec3::new(0.6, 0.53, 0.6).normalize())), "ACT_TERROR_DIE_FROM_STAND");
        // An exact diagonal (0.70711) still clears retail's 0.707.
        assert_eq!(activity(c(Cause::Shotgun, Vec3::new(1., 0., 1.).normalize())), "ACT_TERROR_DIE_FORWARD_FROM_SHOTGUN");
        assert_eq!(activity(Context { running: true, ..c(Cause::Bullet, Vec3::X) }), "ACT_TERROR_DIE_WHILE_RUNNING");
        assert_eq!(activity(Context { head: true, ..c(Cause::Bullet, Vec3::X) }), "ACT_TERROR_DIE_FROM_STAND");
        assert_eq!(activity(Context { ragdoll: true, ..c(Cause::Bullet, Vec3::X) }), "ACT_DIERAGDOLL");
        assert_eq!(activity(Context { far: true, ..c(Cause::Shotgun, Vec3::X) }), "ACT_TERROR_DIE_FROM_STAND");
    }
    #[test]
    fn owned_deaths_draw_the_whole_weighted_activity_including_headshots() {
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
        for cause in [Cause::Bullet, Cause::Shotgun, Cause::Explosion, Cause::Slash] {
            for incoming in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
                let context = Context { cause, incoming, ..Default::default() };
                let seq = select(&sk, context, 17).unwrap();
                let s = &sk.sequences()[seq];
                assert!(s.activity_name == activity(context), "{context:?}: {}", s.label);
                assert!(sk.pose_sequence(seq, 0.5, false).iter().all(|p| p.is_finite()));
            }
        }
        let mut variants = std::collections::HashSet::new();
        for seed in 0..256 {
            variants.insert(sk.sequences()[select(&sk, Context::default(), seed).unwrap()].label.clone());
        }
        assert!(variants.len() > 3);
        assert!(variants.iter().any(|l| l.starts_with("Headshot")), "{variants:?}");
    }
}
