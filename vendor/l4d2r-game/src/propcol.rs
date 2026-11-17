//! Prop collision stand-ins: oriented boxes from each model's MDL hull (the real game uses the `.phy` VPhysics mesh).
//! Native PHY collision when available; authored bounding-box solids use MDL hull bounds.

use crate::mapents::{angle_matrix, is_prop_class, MapEnt};
use crate::trace::{Obb, CONTENTS_SOLID};
use glam::{Mat3, Vec3};
use l4d2_formats::bsp::Bsp;
use std::collections::HashMap;
/// Authored corpse scenery stays visible but has no collision in the rewrite.
pub fn is_baked_corpse_model(name: &str) -> bool {
    name.replace('\\', "/")
        .to_ascii_lowercase()
        .starts_with("models/deadbodies/")
}

/// `bounds(model_path)` -> (mins, maxs) of the model (hull, else view bbox).
pub fn build_obbs(
    bsp: &Bsp,
    ents: &[MapEnt],
    bounds: &mut dyn FnMut(&str) -> Option<(Vec3, Vec3)>,
    phys: &mut dyn FnMut(&str) -> Option<std::sync::Arc<Vec<l4d2_formats::phy::Convex>>>,
) -> Vec<Obb> {
    let mut shapes: HashMap<String, Option<std::sync::Arc<Vec<l4d2_formats::phy::Convex>>>> =
        HashMap::new();
    let mut cache: HashMap<String, Option<(Vec3, Vec3)>> = HashMap::new();
    let mut get = |name: &str,
                   bounds: &mut dyn FnMut(&str) -> Option<(Vec3, Vec3)>|
     -> Option<(Vec3, Vec3)> {
        *cache
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| bounds(name))
    };
    let mut out = Vec::new();
    let mut push = |name: &str,
                    origin: Vec3,
                    angles: Vec3,
                    scale: f32,
                    solid: i32,
                    b: (Vec3, Vec3),
                    out: &mut Vec<Obb>| {
        if is_baked_corpse_model(name) {
            return;
        }
        let (mn, mx) = b;
        let shape = shapes
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| phys(name))
            .clone();
        let n = name.to_ascii_lowercase();
        // A render hull is not a substitute for a missing VPhysics mesh: in
        // Source a SOLID_VPHYSICS prop without a .phy is simply not solid.
        // c1m2's tanker debris, cab, trailer and cable models have none (their
        // blocking comes from clip brushes); their model bounds used to become
        // large invisible boxes across the street after the explosion.
        let _ = &n;
        if solid == 6 && shape.is_none() {
            return;
        }
        if let (Some(sh), true) = (shape, solid == 6 && (scale - 1.0).abs() < 0.01) {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for c in sh.iter() {
                lo = lo.min(c.mins);
                hi = hi.max(c.maxs);
            }
            if lo.x <= hi.x {
                out.push(Obb::with_shape(
                    origin,
                    Mat3::from_mat4(angle_matrix(angles)),
                    lo,
                    hi,
                    sh,
                    CONTENTS_SOLID,
                ));
                return;
            }
        }
        let half = (mx - mn) * 0.5 * scale;
        let local_center = (mn + mx) * 0.5 * scale;
        if half.max_element() < 5.0 {
            return; // clutter
        }
        let rot = Mat3::from_mat4(angle_matrix(angles));
        out.push(Obb::new(
            origin + rot * local_center,
            half,
            rot,
            CONTENTS_SOLID,
        ));
    };
    for sp in &bsp.static_props {
        if sp.solid == 0 {
            continue;
        }
        let Some(name) = bsp.static_prop_names.get(sp.model_idx as usize) else {
            continue;
        };
        if let Some(b) = get(name, bounds) {
            push(
                name,
                sp.origin,
                sp.angles,
                if sp.scale > 0.0 { sp.scale } else { 1.0 },
                sp.solid as i32,
                b,
                &mut out,
            );
        }
    }
    for (ei, e) in ents.iter().enumerate() {
        if !is_prop_class(&e.classname)
            || e.kv_i32("solid") == Some(0)
            || e.kv_i32("startdisabled") == Some(1)
        {
            continue;
        }
        // Doors animate and use brush colliders elsewhere; physics/dynamic props are static here.
        if e.classname.starts_with("prop_door_rotating") || e.classname.starts_with("prop_physics")
        {
            continue;
        }
        let Some(name) = e.mdl.as_deref() else {
            continue;
        };
        if let Some(b) = get(name, bounds) {
            let n0 = out.len();
            push(
                name,
                e.origin,
                e.angles,
                e.kv_f32("modelscale").unwrap_or(1.0),
                e.kv_i32("solid").unwrap_or(6),
                b,
                &mut out,
            );
            for o in &mut out[n0..] {
                o.owner = ei;
            }
        }
    }
    out
}
