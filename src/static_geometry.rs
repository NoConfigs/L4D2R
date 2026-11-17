//! Lossless static geometry packing. Never weld across attribute seams.
use bevy::{
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
};
use std::collections::HashMap;

pub fn index(mesh: &mut Mesh) -> (usize, usize) {
    let count = mesh.count_vertices();
    if count == 0 || mesh.indices().is_some() {
        return (count, count);
    }
    if mesh.attributes().any(|(_, v)| {
        !matches!(
            v,
            VertexAttributeValues::Float32(_)
                | VertexAttributeValues::Float32x2(_)
                | VertexAttributeValues::Float32x3(_)
                | VertexAttributeValues::Float32x4(_)
        )
    }) {
        return (count, count);
    }
    let mut unique = HashMap::<Vec<u8>, u32>::new();
    let mut originals = Vec::new();
    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let mut key = Vec::new();
        for (_, v) in mesh.attributes() {
            let bytes = v.get_bytes();
            let stride = bytes.len() / count;
            key.extend_from_slice(&bytes[i * stride..(i + 1) * stride]);
        }
        let next = originals.len() as u32;
        let id = *unique.entry(key).or_insert_with(|| {
            originals.push(i);
            next
        });
        indices.push(id);
    }
    if originals.len() == count {
        return (count, count);
    }
    // The keys are no longer needed. Release them before allocating packed
    // attributes, and move each old buffer instead of cloning the entire mesh.
    drop(unique);
    let attributes = mesh.attributes().map(|(a, _)| *a).collect::<Vec<_>>();
    for attribute in attributes {
        let value = mesh.remove_attribute(attribute.id).unwrap();
        let packed = match value {
            VertexAttributeValues::Float32(v) => {
                VertexAttributeValues::Float32(originals.iter().map(|i| v[*i]).collect())
            }
            VertexAttributeValues::Float32x2(v) => {
                VertexAttributeValues::Float32x2(originals.iter().map(|i| v[*i]).collect())
            }
            VertexAttributeValues::Float32x3(v) => {
                VertexAttributeValues::Float32x3(originals.iter().map(|i| v[*i]).collect())
            }
            VertexAttributeValues::Float32x4(v) => {
                VertexAttributeValues::Float32x4(originals.iter().map(|i| v[*i]).collect())
            }
            _ => unreachable!(),
        };
        mesh.insert_attribute(attribute, packed);
    }
    mesh.insert_indices(Indices::U32(indices));
    (count, originals.len())
}

/// Append rigid static meshes after baking their world transform. Attribute
/// layouts must agree; animated meshes and ordered transparency stay separate.
pub fn append(target: &mut Mesh, source: &Mesh, transform: &Transform) -> bool {
    if source.indices().is_some()
        || target.indices().is_some()
        || transform.scale != Vec3::ONE
        || source.primitive_topology() != target.primitive_topology()
    {
        return false;
    }
    let attributes = source
        .attributes()
        .map(|(a, v)| (*a, v.clone()))
        .collect::<Vec<_>>();
    if !target
        .attributes()
        .map(|(a, _)| a.id)
        .eq(attributes.iter().map(|(a, _)| a.id))
    {
        return false;
    }
    if attributes.iter().any(|(_, v)| {
        !matches!(
            v,
            VertexAttributeValues::Float32(_)
                | VertexAttributeValues::Float32x2(_)
                | VertexAttributeValues::Float32x3(_)
                | VertexAttributeValues::Float32x4(_)
        )
    }) {
        return false;
    }
    // Validate the entire layout before changing any buffer. A partial append
    // would leave mismatched attribute lengths and corrupt the batch.
    let target_vertices = target.count_vertices();
    if attributes.iter().any(|(a, v)| {
        let existing = target.attribute(a.id).unwrap();
        std::mem::discriminant(existing) != std::mem::discriminant(v)
            || v.len() != source.count_vertices()
            || existing.len() != target_vertices
    }) {
        return false;
    }
    for (attribute, mut value) in attributes {
        if let VertexAttributeValues::Float32x3(values) = &mut value {
            if attribute.id == Mesh::ATTRIBUTE_POSITION.id {
                for p in values {
                    *p = transform.transform_point(Vec3::from_array(*p)).to_array();
                }
            } else if attribute.id == Mesh::ATTRIBUTE_NORMAL.id {
                for n in values {
                    *n = (transform.rotation * Vec3::from_array(*n)).to_array();
                }
            }
        }
        if attribute.id == Mesh::ATTRIBUTE_TANGENT.id {
            if let VertexAttributeValues::Float32x4(values) = &mut value {
                for t in values {
                    let xyz = transform.rotation * Vec3::new(t[0], t[1], t[2]);
                    *t = xyz.extend(t[3]).to_array();
                }
            }
        }
        match (target.attribute_mut(attribute.id).unwrap(), value) {
            (VertexAttributeValues::Float32(a), VertexAttributeValues::Float32(b)) => a.extend(b),
            (VertexAttributeValues::Float32x2(a), VertexAttributeValues::Float32x2(b)) => {
                a.extend(b)
            }
            (VertexAttributeValues::Float32x3(a), VertexAttributeValues::Float32x3(b)) => {
                a.extend(b)
            }
            (VertexAttributeValues::Float32x4(a), VertexAttributeValues::Float32x4(b)) => {
                a.extend(b)
            }
            _ => unreachable!(),
        }
    }
    true
}

pub fn empty_like(source: &Mesh) -> Mesh {
    let mut mesh = source.clone();
    for (a, v) in source.attributes() {
        let empty = match v {
            VertexAttributeValues::Float32(_) => VertexAttributeValues::Float32(vec![]),
            VertexAttributeValues::Float32x2(_) => VertexAttributeValues::Float32x2(vec![]),
            VertexAttributeValues::Float32x3(_) => VertexAttributeValues::Float32x3(vec![]),
            VertexAttributeValues::Float32x4(_) => VertexAttributeValues::Float32x4(vec![]),
            _ => return source.clone(),
        };
        mesh.insert_attribute(*a, empty);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incompatible_batch_is_rejected_without_partial_writes() {
        let mut source = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        source.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0., 0., 0.]; 3]);
        source.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1., 1., 1., 1.]; 3]);
        let mut target = empty_like(&source);
        assert!(!append(
            &mut target,
            &source,
            &Transform::from_scale(Vec3::splat(2.))
        ));
        assert_eq!(target.count_vertices(), 0);
        // Reject malformed input before even the position buffer is appended.
        source.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1., 1., 1., 1.]; 2]);
        assert!(!append(&mut target, &source, &Transform::default()));
        assert!(target.attributes().all(|(_, v)| v.is_empty()));
    }
    #[test]
    fn indexing_preserves_every_triangle_attribute_and_uv_lighting_seams() {
        let mut m = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        m.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0., 0., 0.],
                [1., 0., 0.],
                [0., 1., 0.],
                [0., 0., 0.],
                [0., 1., 0.],
                [1., 1., 0.],
            ],
        );
        m.insert_attribute(
            Mesh::ATTRIBUTE_UV_1,
            vec![[0., 0.], [1., 0.], [0., 1.], [0., 0.], [0.5, 0.5], [1., 1.]],
        );
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1., 0., 0., 1.]; 6]);
        let original = m.clone();
        let (before, after) = index(&mut m);
        assert_eq!((before, after), (6, 5));
        let ids = m.indices().unwrap().iter().collect::<Vec<_>>();
        for (a, v) in original.attributes() {
            let old = v.get_bytes();
            let packed = m.attribute(a.id).unwrap().get_bytes();
            let stride = old.len() / before;
            for (i, id) in ids.iter().enumerate() {
                assert_eq!(
                    &old[i * stride..(i + 1) * stride],
                    &packed[id * stride..(id + 1) * stride]
                );
            }
        }
    }
    #[test]
    fn batching_bakes_positions_normals_and_keeps_light_values() {
        let mut m = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[1., 0., 0.]; 3]);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[1., 0., 0.]; 3]);
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.2, 0.3, 0.4, 1.]; 3]);
        let mut b = empty_like(&m);
        let t = Transform::from_translation(Vec3::Y * 3.).with_rotation(Quat::from_rotation_y(1.));
        assert!(append(&mut b, &m, &t));
        if let VertexAttributeValues::Float32x3(p) = b.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        {
            assert!(Vec3::from_array(p[0]).distance(t.transform_point(Vec3::X)) < 1e-6);
        } else {
            panic!()
        };
        assert_eq!(
            b.attribute(Mesh::ATTRIBUTE_COLOR),
            m.attribute(Mesh::ATTRIBUTE_COLOR)
        );
    }
}
