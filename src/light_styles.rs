//! Switchable light styles. Source bakes a named `light`/`light_spot` into
//! its own lightmap style (32+, `dface_t::styles`) and the engine scales that
//! layer at runtime: on/off from `spawnflags` 1 ("Initially dark") and the
//! TurnOn/TurnOff/Toggle inputs. The rewrite baked only slot 0, so switchable
//! lights were frozen (Hotel fire13-16 and elevator lights, highway/waterfront
//! police taillights).
//!
//! Styled faces keep every slot's flat lightmap (bsp::StyledFace); when a
//! style value changes their atlas texels are rebuilt as the sum of the slots
//! times their values. Not reproduced: animated pattern styles 1-12 and the
//! `pattern` key (none on the checked c1/c2/c5 maps), bump-direction style
//! maps, and lights' dynamic model lighting contribution.
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Resource)]
pub struct Lightmap(pub Handle<Image>);

#[derive(Default)]
struct State {
    map: usize,
    values: Vec<f32>,
    applied: Vec<f32>,
    /// Lower-case targetname -> switchable styles.
    named: HashMap<String, Vec<u8>>,
}

pub fn install(app: &mut App) {
    app.add_systems(Update, update);
}

/// Style values at map start: 1 for every style, 0 for switchable styles of
/// lights flagged "Initially dark".
pub(crate) fn initial(entities: &[std::collections::BTreeMap<String, String>]) -> (Vec<f32>, HashMap<String, Vec<u8>>) {
    let mut values = vec![1.; 64];
    let mut named: HashMap<String, Vec<u8>> = HashMap::new();
    for e in entities {
        let Some(class) = e.get("classname") else { continue };
        if !matches!(class.as_str(), "light" | "light_spot" | "light_glspot") {
            continue;
        }
        let Some(style) = e.get("style").and_then(|v| v.trim().parse::<u8>().ok()).filter(|s| (32..64).contains(s)) else {
            continue;
        };
        let dark = e.get("spawnflags").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0) & 1 != 0;
        values[style as usize] = if dark { 0. } else { 1. };
        if let Some(name) = e.get("targetname").filter(|n| !n.is_empty()) {
            named.entry(name.to_ascii_lowercase()).or_default().push(style);
        }
    }
    (values, named)
}

/// The texel sum for one styled face texel.
pub(crate) fn texel(face: &crate::bsp::StyledFace, index: usize, values: &[f32]) -> [f32; 3] {
    let mut sum = [0.; 3];
    for (slot, map) in face.maps.iter().enumerate() {
        let style = face.styles[slot] as usize;
        let value = values.get(style).copied().unwrap_or(1.);
        if value == 0. {
            continue;
        }
        if let Some(c) = map.get(index) {
            for k in 0..3 {
                sum[k] += c[k] * value;
            }
        }
    }
    sum
}

fn update(
    map: Option<Res<crate::viewer::Map>>,
    logic: Option<ResMut<crate::l4d2r_cinematic::MapLogic>>,
    lightmap: Option<Res<Lightmap>>,
    mut images: ResMut<Assets<Image>>,
    mut state: Local<State>,
) {
    let (Some(map), Some(lightmap)) = (map, lightmap) else {
        return;
    };
    let world = &map.0.world;
    let id = world.light_styles.as_ptr() as usize ^ world.entities.len();
    if state.map != id {
        let (values, named) = initial(&world.entities);
        state.map = id;
        state.values = values;
        state.named = named;
        state.applied = vec![f32::NAN; 64];
    }
    if let Some(mut logic) = logic {
        if !logic.world.unhandled.is_empty() {
            let pending = std::mem::take(&mut logic.world.unhandled);
            for (ent, input, param) in pending {
                let styles = logic
                    .world
                    .ents
                    .get(ent)
                    .filter(|e| e.me.classname.starts_with("light"))
                    .and_then(|e| state.named.get(&e.me.targetname.to_ascii_lowercase()).cloned());
                let action = input.to_ascii_lowercase();
                match (styles, action.as_str()) {
                    (Some(styles), "turnon" | "turnoff" | "toggle") => {
                        for style in styles {
                            let v = &mut state.values[style as usize];
                            *v = match action.as_str() {
                                "turnon" => 1.,
                                "turnoff" => 0.,
                                _ => if *v > 0. { 0. } else { 1. },
                            };
                        }
                    }
                    _ => logic.world.unhandled.push((ent, input, param)),
                }
            }
        }
    }
    if state.values == state.applied {
        return;
    }
    let changed: Vec<bool> = (0..64)
        .map(|i| state.values[i] != state.applied[i] || state.applied[i].is_nan())
        .collect();
    let Some(mut image) = images.get_mut(&lightmap.0) else {
        return;
    };
    let width = image.texture_descriptor.size.width as usize;
    let Some(data) = image.data.as_mut() else {
        return;
    };
    let mut written = 0usize;
    for face in &world.light_styles {
        if !face.styles.iter().any(|&s| s != 255 && changed[s as usize]) {
            continue;
        }
        let [x, y, w, h] = face.tile;
        // The baked tile has a one-texel clamped border.
        for py in 0..h + 2 {
            for px in 0..w + 2 {
                let sx = px.saturating_sub(1).min(w - 1);
                let sy = py.saturating_sub(1).min(h - 1);
                let rgb = texel(face, sy * w + sx, &state.values);
                let o = ((y + py - 1) * width + x + px - 1) * 8;
                if let Some(dst) = data.get_mut(o..o + 6) {
                    for k in 0..3 {
                        dst[k * 2..k * 2 + 2].copy_from_slice(&half::f16::from_f32(rgb[k]).to_le_bytes());
                    }
                    written += 1;
                }
            }
        }
    }
    let _ = written;
    state.applied = state.values.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    #[test]
    fn named_lights_start_from_their_dark_flag_and_sum_by_style() {
        let (values, named) = initial(&[
            entity(&[("classname", "light"), ("targetname", "fire13_light"), ("style", "32"), ("spawnflags", "1")]),
            entity(&[("classname", "light_spot"), ("targetname", "elevator_spot"), ("style", "36"), ("spawnflags", "0")]),
            entity(&[("classname", "light"), ("style", "0")]),
        ]);
        assert_eq!(values[0], 1.);
        assert_eq!(values[32], 0.);
        assert_eq!(values[36], 1.);
        assert_eq!(named["fire13_light"], vec![32]);
        let face = crate::bsp::StyledFace {
            tile: [0, 0, 1, 1],
            styles: [0, 32, 36, 255],
            maps: vec![vec![[0.1, 0.1, 0.1]], vec![[0.5, 0.2, 0.0]], vec![[0.0, 0.0, 0.3]]],
        };
        let off = texel(&face, 0, &values);
        assert!((off[0] - 0.1).abs() < 1e-6 && (off[2] - 0.4).abs() < 1e-6);
        let mut lit = values.clone();
        lit[32] = 1.;
        let on = texel(&face, 0, &lit);
        assert!((on[0] - 0.6).abs() < 1e-6 && (on[1] - 0.3).abs() < 1e-6);
    }
}
#[cfg(test)]
mod hotel {
    /// Hotel's fire13_light TurnOn reaches the light-style system.
    #[test]
    fn hotel_fire_light_inputs_reach_light_styles() {
        let root = std::path::Path::new("C:/Program Files (x86)/Steam/steamapps/common/Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp");
        if !path.is_file() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &path, true).unwrap();
        let id = logic.world.find("fire13_light")[0];
        logic.world.unhandled.clear();
        logic.world.input(id, "TurnOn", "", None, None);
        assert!(
            logic.world.unhandled.iter().any(|(e, i, _)| *e == id && i.eq_ignore_ascii_case("TurnOn")),
            "unhandled: {:?}",
            logic.world.unhandled
        );
        let world = crate::bsp::load_file(&path, root).unwrap();
        let (values, named) = super::initial(&world.entities);
        let style = named["fire13_light"][0];
        assert_eq!(values[style as usize], 0., "fire13_light starts dark");
        assert!(world.light_styles.iter().any(|f| f.styles.contains(&style)));
    }
}
