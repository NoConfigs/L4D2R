//! Bounded native bullet-hole and infected-blood effects.
//!
//! Bullet holes follow retail's scripts/decals_subrect.txt: the hit
//! surface's game material picks an Impact.* group, whose subrect materials
//! cut a hole out of decals/decals_bulletsheet (LightmappedGeneric,
//! translucent: tinted by the wall's lighting) or decals/decals_mod2x
//! (DecalModulate: multiplies the wall, used by glass). The decal is
//! $Size texels x $decalscale units, varied by $decalScaleVariation, and
//! upright on walls (texture up follows world up). Sand, grass and leaves
//! map to the blank decal and get no hole.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
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
    /// Bullet holes to place: (position, surface normal, game material).
    pub hits: Vec<(Vec3, Vec3, u8)>,
    pub blood: Vec<BloodHit>,
    pub gore: Vec<crate::infected_gore::Hit>,
    live: std::collections::VecDeque<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    blood_material: Option<Handle<StandardMaterial>>,
    /// The bullet sheet atlas, its Source-lit material (made on first use,
    /// once the lighting table exists) and the multiplied mod2x material.
    sheet_image: Option<Handle<Image>>,
    sheet: Option<Handle<crate::viewer::SourceMaterial>>,
    modulate: Option<Handle<StandardMaterial>>,
    /// Game material character -> the Impact group's decals (empty: none).
    decals: std::collections::HashMap<u8, Vec<Decal>>,
    /// The "Blood" group (UTIL_BloodDecalTrace for red blood): DecalModulate
    /// splats from decals_mod2x.
    blood_decals: Vec<Decal>,
    seed: u32,
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
/// One subrect decal: its atlas UV rectangle, size in metres, size
/// variation and which atlas (mod2x multiplies instead of blending).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decal {
    pub uv: [f32; 4],
    pub size: Vec2,
    pub variation: f32,
    pub modulate: bool,
}
/// The Impact groups of decals_subrect.txt by game material, resolving each
/// subrect material with `vmt` and each atlas's texel size with `atlas`.
/// Materials whose group has only zero-sized (blank) decals map to none.
pub fn impact_table(
    script: &str,
    mut vmt: impl FnMut(&str) -> Option<String>,
    mut atlas: impl FnMut(&str) -> Option<(u32, u32)>,
) -> std::collections::HashMap<u8, Vec<Decal>> {
    let root = crate::kv::blocks(script);
    let mut table = std::collections::HashMap::new();
    let Some(translation) = root.children.get("translationdata") else { return table };
    for (key, group) in &translation.values {
        // The KeyValues reader lowercases keys; game materials are uppercase.
        let Some(code) = key.as_bytes().first().map(u8::to_ascii_uppercase) else { continue };
        table.insert(code, group_decals(&root, group, &mut vmt, &mut atlas));
    }
    table
}
/// One named decal group of decals_subrect.txt (e.g. "Blood").
pub fn named_group(
    script: &str,
    group: &str,
    mut vmt: impl FnMut(&str) -> Option<String>,
    mut atlas: impl FnMut(&str) -> Option<(u32, u32)>,
) -> Vec<Decal> {
    group_decals(&crate::kv::blocks(script), group, &mut vmt, &mut atlas)
}
fn group_decals(
    root: &crate::kv::Block,
    group: &str,
    vmt: &mut dyn FnMut(&str) -> Option<String>,
    atlas: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> Vec<Decal> {
    let mut decals = vec![];
    let Some(entries) = root.children.get(&group.to_ascii_lowercase()) else { return decals };
    for name in entries.values.keys() {
        let Some(text) = vmt(name) else { continue };
        let block = crate::kv::blocks(&text);
        let Some(body) = block.children.values().next() else { continue };
        let get = |k: &str| body.values.get(k).map(|v| v.trim().to_string());
        let pair = |k: &str| -> Option<Vec2> {
            let v: Vec<f32> = get(k)?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 2).then(|| Vec2::new(v[0], v[1]))
        };
        let (Some(material), Some(pos), Some(size)) = (get("$material"), pair("$pos"), pair("$size")) else {
            continue;
        };
        if size.x <= 0. || size.y <= 0. {
            continue;
        }
        let material = material.replace('\\', "/").to_ascii_lowercase();
        let Some((w, h)) = atlas(&material) else { continue };
        let scale: f32 = get("$decalscale").and_then(|v| v.parse().ok()).unwrap_or(1.);
        let variation = get("$decalscalevariation").and_then(|v| v.parse().ok()).unwrap_or(0.);
        decals.push(Decal {
            uv: [pos.x / w as f32, pos.y / h as f32, (pos.x + size.x) / w as f32, (pos.y + size.y) / h as f32],
            size: size * scale * crate::collision::UNIT,
            variation,
            modulate: material.ends_with("mod2x"),
        });
    }
    decals
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
        let mut sizes = std::collections::HashMap::new();
        for (name, modulate) in [("decals/decals_bulletsheet", false), ("decals/decals_mod2x", true)] {
            let Some(mut t) = assets
                .read(&format!("materials/{name}.vtf"))
                .ok()
                .and_then(|b| crate::vtf::decode(&b, 4096).ok())
            else {
                continue;
            };
            sizes.insert(name.to_string(), (t.width, t.height));
            if modulate {
                // DecalModulate: framebuffer x 2 x lerp(0.5, texel, alpha);
                // the multiply blend can only darken, so brightening clamps.
                for px in t.rgba.chunks_exact_mut(4) {
                    let a = px[3] as f32 / 255.;
                    for c in &mut px[..3] {
                        let v = 0.5 + (*c as f32 / 255. - 0.5) * a;
                        *c = (v * 2. * 255.).round().min(255.) as u8;
                    }
                    px[3] = 255;
                }
            }
            let image = images.add(Image::new(
                Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
                TextureDimension::D2,
                t.rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            ));
            if modulate {
                data.modulate = Some(materials.add(StandardMaterial {
                    base_color_texture: Some(image),
                    unlit: true,
                    alpha_mode: AlphaMode::Multiply,
                    cull_mode: None,
                    ..default()
                }));
            } else {
                data.sheet_image = Some(image);
            }
        }
        if let Some(script) = assets
            .read("scripts/decals_subrect.txt")
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
        {
            let mut vmt = |name: &str| {
                let path = format!("materials/{}.vmt", name.replace('\\', "/").to_ascii_lowercase());
                assets.read(&path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
            };
            data.decals = impact_table(&script, &mut vmt, |material| sizes.get(material).copied());
            data.blood_decals = named_group(&script, "Blood", &mut vmt, |material| sizes.get(material).copied());
        }
        if data.decals.values().all(|d| d.is_empty()) {
            eprintln!("Bullet decals: scripts/decals_subrect.txt gave no impact decals");
        }
    }
    let mesh = meshes.add(Plane3d::default().mesh().size(1., 1.));
    // Let map render warmup prepare decal variants before the first shot.
    // These zero-sized draws stay hidden during normal play.
    for material in [data.material.as_ref(), data.blood_material.as_ref(), data.modulate.as_ref()]
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
/// Baked light on a surface at `position` facing `normal` (renderer space):
/// the ambient cube along the normal plus the visible sun, the same vertex
/// lighting static props get (the shader applies exposure).
fn surface_light(
    world: Option<&crate::bsp::World>,
    shadows: Option<&crate::lighting::WorldShadow>,
    position: Vec3,
    normal: Vec3,
) -> Vec3 {
    let (Some(world), Some(shadows)) = (world, shadows) else { return Vec3::ONE };
    let probe = position + normal * 0.05;
    let cube = crate::viewer::model_ambient(world, shadows, probe);
    let lit = crate::lighting::ambient(&cube, [normal.x, -normal.z, normal.y]);
    let mut light = Vec3::new(lit[0], lit[1], lit[2]);
    if let Some(sun) = world.suns.first() {
        let ray = Vec3::from_array(sun.direction).normalize_or_zero();
        let facing = normal.dot(ray);
        if facing > 0. && !shadows.blocked(probe.to_array(), ray.to_array()) {
            light += Vec3::from_array(sun.intensity) * facing;
        }
    }
    light
}
/// A bullet hole quad centred on the origin in the XZ plane (+Y out of the
/// wall), texture up along -Z, tinted by `light`.
fn hole_mesh(decal: &Decal, size: Vec2, light: Vec3) -> Mesh {
    let (hx, hz) = (size.x * 0.5, size.y * 0.5);
    let [u0, v0, u1, v1] = decal.uv;
    let color = [light.x, light.y, light.z, 1.];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[-hx, 0., -hz], [hx, 0., -hz], [hx, 0., hz], [-hx, 0., hz]],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 1., 0.]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![color; 4])
        .with_inserted_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]))
}
/// Orientation of a hole: +Y along the normal, texture up (-Z) toward world
/// up on walls; floors and ceilings fall back to +X.
fn hole_rotation(normal: Vec3) -> Quat {
    let n = normal.normalize();
    let mut up = (Vec3::Y - n * n.dot(Vec3::Y)).normalize_or_zero();
    if n.y.abs() > 0.99 || up == Vec3::ZERO {
        up = (Vec3::X - n * n.dot(Vec3::X)).normalize_or_zero();
    }
    let right = up.cross(n);
    Quat::from_mat3(&Mat3::from_cols(right, n, -up))
}
#[allow(clippy::too_many_arguments)]
fn bullet_hole(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lit: &mut Option<(ResMut<Assets<crate::viewer::SourceMaterial>>, Res<crate::viewer::LightingTable>)>,
    data: &mut Impacts,
    world: Option<&crate::bsp::World>,
    shadows: Option<&crate::lighting::WorldShadow>,
    position: Vec3,
    normal: Vec3,
    material: u8,
) {
    if !position.is_finite() || !normal.is_finite() || normal.length_squared() < 0.5 {
        return;
    }
    if data.decals.is_empty() {
        // No retail decal data (missing game files): the single fallback hole.
        mark(commands, data, position, normal, 0.12, false);
        return;
    }
    // Material 0: no world surface was hit (a static prop). Retail projects
    // a model decal onto the prop's triangles; a flat world quad would hang
    // across fence gaps and curved props, so none is placed.
    if material == 0 {
        return;
    }
    // Surfaces without a translation use the concrete group, like the
    // default surface; translated blank groups (sand, grass) get nothing.
    let choices = match data.decals.get(&material) {
        Some(found) => found.clone(),
        None => data.decals.get(&b'C').cloned().unwrap_or_default(),
    };
    if choices.is_empty() {
        return;
    }
    let (pick, size) = pick(data, &choices);
    let normal = normal.normalize();
    let transform = Transform::from_translation(position + normal * 0.003).with_rotation(hole_rotation(normal));
    let entity = if pick.modulate {
        // DecalModulate takes its light from the wall it multiplies.
        let Some(handle) = data.modulate.clone() else { return };
        commands
            .spawn((Mesh3d(meshes.add(hole_mesh(&pick, size, Vec3::ONE))), MeshMaterial3d(handle), transform, bevy::light::NotShadowCaster))
            .id()
    } else {
        if data.sheet.is_none() {
            if let (Some(image), Some((materials, table))) = (data.sheet_image.clone(), lit.as_mut()) {
                data.sheet = Some(materials.add(crate::viewer::vertex_lit_material(
                    StandardMaterial {
                        base_color_texture: Some(image),
                        alpha_mode: AlphaMode::Blend,
                        cull_mode: None,
                        ..default()
                    },
                    table,
                )));
            }
        }
        let Some(handle) = data.sheet.clone() else { return };
        let light = surface_light(world, shadows, position, normal);
        commands
            .spawn((Mesh3d(meshes.add(hole_mesh(&pick, size, light))), MeshMaterial3d(handle), transform, bevy::light::NotShadowCaster))
            .id()
    };
    data.live.push_back(entity);
    if data.live.len() > 256 {
        if let Some(old) = data.live.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
    data.bullet_marks += 1;
}
/// Picks one of `choices` with retail's size variation.
fn pick(data: &mut Impacts, choices: &[Decal]) -> (Decal, Vec2) {
    data.seed = data.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let pick = choices[(data.seed >> 8) as usize % choices.len()];
    data.seed = data.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let r = ((data.seed >> 8) & 0xffff) as f32 / 65535. * 2. - 1.;
    (pick, pick.size * (1. + pick.variation * r))
}
/// A blood splat from the retail "Blood" group: DecalModulate, so the wall
/// keeps its own light and the pale texels around the blood leave it as is.
fn blood_splat(commands: &mut Commands, meshes: &mut Assets<Mesh>, data: &mut Impacts, position: Vec3, normal: Vec3, shotgun: bool) {
    if !position.is_finite() || !normal.is_finite() || normal.length_squared() < 0.5 {
        return;
    }
    // Low Violence: no blood decals (client blood effects 100f1940/1022f380).
    if crate::low_violence::on() {
        return;
    }
    let (Some(handle), false) = (data.modulate.clone(), data.blood_decals.is_empty()) else {
        // No retail decal data (missing game files): the single fallback splat.
        mark(commands, data, position, normal, if shotgun { 0.6 } else { 0.4 }, true);
        return;
    };
    let choices = data.blood_decals.clone();
    let (decal, size) = pick(data, &choices);
    let normal = normal.normalize();
    let entity = commands
        .spawn((
            Mesh3d(meshes.add(hole_mesh(&decal, size, Vec3::ONE))),
            MeshMaterial3d(handle),
            Transform::from_translation(position + normal * 0.002).with_rotation(hole_rotation(normal)),
            bevy::light::NotShadowCaster,
        ))
        .id();
    data.live.push_back(entity);
    if data.live.len() > 256 {
        if let Some(old) = data.live.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
    data.blood_marks += 1;
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
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    map: Option<Res<crate::viewer::Map>>,
    shadows: Option<Res<crate::lighting::WorldShadow>>,
    lit: (Option<ResMut<Assets<crate::viewer::SourceMaterial>>>, Option<Res<crate::viewer::LightingTable>>),
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
    // L4D2R_DECAL_BLOOD: debug, also splat blood where each bullet hole goes
    // (wall blood captures without needing infected in the shot).
    if std::env::var_os("L4D2R_DECAL_BLOOD").is_some() {
        let extra: Vec<BloodHit> = impacts
            .hits
            .iter()
            .map(|(p, n, _)| BloodHit { position: *p + *n * 0.3, direction: -*n, shotgun: false })
            .collect();
        impacts.blood.extend(extra);
    }
    let world = map.as_deref().map(|m| &m.0.world);
    let mut lit = match lit {
        (Some(materials), Some(table)) => Some((materials, table)),
        _ => None,
    };
    for (position, normal, material) in std::mem::take(&mut impacts.hits).into_iter().take(32) {
        bullet_hole(&mut commands, &mut meshes, &mut lit, &mut impacts, world, shadows.as_deref(), position, normal, material);
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
            // Only movable entities veto a splat (not trigger or fog volumes).
            let moving = crate::game::decal_on_movable(Some(&logic), position);
            if !moving {
                blood_splat(&mut commands, &mut meshes, &mut impacts, position, trace.normal, hit.shotgun);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn impact_groups_pick_subrects_and_blank_surfaces_get_none() {
        let script = r#"
"TranslationData" { "-" "" "C" "Impact.Concrete" "Y" "Impact.Glass" "N" "Impact.Sand" }
"Impact.Concrete" { "decals/concrete/concrete1_subrect" "1" }
"Impact.Glass" { "decals/glass/shot1_subrect" "1" }
"Impact.Sand" { "decals/blank/shot1_subrect" "1" }
"#;
        let vmt = |name: &str| {
            Some(match name {
                "decals/concrete/concrete1_subrect" => r#"Subrect { $Material "decals\Decals_bulletsheet" $Pos "128 128" $Size "64 64" $decalscale "0.12" $decalScaleVariation "0.2" }"#,
                "decals/glass/shot1_subrect" => r#"Subrect { $Material "decals/decals_mod2x" $Pos "0 128" $Size "128 128" $decalscale 0.11 }"#,
                "decals/blank/shot1_subrect" => r#"Subrect { $Material "decals/decals_mod2x" $Pos "0 0" $Size "0 0" $decalscale 0.40 }"#,
                _ => return None,
            }.to_string())
        };
        let table = impact_table(script, vmt, |m| match m {
            "decals/decals_bulletsheet" => Some((512, 512)),
            "decals/decals_mod2x" => Some((256, 256)),
            _ => None,
        });
        let concrete = table[&b'C'][0];
        assert_eq!(concrete.uv, [0.25, 0.25, 0.375, 0.375]);
        assert!((concrete.size.x / crate::collision::UNIT - 7.68).abs() < 1e-3);
        assert!(!concrete.modulate && (concrete.variation - 0.2).abs() < 1e-6);
        assert!(table[&b'Y'][0].modulate);
        assert!(table[&b'N'].is_empty(), "sand gets the blank decal: no hole");
        assert!(table[&b'-'].is_empty());
        let blood = named_group(
            r#""Blood" { "decals/blood1_subrect" "1" }"#,
            "Blood",
            |_| Some(r#"Subrect { $Material "decals/Decals_Mod2x" $Pos "256 0" $Size "256 256" $decalscale 0.25 }"#.into()),
            |m| (m == "decals/decals_mod2x").then_some((1024, 512)),
        );
        assert_eq!(blood.len(), 1);
        assert!(blood[0].modulate, "blood is DecalModulate: no white fringe");
        assert_eq!(blood[0].uv, [0.25, 0., 0.5, 0.5]);
        assert!((blood[0].size.x / crate::collision::UNIT - 64.).abs() < 1e-3);
    }
    #[test]
    fn holes_face_out_of_the_wall_with_texture_up_toward_the_sky() {
        let q = hole_rotation(Vec3::Z);
        assert!((q * Vec3::Y).distance(Vec3::Z) < 1e-5);
        assert!((q * Vec3::NEG_Z).distance(Vec3::Y) < 1e-5);
        let floor = hole_rotation(Vec3::Y);
        assert!((floor * Vec3::Y).distance(Vec3::Y) < 1e-5);
    }
}
