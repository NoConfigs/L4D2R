use crate::{binary::*, kv, source_assets::SourceAssets};
use std::{collections::BTreeMap, io};
#[derive(Clone)]
pub struct Material {
    pub base: Option<String>,
    /// WorldVertexTransition `$basetexture2`, blended in by displacement
    /// vertex alpha.
    pub base2: Option<String>,
    /// WorldVertexTransition `$blendmodulatetexture`: per texel, its green
    /// and red channels turn the painted vertex blend into
    /// smoothstep(g - r, g + r, alpha) (Source lightmappedgeneric
    /// BLENDMODULATETEXTURE), giving hard-edged grime and dirt patches.
    pub blend_modulate: Option<String>,
    pub normal: Option<String>,
    /// LightmappedGeneric/WorldVertexTransition `$bumpmap` (a normal map or,
    /// with `$ssbump`, basis weights) for the bumped-lightmap path; kept
    /// apart from `normal`, which Bevy's normal mapping would also use.
    pub bump: Option<String>,
    pub palette: Option<(String, usize, usize)>,
    pub color: [f32; 4],
    pub alpha: u8,
    pub cutoff: f32,
    /// Valve's fence render state (AlphaTest + Translucent with
    /// $allowfencerenderstatehack, and translucent chainlink): texels under
    /// `cutoff` are discarded and depth is written, but the survivors are
    /// alpha blended, so wire edges stay see-through. A hard cutout drew
    /// c1m2's fence002 wires thick and near-black (retail comparison 19 Nov).
    pub fence: bool,
    /// `$halflambert`: direct light wraps around as (N.L x 0.5 + 0.5)^2
    /// (view-model weapons, many props), so surfaces facing away from the
    /// sun still catch some of it.
    pub half_lambert: bool,
    /// `$ssbump`: the bump map is a self-shadowing basis-weight map, not a
    /// normal map.
    pub ssbump: bool,
    pub nocull: bool,
    pub unlit: bool,
    pub uv: [f32; 7],
    pub shader: String,
    pub hdr_compressed: bool,
    pub emission: Option<String>,
    pub emission_mask: Option<String>,
    pub emission_tint: [f32;3],
    pub water: Option<WaterSettings>,
    pub wound: Option<WoundSettings>,
    pub infected_phong: Option<InfectedPhongSettings>,
    pub infected_detail: Option<String>,
    pub infected_burn: Option<String>,
    pub cap_phong: Option<CapPhongSettings>,
    /// `Proxies { PlayerTeam { team N } }`: drawn only for the local
    /// player's team N (alpha = 1 on that team, 0 otherwise), e.g.
    /// tools/climb_versus, the infected ladder claw marks (team 3).
    pub player_team: Option<u8>,
    /// `$envmap` cubemap reflection (LightmappedGeneric, VertexLitGeneric,
    /// WorldVertexTransition).
    pub envmap: Option<EnvmapSettings>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EnvmapSettings {
    /// "env_cubemap" (nearest capture) or a named capture,
    /// "maps/<map>/c<x>_<y>_<z>" (lower case).
    pub name: String,
    pub tint: [f32; 3],
    pub saturation: f32,
    pub contrast: f32,
    /// $basealphaenvmapmask / $normalmapalphaenvmapmask.
    pub base_alpha_mask: bool,
    pub normal_alpha_mask: bool,
}
impl EnvmapSettings {
    fn parse(shader: &str, v: &BTreeMap<String, String>) -> Option<Self> {
        // World surfaces only. Model envmaps (VertexLitGeneric) are masked
        // and dimmed by $envmapmask, normal-map alpha and the Phong path in
        // retail; drawn unmasked they turned props and guns gold/chrome.
        // L4D2R_NO_ENVMAP: debug, world envmaps off.
        if !matches!(shader, "lightmappedgeneric" | "worldvertextransition") || std::env::var_os("L4D2R_NO_ENVMAP").is_some() {
            return None;
        }
        let name = v.get("$envmap")?.trim().to_ascii_lowercase().replace('\\', "/");
        if name.is_empty() || name == "0" {
            return None;
        }
        // Masked by a separate texture: not drawn yet (no binding for it).
        if v.contains_key("$envmapmask") {
            return None;
        }
        let number = |k: &str, default: f32| v.get(k).and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default);
        let mut tint = [1.; 3];
        if let Some(t) = v.get("$envmaptint") {
            let scale = if t.contains('{') { 1. / 255. } else { 1. };
            let parts: Vec<f32> = t.trim_matches(['[', ']', '{', '}', ' ']).split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if parts.len() >= 3 {
                tint = [parts[0] * scale, parts[1] * scale, parts[2] * scale];
            } else if let Some(&x) = parts.first() {
                tint = [x * scale; 3];
            }
        }
        let flag = |k: &str| number(k, 0.) != 0.;
        Some(Self {
            name: name.trim_end_matches(".vtf").trim_start_matches("materials/").to_string(),
            tint,
            saturation: number("$envmapsaturation", 1.),
            contrast: number("$envmapcontrast", 0.),
            base_alpha_mask: flag("$basealphaenvmapmask"),
            normal_alpha_mask: flag("$normalmapalphaenvmapmask"),
        })
    }
}
/// Narrow native Phong path for installed wound interiors and gibs. Materials
/// with exponent/warp/environment textures need their own shader permutations.
#[derive(Clone, Debug)]
pub struct CapPhongSettings {
    pub params: [f32; 4], // explicit exponent, diffuse exponent, boost, enabled
    pub fresnel: [f32; 4],
    /// base alpha mask, base luminance mask, half-Lambert, exponent texture
    pub controls: [f32; 4],
    /// $phongtint RGB; w: $phongalbedotint
    pub tint: [f32; 4],
    /// $phongexponenttexture (linear): R exponent (1 + 149 R), G albedo-tint
    /// amount. Bound in the bump-map slot, which models do not use.
    pub exponent_texture: Option<String>,
    /// `$lightwarptexture`: the diffuse term is 2 x this ramp at the
    /// half-Lambert N.L * 0.5 + 0.5, read linear (the survivor head ramps end
    /// at 127: 2 x 127/255 = Lambert's 1 at full light). Bound in the
    /// emission slot (fresnel.w).
    pub lightwarp: Option<String>,
}
impl CapPhongSettings {
    fn parse(name: &str, shader: &str, v: &BTreeMap<String, String>) -> Option<Self> {
        // Any VertexLitGeneric $phong material in the reconstructed permutation
        // (wound caps and gibs first; view-model weapons and survivors use the
        // same phong_ps30 path).
        let _ = name;
        if shader != "vertexlitgeneric"
            || !v.get("$phong").is_some_and(|s| s == "1") { return None; }
        // Do not quietly treat unimplemented permutations as the ordinary cap
        // shader. The shipped cap/gib materials provide numeric exponents.
        // $envmap is accepted but not drawn on models (EnvmapSettings is
        // world-only); the exponent texture and albedo tint are the skin
        // shader's: exponent 1 + 149 R, tint lerp(1, albedo, G).
        if ["$phongwarptexture", "$detail"]
            .iter().any(|key| v.contains_key(*key))
            || ["$rimlight", "$shinyblood"].iter().any(|key| v.get(*key).is_some_and(|s| s == "1")) { return None; }
        let exponent_texture = texture_path(v.get("$phongexponenttexture"));
        // An explicit $phongexponent overrides the texture's.
        let exponent = match v.get("$phongexponent").and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite() && *v > 0.) {
            Some(e) => e,
            None if exponent_texture.is_some() => -1.,
            None => return None,
        };
        let number = |key: &str, fallback: f32| v.get(key).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback);
        let vector = |key: &str, mut fallback: [f32; 3]| {
            if let Some(s) = v.get(key) {
                for (out, value) in fallback.iter_mut().zip(s.trim_matches(['[', ']']).split_whitespace()) {
                    *out = value.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(*out);
                }
            }
            fallback
        };
        let f = vector("$phongfresnelranges", [0., 0.5, 1.]);
        let tint = vector("$phongtint", [1.; 3]);
        // A light warp needs the emission slot; self-illuminated materials
        // keep the old path.
        // L4D2R_NO_LIGHTWARP: debug, plain Phong diffuse on light-warp heads.
        let lightwarp = texture_path(v.get("$lightwarptexture")).filter(|_| std::env::var_os("L4D2R_NO_LIGHTWARP").is_none());
        if lightwarp.is_some() && v.get("$selfillum").is_some_and(|s| s == "1") {
            return None;
        }
        Some(Self { params: [exponent, number("$diffuseexp", 1.).max(0.0001), number("$phongboost", 1.).max(0.), 1.],
            fresnel: [f[0], f[1], f[2], if lightwarp.is_some() { 1. } else { 0. }],
            controls: [
                number("$basemapalphaphongmask", 0.).clamp(0., 1.),
                number("$basemapluminancephongmask", 0.).clamp(0., 1.),
                // z: $halflambert in the phong diffuse.
                if v.get("$halflambert").is_some_and(|s| s == "1") { 1. } else { 0. },
                // w: exponent texture bound (bump-map slot).
                if exponent_texture.is_some() { 1. } else { 0. },
            ],
            tint: [tint[0], tint[1], tint[2], if v.get("$phongalbedotint").is_some_and(|s| s == "1") { 1. } else { 0. }],
            exponent_texture, lightwarp })
    }
}
/// Infected's authored Phong controls; these are not a PBR roughness value.
#[derive(Clone, Debug)]
pub struct InfectedPhongSettings {
    pub exponents: [f32; 4], // default, skin, blood, blood specular boost
    pub fresnel: [f32; 4], // front, middle, grazing, overall boost
    pub tint: [f32; 4], // RGB; W enables Phong
    pub detail: [f32; 4], // detail exponent, UV scale X/Y, flags (detail=1, burn=2, disable variation=4)
}
impl InfectedPhongSettings {
    fn parse(v: &BTreeMap<String, String>) -> Option<Self> {
        let enabled = v.get("$phong").is_some_and(|s| s == "1");
        let number = |key: &str, fallback: f32| v.get(key).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback);
        let vector = |key: &str, mut fallback: [f32; 3]| {
            if let Some(s) = v.get(key) {
                for (out, value) in fallback.iter_mut().zip(s.trim_matches(['[', ']']).split_whitespace()) {
                    *out = value.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(*out);
                }
            }
            fallback
        };
        let tint = vector("$phongtint", [1.; 3]);
        let fresnel = vector("$phongfresnelranges", [0., 0.5, 1.]);
        let mut scales = [4.; 2]; // Native InitParams DAT_10070e60.
        if let Some(scale) = v.get("$detailscale") {
            let values: Vec<_> = scale.trim_matches(['[', ']']).split_whitespace()
                .filter_map(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).collect();
            if let Some(x) = values.first() { scales = [*x, *values.get(1).unwrap_or(x)]; }
        }
        // Retail's static DETAILTEXTURE switch tests texture presence; its
        // Infected draw does not use the generic $detailblendfactor control.
        let flags = u8::from(v.contains_key("$detail")) + 2 * u8::from(v.contains_key("$burndetailtexture"))
            + 4 * u8::from(v.get("$disablevariation").is_some_and(|s| s == "1"));
        Some(Self {
            // Native InitParams 10017070 sets all three missing exponents to
            // DAT_10070e5c (5.0), and missing blood specular boost to 1.0.
            exponents: [number("$defaultphongexponent", 5.).max(0.), number("$skinphongexponent", 5.).max(0.),
                number("$bloodphongexponent", 5.).max(0.), number("$bloodspecboost", 1.).max(0.)],
            fresnel: [fresnel[0], fresnel[1], fresnel[2], number("$phongboost", 1.).max(0.)],
            tint: [tint[0], tint[1], tint[2], if enabled { 1. } else { 0. }],
            detail: [number("$detailphongexponent", 5.).max(0.), scales[0], scales[1], flags as f32],
        })
    }
}
/// Installed Infected VMT mask data; sampled as linear shader data.
#[derive(Clone, Debug)]
pub struct WoundSettings {
    pub texture: String,
    pub mapping_scale: f32,
    pub bias: f32,
    pub blood: [f32; 3],
    pub blood_range: [f32; 2],
    pub sheet: u32,
}
impl WoundSettings {
    fn parse(v: &BTreeMap<String, String>) -> Option<Self> {
        let texture = texture_path(v.get("$woundcutouttexture"))?;
        let number = |key: &str, fallback: f32| v.get(key).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback);
        Some(Self { texture, mapping_scale: number("$cutoutdecalmappingscale", 1.).max(0.0001),
            sheet: v.get("$sheetindex").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0).min(3),
            bias: number("$cutouttexturebias", 0.95),
            blood: { let mut rgb = [0.15, 0., 0.]; if let Some(value) = v.get("$bloodcolor") {
                let scale = if value.contains('{') { 1. / 255. } else { 1. };
                for (out, s) in rgb.iter_mut().zip(value.trim_matches(['[', ']', '{', '}']).split_whitespace()) {
                    *out = s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(*out / scale) * scale;
                }
            } rgb },
            blood_range: { let mut r = [0.1, 0.9]; if let Some(value) = v.get("$bloodmaskrange") {
                for (r, v) in r.iter_mut().zip(value.trim_matches(['[', ']']).split_whitespace()) {
                    *r = v.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(*r);
                }
            } r },
        })
    }
}
/// Authored Water parameters. Colours remain sRGB here; convert once at upload.
#[derive(Clone, Debug)]
pub struct WaterSettings {
    pub normal: Option<String>,
    pub flow: Option<String>,
    pub noise: Option<String>,
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog: bool,
    pub reflect_tint: [f32; 3],
    pub refract_tint: [f32; 3],
    /// Distortion scales, not reflection/refraction opacities.
    pub reflect_amount: f32,
    pub refract_amount: f32,
    pub reflect: bool,
    pub refract: bool,
    pub above: bool,
    pub force_cheap: bool,
    pub force_expensive: bool,
    /// `$lightmapwaterfog`: the fog colour is lit by the water surface's own
    /// lightmap (c4m1's sunlit river reads brown, not flat fog colour).
    pub lightmap_fog: bool,
    pub envmap: Option<String>,
    pub bottom: Option<String>,
    pub normal_scale: f32,
    pub world_scale: f32,
    pub interval: f32,
    pub scroll_distance: f32,
    pub strength: f32,
    pub time_scale: f32,
    pub noise_scale: f32,
    pub scroll: [[f32; 2]; 2],
}
impl WaterSettings {
    pub fn textures(&self) -> impl Iterator<Item = &String> {
        self.normal.iter().chain(self.flow.iter()).chain(self.noise.iter())
    }
    fn parse(v: &BTreeMap<String, String>) -> Self {
        let number = |k: &str, default: f32| v.get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default);
        let color = |k: &str, default: [f32; 3]| {
            let Some(s) = v.get(k) else { return default; };
            let scale = if s.contains('{') { 1. / 255. } else { 1. };
            let mut rgb = default;
            for (out, s) in rgb.iter_mut().zip(s.trim_matches(['[',']','{','}']).split_whitespace()) {
                *out = s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(*out / scale).max(0.) * scale;
            }
            rgb
        };
        let scroll = |k: &str| {
            let mut xy = [0.; 2];
            if let Some(s) = v.get(k) {
                for (out, s) in xy.iter_mut().zip(s.trim_matches(['[',']']).split_whitespace()) {
                    *out = s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.);
                }
            }
            xy
        };
        Self {
            normal: texture_path(v.get("$normalmap").or_else(|| v.get("$bumpmap"))),
            flow: texture_path(v.get("$flowmap")), noise: texture_path(v.get("$flow_noise_texture")),
            fog_color: color("$fogcolor", [0.03; 3]),
            fog_start: number("$fogstart", 0.), fog_end: number("$fogend", 1000.),
            fog: number("$fogenable", 1.) != 0.,
            reflect_tint: color("$reflecttint", [1.;3]), refract_tint: color("$refracttint", [1.;3]),
            reflect_amount: number("$reflectamount", 0.8), refract_amount: number("$refractamount", 0.),
            reflect: v.contains_key("$reflecttexture") || v.contains_key("$envmap"),
            refract: number("$refract", 1.) != 0., above: number("$abovewater", 1.) != 0.,
            force_cheap: number("$forcecheap", 0.) != 0., force_expensive: number("$forceexpensive", 0.) != 0.,
            lightmap_fog: number("$lightmapwaterfog", 0.) != 0.,
            envmap: v.get("$envmap").cloned(), bottom: v.get("$bottommaterial").cloned(),
            normal_scale: number("$flow_normaluvscale", 180.).max(0.001),
            world_scale: number("$flow_worlduvscale", 1.).max(0.001),
            interval: number("$flow_timeintervalinseconds", 0.4).max(0.001),
            scroll_distance: number("$flow_uvscrolldistance", 0.2),
            strength: number("$flow_bumpstrength", 1.), time_scale: number("$flow_timescale", 1.),
            noise_scale: number("$flow_noise_scale", 0.0006), scroll: [scroll("$scroll1"), scroll("$scroll2")],
        }
    }
}
pub fn surface_property(assets: &mut SourceAssets, material: &str) -> String {
    values(assets, &format!("materials/{material}.vmt"), 0)
        .ok()
        .and_then(|(_, v)| v.get("$surfaceprop").cloned())
        .unwrap_or_else(|| "default".into())
        .to_lowercase()
}
fn values(
    assets: &mut SourceAssets,
    path: &str,
    depth: usize,
) -> io::Result<(String, BTreeMap<String, String>)> {
    if depth > 8 {
        return Err(invalid("Cyclic/deep VMT patch include"));
    }
    let bytes = assets.read(path)?;
    let t = kv::tokens(&String::from_utf8_lossy(&bytes));
    let shader = t
        .first()
        .cloned()
        .ok_or_else(|| invalid("Empty material"))?
        .to_lowercase();
    let mut map = BTreeMap::new();
    let mut inserts = BTreeMap::new();
    let mut replaces = BTreeMap::new();
    let mut section = String::new();
    let mut subsection = String::new();
    let mut nesting = 0;
    let mut i = 1;
    while i < t.len() {
        if t[i] == "{" {
            nesting += 1;
            i += 1;
            continue;
        }
        if t[i] == "}" {
            if nesting == 2 {
                section.clear();
            }
            if nesting == 3 {
                subsection.clear();
            }
            nesting -= 1;
            i += 1;
            continue;
        }
        if i + 1 < t.len() && t[i + 1] != "{" && t[i + 1] != "}" {
            if nesting == 1 {
                map.insert(t[i].to_lowercase(), t[i + 1].clone());
            } else if nesting == 3
                && section == "proxies"
                && subsection == "playerteam"
                && t[i].eq_ignore_ascii_case("team")
            {
                map.insert("%playerteam".into(), t[i + 1].clone());
            } else if shader == "patch" && nesting == 2 {
                match section.as_str() {
                    "insert" => {
                        inserts.insert(t[i].to_lowercase(), t[i + 1].clone());
                    }
                    "replace" => {
                        replaces.insert(t[i].to_lowercase(), t[i + 1].clone());
                    }
                    _ => {}
                }
            } else if shader == "shatteredglass" && nesting == 2 && section == "shatteredglass_hdr_dx9" {
                // The renderer uses HDR world lighting. Shipped glass can omit
                // the LDR VTF entirely, so select its authored HDR block rather
                // than silently retaining an unavailable base texture.
                map.insert(t[i].to_lowercase(), t[i + 1].clone());
            } else if shader == "water" && nesting == 2 && matches!(section.as_str(), ">=dx90" | ">=dx90_20b" | ">=dx90_30") {
                // These supported DX9 overrides supply e.g. urban river fog.
                map.insert(t[i].to_lowercase(), t[i + 1].clone());
            }
            i += 2;
        } else {
            if nesting == 1 && t.get(i + 1).is_some_and(|s| s == "{") {
                section = t[i].to_lowercase();
            }
            if nesting == 2 && t.get(i + 1).is_some_and(|s| s == "{") {
                subsection = t[i].to_lowercase();
            }
            i += 1;
        }
    }
    // Shader-quality conditionals ("GPU>=2?$phong"): Source evaluates them
    // against the machine's gpu_level, 2 on a PC at high settings. A met
    // condition overrides the unconditional key; others are dropped.
    const GPU_LEVEL: i32 = 2;
    let conditional: Vec<(String, String)> = map
        .iter()
        .filter(|(k, _)| k.contains('?'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (key, value) in conditional {
        map.remove(&key);
        let Some((condition, name)) = key.split_once('?') else { continue };
        let met = if let Some(n) = condition.strip_prefix("gpu>=") {
            n.trim().parse::<i32>().is_ok_and(|n| GPU_LEVEL >= n)
        } else if let Some(n) = condition.strip_prefix("gpu<") {
            n.trim().parse::<i32>().is_ok_and(|n| GPU_LEVEL < n)
        } else {
            false
        };
        if met {
            map.insert(name.to_string(), value);
        }
    }
    if shader == "patch" {
        let include = map
            .remove("include")
            .ok_or_else(|| invalid("VMT patch has no include"))?;
        let include = material_path(&include);
        let (shader, mut base) = values(assets, &include, depth + 1)?;
        base.extend(map);
        for (key, value) in inserts {
            // Native infected patches supply their own palette/phong values
            // over the shared ci_body_include defaults (CEDA, clown, mud).
            if shader == "infected" {
                base.insert(key, value);
            } else {
                base.entry(key).or_insert(value);
            }
        }
        for (key, value) in replaces {
            if let Some(existing) = base.get_mut(&key) {
                *existing = value;
            }
        }
        Ok((shader, base))
    } else {
        Ok((shader, map))
    }
}
pub fn sprite_settings(
    assets: &mut SourceAssets,
    material: &str,
) -> io::Result<([f32; 4], f32, [f32; 4], [f32; 4])> {
    let (_, v) = values(assets, &material_path(material), 0)?;
    let number =
        |key: &str, fallback: f32| v.get(key).and_then(|s| s.parse().ok()).unwrap_or(fallback);
    Ok((
        [
            number("$blendframes", 1.),
            number("$maxlumframeblend1", 0.),
            number("$overbrightfactor", 1.),
            number("$additive", 0.),
        ],
        number("$addself", 0.),
        [
            number("$dualsequence", 0.),
            number("$sequence_blend_mode", 0.),
            number("$maxlumframeblend2", 0.),
            number("$zoomanimateseq2", 1.).max(1.),
        ],
        [
            number("$minsize", 0.),
            number("$maxsize", 20.),
            number("$startfadesize", 10.),
            number("$endfadesize", 20.),
        ],
    ))
}
/// Refract particles have a normal map rather than a SpriteCard base map.
pub fn particle_refraction(
    assets: &mut SourceAssets,
    material: &str,
) -> io::Result<Option<(String, f32, bool)>> {
    let (shader, values) = values(assets, &material_path(material), 0)?;
    if shader != "refract" {
        return Ok(None);
    }
    Ok(texture_path(values.get("$normalmap")).map(|normal| {
        let amount = values
            .get("$refractamount")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(0.1);
        (
            normal,
            amount,
            values
                .get("$vertexcolormodulate")
                .is_some_and(|value| value == "1"),
        )
    }))
}
/// SpriteCard `$cropfactor "[x y]"`: each frame samples only its centred
/// x by y part, stretched over the whole sprite (CROP combo).
pub fn particle_crop(assets: &mut SourceAssets, material: &str) -> io::Result<[f32; 2]> {
    let (_, values) = values(assets, &material_path(material), 0)?;
    let Some(text) = values.get("$cropfactor") else { return Ok([1., 1.]) };
    let numbers: Vec<f32> = text
        .trim_matches(|c| c == '[' || c == ']' || c == '{' || c == '}')
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .filter(|n: &f32| n.is_finite() && *n > 0.)
        .collect();
    Ok(match numbers.as_slice() {
        [x, y, ..] => [x.min(1.), y.min(1.)],
        _ => [1., 1.],
    })
}
/// SpriteCard soft-particle fade: `$depthblend` (on by default on PC, as
/// GetDefaultDepthFeatheringValue) over `$depthblendscale` units (50).
pub fn particle_depth_blend(assets: &mut SourceAssets, material: &str) -> io::Result<f32> {
    let (shader, values) = values(assets, &material_path(material), 0)?;
    if !shader.eq_ignore_ascii_case("spritecard") || values.get("$depthblend").is_some_and(|v| v.trim() == "0") {
        return Ok(0.);
    }
    let scale = values.get("$depthblendscale").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite() && *v > 0.).unwrap_or(50.);
    Ok(scale * 0.0254)
}
/// SpriteCard `$mod2x 1`: a 2x modulate blend.
pub fn particle_mod2x(assets: &mut SourceAssets, material: &str) -> io::Result<bool> {
    let (shader, values) = values(assets, &material_path(material), 0)?;
    Ok(shader.eq_ignore_ascii_case("spritecard") && values.get("$mod2x").is_some_and(|value| value == "1"))
}
/// Particle fog, as ParticlePull `effects.y`: 0 none ($nofog), 1 the world's
/// pixel fog, 2 vertex fog only. SpriteCard fogs per vertex
/// ($vertexfogamount), which Source's HDR pass does not apply: in a live
/// retail c1m1 comparison (19 Nov) the rooftop smoke column took no colour
/// from a forced red fog_override, where world brushes went fully red. The
/// 3D-skybox path still applies the sky_camera fog to vertex-fogged sprites.
pub fn particle_fog_mode(assets: &mut SourceAssets, material: &str) -> io::Result<f32> {
    let (shader, _) = values(assets, &material_path(material), 0)?;
    Ok(match (particle_fog_enabled(assets, material)?, shader.eq_ignore_ascii_case("spritecard")) {
        (false, _) => 0.,
        (true, true) => 2.,
        (true, false) => 1.,
    })
}
pub fn particle_fog_enabled(assets: &mut SourceAssets, material: &str) -> io::Result<bool> {
    let (_, values) = values(assets, &material_path(material), 0)?;
    Ok(values.get("$nofog").is_none_or(|value| value != "1"))
}
pub fn particle_spitter_noise(
    assets: &mut SourceAssets,
    material: &str,
) -> io::Result<Option<String>> {
    let (shader, values) = values(assets, &material_path(material), 0)?;
    Ok(if shader == "refract" {
        texture_path(values.get("$spitternoisetexture"))
    } else {
        None
    })
}
pub fn material_path(name: &str) -> String {
    let name = name.replace('\\', "/").to_lowercase();
    let name = name
        .trim_start_matches("materials/")
        .trim_end_matches(".vmt");
    format!("materials/{name}.vmt")
}
pub fn crack_material(assets: &mut SourceAssets, name: &str) -> io::Result<Option<String>> {
    let (_,v)=values(assets,&material_path(name),0)?;
    Ok(v.get("$crackmaterial").filter(|s|!s.is_empty()).map(|s|s.replace('\\',"/").to_ascii_lowercase()
        .trim_start_matches("materials/").trim_end_matches(".vmt").to_string()))
}
fn texture_path(name: Option<&String>) -> Option<String> {
    name.filter(|s| !s.is_empty() && !s.starts_with('_'))
        .map(|s| {
            let s = s.replace('\\', "/").to_lowercase();
            format!(
                "materials/{}.vtf",
                s.trim_start_matches("materials/").trim_end_matches(".vtf")
            )
        })
}
pub fn load(assets: &mut SourceAssets, name: &str) -> io::Result<Material> {
    let (shader, v) = values(assets, &material_path(name), 0)?;
    let yes = |k: &str| v.get(k).is_some_and(|s| s == "1");
    let chainlink = v
        .get("$surfaceprop")
        .is_some_and(|s| s.eq_ignore_ascii_case("chainlink"));
    let fence_depth = yes("$alphatest") && yes("$allowfencerenderstatehack");
    let alpha = if fence_depth || (yes("$translucent") && chainlink) {
        // Rooftop fence001/002/003 combine AlphaTest and Translucent with
        // Valve's fence state hack. Preserve depth-tested wire coverage.
        1
    } else if yes("$additive") {
        3
    } else if yes("$translucent") {
        2
    } else if yes("$alphatest") {
        1
    } else {
        0
    };
    let mut color = [1.; 4];
    if let Some(c) = v.get("$color2").or_else(|| v.get("$color")) {
        let scale = if c.contains('{') { 1. / 255. } else { 1. };
        let parts: Vec<f32> = c
            .trim_matches(['[', ']', '{', '}', ' '])
            .split_whitespace()
            .map(|part| part.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(1.) * scale)
            .collect();
        // A single value applies to all three channels.
        match parts.as_slice() {
            [x] => color[..3].copy_from_slice(&[*x; 3]),
            _ => {
                for (out, v) in color[..3].iter_mut().zip(parts) {
                    *out = v;
                }
            }
        }
    }
    color[3] = v
        .get("$alpha")
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(1.)
        .clamp(0., 1.);
    let mut uv = [0.5, 0.5, 1., 1., 0., 0., 0.];
    if let Some(transform) = v.get("$basetexturetransform") {
        let t: Vec<_> = transform.split_whitespace().collect();
        let mut i = 0;
        while i < t.len() {
            let (start, n) = match t[i].to_lowercase().as_str() {
                "center" => (0, 2),
                "scale" => (2, 2),
                "rotate" => (4, 1),
                "translate" => (5, 2),
                _ => (0, 0),
            };
            if n == 0 {
                i += 1;
                continue;
            }
            for j in 0..n {
                if let Some(x) = t
                    .get(i + 1 + j)
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                {
                    uv[start + j] = x;
                }
            }
            i += n + 1;
        }
    }
    Ok(Material {
        base2: (shader == "worldvertextransition")
            .then(|| texture_path(v.get("$basetexture2")))
            .flatten(),
        blend_modulate: (shader == "worldvertextransition")
            .then(|| texture_path(v.get("$blendmodulatetexture")))
            .flatten(),
        base: texture_path(
            v.get("$hdrcompressedtexture")
                .or_else(|| v.get("$hdrbasetexture"))
                .or_else(|| v.get("$basetexture"))
                .or_else(|| v.get("$iris")),
        ),
        bump: matches!(shader.as_str(), "lightmappedgeneric" | "worldvertextransition")
            .then(|| texture_path(v.get("$bumpmap")))
            .flatten(),
        // Source's self-shadowed bump coefficients are not XYZ tangent normals.
        normal: if yes("$ssbump") {
            None
        } else {
            texture_path(v.get("$bumpmap"))
        },
        palette: if shader == "infected" && !yes("$disablevariation") {
            texture_path(v.get("$gradienttexture")).map(|path| {
                (
                    path,
                    v.get("$skintintgradient")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    v.get("$colortintgradient")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                )
            })
        } else {
            None
        },
        emission: if yes("$selfillum") {texture_path(v.get("$basetexture"))} else {None},
        emission_mask: texture_path(v.get("$selfillummask")),
        emission_tint: {
            let mut tint=[1.;3];
            if let Some(value)=v.get("$selfillumtint") {
                let parts:Vec<f32>=value.trim_matches(|c|c=='['||c==']'||c=='{'||c=='}'||c==' ').split_whitespace()
                    .map(|v|v.parse::<f32>().ok().filter(|v|v.is_finite()).unwrap_or(1.).max(0.)).collect();
                // One value applies to all channels (c11m1's greenhouse
                // glass ".015" drew (0.015, 1, 1): bright cyan skylights).
                match parts.as_slice() {
                    [x] => tint=[*x;3],
                    _ => for (out,v) in tint.iter_mut().zip(parts) { *out=v; },
                }
            }
            tint
        },
        hdr_compressed: v.contains_key("$hdrcompressedtexture"),
        water: (shader == "water").then(|| WaterSettings::parse(&v)),
        wound: WoundSettings::parse(&v),
        infected_phong: (shader == "infected").then(|| InfectedPhongSettings::parse(&v)).flatten(),
        infected_detail: (shader == "infected").then(|| texture_path(v.get("$detail"))).flatten(),
        infected_burn: (shader == "infected").then(|| texture_path(v.get("$burndetailtexture"))).flatten(),
        cap_phong: CapPhongSettings::parse(name, &shader, &v),
        player_team: v.get("%playerteam").and_then(|s| s.trim().parse().ok()),
        envmap: EnvmapSettings::parse(&shader, &v),
        // Source's Black shader draws solid black, ignoring any texture. Valve
        // uses it for skybox silhouettes (c2m1's town and ferris wheel); with
        // no texture to sample they rendered plain white against the sky.
        color: if shader == "black" { [0., 0., 0., color[3]] } else { color },
        alpha,
        fence: alpha == 1 && (fence_depth || (yes("$translucent") && chainlink)),
        half_lambert: yes("$halflambert"),
        ssbump: yes("$ssbump"),
        cutoff: v
            .get("$alphatestreference")
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.5),
        // Converting thin translucent wire to cutout must retain the two-sided
        // rendering it received in the transparent path.
        nocull: yes("$nocull") || chainlink,
        unlit: shader == "unlitgeneric" || shader == "black",
        uv,
        shader,
    })
}
/// Keep Infected's original packed base and colour lookup for GPU sampling.
/// Baking the palette into diffuse loses the separate quarter-sheet blood mask
/// and changes linear interpolation into interpolation of encoded sRGB bytes.
pub fn prepare_palette(
    assets: &mut SourceAssets,
    material: &mut Material,
    textures: &mut std::collections::HashMap<String, crate::vtf::Texture>,
) -> io::Result<()> {
    if material.shader != "infected" {
        return Ok(());
    }
    let Some(base) = &material.base else {
        return Ok(());
    };
    for path in std::iter::once(base).chain(material.palette.iter().map(|(p, _, _)| p)) {
        if !textures.contains_key(path) {
            // Infected luminance/masks are authored at 2048. The world budget
            // must not discard their highest mip before video settings apply.
            let limit = if path == base { 2048 } else { 1024 };
            textures.insert(path.clone(), if path == base { crate::vtf::decode_world(&assets.read(path)?, limit)? } else { crate::vtf::decode(&assets.read(path)?, limit)? });
        }
    }
    Ok(())
}
impl Default for Material {
    fn default() -> Self {
        Self {
            base: None,
            base2: None,
            normal: None,
            bump: None,
            palette: None,
            color: [1., 0., 1., 1.],
            alpha: 0,
            fence: false,
            half_lambert: false,
            ssbump: false,
            cutoff: 0.5,
            nocull: true,
            unlit: false,
            uv: [0.5, 0.5, 1., 1., 0., 0., 0.],
            shader: "missing".into(),
            hdr_compressed: false,
            emission: None,
            emission_mask: None,
            emission_tint: [1.;3],
            water: None,
            wound: None,
            infected_phong: None,
            infected_detail: None,
            infected_burn: None,
            cap_phong: None,
            player_team: None,
            envmap: None,
            blend_modulate: None,
        }
    }
}
pub fn transform_uv(uv: [f32; 2], t: [f32; 7]) -> [f32; 2] {
    let x = (uv[0] - t[0]) * t[2];
    let y = (uv[1] - t[1]) * t[3];
    let (s, c) = t[4].to_radians().sin_cos();
    [c * x - s * y + t[0] + t[5], s * x + c * y + t[1] + t[6]]
}
#[cfg(test)]
mod tests {
    use super::*;
    /// c2m1's skybox town and ferris wheel use the Black shader: solid
    /// black silhouettes, not untextured white.
    #[test]
    fn black_shader_draws_solid_black() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for name in ["models/props_skybox/milltown001", "models/props_unique/reflectivity_50b"] {
            let m = load(&mut assets, name).unwrap();
            assert_eq!(m.shader, "black", "{name}");
            assert_eq!(&m.color[..3], &[0., 0., 0.], "{name}");
            assert!(m.unlit, "{name}");
        }
        let lit = load(&mut assets, "models/survivors/gambler/gambler_body").unwrap();
        assert_eq!(&lit.color[..3], &[1., 1., 1.]);
    }
    /// c1m1's roof displacements blend two roof textures through a
    /// modulation mask (their dark grime patches).
    #[test]
    fn world_vertex_transition_keeps_its_second_texture_and_blend_mask() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let m = load(&mut assets, "buildings/blend_roof_01").unwrap();
        assert_eq!(m.base2.as_deref(), Some("materials/buildings/building_roof_02.vtf"));
        assert_eq!(m.blend_modulate.as_deref(), Some("materials/nature/blend_texture_01.vtf"));
        assert!(!assets.read(m.blend_modulate.as_deref().unwrap()).unwrap().is_empty());
    }
    #[test]
    fn infected_ladder_claw_material_is_for_the_infected_team_only() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        assert_eq!(load(&mut assets, "tools/climb_versus").unwrap().player_team, Some(3));
        assert_eq!(load(&mut assets, "tools/toolsnodraw").unwrap().player_team, None);
        // c1m3 has climb_versus faces in the world model, not only on
        // func_simpleladder brushes; the scene must carry the team gate.
        let path = root.join("left4dead2/maps/c1m3_mall.bsp");
        let world = crate::bsp::load_file(&path, root).unwrap();
        let scene = crate::scene::Scene::load(root, world).unwrap();
        let world_claws: Vec<_> = scene
            .world
            .surfaces
            .iter()
            .filter(|s| s.model == 0 && s.material.eq_ignore_ascii_case("tools/climb_versus"))
            .collect();
        assert!(!world_claws.is_empty());
        for s in world_claws {
            assert_eq!(scene.materials.get(&s.material).and_then(|m| m.player_team), Some(3), "{}", s.material);
        }
    }
    #[test]
    fn installed_infected_phong_controls_keep_native_exponents_and_tint() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let material = load(&mut assets, "models/infected/common/l4d2/cim_tshirt_cargos").unwrap();
        let p = material.infected_phong.unwrap();
        assert_eq!(p.exponents, [30., 16., 100., 5.]);
        assert_eq!(p.fresnel, [0.75, 0.25, 30., 1.2]);
        assert_eq!(p.tint, [0.62, 0.72, 1., 1.]);
        assert_eq!(p.detail, [5., 2., 2., 3.]);
        let old = load(&mut assets, "models/infected/common/common_male_head_02_wounded").unwrap();
        let p = old.infected_phong.unwrap();
        assert_eq!(p.exponents, [8., 8., 100., 3.]);
        assert_eq!(p.fresnel, [1., 0., 0.2, 0.8]);
        assert_eq!(p.detail[3], 6.);
        let mut detail = BTreeMap::from([("$phong".into(), "1".into()), ("$detail".into(), "detail/skin".into()), ("$detailblendfactor".into(), "1".into())]);
        assert_eq!(InfectedPhongSettings::parse(&detail).unwrap().detail[3], 1.);
        detail.insert("$phong".into(), "0".into());
        assert_eq!(InfectedPhongSettings::parse(&detail).unwrap().tint[3], 0.);
    }
    #[test]
    fn installed_wound_material_retains_linear_mask_and_native_controls() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for (path, mapping, red) in [("models/infected/common/l4d2/ci_body_include", 7., 0.15),
            ("models/infected/common/common_male_head_02_wounded", 6.5, 0.33)] {
            let material = load(&mut assets, path).unwrap();
            let w = material.wound.unwrap();
            assert!(w.texture.ends_with("wound_combined_cull_texture.vtf"));
            assert_eq!(w.mapping_scale, mapping);
            assert_eq!(w.bias, 0.95);
            assert_eq!(w.blood, [red, 0., 0.]);
            assert_eq!(w.blood_range, [0.1, 0.9]);
            let mask = crate::vtf::decode(&assets.read(&w.texture).unwrap(), 1024).unwrap();
            assert_eq!((mask.width, mask.height), (1024, 512));
            assert!(mask.rgba.chunks_exact(4).any(|p| p[0] > 242));
            assert!(mask.rgba.chunks_exact(4).any(|p| p[0] > 25 && p[0] < 230));
        }
    }
    #[test]
    fn water_native_tints_flow_and_dx9_overrides_are_not_diffuse_defaults() {
        use std::io::{Cursor, Write};
        let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
        zip.start_file("materials/river.vmt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"Water {
            $normalmap liquids/normal $flowmap liquids/flow $flow_noise_texture liquids/noise
            $abovewater 1 $reflecttexture _rt_WaterReflection $reflectamount 0.4
            $refract 1 $refractamount 0.25 $refracttint "{151 135 34}"
            $flow_normaluvscale 180 $flow_timeintervalinseconds 0.5
            $fogstart 0 $fogend 30 $lightmapwaterfog 1
            ">=DX90_20b" { $fogcolor "{9 7 1}" }
            "<DX90_20b" { $fogcolor "{255 0 0}" }
        }"#).unwrap();
        let mut assets = SourceAssets::new(std::path::Path::new("target/nonexistent-water-test"), zip.finish().unwrap().into_inner()).unwrap();
        let material = load(&mut assets, "river").unwrap();
        let w = material.water.unwrap();
        assert!(material.base.is_none());
        assert_eq!(w.textures().count(), 3);
        assert!(w.lightmap_fog, "$lightmapwaterfog");
        assert!(w.fog_color.iter().zip([9./255., 7./255., 1./255.]).all(|(a,b)| (*a-b).abs() < 1e-7));
        assert!(w.refract_tint.iter().zip([151./255., 135./255., 34./255.]).all(|(a,b)| (*a-b).abs() < 1e-7));
        assert_eq!((w.fog_start, w.fog_end, w.interval), (0.,30.,0.5));
        assert_eq!((w.reflect_amount, w.refract_amount), (0.4,0.25));
        assert!(w.above && w.reflect && w.refract);
    }
    #[test]
    fn installed_waterfront_material_preserves_authored_parameters() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for name in ["liquids/water_c5m1_flow", "liquids/urbanwater_river", "nature/water_swamp_beneath"] {
            let m = load(&mut assets, name).unwrap();
            let w = m.water.unwrap();
            for path in w.textures() { assert!(assets.exists(path), "{name}: {path}"); }
            assert!(w.fog && w.fog_end > w.fog_start);
            if name.contains("c5m1") {
                assert!(w.above && w.reflect && w.refract);
                assert_eq!((w.fog_end, w.normal_scale, w.interval), (30.,180.,0.5));
            }
            if name.contains("urbanwater") { assert!(w.fog_color.iter().zip([9./255.,7./255.,1./255.]).all(|(a,b)| (*a-b).abs() < 1e-7)); }
            if name.contains("swamp") {
                assert!(!w.above && w.force_expensive);
                assert_eq!(w.scroll, [[0.01,0.01],[-0.025,0.025]]);
            }
        }
    }
    #[test]
    fn selfillum_preserves_base_alpha_and_independent_rgb_mask_policy() {
        use std::io::{Cursor,Write};
        let mut zip=zip::ZipWriter::new(Cursor::new(vec![]));
        zip.start_file("materials/glow.vmt",zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"VertexLitGeneric {$basetexture lamps/base $selfillum 1 $selfillummask lamps/mask $selfillumtint "[0.5 1 2]"}"#).unwrap();
        let mut assets=SourceAssets::new(std::path::Path::new("target/nonexistent-material-test"),zip.finish().unwrap().into_inner()).unwrap();
        let m=load(&mut assets,"glow").unwrap();assert_eq!(m.emission.as_deref(),Some("materials/lamps/base.vtf"));assert_eq!(m.emission_mask.as_deref(),Some("materials/lamps/mask.vtf"));assert_eq!(m.emission_tint,[0.5,1.,2.]);assert_eq!(m.alpha,0);
    }
    #[test]
    fn a_single_selfillumtint_value_applies_to_every_channel() {
        use std::io::{Cursor,Write};
        let mut zip=zip::ZipWriter::new(Cursor::new(vec![]));
        zip.start_file("materials/glass.vmt",zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(br#"LightmappedGeneric {$basetexture glass/g $selfillum 1 $selfillumtint ".015" $color "[.5]"}"#).unwrap();
        let mut assets=SourceAssets::new(std::path::Path::new("target/nonexistent-material-test"),zip.finish().unwrap().into_inner()).unwrap();
        let m=load(&mut assets,"glass").unwrap();
        assert_eq!(m.emission_tint,[0.015;3]);
        assert_eq!(&m.color[..3],&[0.5;3]);
    }
    #[test]
    fn owned_exit_and_lighting_materials_retain_their_native_selfillum() {
        let root=std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");if !root.exists(){return;}
        let mut assets=SourceAssets::new(root,vec![]).unwrap();let mut count=0;
        for path in assets.matching_paths("materials/").into_iter().filter(|p|p.ends_with(".vmt")&&(p.contains("exit")||p.contains("walllight"))) {
            let Ok(m)=load(&mut assets,&path) else {continue};
            if let Some(base)=m.emission {assert!(assets.exists(&base),"{path}: {base}");println!("Native selfillum: {path}, mask {:?}, tint {:?}",m.emission_mask,m.emission_tint);count+=1;}
        }
        assert!(count>0,"owned exit/light fixtures must exercise emission");
    }
    #[test]
    fn owned_rooftop_fence_combined_flags_keep_depth_and_authored_cutoff() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        for name in [
            "models/props_urban/fence001",
            "models/props_urban/fence002",
            "models/props_urban/fence003",
        ] {
            let material = load(&mut assets, name).unwrap();
            assert_eq!(material.alpha, 1);
            assert!(material.fence, "{name}: fence render state");
            assert!(material.nocull);
            assert!((material.cutoff - 0.3).abs() < 0.00001);
        }
    }
    #[test]
    fn view_model_pistol_and_survivor_skin_read_half_lambert() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        assert!(load(&mut assets, "models/v_models/weapons/pistol/v_pistol_reference").unwrap().half_lambert);
        // Explicit "$halflambert 0" stays off.
        assert!(!load(&mut assets, "models/survivors/gambler/gambler_body").unwrap().half_lambert);
    }
    #[test]
    fn owned_chainlink_cutout_retains_two_sided_rendering() {
        let root =
            std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let material = load(&mut assets, "models/props_street/chainlinkfence_alpha").unwrap();
        assert_eq!(material.alpha, 1);
        assert!(material.nocull);
        assert_eq!(
            material.base.as_deref(),
            Some("materials/models/props_c17/fence_alpha.vtf")
        );
    }
    #[test]
    fn palette_preparation_preserves_owned_packed_masks_and_all_mips() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() { return; }
        let mut assets = SourceAssets::new(root, vec![]).unwrap();
        let mut material = load(&mut assets, "models/infected/common/l4d2/cim_tshirt_cargos").unwrap();
        let original = material.base.clone().unwrap();
        let palette = material.palette.as_ref().unwrap().0.clone();
        // Owned stdshader_dx9 10017070 initializes both gradient indices to
        // zero; per-instance retail variation is a separate render binding.
        assert_eq!(material.palette.as_ref().map(|(_, s, c)| (*s, *c)), Some((0, 0)));
        let expected = crate::vtf::decode(&assets.read(&original).unwrap(), 2048).unwrap();
        assert_eq!((expected.width, expected.height), (2048, 2048));
        let mut textures = std::collections::HashMap::new();
        prepare_palette(&mut assets, &mut material, &mut textures).unwrap();
        assert_eq!(material.base.as_ref(), Some(&original));
        assert_eq!(textures[&original].rgba, expected.rgba);
        assert_eq!(textures[&original].mips, expected.mips);
        assert!(textures.contains_key(&palette));
        assert!(textures[&palette].no_mip, "native colour rows must not be averaged across mip levels");
        assert_eq!(textures[&palette].mips.len(), 1);
        assert!(textures[&original].rgba.chunks_exact(4).any(|p| p[3] != 255));
        let before = textures.len();
        prepare_palette(&mut assets, &mut material, &mut textures).unwrap();
        assert_eq!(textures.len(), before);
    }
    #[test]
    fn native_blood_pattern_sheet_is_preserved() {
        for sheet in 0..4 {
            let values = BTreeMap::from([
                ("$woundcutouttexture".into(), "models/infected/common/body_wounds/wound_combined_cull_texture".into()),
                ("$sheetindex".into(), sheet.to_string()),
            ]);
            assert_eq!(WoundSettings::parse(&values).unwrap().sheet, sheet);
        }
    }
    #[test]
    fn patch_insert_preserves_existing_values_and_replace_overrides() {
        use std::io::{Cursor, Write};
        let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
        for (name, text) in [
            (
                "materials/base.vmt",
                "LightmappedGeneric { $basetexture original }",
            ),
            (
                "materials/patch.vmt",
                "Patch { include materials/base.vmt insert { $basetexture ignored $alphatest 1 } replace { $basetexture replacement } }",
            ),
        ] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        let mut assets = SourceAssets::new(
            std::path::Path::new("target/nonexistent-material-test"),
            zip.finish().unwrap().into_inner(),
        )
        .unwrap();
        let m = load(&mut assets, "patch").unwrap();
        assert_eq!(m.base.as_deref(), Some("materials/replacement.vtf"));
        assert_eq!(m.alpha, 1);
        assert_eq!(m.shader, "lightmappedgeneric");
    }
    #[test]
    fn material_paths_do_not_duplicate_prefixes() {
        assert_eq!(
            material_path("materials/BRICK/foo.vmt"),
            "materials/brick/foo.vmt"
        );
    }
    #[test]
    fn uv_transform_rotates_around_center() {
        let p = transform_uv([1., 0.5], [0.5, 0.5, 1., 1., 90., 0., 0.]);
        assert!((p[0] - 0.5).abs() < 0.0001);
        assert!((p[1] - 1.).abs() < 0.0001);
    }
}
