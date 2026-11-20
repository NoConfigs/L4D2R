//! Authored map fog, view-point volume selection and controller transitions.
use bevy::{
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
};
use std::collections::BTreeMap;

fn controller(entities: &[BTreeMap<String, String>]) -> Option<&BTreeMap<String, String>> {
    let name = entities
        .iter()
        .find(|e| e.get("classname").is_some_and(|c| c == "worldspawn"))
        .and_then(|e| e.get("fogcontroller"));
    let controllers: Vec<_> = entities
        .iter()
        .filter(|e| {
            e.get("classname")
                .is_some_and(|c| c == "env_fog_controller")
        })
        .collect();
    name.and_then(|n| {
        controllers
            .iter()
            .copied()
            .find(|e| e.get("targetname") == Some(n))
    })
    .or_else(|| {
        controllers.iter().copied().find(|e| {
            e.get("spawnflags")
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|f| f & 1 != 0)
        })
    })
    .or_else(|| controllers.first().copied())
}
pub(crate) fn far_distance(entities: &[BTreeMap<String, String>]) -> Option<f32> {
    controller(entities)?
        .get("farz")?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.)
        .map(|v| v * crate::collision::UNIT)
}
/// Per-map values for the 3D skybox and the world far clip, packed for the
/// reserved lighting-table slot read by source_lighting.wgsl:
/// [0] sky fog start, end (Bevy units, already multiplied by the sky scale),
///     max density, enabled; [1] sky fog linear colour;
/// [2] world farz (0 = none), clip_3D_skybox_near_to_world_far, its offset.
///
/// Source renders the 3D skybox from the sky_camera with that entity's own
/// fog (in skybox units), then draws the world clipped at the fog
/// controller's farz, so the skybox shows beyond farz. The rewrite draws the
/// skybox scaled up in world space, so the distances are scaled to match.
pub(crate) fn sky_globals(entities: &[BTreeMap<String, String>]) -> [Vec4; 3] {
    let far = far_distance(entities).unwrap_or(0.);
    let Some(camera) = entities
        .iter()
        .find(|e| e.get("classname").is_some_and(|c| c == "sky_camera"))
    else {
        return [Vec4::ZERO, Vec4::ZERO, Vec4::new(far, 0., 0., 0.)];
    };
    let number = |key: &str, default: f32| {
        camera
            .get(key)
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    };
    let scale = number("scale", 16.).clamp(1., 128.);
    // The sky_camera's fog range applies to world-scale distance, not to
    // sky units x scale: live retail c4m1 (19 Nov) fogs the distant skybox
    // terrain fully at the map's fogend 2000 and still fully at a forced
    // 8000, and only partly at 2000 x 32 = 64000, which is what the rewrite
    // drew (a black band under the horizon instead of retail's haze).
    let _ = scale;
    let unit = crate::collision::UNIT;
    let start = number("fogstart", 0.) * unit;
    let end = number("fogend", 0.) * unit;
    // L4D2R_NO_SKY_FOG: debug, turns the 3D skybox fog off.
    let enabled = camera.get("fogenable").is_some_and(|v| v == "1") && end > start
        && std::env::var_os("L4D2R_NO_SKY_FOG").is_none();
    let mut rgb = [255.; 3];
    if let Some(color) = camera.get("fogcolor") {
        for (out, v) in rgb.iter_mut().zip(color.split_whitespace()) {
            *out = v.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(255.).clamp(0., 255.);
        }
    }
    let linear = Color::srgb(rgb[0] / 255., rgb[1] / 255., rgb[2] / 255.).to_linear();
    [
        Vec4::new(
            start,
            end,
            number("fogmaxdensity", 1.).clamp(0., 1.),
            if enabled { 1. } else { 0. },
        ),
        Vec4::new(linear.red, linear.green, linear.blue, 0.),
        Vec4::new(
            far,
            if camera.get("clip_3d_skybox_near_to_world_far").is_some_and(|v| v == "1") {
                1.
            } else {
                0.
            },
            number("clip_3d_skybox_near_to_world_far_offset", 0.) * crate::collision::UNIT,
            0.,
        ),
    ]
}
pub(crate) fn from_entities(entities: &[BTreeMap<String, String>]) -> Option<DistanceFog> {
    let controller = controller(entities)?;
    if controller.get("fogenable").is_none_or(|v| v != "1") {
        return None;
    }
    let number = |key: &str, default| {
        controller
            .get(key)
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    };
    let start = number("fogstart", 0.) * crate::collision::UNIT;
    let end = number("fogend", 0.) * crate::collision::UNIT;
    if end <= start {
        return None;
    }
    let mut rgb = [255.; 3];
    if let Some(color) = controller.get("fogcolor") {
        for (out, v) in rgb.iter_mut().zip(color.split_whitespace()) {
            *out = v
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(255.)
                .clamp(0., 255.);
        }
    }
    Some(DistanceFog {
        color: Color::srgba(
            rgb[0] / 255.,
            rgb[1] / 255.,
            rgb[2] / 255.,
            number("fogmaxdensity", 1.).clamp(0., 1.),
        ),
        directional_light_color: Color::NONE,
        falloff: FogFalloff::Linear { start, end },
        ..default()
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    color: Vec3, // Native colour bytes: blend before conversion to linear RGB.
    start: f32,
    end: f32,
    density: f32,
    enabled: bool,
    far: f32, // 0 = use the camera's default far plane.
    duration: f32,
    secondary: Vec3,
    direction: Vec3,
    directional: bool,
    hdr_scale: f32,
}
impl Default for Settings { fn default() -> Self {Self::read(|_| None)} }
impl Settings {
    fn read<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
        let number = |k: &str, fallback: f32| get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback);
        let mut color = Vec3::splat(255.);
        if let Some(rgb) = get("fogcolor") {
            for (i,s) in rgb.split_whitespace().take(3).enumerate() {
                color[i] = s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(255.).clamp(0.,255.);
            }
        }
        let mut secondary = color;
        if let Some(rgb) = get("fogcolor2") {for (i,s) in rgb.split_whitespace().take(3).enumerate() {secondary[i]=s.parse::<f32>().ok().filter(|v|v.is_finite()).unwrap_or(color[i]).clamp(0.,255.);}}
        let vector = |key: &str, fallback: [f32;3]| {
            let mut v = fallback;
            if let Some(s) = get(key) {for (out,s) in v.iter_mut().zip(s.split_whitespace()) {*out=s.parse::<f32>().ok().filter(|v|v.is_finite()).unwrap_or(*out);}}
            v
        };
        let direction = if number("use_angles",0.) != 0. {
            let a = vector("angles",[0.;3]);
            let (sp,cp)=a[0].to_radians().sin_cos(); let (sy,cy)=a[1].to_radians().sin_cos();
            Vec3::new(-cp*cy,sp,cp*sy)
        } else { Vec3::from_array(crate::binary::source_normal(vector("fogdir",[1.,0.,0.]))) };
        Self {
            color, start: number("fogstart",0.) * crate::collision::UNIT,
            end: number("fogend",0.) * crate::collision::UNIT,
            density: number("fogmaxdensity",1.).clamp(0.,1.), enabled: number("fogenable",0.) != 0.,
            far: number("farz",0.).max(0.) * crate::collision::UNIT,
            duration: number("foglerptime",0.).max(0.),
            secondary, direction: direction.normalize_or_zero(), directional: number("fogblend",0.) != 0.,
            hdr_scale: number("hdrcolorscale",1.).max(0.),
        }
    }
    fn controller(world: &l4d2r_game_core::entities::EntityWorld, id: usize) -> Option<Self> {
        let e = world.ents.get(id).filter(|e| e.alive)?;
        Some(Self::read(|k| e.me.kv(k)))
    }
    fn animated(mut self, world: &l4d2r_game_core::entities::EntityWorld, id: usize, now: f32) -> Self {
        if let Some(v)=world.fog.sample_from(id,"fogcolor",self.color.to_array(),now) {self.color=Vec3::from_array(v).clamp(Vec3::ZERO,Vec3::splat(255.));}
        if let Some(v)=world.fog.sample_from(id,"fogcolor2",self.secondary.to_array(),now) {self.secondary=Vec3::from_array(v).clamp(Vec3::ZERO,Vec3::splat(255.));}
        if let Some(v)=world.fog.sample_from(id,"fogstart",[self.start/crate::collision::UNIT,0.,0.],now) {self.start=v[0]*crate::collision::UNIT;}
        if let Some(v)=world.fog.sample_from(id,"fogend",[self.end/crate::collision::UNIT,0.,0.],now) {self.end=v[0]*crate::collision::UNIT;}
        if let Some(v)=world.fog.sample_from(id,"fogmaxdensity",[self.density,0.,0.],now) {self.density=v[0].clamp(0.,1.);}
        self
    }
    fn color(self, forward: Vec3) -> LinearRgba {
        let amount = if self.directional { (0.5+0.5*forward.normalize_or_zero().dot(self.direction)).clamp(0.,1.) } else {1.};
        let rgb=self.secondary.lerp(self.color,amount)/255.;
        let c=Color::srgb(rgb.x,rgb.y,rgb.z).to_linear();
        LinearRgba::new(c.red*self.hdr_scale,c.green*self.hdr_scale,c.blue*self.hdr_scale,1.)
    }
    fn fog(self, forward: Vec3) -> DistanceFog {
        let valid = self.enabled && self.end > self.start;
        let c=self.color(forward);
        // Source scales the fog colour by the tonemap scale but caps its
        // luminance at 1 (native_tonemap::fog_before_exposure).
        let [r,g,b] = crate::native_tonemap::fog_before_exposure([c.red,c.green,c.blue]);
        DistanceFog {
            color: Color::linear_rgba(r,g,b,if valid {self.density} else {0.}),
            directional_light_color: Color::NONE,
            falloff: FogFalloff::Linear {start: self.start, end: self.end.max(self.start + 0.0001)},
            ..default()
        }
    }
}

/// One view's fog. The renderer reads the same far value for PVS and the GPU
/// clip slot; changes never reset model-lighting slots or mutate image assets.
#[derive(Resource, Default)]
pub(crate) struct Runtime {
    map: usize,
    master: Option<usize>,
    selected: Option<usize>,
    volumes: Vec<usize>,
    volume_controllers: std::collections::HashMap<usize, usize>,
    registered: Vec<usize>,
    revision: u64,
    current: Option<Settings>,
    from: Option<Settings>,
    target: Option<Settings>,
    since: f32,
    registry_revision: u64,
    forward: Vec3,
    sky: Option<Settings>,
    clock: f32,
    rendered: Option<Settings>,
    /// Exposure scale the fog colour was last divided by.
    exposure: f32,
}
impl Runtime {
    pub(crate) fn far(&self) -> Option<f32> { (self.map != 0).then(|| self.current.map_or(0., |s| s.far)) }
    pub(crate) fn sky_color(&self) -> Option<Vec4> {
        // Like world fog: tonemap-scaled, luminance capped at 1 (shaderapidx9
        // 10011150), pre-divided by Bevy's exposure.
        self.sky.map(|s| {let c=s.color(self.forward); let [r,g,b]=crate::native_tonemap::fog_before_exposure([c.red,c.green,c.blue]); Vec4::new(r,g,b,0.)})
    }
    fn render_time(&mut self, simulation: f32, overstep: f32, paused: bool) -> f32 {
        if !paused {self.clock=self.clock.max(simulation+overstep);}
        self.clock
    }
    fn reset(&mut self, world: &l4d2r_game_core::entities::EntityWorld) {
        *self = Self {map: std::sync::Arc::as_ptr(&world.bsp) as usize, ..default()};
        self.clock=world.time;
        self.reindex(world);
    }
    fn reindex(&mut self, world: &l4d2r_game_core::entities::EntityWorld) {
        self.registry_revision=world.fog.revision;
        self.registered=world.fog.volumes.clone();
        self.volume_controllers.clear();
        let controllers: Vec<_> = world.ents.iter().enumerate().filter(|(_,e)| e.alive && e.me.classname == "env_fog_controller").collect();
        let name = world.ents.iter().find(|e| e.me.classname == "worldspawn").and_then(|e| e.me.kv("fogcontroller"));
        self.master = name.and_then(|name| controllers.iter().find(|(_,e)| e.me.targetname.eq_ignore_ascii_case(name)))
            .or_else(|| controllers.iter().find(|(_,e)| e.me.spawnflags() & 1 != 0))
            .or_else(|| controllers.first()).map(|(id,_)| *id);
        self.volumes = world.ents.iter().enumerate().filter(|(_,e)| e.me.classname == "fog_volume" && e.me.brush.is_some()).map(|(id,_)| id).collect();
        for &id in &self.volumes {
            if let Some(controller) = world.ents[id].me.kv("FogName").and_then(|name| controllers.iter().find(|(_,e)| e.me.targetname.eq_ignore_ascii_case(name))).map(|(id,_)| *id) {
                self.volume_controllers.insert(id,controller);
            }
        }
        self.sky = world.ents.iter().find(|e| e.alive && e.me.classname=="sky_camera").map(|e| Settings::read(|k|e.me.kv(k)));
    }
    pub(crate) fn contains(world: &l4d2r_game_core::entities::EntityWorld, id: usize, point: Vec3) -> bool {
        let Some(e) = world.ents.get(id) else {return false;};
        let Some(model) = e.me.brush.and_then(|m| world.bsp.models.get(m)) else {return false;};
        let source = l4d2r_math::Vec3::new(point.x,-point.z,point.y)/crate::collision::UNIT;
        let local = world.world_matrix(id).inverse().transform_point3(source);
        // Owned server.dll 10297ea0 -> 100946b0: squared distance to OBB,
        // strictly less than the square of a one-Source-unit tolerance.
        let outside = (model.mins-local).max(l4d2r_math::Vec3::ZERO) + (local-model.maxs).max(l4d2r_math::Vec3::ZERO);
        outside.length_squared() < 1.
    }
    fn select(&mut self, world: &l4d2r_game_core::entities::EntityWorld, point: Vec3) -> Option<usize> {
        // Retail server.dll 102981c0 appends enabled volumes; 10297ea0 returns
        // the first whose collision volume contains the view point.
        if self.registry_revision != world.fog.revision {self.reindex(world);}
        if self.revision != world.player_fog_revision {
            self.revision = world.player_fog_revision;
            return world.player_fog_controller.or(self.master);
        }
        if self.volumes.is_empty() {
            return self.selected.filter(|&id| world.ents.get(id).is_some_and(|e| e.alive)).or(self.master);
        }
        for &id in &self.registered {
            if Self::contains(world,id,point) {
                return self.volume_controllers.get(&id).copied().filter(|&i| world.ents[i].alive).or(self.master);
            }
        }
        self.master
    }
    fn blend(&mut self, selected: Option<usize>, settings: Option<Settings>, now: f32) {
        // Source copies the new controller immediately, then blends only its
        // primary colour/start/end. Enable, max density and farz change at once.
        // Interrupted transitions start from the colour/ranges actually shown.
        self.advance(now);
        if self.current.is_none() || selected != self.selected {
            self.from = self.current;
            self.target = settings;
            self.since = now;
            self.selected = selected;
            if self.from.is_none() { self.current = settings; }
        } else if self.from.is_none() {
            self.current = settings;
            self.target = settings;
        }
        self.advance(now);
    }
    fn advance(&mut self, now: f32) {
        let Some(mut target) = self.target else { self.current = None; self.from = None; return; };
        if let Some(from) = self.from {
            let t = if target.duration > 0. { ((now-self.since)/target.duration).clamp(0.,1.) } else {1.};
            if t < 1. {
                target.color = from.color.lerp(target.color,t).floor();
                target.start = from.start + (target.start-from.start)*t;
                target.end = from.end + (target.end-from.end)*t;
            } else { self.from = None; }
        }
        self.current = Some(target);
    }
}

pub(crate) fn sync(
    mut commands: Commands,
    logic: Res<crate::l4d2r_cinematic::MapLogic>,
    mut state: ResMut<Runtime>,
    fixed: Res<Time<Fixed>>,
    game: Res<crate::game_state::Match>,
    mut camera: Query<(Entity, &GlobalTransform, &crate::game::Body, Option<&mut DistanceFog>), With<crate::viewer::GameCamera>>,
) {
    let _timing = crate::performance::scope("viewer::fog");
    let Ok((entity, transform, body, fog)) = camera.single_mut() else { return; };
    if state.map != std::sync::Arc::as_ptr(&logic.world.bsp) as usize { state.reset(&logic.world); }
    let now=state.render_time(logic.world.time,fixed.overstep().as_secs_f32(),game.paused);
    let (point,forward)=view_point(body,game.incap,logic.world.cine.view_control.is_some() || game.dead,transform);
    let selected = state.select(&logic.world, point);
    let settings = selected.and_then(|id| Settings::controller(&logic.world,id));
    let previous_forward=state.forward;
    state.forward=forward;
    let old_selection = state.selected;
    state.blend(selected, settings, now);
    let rendered=state.current.map(|s| selected.map_or(s,|id|s.animated(&logic.world,id,now)));
    if old_selection != selected && std::env::args().any(|a| a == "--fog-diagnostics") {
        println!("Fog transition: time={:.3} controller={} view={:?} settings={:?}", logic.world.time, selected.map_or("none",|id| logic.world.ents[id].me.targetname.as_str()),transform.translation(), settings);
    }
    let scale = crate::native_tonemap::exposure_scale();
    let exposure_moved = (scale - state.exposure).abs() > state.exposure.abs() * 0.002;
    if exposure_moved { state.exposure = scale; }
    if state.rendered != rendered || previous_forward != forward || fog.is_none() || exposure_moved {
        state.rendered=rendered;
        // L4D2R_NO_FOG: debug, like fog_override 1 / fog_enable 0.
        let rendered = if std::env::var_os("L4D2R_NO_FOG").is_some() { None } else { rendered };
        let next = rendered.map(|s|s.fog(forward)).unwrap_or(DistanceFog {color: Color::NONE, directional_light_color: Color::NONE, falloff: FogFalloff::Linear {start: 0.,end: 1.}, ..default()});
        if let Some(mut fog) = fog { *fog = next; } else { commands.entity(entity).insert(next); }
    }
}
pub(crate) fn view_point(body: &crate::game::Body, incap: bool, external_view: bool, camera: &GlobalTransform) -> (Vec3,Vec3) {
    if external_view {return (camera.translation(),*camera.forward());}
    let rotation=Quat::from_euler(EulerRot::YXZ,body.yaw,body.pitch,0.);
    (body.feet+Vec3::Y*if incap {0.5} else {body.stance.eye()},rotation*Vec3::NEG_Z)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controller_lerp_stages_only_changed_fields_then_commits_at_native_think_time() {
        let mut w=io_world();
        w.input(0,"AddOutput","foglerptime 2",None,None);
        w.input(0,"SetColor","10 20 30",None,None);
        w.input(0,"SetEndDist","500",None,None);
        w.input(0,"SetMaxDensity","0.8",None,None);
        for (input,param) in [("SetStartDistLerpTo","101"),("SetColorLerpTo","210 120 80"),("SetColorSecondaryLerpTo","100 110 120"),("SetMaxDensityLerpTo","0.2")] {
            w.input(0,input,param,None,None);
        }
        assert!(w.fog.sample(0,&w.ents[0],"fogstart",0.).is_none());
        assert_eq!(w.ents[0].me.kv_f32("fogstart"),Some(1.));
        w.input(0,"StartFogTransition","",None,None);
        let before=w.fog.sample(0,&w.ents[0],"fogstart",0.).unwrap()[0];
        assert!((before+4.).abs()<1e-4); // Native .1 padding yields -.05 initially.
        w.tick(1.);
        let cfg=Settings::controller(&w,0).unwrap().animated(&w,0,1.);
        assert!((cfg.start/crate::collision::UNIT-46.).abs()<1e-3);
        assert!((cfg.density-0.53).abs()<1e-5);
        assert!((cfg.color.x-100.).abs()<1e-3);
        assert_eq!(cfg.end,500.*crate::collision::UNIT);
        w.tick(1.);
        assert_eq!(w.ents[0].me.kv_f32("fogstart"),Some(101.));
        assert_eq!(w.ents[0].me.kv("fogcolor"),Some("210 120 80"));
        assert!(w.fog.sample(0,&w.ents[0],"fogstart",2.).is_none());
        // A new transition cannot reuse dirty fields from the previous one.
        w.input(0,"SetEndDistLerpTo","900",None,None);
        w.input(0,"StartFogTransition","",None,None);w.tick(2.);
        assert_eq!(w.ents[0].me.kv_f32("fogstart"),Some(101.));
        assert_eq!(w.ents[0].me.kv_f32("fogend"),Some(900.));
    }
    #[test]
    fn directional_fog_blends_gamma_colours_before_hdr_scaling_and_uses_angle_direction() {
        let data=crate::kv::entities(r#"{"fogcolor" "255 0 0" "fogcolor2" "0 0 255" "fogblend" "1" "use_angles" "1" "angles" "0 90 0" "HDRColorScale" "2"}"#);
        let cfg=Settings::read(|k| data[0].get(k).map(String::as_str));
        assert!(cfg.direction.abs_diff_eq(Vec3::Z,1e-5));
        let primary=cfg.color(Vec3::Z);let secondary=cfg.color(Vec3::NEG_Z);let side=cfg.color(Vec3::X);
        assert!((primary.red-2.).abs()<1e-5 && primary.blue<1e-5);
        assert!((secondary.blue-2.).abs()<1e-5 && secondary.red<1e-5);
        assert!((side.red-0.4280823).abs()<1e-4 && (side.blue-side.red).abs()<1e-4);
    }
    #[test]
    fn render_clock_interpolates_between_ticks_and_freezes_through_pause() {
        let mut state=Runtime::default();
        assert_eq!(state.render_time(1.,0.01,false),1.01);
        assert_eq!(state.render_time(1.,0.02,false),1.02);
        assert_eq!(state.render_time(1.,0.,true),1.02);
        assert_eq!(state.render_time(1.,0.025,true),1.02);
        assert_eq!(state.render_time(1.,0.001,false),1.02);
        assert_eq!(state.render_time(1.033,0.005,false),1.038);
    }
    #[test]
    fn third_person_camera_does_not_move_survivor_fog_selection_into_another_room() {
        let body=crate::game::Body::at(Vec3::new(2.,3.,4.));
        let camera=GlobalTransform::from_translation(Vec3::new(200.,300.,400.));
        let (point,_)=view_point(&body,false,false,&camera);
        assert_eq!(point,body.feet+Vec3::Y*body.stance.eye());
        assert_eq!(view_point(&body,true,false,&camera).0,body.feet+Vec3::Y*0.5);
        assert_eq!(view_point(&body,false,true,&camera).0,camera.translation());
    }
    #[test]
    fn dynamic_fog_bindings_and_same_frame_enable_cycles_keep_native_registration_order() {
        let mut w=io_world();
        std::sync::Arc::get_mut(&mut w.bsp).unwrap().models.push(l4d2_formats::bsp::Model {
            mins:l4d2r_math::Vec3::splat(-10.),maxs:l4d2r_math::Vec3::splat(10.),origin:l4d2r_math::Vec3::ZERO,head_node:0,first_face:0,num_faces:0,
        });
        let mut state=Runtime::default();state.reset(&w);
        let mut volume=w.ents[4].clone();volume.me.brush=Some(0);volume.me.targetname="dynamic_volume".into();
        l4d2r_game_core::fog::set_key(&mut volume,"StartDisabled","0".into());
        l4d2r_game_core::fog::set_key(&mut volume,"FogName","b".into());
        let first=w.add_runtime_entity(volume.clone());
        assert_eq!(state.select(&w,Vec3::ZERO),Some(1));
        w.input(1,"AddOutput","targetname renamed",None,None);
        assert_eq!(state.select(&w,Vec3::ZERO),Some(0));
        w.input(first,"AddOutput","FogName renamed",None,None);
        assert_eq!(state.select(&w,Vec3::ZERO),Some(1));
        l4d2r_game_core::fog::set_key(&mut volume,"FogName","a".into());volume.me.targetname="second_volume".into();
        let second=w.add_runtime_entity(volume);
        w.input(first,"Disable","",None,None);w.input(first,"Enable","",None,None);
        assert_eq!(state.select(&w,Vec3::ZERO),Some(0));
        assert_eq!(state.registered,vec![second,first]);
        w.input(second,"Kill","",None,None);
        assert_eq!(state.select(&w,Vec3::ZERO),Some(1));
        w.input(1,"Kill","",None,None);
        assert_eq!(state.select(&w,Vec3::ZERO),Some(0));
        // Disabling the last volume must restore the master rather than retain
        // the controller selected while the volume was enabled.
        w.ents[1].alive=true;w.fog.changed(1,&w.ents[1]);
        state.selected=Some(1);
        w.input(first,"Disable","",None,None);
        assert!(w.fog.volumes.is_empty());
        assert_eq!(state.select(&w,Vec3::ZERO),Some(0));
    }
    #[test]
    fn controller_blends_color_and_ranges_without_blending_far_or_restarting_each_frame() {
        let a = Settings {color: Vec3::ZERO, start: 1., end: 10., density: 1., enabled: true, far: 500., duration: 2., ..default()};
        let b = Settings {color: Vec3::new(200.,100.,50.),start: 11.,end: 30.,density: 0.7,far: 30.,..a};
        let mut state = Runtime::default();
        state.blend(Some(1),Some(a),0.);
        assert_eq!(state.current,Some(a)); // Initial controller snaps.
        state.blend(Some(2),Some(b),0.);
        assert_eq!(state.current.unwrap().far,30.);
        assert_eq!(state.current.unwrap().density,0.7);
        state.blend(Some(2),Some(b),1.);
        let half = state.current.unwrap();
        assert_eq!(half.color,Vec3::new(100.,50.,25.));
        assert_eq!((half.start,half.end),(6.,20.));
        state.blend(Some(2),Some(b),1.); // Paused clock must stay frozen.
        assert_eq!(state.current,Some(half));
        state.blend(Some(3),Some(a),1.); // Interrupt without snapping to old start.
        assert_eq!(state.current.unwrap().color,half.color);
        state.blend(Some(3),Some(a),2.);
        assert_eq!(state.current.unwrap().color,Vec3::new(50.,25.,12.));
        state.blend(Some(3),Some(a),3.);
        assert_eq!(state.current,Some(a));
    }
    fn io_world() -> l4d2r_game_core::entities::EntityWorld {
        use l4d2r_game_core::mapents::MapEnt;
        let mut data = vec![0u8;1036]; data[..4].copy_from_slice(b"VBSP"); data[4..8].copy_from_slice(&21i32.to_le_bytes());
        let bsp = std::sync::Arc::new(l4d2_formats::bsp::Bsp::parse(data).unwrap());
        let entity = |index, class: &str, name: &str, keys: &[(&str,&str)]| {
            let mut values = vec![("classname".into(),class.into()),("targetname".into(),name.into())];
            values.extend(keys.iter().map(|(k,v)| (k.to_string(),v.to_string())));
            MapEnt {index,classname:class.into(),targetname:name.into(),origin:l4d2r_math::Vec3::ZERO,angles:l4d2r_math::Vec3::ZERO,mdl:None,brush:None,ent:l4d2_formats::bsp::Entity(values)}
        };
        l4d2r_game_core::entities::EntityWorld::new(bsp,vec![
            entity(0,"env_fog_controller","a",&[("fogstart","1")]),
            entity(1,"env_fog_controller","b",&[("fogstart","20")]),
            entity(2,"trigger_multiple","trigger",&[("OnStartTouch","relay,Trigger,,0.1,-1")]),
            entity(3,"logic_relay","relay",&[("OnTrigger","!activator,SetFogController,b,0.2,-1")]),
            entity(4,"fog_volume","volume",&[("StartDisabled","1")]),
        ],1)
    }
    #[test]
    fn delayed_activator_fog_input_retains_player_identity_and_ignores_bot_contacts() {
        let mut w = io_world();
        w.touch_actor(2,1,None); // bot in a different room
        w.tick(0.15); w.tick(0.25);
        assert_eq!(w.player_fog_controller,None);
        w.touch_actor(2,0,None);
        w.tick(0.15); w.tick(0.25);
        assert_eq!(w.player_fog_controller,Some(1));
        w.queue_input("!player","SetFogController","a",0.,None); w.flush_io();
        assert_eq!(w.player_fog_controller,Some(0));
        w.queue_input("!player","SetFogController","does_not_exist",0.,None); w.flush_io();
        assert_eq!(w.player_fog_controller,Some(0));
    }
    #[test]
    fn volume_contains_uses_rotated_bounds_and_native_euclidean_edge_tolerance() {
        let mut w = io_world();
        std::sync::Arc::get_mut(&mut w.bsp).unwrap().models.push(l4d2_formats::bsp::Model {
            mins: l4d2r_math::Vec3::new(-10.,-2.,-2.), maxs: l4d2r_math::Vec3::new(10.,2.,2.),
            origin: l4d2r_math::Vec3::ZERO,head_node:0,first_face:0,num_faces:0,
        });
        w.ents[4].me.brush=Some(0);
        w.ents[4].me.origin=l4d2r_math::Vec3::new(100.,200.,300.);
        w.ents[4].me.angles.y=45.;
        let point = |p: [f32;3]| Vec3::from_array(crate::binary::source_position(w.world_matrix(4).transform_point3(l4d2r_math::Vec3::from_array(p)).to_array()));
        assert!(Runtime::contains(&w,4,point([0.,0.,0.])));
        assert!(Runtime::contains(&w,4,point([10.75,0.,0.])));
        assert!(!Runtime::contains(&w,4,point([11.25,0.,0.])));
        assert!(!Runtime::contains(&w,4,point([10.75,2.75,0.])));
    }
    #[test]
    fn fog_volume_inputs_and_live_controller_values_use_existing_map_io() {
        let mut w = io_world();
        w.input(4,"Enable","",None,None);
        assert_eq!(w.ents[4].me.kv_i32("StartDisabled"),Some(0));
        w.input(4,"Toggle","",None,None);
        assert_eq!(w.ents[4].me.kv_i32("StartDisabled"),Some(1));
        for (input,param) in [("TurnOn",""),("SetStartDist","-128"),("SetEndDist","1024"),("SetFarZ","2048"),("SetColor","30 40 50")] {w.input(0,input,param,None,None);}
        w.input(0,"SetEndDist","NaN",None,None);
        let settings = Settings::read(|k| w.ents[0].me.kv(k));
        assert!(settings.enabled);
        assert_eq!(settings.color,Vec3::new(30.,40.,50.));
        assert_eq!(settings.start,-128.*crate::collision::UNIT);
        assert_eq!(settings.end,1024.*crate::collision::UNIT);
        assert_eq!(settings.far,2048.*crate::collision::UNIT);
    }
    #[test]
    fn installed_hotel_view_point_selects_rooftop_floors_lobby_and_checkpoint_volumes() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        let path = root.join("left4dead2/maps/c1m1_hotel.bsp"); if !path.exists() {return;}
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root,&path,true).unwrap();
        let mut state = Runtime::default(); state.reset(&logic.world);
        for name in ["fog_rooftop","fog_topfloors","fog_lobby","foginteriorcontroller"] {
            let mut found = None;
            for &id in &state.volumes.clone() {
                if logic.world.ents[id].me.kv("FogName") != Some(name) {continue;}
                let (lo,hi) = logic.bounds(id).unwrap();
                for x in [0.2,0.5,0.8] {for y in [0.2,0.5,0.8] {for z in [0.2,0.5,0.8] {
                    let p = lo+(hi-lo)*Vec3::new(x,y,z);
                    let selected = state.select(&logic.world,p);
                    if selected.is_some_and(|id| logic.world.ents[id].me.targetname == name) {found=Some((id,p,selected));}
                }}}
            }
            let (volume,point,_selected) = found.unwrap_or_else(|| panic!("No view point selected {name}"));
            println!("Native fog volume verified: {name} at {:?}", Vec3::new(point.x,-point.z,point.y)/crate::collision::UNIT);
            let outside = logic.bounds(volume).unwrap().1 + Vec3::splat(crate::collision::UNIT);
            assert!(!Runtime::contains(&logic.world,volume,outside));
            logic.world.input(volume,"Disable","",None,None);
            state.select(&logic.world,point);
            assert!(!state.registered.contains(&volume),"Disabled volume stays registered");
            logic.world.input(volume,"Enable","",None,None);
            state.select(&logic.world,point);
            assert_eq!(state.registered.last(),Some(&volume));
        }
    }
    #[test]
    fn sky_camera_fog_uses_world_scale_distances_with_far_clip() {
        let entities = crate::kv::entities(
            r#"{"classname" "env_fog_controller" "spawnflags" "1" "fogenable" "1" "fogstart" "256" "fogend" "2500" "farz" "5000"}
{"classname" "sky_camera" "scale" "32" "fogenable" "1" "fogstart" "256" "fogend" "2500" "fogmaxdensity" "1" "fogcolor" "130 117 107" "clip_3D_skybox_near_to_world_far" "1" "clip_3D_skybox_near_to_world_far_offset" "-175"}"#,
        );
        let [fog, color, clip] = sky_globals(&entities);
        let unit = crate::collision::UNIT;
        // Not multiplied by the sky scale (retail c4m1 measurement).
        assert!((fog.x - 256. * unit).abs() < 1e-3);
        assert!((fog.y - 2500. * unit).abs() < 1e-3);
        assert_eq!((fog.z, fog.w), (1., 1.));
        assert!(color.x > 0.1 && color.x < 0.3);
        assert!((clip.x - 5000. * unit).abs() < 1e-3);
        assert_eq!(clip.y, 1.);
        assert!((clip.z + 175. * unit).abs() < 1e-4);
        // No sky_camera: only the far clip is provided.
        let world = crate::kv::entities(
            r#"{"classname" "env_fog_controller" "fogenable" "1" "farz" "3000"}"#,
        );
        assert_eq!(sky_globals(&world)[0], Vec4::ZERO);
    }
    #[test]
    fn far_plane_uses_selected_controller_and_ignores_invalid_values() {
        for value in ["-1", "0", "NaN", "inf"] {
            let entities = crate::kv::entities(&format!(
                r#"{{"classname" "env_fog_controller" "farz" "{value}"}}"#
            ));
            assert!(far_distance(&entities).is_none());
        }
        let entities = crate::kv::entities(
            r#"{"classname" "worldspawn" "fogcontroller" "main"}
            {"classname" "env_fog_controller" "farz" "100"}
            {"classname" "env_fog_controller" "targetname" "main" "farz" "3000"}"#,
        );
        assert_eq!(
            far_distance(&entities),
            Some(3000. * crate::collision::UNIT)
        );
    }
    #[test]
    fn master_fog_uses_native_units_color_and_max_density() {
        let entities = crate::kv::entities(
            r#"{"classname" "env_fog_controller" "fogenable" "0"}
            {"classname" "env_fog_controller" "spawnflags" "1" "fogenable" "1" "fogstart" "300" "fogend" "3000" "fogcolor" "80 120 140" "fogmaxdensity" ".7"}"#,
        );
        let fog = from_entities(&entities).unwrap();
        assert_eq!(
            fog.color,
            Color::srgba(80. / 255., 120. / 255., 140. / 255., 0.7)
        );
        assert!(
            matches!(fog.falloff, FogFalloff::Linear {start, end} if start == 300. * crate::collision::UNIT && end == 3000. * crate::collision::UNIT)
        );
    }
    #[test]
    fn disabled_or_invalid_fog_stays_disabled() {
        for data in [
            r#"{"classname" "env_fog_controller" "fogenable" "0"}"#,
            r#"{"classname" "env_fog_controller" "fogenable" "1" "fogend" "NaN"}"#,
        ] {
            assert!(from_entities(&crate::kv::entities(data)).is_none());
        }
    }
}
