//! On-screen touch controls (Android / iOS, or `--touch` on desktop).
//!
//! Touches are turned into the inputs the game already reads, so movement,
//! weapons and menus need no touch-specific code:
//! - one side: a floating move stick (W/A/S/D; a light push walks with Shift),
//! - elsewhere: drag anywhere free to look (mouse motion),
//! - buttons: fire (left mouse), shove (right mouse), jump, crouch (toggle),
//!   use, reload, next weapon and pause (Escape).
//! While the game is paused, the first tap resumes it (as a left click does).
//!
//! Button placement comes from a `TouchLayout`: built-in presets (Default,
//! Compact, Large, Left-handed) or three custom slots the player edits on the
//! TOUCH & HUD page (Options flyout). Positions are kept relative to a screen
//! edge in units of screen height, so a layout carries across devices, and
//! everything stays inside the safe area (ui_fit::SafeArea).
use bevy::{
    ui::ComputedNode,
    input::{mouse::AccumulatedMouseMotion, touch::Touches},
    prelude::*,
};
use std::collections::HashMap;

/// Radians of look per pixel dragged, relative to the mouse's 0.002.
const LOOK_SCALE: f32 = 1.6;
/// Stick travel (fraction of screen height) for full deflection.
const STICK_RADIUS: f32 = 0.11;
/// Below this deflection the survivor walks (Shift).
const WALK_DEFLECTION: f32 = 0.55;
/// Dead zone of the stick.
const DEAD_ZONE: f32 = 0.18;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Button {
    Fire,
    Shove,
    Jump,
    Crouch,
    Use,
    Reload,
    Next,
    Pause,
}
impl Button {
    pub(crate) const ALL: [Button; 8] = [
        Button::Fire,
        Button::Shove,
        Button::Jump,
        Button::Crouch,
        Button::Use,
        Button::Reload,
        Button::Next,
        Button::Pause,
    ];
    fn label(self) -> &'static str {
        match self {
            Button::Fire => "FIRE",
            Button::Shove => "SHOVE",
            Button::Jump => "JUMP",
            Button::Crouch => "DUCK",
            Button::Use => "USE",
            Button::Reload => "RELOAD",
            Button::Next => "NEXT",
            Button::Pause => "II",
        }
    }
    fn index(self) -> usize {
        Button::ALL.iter().position(|b| *b == self).unwrap_or(0)
    }
}

/// Which screen edge a button's x is measured from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Anchor {
    Left,
    Centre,
    Right,
}
/// One button: x from its anchor edge (towards the middle), y from the top,
/// and radius, all in units of the usable screen height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub anchor: Anchor,
    pub x: f32,
    pub y: f32,
    pub r: f32,
}
const fn right(x: f32, y: f32, r: f32) -> Place {
    Place { anchor: Anchor::Right, x, y, r }
}
/// A complete control layout.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub places: [Place; 8],
    /// The move stick takes the left 45% (true) or the right 45%.
    pub stick_left: bool,
}
impl Default for Layout {
    /// Fire under the right thumb with the others in an arc around it, kept
    /// above the bottom HUD strip (survivor health bars).
    fn default() -> Self {
        let small = 0.065;
        Self {
            places: [
                right(0.17, 0.6, 0.1),           // Fire
                right(0.41, 0.64, small),        // Shove
                right(0.14, 0.8, small),         // Jump
                right(0.34, 0.8, small),         // Crouch
                right(0.21, 0.39, small),        // Use
                right(0.38, 0.45, small),        // Reload
                right(0.36, 0.1, small),         // Next
                Place { anchor: Anchor::Centre, x: 0., y: 0.07, r: 0.055 }, // Pause
            ],
            stick_left: true,
        }
    }
}
impl Layout {
    /// Every button scaled about Fire (positions and radii).
    fn scaled(&self, factor: f32) -> Self {
        let fire = self.places[0];
        let mut out = self.clone();
        for (i, p) in out.places.iter_mut().enumerate() {
            p.r *= factor;
            if i == Button::Pause.index() || i == Button::Next.index() {
                continue;
            }
            if p.anchor == fire.anchor {
                p.x = fire.x + (p.x - fire.x) * factor;
                p.y = fire.y + (p.y - fire.y) * factor;
            }
        }
        out
    }
    /// Left-right mirror (left-handed play).
    fn mirrored(&self) -> Self {
        let mut out = self.clone();
        for p in &mut out.places {
            p.anchor = match p.anchor {
                Anchor::Left => Anchor::Right,
                Anchor::Right => Anchor::Left,
                Anchor::Centre => {
                    p.x = -p.x;
                    Anchor::Centre
                }
            };
        }
        out.stick_left = !out.stick_left;
        out
    }
    /// Button centre and radius in logical pixels inside `usable`.
    pub fn resolve(&self, usable: Rect) -> Vec<(Button, Vec2, f32)> {
        let u = usable.height();
        Button::ALL
            .iter()
            .zip(self.places.iter())
            .map(|(b, p)| {
                let x = match p.anchor {
                    Anchor::Left => usable.min.x + p.x * u,
                    Anchor::Right => usable.max.x - p.x * u,
                    Anchor::Centre => usable.center().x + p.x * u,
                };
                (*b, Vec2::new(x, usable.min.y + p.y * u), p.r * u)
            })
            .collect()
    }
    /// Stores `button` centred at `point` (screen pixels), measured from the
    /// nearest edge so the layout carries over to other screen widths.
    fn place_at(&mut self, button: Button, point: Vec2, usable: Rect) {
        let u = usable.height().max(1.);
        let r = self.places[button.index()].r;
        // Keep the whole button on screen.
        let point = Vec2::new(
            point.x.clamp(usable.min.x + r * u, usable.max.x - r * u),
            point.y.clamp(usable.min.y + r * u, usable.max.y - r * u),
        );
        let third = usable.width() / 3.;
        let (anchor, x) = if point.x < usable.min.x + third {
            (Anchor::Left, (point.x - usable.min.x) / u)
        } else if point.x > usable.max.x - third {
            (Anchor::Right, (usable.max.x - point.x) / u)
        } else {
            (Anchor::Centre, (point.x - usable.center().x) / u)
        };
        self.places[button.index()] = Place { anchor, x, y: (point.y - usable.min.y) / u, r };
    }
    fn encode(&self) -> String {
        let mut out = String::from(if self.stick_left { "L" } else { "R" });
        for p in &self.places {
            let a = match p.anchor {
                Anchor::Left => 'L',
                Anchor::Centre => 'C',
                Anchor::Right => 'R',
            };
            out += &format!(";{a}:{:.4}:{:.4}:{:.4}", p.x, p.y, p.r);
        }
        out
    }
    fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split(';');
        let stick_left = match parts.next()? {
            "L" => true,
            "R" => false,
            _ => return None,
        };
        let mut places = Layout::default().places;
        for place in &mut places {
            let mut f = parts.next()?.split(':');
            let anchor = match f.next()? {
                "L" => Anchor::Left,
                "C" => Anchor::Centre,
                "R" => Anchor::Right,
                _ => return None,
            };
            let mut num = || f.next()?.parse::<f32>().ok().filter(|v| v.is_finite());
            let (x, y, r) = (num()?, num()?, num()?);
            *place = Place { anchor, x, y: y.clamp(0., 1.), r: r.clamp(0.025, 0.2) };
        }
        Some(Self { places, stick_left })
    }
}

/// Built-in presets, then the custom slots.
pub(crate) const PRESETS: [&str; 7] = ["Default", "Compact", "Large", "Left-handed", "Custom 1", "Custom 2", "Custom 3"];
const BUILT_IN: usize = 4;
fn built_in(index: usize) -> Layout {
    match index {
        1 => Layout::default().scaled(0.82),
        2 => Layout::default().scaled(1.18),
        3 => Layout::default().mirrored(),
        _ => Layout::default(),
    }
}

/// The active layout, its preset, the custom slots and button opacity,
/// saved in runtime/interface.ini.
#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct TouchLayout {
    pub preset: usize,
    pub current: Layout,
    pub customs: [Option<Layout>; 3],
    /// Button fill/outline opacity, 0.1..1.
    pub opacity: f32,
}
impl Default for TouchLayout {
    fn default() -> Self {
        Self { preset: 0, current: Layout::default(), customs: [None, None, None], opacity: 0.35 }
    }
}
impl TouchLayout {
    pub fn load() -> Self {
        use crate::game::ui_fit::read_value;
        let text = std::fs::read_to_string(crate::game::ui_fit::INTERFACE_FILE).unwrap_or_default();
        let customs = [1, 2, 3].map(|n| read_value(&text, &format!("touch_custom{n}")).and_then(|v| Layout::decode(&v)));
        let preset = read_value(&text, "touch_preset").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(PRESETS.len() - 1);
        let opacity = read_value(&text, "touch_opacity")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map_or(0.35, |v| v.clamp(0.1, 1.));
        let mut layout = Self { preset, current: Layout::default(), customs, opacity };
        layout.select(preset);
        layout
    }
    pub fn save(&self) {
        let mut values = vec![
            ("touch_preset".to_string(), self.preset.to_string()),
            ("touch_opacity".to_string(), format!("{:.2}", self.opacity)),
        ];
        for (i, custom) in self.customs.iter().enumerate() {
            if let Some(custom) = custom {
                values.push((format!("touch_custom{}", i + 1), custom.encode()));
            }
        }
        crate::game::ui_fit::write_values(&values);
    }
    /// Switches to preset `index` (an empty custom slot starts as Default).
    pub fn select(&mut self, index: usize) {
        self.preset = index % PRESETS.len();
        self.current = if self.preset < BUILT_IN {
            built_in(self.preset)
        } else {
            self.customs[self.preset - BUILT_IN].clone().unwrap_or_default()
        };
    }
    /// Before an edit: built-in presets are copied to the first custom slot
    /// (so they stay as shipped); custom slots are edited in place.
    fn begin_edit(&mut self) {
        if self.preset < BUILT_IN {
            self.preset = BUILT_IN;
        }
    }
    fn end_edit(&mut self) {
        if self.preset >= BUILT_IN {
            self.customs[self.preset - BUILT_IN] = Some(self.current.clone());
        }
        self.save();
    }
}

/// Button centre and radius in logical pixels for a full `width` x `height`
/// screen with the default layout.
pub(crate) fn layout(width: f32, height: f32) -> Vec<(Button, Vec2, f32)> {
    Layout::default().resolve(Rect::new(0., 0., width, height))
}
/// The button under `point`, if any (a little larger than drawn).
fn hit_in(places: &[(Button, Vec2, f32)], point: Vec2) -> Option<Button> {
    places
        .iter()
        .filter(|(_, centre, radius)| centre.distance(point) <= radius * 1.3)
        // The nearest button wins where enlarged tap areas overlap.
        .min_by(|a, b| (a.1.distance(point) / a.2).total_cmp(&(b.1.distance(point) / b.2)))
        .map(|(b, _, _)| *b)
}
/// The default-layout button under `point` on a full screen.
pub(crate) fn hit(width: f32, height: f32, point: Vec2) -> Option<Button> {
    hit_in(&layout(width, height), point)
}
/// Touches starting in the stick's 45% of the usable width (below the top
/// fifth) drive the move stick.
fn in_stick_zone(stick_left: bool, usable: Rect, point: Vec2) -> bool {
    let band = usable.width() * 0.45;
    let side = if stick_left { point.x < usable.min.x + band } else { point.x > usable.max.x - band };
    side && point.y > usable.min.y + usable.height() * 0.2
}
pub(crate) fn is_stick_zone(width: f32, height: f32, point: Vec2) -> bool {
    in_stick_zone(true, Rect::new(0., 0., width, height), point)
}
/// Keys held for a stick deflection (screen space, +y down, length 0..1).
pub(crate) fn stick_keys(deflection: Vec2) -> Vec<KeyCode> {
    let length = deflection.length().min(1.);
    if length < DEAD_ZONE {
        return vec![];
    }
    let d = deflection / deflection.length();
    let mut keys = vec![];
    // 8-way: a component above sin(22.5 deg) presses its key.
    let threshold = 0.38;
    if d.y < -threshold {
        keys.push(KeyCode::KeyW);
    }
    if d.y > threshold {
        keys.push(KeyCode::KeyS);
    }
    if d.x < -threshold {
        keys.push(KeyCode::KeyA);
    }
    if d.x > threshold {
        keys.push(KeyCode::KeyD);
    }
    if length < WALK_DEFLECTION {
        keys.push(KeyCode::ShiftLeft);
    }
    keys
}

#[derive(Resource, Default)]
pub(crate) struct TouchControls {
    pub enabled: bool,
    /// Stick touch id, where it went down, and where it is now.
    stick: Option<(u64, Vec2, Vec2)>,
    look: Option<(u64, Vec2)>,
    buttons: HashMap<u64, Button>,
    /// Last position of each held button touch: dragging a held action
    /// button (fire, shove, use, jump, duck, reload) also looks around, so a
    /// thumb holding FIRE can still aim.
    drags: HashMap<u64, Vec2>,
    crouched: bool,
    /// Logical size of the overlay, from its laid-out root (captures have
    /// no window).
    screen: Vec2,
    keys: Vec<KeyCode>,
    mouse: Vec<MouseButton>,
}
impl TouchControls {
    fn wanted(&self) -> bool {
        self.enabled
    }
}

#[derive(Component)]
pub(crate) struct TouchRoot;
#[derive(Component)]
pub(crate) struct TouchButton(Button);
#[derive(Component)]
pub(crate) struct StickBase;
#[derive(Component)]
pub(crate) struct StickKnob;

pub(crate) fn enabled_by_default() -> bool {
    cfg!(any(target_os = "android", target_os = "ios")) || std::env::args().any(|a| a == "--touch")
}

pub(crate) fn setup(mut commands: Commands, camera: Option<Single<Entity, With<crate::game::ViewCamera>>>) {
    commands.insert_resource(TouchControls { enabled: enabled_by_default(), ..default() });
    let circle = |size: f32| Node {
        position_type: PositionType::Absolute,
        width: Val::Px(size),
        height: Val::Px(size),
        border: UiRect::all(Val::Px(2.)),
        border_radius: BorderRadius::MAX,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    let mut root = commands.spawn_empty();
    if let Some(camera) = camera {
        // Same camera as the HUD (game_ui::target_camera).
        root.insert(UiTargetCamera(*camera));
    }
    root
        .insert((
            TouchRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                ..default()
            },
            Visibility::Hidden,
            // Above the menu (100) so the editor can show the controls.
            GlobalZIndex(110),
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            for button in Button::ALL {
                root.spawn((
                    TouchButton(button),
                    circle(10.),
                    BackgroundColor(Color::srgba(0., 0., 0., 0.35)),
                    BorderColor::all(Color::srgba(1., 1., 1., 0.45)),
                    Pickable::IGNORE,
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(button.label()),
                        TextFont { font_size: FontSize::Vh(1.6), ..default() },
                        TextColor(Color::srgba(1., 1., 1., 0.8)),
                        Pickable::IGNORE,
                    ));
                });
            }
            root.spawn((
                StickBase,
                circle(10.),
                BackgroundColor(Color::srgba(0., 0., 0., 0.25)),
                BorderColor::all(Color::srgba(1., 1., 1., 0.35)),
                Visibility::Hidden,
                Pickable::IGNORE,
            ));
            root.spawn((
                StickKnob,
                circle(10.),
                BackgroundColor(Color::srgba(1., 1., 1., 0.35)),
                BorderColor::all(Color::srgba(1., 1., 1., 0.6)),
                Visibility::Hidden,
                Pickable::IGNORE,
            ));
        });
}

/// Restore the overlay after scene replacement and release old synthetic input.
pub(crate) fn rebuild(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    let previous = world.get_resource::<TouchControls>()
        .map(|c| (c.enabled, c.keys.clone(), c.mouse.clone()));
    if let Some((_, keys, mouse)) = &previous {
        if let Some(mut input) = world.get_resource_mut::<ButtonInput<KeyCode>>() {
            for key in keys { input.release(*key); }
        }
        if let Some(mut input) = world.get_resource_mut::<ButtonInput<MouseButton>>() {
            for button in mouse { input.release(*button); }
        }
    }
    let roots = world.query_filtered::<Entity, Or<(With<TouchRoot>, With<EditorRoot>)>>().iter(world).collect::<Vec<_>>();
    for root in roots { world.despawn(root); }
    world.run_system_once(setup).expect("Rebuild touch controls");
    // The TOUCH & HUD editor targets the same camera; without it the page
    // opened in game shows nothing and has no DONE button.
    world.run_system_once(editor_setup).expect("Rebuild touch editor");
    if previous.is_some_and(|p| p.0) { world.resource_mut::<TouchControls>().enabled = true; }
}

/// Teammate panels' new offset (screen-height fractions) for a drag of
/// `delta` logical pixels that started with the group at `rect`; the whole
/// group stays inside the usable `area`.
fn team_offset_for_drag(delta: Vec2, rect: Rect, area: Rect, height: f32, start: Vec2) -> Vec2 {
    let low = area.min - rect.min;
    let high = area.max - rect.max;
    let clamped = Vec2::new(
        delta.x.clamp(low.x.min(0.), high.x.max(0.)),
        delta.y.clamp(low.y.min(0.), high.y.max(0.)),
    );
    start + clamped / height.max(1.)
}
/// The usable screen rectangle (safe area) for a `size` screen.
fn usable(size: Vec2, safe: Option<&crate::game::ui_fit::SafeArea>) -> Rect {
    safe.copied().unwrap_or_default().rect(size)
}

/// PreUpdate, after Bevy's input systems: turn touches into game input.
#[allow(clippy::too_many_arguments)]
pub(crate) fn inject(
    touches: Res<Touches>,
    // The first window, like game::controls (captures have no PrimaryWindow).
    window: Query<&Window>,
    mut controls: ResMut<TouchControls>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut motion: ResMut<AccumulatedMouseMotion>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    mut state: Option<ResMut<crate::game_state::Match>>,
    placement: (Option<Res<TouchLayout>>, Option<Res<crate::game::ui_fit::SafeArea>>, Option<Res<crate::game::ui_fit::Interface>>),
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
) {
    if !controls.enabled && touches.iter_just_pressed().next().is_some() {
        // A real touchscreen: turn the controls on.
        controls.enabled = true;
    }
    let playing = frontend.as_ref().is_some_and(|f| f.playing());
    let size = window.iter().next().map_or(controls.screen, |w| Vec2::new(w.width(), w.height()));
    if size.x <= 0. || size.y <= 0. {
        return;
    }
    let area = usable(size, placement.1.as_deref());
    let default_layout = Layout::default();
    let current = placement.0.as_deref().map_or(&default_layout, |l| &l.current);
    let places = current.resolve(area);
    let height = area.height();
    let look_scale = LOOK_SCALE
        * placement.2.as_ref().map_or(crate::game::ui_fit::DEFAULT_LOOK_SENSITIVITY, |i| i.look_sensitivity);
    let paused = state.as_ref().is_some_and(|s| s.paused);
    // Dead: only the pause (menu) button stays live.
    let dead = state.as_ref().is_some_and(|s| s.dead);
    // Hidden and ignored while a map cutscene (intro camera) plays.
    let active = controls.wanted() && playing && !cutscene(cinematic.as_deref());
    let mut want_keys: Vec<KeyCode> = vec![];
    let mut want_mouse: Vec<MouseButton> = vec![];
    let mut tap_resume = false;
    let mut next_weapon = false;
    let mut pause = false;
    if active {
        for touch in touches.iter_just_pressed() {
            let point = touch.position();
            if paused {
                tap_resume = true;
                continue;
            }
            if dead {
                if hit_in(&places, point) == Some(Button::Pause) {
                    pause = true;
                }
                continue;
            }
            if let Some(button) = hit_in(&places, point) {
                controls.buttons.insert(touch.id(), button);
                if !matches!(button, Button::Pause | Button::Next) {
                    controls.drags.insert(touch.id(), point);
                }
                match button {
                    Button::Crouch => controls.crouched = !controls.crouched,
                    Button::Next => next_weapon = true,
                    Button::Pause => pause = true,
                    _ => {}
                }
            } else if controls.stick.is_none() && in_stick_zone(current.stick_left, area, point) {
                controls.stick = Some((touch.id(), point, point));
            } else if controls.look.is_none() {
                controls.look = Some((touch.id(), point));
            }
        }
        // A look finger whose release was never seen must not block look.
        if let Some((id, _)) = controls.look {
            if touches.get_pressed(id).is_none() {
                controls.look = None;
            }
        }
        controls.drags.retain(|id, _| touches.get_pressed(*id).is_some());
        for touch in touches.iter() {
            if let Some(previous) = controls.drags.get(&touch.id()).copied() {
                // Only when no separate look finger is down, so two fingers
                // never double the turn speed.
                if controls.look.is_none() {
                    motion.delta += (touch.position() - previous) * look_scale;
                }
                controls.drags.insert(touch.id(), touch.position());
            }
            if let Some((id, origin, _)) = controls.stick {
                if id == touch.id() {
                    controls.stick = Some((id, origin, touch.position()));
                }
            }
            if let Some((id, previous)) = controls.look {
                if id == touch.id() {
                    // Bevy retains Touch::delta when no new touch event
                    // arrives. Track our consumed position to prevent drift.
                    motion.delta += (touch.position() - previous) * look_scale;
                    controls.look = Some((id, touch.position()));
                }
            }
        }
    }
    for touch in touches.iter_just_released().chain(touches.iter_just_canceled()) {
        controls.buttons.remove(&touch.id());
        controls.drags.remove(&touch.id());
        if controls.stick.is_some_and(|(id, _, _)| id == touch.id()) {
            controls.stick = None;
        }
        if controls.look.is_some_and(|(id, _)| id == touch.id()) {
            controls.look = None;
        }
    }
    if !active || dead {
        controls.stick = None;
        controls.look = None;
        controls.buttons.clear();
        controls.drags.clear();
        controls.crouched = false;
    }
    if let Some((_, origin, now)) = controls.stick {
        want_keys.extend(stick_keys((now - origin) / (STICK_RADIUS * height)));
    }
    for button in controls.buttons.values() {
        match button {
            Button::Fire => want_mouse.push(MouseButton::Left),
            Button::Shove => want_mouse.push(MouseButton::Right),
            Button::Jump => want_keys.push(KeyCode::Space),
            Button::Use => want_keys.push(KeyCode::KeyE),
            Button::Reload => want_keys.push(KeyCode::KeyR),
            Button::Crouch | Button::Next | Button::Pause => {}
        }
    }
    if controls.crouched {
        want_keys.push(KeyCode::ControlLeft);
    }
    if tap_resume {
        // One frame of left click, which is what resumes a paused game.
        want_mouse.push(MouseButton::Left);
    }
    if pause {
        keys.press(KeyCode::Escape);
        controls.keys.push(KeyCode::Escape);
    }
    // Release only what touch pressed and no longer wants.
    let held = std::mem::take(&mut controls.keys);
    for key in held {
        if !want_keys.contains(&key) {
            keys.release(key);
        }
    }
    for key in &want_keys {
        keys.press(*key);
    }
    controls.keys = want_keys;
    let held = std::mem::take(&mut controls.mouse);
    for button in held {
        if !want_mouse.contains(&button) {
            mouse.release(button);
        }
    }
    for button in &want_mouse {
        mouse.press(*button);
    }
    controls.mouse = if tap_resume { vec![] } else { want_mouse };
    if tap_resume {
        // Released next frame (not held as a shot).
        controls.mouse.push(MouseButton::Left);
    }
    if next_weapon {
        if let Some(state) = state.as_mut() {
            state.cycle_inventory(1);
        }
    }
}

/// A map cutscene is playing (the scripted intro/outro camera).
fn cutscene(logic: Option<&crate::l4d2r_cinematic::MapLogic>) -> bool {
    logic.is_some_and(|l| l.active)
}

fn editing(frontend: Option<&crate::game_menu::Frontend>) -> bool {
    frontend.is_some_and(|f| f.page == crate::game_menu::Page::Touch)
}

/// Place and show the on-screen controls (also on the TOUCH & HUD page).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    mut controls: ResMut<TouchControls>,
    frontend: Option<Res<crate::game_menu::Frontend>>,
    state: Option<Res<crate::game_state::Match>>,
    placement: (Option<Res<TouchLayout>>, Option<Res<crate::game::ui_fit::SafeArea>>, Option<Res<TouchEditor>>),
    mut root: Query<(&mut Visibility, &ComputedNode), (With<TouchRoot>, Without<StickBase>, Without<StickKnob>)>,
    mut buttons: Query<(&TouchButton, &mut Node, &mut BackgroundColor, &mut BorderColor, &Children), (Without<StickBase>, Without<StickKnob>)>,
    mut labels: Query<(&mut TextColor, &mut TextFont)>,
    mut base: Query<(&mut Node, &mut Visibility), (With<StickBase>, Without<StickKnob>, Without<TouchRoot>, Without<TouchButton>)>,
    mut knob: Query<(&mut Node, &mut Visibility), (With<StickKnob>, Without<StickBase>, Without<TouchRoot>, Without<TouchButton>)>,
    cinematic: Option<Res<crate::l4d2r_cinematic::MapLogic>>,
) {
    let edit = controls.enabled && editing(frontend.as_deref());
    let show = edit
        || (controls.enabled
            && frontend.as_ref().is_some_and(|f| f.playing())
            && !state.as_ref().is_some_and(|s| s.paused)
            && !cutscene(cinematic.as_deref()));
    // Dead: only the pause (menu) button shows.
    let dead = !edit && state.as_ref().is_some_and(|s| s.dead);
    let mut size = Vec2::ZERO;
    for (mut v, node) in &mut root {
        v.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
        size = node.size() * node.inverse_scale_factor();
    }
    if size.x > 0. && controls.screen != size {
        controls.screen = size;
    }
    if !show || size.x <= 0. {
        return;
    }
    let area = usable(size, placement.1.as_deref());
    let default_layout = TouchLayout::default();
    let layout = placement.0.as_deref().unwrap_or(&default_layout);
    let selected = placement.2.as_deref().and_then(|e| e.selected).filter(|_| edit);
    let alpha = layout.opacity;
    let pressed: Vec<Button> = controls.buttons.values().copied().collect();
    for (button, centre, radius) in layout.current.resolve(area) {
        for (b, mut node, mut color, mut border, children) in &mut buttons {
            if b.0 != button {
                continue;
            }
            let display = if dead && button != Button::Pause { Display::None } else { Display::Flex };
            if node.display != display {
                node.display = display;
            }
            node.left = Val::Px(centre.x - radius);
            node.top = Val::Px(centre.y - radius);
            node.width = Val::Px(radius * 2.);
            node.height = Val::Px(radius * 2.);
            let on = pressed.contains(&button) || (button == Button::Crouch && controls.crouched);
            color.0 = if on || selected == Some(button) {
                Color::srgba(0.75, 0.1, 0.1, (alpha + 0.2).min(1.))
            } else {
                Color::srgba(0., 0., 0., alpha)
            };
            *border = BorderColor::all(if selected == Some(button) {
                Color::srgb(0.94, 0., 0.)
            } else {
                Color::srgba(1., 1., 1., (alpha + 0.1).min(1.))
            });
            for child in children.iter() {
                if let Ok((mut text, mut font)) = labels.get_mut(child) {
                    text.0 = Color::srgba(1., 1., 1., (alpha * 2.).clamp(0.5, 1.));
                    // Label size follows the button.
                    font.font_size = FontSize::Px((radius * 0.42).max(8.));
                }
            }
        }
    }
    let radius = STICK_RADIUS * area.height();
    let place = |node: &mut Node, at: Vec2, r: f32| {
        node.left = Val::Px(at.x - r);
        node.top = Val::Px(at.y - r);
        node.width = Val::Px(r * 2.);
        node.height = Val::Px(r * 2.);
    };
    // In the editor the panel covers the stick's side; no stick preview.
    let stick = if edit || dead { None } else { controls.stick };
    for (mut node, mut v) in &mut base {
        if let Some((_, origin, _)) = stick {
            place(&mut node, origin, radius);
        }
        v.set_if_neq(if stick.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
    for (mut node, mut v) in &mut knob {
        if let Some((_, origin, now)) = stick {
            let offset = (now - origin).clamp_length_max(radius);
            place(&mut node, origin + offset, radius * 0.45);
        }
        v.set_if_neq(if stick.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
}

// ---------------------------------------------------------------------------
// TOUCH & HUD page

/// Editor state: the selected button and an active drag (pointer id, button,
/// grab offset from its centre). Mouse drags use pointer id u64::MAX.
#[derive(Resource, Default)]
pub(crate) struct TouchEditor {
    selected: Option<Button>,
    drag: Option<(u64, Button, Vec2)>,
    /// Teammate panels drag: pointer, start point, start offset, start rect.
    hud_drag: Option<(u64, Vec2, Vec2, Rect)>,
    /// PREVIEW hides the panel to show the whole HUD.
    panel_hidden: bool,
}
/// The SETTINGS button shown while the panel is hidden.
#[derive(Component)]
pub(crate) struct ShowPanelButton;
#[derive(Component)]
pub(crate) struct EditorRoot;
#[derive(Component)]
pub(crate) struct EditorPanel;
/// Rows only meaningful with touch controls (hidden on desktop without them).
#[derive(Component)]
pub(crate) struct TouchOnly;
#[derive(Component, Clone, Copy, PartialEq)]
pub(crate) enum EditAction {
    PreviousPreset,
    NextPreset,
    Smaller,
    Bigger,
    Fainter,
    Bolder,
    SwapSides,
    Reset,
    HudSmaller,
    HudBigger,
    LookSlower,
    LookFaster,
    RenderLower,
    RenderHigher,
    Preview,
    ShowPanel,
    Done,
}
#[derive(Component, Clone, Copy, PartialEq)]
pub(crate) enum EditorText {
    Preset,
    Size,
    Opacity,
    Hud,
    Look,
    Render,
}

pub(crate) fn editor_setup(mut commands: Commands, camera: Option<Single<Entity, With<crate::game::ViewCamera>>>) {
    commands.init_resource::<TouchEditor>();
    let mut root = commands.spawn_empty();
    if let Some(camera) = camera {
        root.insert(UiTargetCamera(*camera));
    }
    let root = root
        .insert((
            EditorRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.),
                height: Val::Percent(100.),
                ..default()
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.35)),
            Visibility::Hidden,
            GlobalZIndex(105),
            Pickable::IGNORE,
        ))
        .id();
    let panel = commands
        .spawn((
            EditorPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Vh(5.),
                width: Val::Vh(74.),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Vh(1.6)),
                row_gap: Val::Vh(0.9),
                border: UiRect::all(Val::Px(1.)),
                ..default()
            },
            BackgroundColor(Color::srgba(0., 0., 0., 0.82)),
            BorderColor::all(Color::srgba(1., 1., 1., 0.25)),
            ChildOf(root),
        ))
        .id();
    let title = |text: &str| (
        Text::new(text),
        TextFont { font_size: FontSize::Vh(4.), ..default() },
        TextColor(Color::WHITE),
    );
    commands.spawn((title("TOUCH & HUD"), ChildOf(panel)));
    commands.spawn((
        Text::new("Drag a control to move it; tap one to change its size. Drag the survivor health panels to move them. PREVIEW shows the whole HUD."),
        TextFont { font_size: FontSize::Vh(2.3), ..default() },
        TextColor(Color::srgb(0.7, 0.7, 0.7)),
        TouchOnly,
        ChildOf(panel),
    ));
    let row = |commands: &mut Commands, touch_only: bool| {
        let mut e = commands.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Vh(1.2),
                ..default()
            },
            ChildOf(panel),
        ));
        if touch_only {
            e.insert(TouchOnly);
        }
        e.id()
    };
    let button = |commands: &mut Commands, parent: Entity, label: &str, action: EditAction, width: f32| {
        commands
            .spawn((
                bevy::prelude::Button,
                action,
                Node {
                    width: Val::Vh(width),
                    height: Val::Vh(7.),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border: UiRect::all(Val::Px(1.)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.12, 0.12, 0.12, 0.95)),
                BorderColor::all(Color::srgba(1., 1., 1., 0.3)),
                ChildOf(parent),
            ))
            .with_child((
                Text::new(label),
                TextFont { font_size: FontSize::Vh(3.2), ..default() },
                TextColor(Color::WHITE),
                Pickable::IGNORE,
            ));
    };
    let value = |commands: &mut Commands, parent: Entity, kind: EditorText, width: f32| {
        commands.spawn((
            Text::new(""),
            kind,
            TextFont { font_size: FontSize::Vh(3.), ..default() },
            TextColor(Color::WHITE),
            TextLayout::default().with_justify(Justify::Center),
            Node { width: Val::Vh(width), ..default() },
            ChildOf(parent),
        ));
    };
    let r = row(&mut commands, true);
    button(&mut commands, r, "<", EditAction::PreviousPreset, 10.);
    value(&mut commands, r, EditorText::Preset, 44.);
    button(&mut commands, r, ">", EditAction::NextPreset, 10.);
    let r = row(&mut commands, true);
    button(&mut commands, r, "-", EditAction::Smaller, 10.);
    value(&mut commands, r, EditorText::Size, 44.);
    button(&mut commands, r, "+", EditAction::Bigger, 10.);
    let r = row(&mut commands, true);
    button(&mut commands, r, "-", EditAction::Fainter, 10.);
    value(&mut commands, r, EditorText::Opacity, 44.);
    button(&mut commands, r, "+", EditAction::Bolder, 10.);
    let r = row(&mut commands, false);
    button(&mut commands, r, "-", EditAction::HudSmaller, 10.);
    value(&mut commands, r, EditorText::Hud, 44.);
    button(&mut commands, r, "+", EditAction::HudBigger, 10.);
    let r = row(&mut commands, true);
    button(&mut commands, r, "-", EditAction::LookSlower, 10.);
    value(&mut commands, r, EditorText::Look, 44.);
    button(&mut commands, r, "+", EditAction::LookFaster, 10.);
    let r = row(&mut commands, true);
    button(&mut commands, r, "-", EditAction::RenderLower, 10.);
    value(&mut commands, r, EditorText::Render, 44.);
    button(&mut commands, r, "+", EditAction::RenderHigher, 10.);
    let r = row(&mut commands, false);
    button(&mut commands, r, "SWAP SIDES", EditAction::SwapSides, 32.);
    button(&mut commands, r, "RESET", EditAction::Reset, 33.2);
    let r = row(&mut commands, false);
    button(&mut commands, r, "PREVIEW", EditAction::Preview, 32.);
    button(&mut commands, r, "DONE", EditAction::Done, 33.2);
    // While previewing: one small button brings the panel back.
    commands
        .spawn((
            bevy::prelude::Button,
            EditAction::ShowPanel,
            ShowPanelButton,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Vh(3.),
                width: Val::Vh(30.),
                height: Val::Vh(8.),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.12, 0.12, 0.12, 0.95)),
            BorderColor::all(Color::srgba(1., 1., 1., 0.3)),
            ChildOf(root),
        ))
        .with_child((
            Text::new("SETTINGS"),
            TextFont { font_size: FontSize::Vh(3.4), ..default() },
            TextColor(Color::WHITE),
            Pickable::IGNORE,
        ));
}

/// Shows the TOUCH & HUD page, handles its buttons and drags controls.
#[allow(clippy::too_many_arguments)]
pub(crate) fn editor_update(
    mut frontend: Option<ResMut<crate::game_menu::Frontend>>,
    controls: Option<Res<TouchControls>>,
    mut layout: Option<ResMut<TouchLayout>>,
    mut interface: Option<ResMut<crate::game::ui_fit::Interface>>,
    mut editor: ResMut<TouchEditor>,
    safe: Option<Res<crate::game::ui_fit::SafeArea>>,
    input: (Res<Touches>, Res<ButtonInput<MouseButton>>, Query<&Window>),
    mut roots: Query<&mut Visibility, With<EditorRoot>>,
    mut panel: Query<(&mut Node, &ComputedNode, &UiGlobalTransform), (With<EditorPanel>, Without<ShowPanelButton>)>,
    mut touch_only: Query<&mut Node, (With<TouchOnly>, Without<EditorPanel>, Without<ShowPanelButton>)>,
    pressed: Query<(&Interaction, &EditAction), Changed<Interaction>>,
    mut texts: Query<(&EditorText, &mut Text)>,
    mut show_button: Query<(&mut Node, &ComputedNode, &UiGlobalTransform), (With<ShowPanelButton>, Without<EditorPanel>, Without<TouchOnly>)>,
    team: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<crate::game_ui::TeamPanel>>,
) {
    let (touches, mouse, windows) = input;
    let open = editing(frontend.as_deref());
    for mut v in &mut roots {
        v.set_if_neq(if open { Visibility::Inherited } else { Visibility::Hidden });
    }
    if !open {
        editor.drag = None;
        editor.hud_drag = None;
        editor.panel_hidden = false;
        return;
    }
    let touch_enabled = controls.as_ref().is_some_and(|c| c.enabled);
    for mut node in &mut touch_only {
        let display = if touch_enabled { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    // Captures have no window: use the overlay's laid-out size.
    let window = windows.iter().next();
    let size = window.map_or(controls.as_ref().map_or(Vec2::ZERO, |c| c.screen), |w| Vec2::new(w.width(), w.height()));
    if size.x <= 0. || size.y <= 0. {
        return;
    }
    let area = usable(size, safe.as_deref());
    let Some(layout) = layout.as_mut() else { return };
    // The panel sits on the side away from the buttons (where the stick is).
    let mut panel_rect = Rect::default();
    let hidden = editor.panel_hidden;
    for (mut node, computed, global) in &mut show_button {
        let display = if hidden { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        let left = Val::Px(area.min.x + area.height() * 0.04);
        if node.left != left {
            node.left = left;
        }
        if hidden {
            panel_rect = Rect::from_center_size(
                global.translation * computed.inverse_scale_factor(),
                computed.size() * computed.inverse_scale_factor(),
            );
        }
    }
    for (mut node, computed, global) in &mut panel {
        let display = if hidden { Display::None } else { Display::Flex };
        if node.display != display {
            node.display = display;
        }
        if hidden {
            continue;
        }
        let width = computed.size().x * computed.inverse_scale_factor();
        let margin = area.height() * 0.04;
        let left = if layout.current.stick_left || !touch_enabled {
            area.min.x + margin
        } else {
            area.max.x - margin - width
        };
        if node.left != Val::Px(left) {
            node.left = Val::Px(left);
        }
        let centre = global.translation * computed.inverse_scale_factor();
        panel_rect = Rect::from_center_size(centre, computed.size() * computed.inverse_scale_factor());
    }
    // Panel buttons.
    for (interaction, action) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            EditAction::PreviousPreset => {
                let n = (layout.preset + PRESETS.len() - 1) % PRESETS.len();
                layout.select(n);
                editor.selected = None;
                layout.save();
            }
            EditAction::NextPreset => {
                let n = (layout.preset + 1) % PRESETS.len();
                layout.select(n);
                editor.selected = None;
                layout.save();
            }
            EditAction::Smaller | EditAction::Bigger => {
                let factor = if *action == EditAction::Bigger { 1.1 } else { 1. / 1.1 };
                layout.begin_edit();
                match editor.selected {
                    Some(b) => {
                        let p = &mut layout.current.places[b.index()];
                        p.r = (p.r * factor).clamp(0.025, 0.2);
                    }
                    None => {
                        let scaled = layout.current.scaled(factor);
                        layout.current = scaled;
                    }
                }
                layout.end_edit();
            }
            EditAction::Fainter | EditAction::Bolder => {
                let step = if *action == EditAction::Bolder { 0.1 } else { -0.1 };
                layout.opacity = (layout.opacity + step).clamp(0.1, 1.);
                layout.save();
            }
            EditAction::SwapSides => {
                layout.begin_edit();
                let mirrored = layout.current.mirrored();
                layout.current = mirrored;
                layout.end_edit();
            }
            EditAction::LookSlower | EditAction::LookFaster => {
                if let Some(interface) = interface.as_mut() {
                    let factor = if *action == EditAction::LookFaster { 1.25 } else { 0.8 };
                    let v = (interface.look_sensitivity * factor * 100.).round() / 100.;
                    interface.look_sensitivity = v.clamp(0.25, 4.);
                    interface.save();
                }
            }
            EditAction::RenderLower | EditAction::RenderHigher => {
                if let Some(interface) = interface.as_mut() {
                    let step = if *action == EditAction::RenderHigher { 0.5 } else { -0.5 };
                    interface.render_scale = (interface.render_scale + step).clamp(1., 3.);
                    interface.save();
                }
            }
            EditAction::Preview => {
                editor.panel_hidden = true;
                editor.selected = None;
            }
            EditAction::ShowPanel => {
                editor.panel_hidden = false;
            }
            EditAction::Reset => {
                if let Some(interface) = interface.as_mut() {
                    interface.team_offset = Vec2::ZERO;
                    interface.save();
                }
                let preset = layout.preset;
                if preset >= BUILT_IN {
                    layout.customs[preset - BUILT_IN] = None;
                }
                layout.select(preset);
                layout.save();
                editor.selected = None;
            }
            EditAction::HudSmaller | EditAction::HudBigger => {
                if let Some(interface) = interface.as_mut() {
                    let step = if *action == EditAction::HudBigger { 0.1 } else { -0.1 };
                    interface.hud_scale = ((interface.hud_scale + step) * 10.).round() / 10.;
                    interface.hud_scale = interface.hud_scale.clamp(0.6, 1.6);
                    interface.save();
                }
            }
            EditAction::Done => {
                if let Some(frontend) = frontend.as_mut() {
                    frontend.page = crate::game_menu::Page::Options;
                }
                editor.selected = None;
                editor.drag = None;
                editor.hud_drag = None;
                editor.panel_hidden = false;
                return;
            }
        }
    }
    // Drags: touches, or the mouse on desktop.
    let pointer = |id: u64| {
        if id == u64::MAX {
            window.and_then(|w| w.cursor_position()).filter(|_| mouse.pressed(MouseButton::Left))
        } else {
            touches.get_pressed(id).map(|t| t.position())
        }
    };
    let places = layout.current.resolve(area);
    let mut presses: Vec<(u64, Vec2)> = touches.iter_just_pressed().map(|t| (t.id(), t.position())).collect();
    if mouse.just_pressed(MouseButton::Left) {
        if let Some(p) = window.and_then(|w| w.cursor_position()) {
            presses.push((u64::MAX, p));
        }
    }
    // The teammate panels, as one rectangle (they move together).
    let team_rect = team
        .iter()
        .filter(|(_, _, v)| v.get())
        .map(|(computed, global, _)| {
            Rect::from_center_size(
                global.translation * computed.inverse_scale_factor(),
                computed.size() * computed.inverse_scale_factor(),
            )
        })
        .reduce(|a, b| a.union(b));
    for (id, point) in presses {
        if panel_rect.contains(point) || editor.drag.is_some() || editor.hud_drag.is_some() {
            continue;
        }
        if let Some(b) = hit_in(&places, point).filter(|_| touch_enabled) {
            let centre = places[b.index()].1;
            editor.selected = Some(b);
            editor.drag = Some((id, b, point - centre));
        } else if let Some(rect) = team_rect.filter(|r| r.contains(point)) {
            let offset = interface.as_ref().map_or(Vec2::ZERO, |i| i.team_offset);
            editor.hud_drag = Some((id, point, offset, rect));
            editor.selected = None;
        } else {
            editor.selected = None;
        }
    }
    if let Some((id, button, offset)) = editor.drag {
        match pointer(id) {
            Some(p) => {
                let target = p - offset;
                let before = layout.current.places[button.index()];
                let mut moved = layout.current.clone();
                moved.place_at(button, target, area);
                if moved.places[button.index()] != before {
                    layout.begin_edit();
                    layout.current = moved;
                }
            }
            None => {
                editor.drag = None;
                layout.end_edit();
            }
        }
    }
    if let Some((id, start, offset, rect)) = editor.hud_drag {
        match pointer(id) {
            Some(p) => {
                let wanted = team_offset_for_drag(p - start, rect, area, size.y, offset);
                if let Some(interface) = interface.as_mut() {
                    if interface.team_offset != wanted {
                        interface.team_offset = wanted;
                    }
                }
            }
            None => {
                editor.hud_drag = None;
                if let Some(interface) = interface.as_ref() {
                    interface.save();
                }
            }
        }
    }
    // Labels.
    let hud = interface.as_ref().map_or(1., |i| i.hud_scale);
    for (kind, mut text) in &mut texts {
        let value = match kind {
            EditorText::Preset => format!("PRESET: {}", PRESETS[layout.preset].to_uppercase()),
            EditorText::Size => match editor.selected {
                Some(b) => format!("{} SIZE", b.label()),
                None => "ALL CONTROLS SIZE".into(),
            },
            EditorText::Opacity => format!("OPACITY {:.0}%", layout.opacity * 100.),
            EditorText::Hud => format!("HUD SIZE {:.0}%", hud * 100.),
            EditorText::Look => format!(
                "LOOK SPEED {:.0}%",
                interface.as_ref().map_or(crate::game::ui_fit::DEFAULT_LOOK_SENSITIVITY, |i| i.look_sensitivity) * 100.
            ),
            EditorText::Render => format!(
                "RENDER RES {:.0}%",
                interface.as_ref().map_or(crate::game::ui_fit::DEFAULT_RENDER_SCALE, |i| i.render_scale) / 3. * 100.
            ),
        };
        if text.0 != value {
            text.0 = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::touch::{TouchInput, TouchPhase};
    use bevy::window::PrimaryWindow;
    #[test]
    fn map_reload_rebuilds_the_touch_editor_on_the_new_camera() {
        let mut world = World::new();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<ButtonInput<MouseButton>>();
        let old_camera = world.spawn(crate::game::ViewCamera).id();
        rebuild(&mut world);
        world.despawn(old_camera);
        let camera = world.spawn(crate::game::ViewCamera).id();
        rebuild(&mut world);
        let roots: Vec<_> = world
            .query_filtered::<&UiTargetCamera, With<EditorRoot>>()
            .iter(&world)
            .map(|t| t.0)
            .collect();
        assert_eq!(roots, vec![camera], "one editor, on the new camera");
        assert_eq!(world.query_filtered::<(), With<EditorPanel>>().iter(&world).count(), 1);
        let done = world
            .query::<&EditAction>()
            .iter(&world)
            .filter(|a| **a == EditAction::Done)
            .count();
        assert_eq!(done, 1, "the in-game editor must offer DONE");
    }
    #[test]
    fn survivor_panels_drag_as_a_group_and_stay_on_screen() {
        let area = Rect::new(59., 0., 793., 383.);
        let group = Rect::new(60., 330., 420., 383.);
        let height = 393.;
        let moved = team_offset_for_drag(Vec2::new(100., -50.), group, area, height, Vec2::ZERO);
        assert!((moved - Vec2::new(100., -50.) / height).length() < 1e-6);
        // Past the left/bottom edges: clamped to the safe area.
        let clamped = team_offset_for_drag(Vec2::new(-80., 90.), group, area, height, Vec2::ZERO);
        assert!((clamped - Vec2::new(-1., 0.) / height).length() < 1e-6, "{clamped:?}");
        // Far right: the whole group stays inside.
        let right = team_offset_for_drag(Vec2::new(900., 0.), group, area, height, Vec2::ZERO);
        assert!((right.x * height - (793. - 420.)).abs() < 1e-3);
    }
    #[test]
    fn map_reload_rebuilds_all_touch_buttons_on_the_new_camera() {
        let mut world = World::new();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<ButtonInput<MouseButton>>();
        let old_camera = world.spawn(crate::game::ViewCamera).id();
        rebuild(&mut world);
        world.resource_mut::<TouchControls>().enabled = true;
        world.resource_mut::<TouchControls>().keys.push(KeyCode::KeyW);
        world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
        let root = world.query_filtered::<Entity, With<TouchRoot>>().single(&world).unwrap();
        world.despawn(root);
        world.despawn(old_camera);
        let camera = world.spawn(crate::game::ViewCamera).id();
        rebuild(&mut world);
        rebuild(&mut world); // Repeated rebuilds must not duplicate the overlay.
        let root = world.query_filtered::<Entity, With<TouchRoot>>().single(&world).unwrap();
        assert_eq!(world.get::<UiTargetCamera>(root).unwrap().0, camera);
        assert_eq!(world.query::<&TouchButton>().iter(&world).count(), 8);
        assert!(world.resource::<TouchControls>().enabled);
        assert!(!world.resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
    }
    #[test]
    fn stick_maps_to_eight_way_keys_and_walks_when_pushed_lightly() {
        assert!(stick_keys(Vec2::new(0.05, 0.)).is_empty());
        assert_eq!(stick_keys(Vec2::new(0., -1.)), vec![KeyCode::KeyW]);
        assert_eq!(stick_keys(Vec2::new(0.7, -0.7)), vec![KeyCode::KeyW, KeyCode::KeyD]);
        assert_eq!(stick_keys(Vec2::new(-0.3, 0.)), vec![KeyCode::KeyA, KeyCode::ShiftLeft]);
    }
    #[test]
    fn buttons_do_not_overlap_and_fit_on_a_phone_screen() {
        for (w, h) in [(2400., 1080.), (1920., 1080.), (1334., 750.), (2778., 1284.), (1024., 768.)] {
            for preset in 0..BUILT_IN {
                let layout = built_in(preset);
                let area = Rect::new(0., 0., w, h);
                let l = layout.resolve(area);
                for (i, (a, ca, ra)) in l.iter().enumerate() {
                    assert!(ca.x - ra >= 0. && ca.x + ra <= w && ca.y - ra >= 0. && ca.y + ra <= h, "{a:?} off screen at {w}x{h} preset {preset}");
                    assert!(!in_stick_zone(layout.stick_left, area, *ca) || *a == Button::Pause, "{a:?} in the stick zone, preset {preset}");
                    for (b, cb, rb) in &l[i + 1..] {
                        assert!(ca.distance(*cb) >= ra + rb, "{a:?} overlaps {b:?} at {w}x{h} preset {preset}");
                    }
                    assert_eq!(hit_in(&l, *ca), Some(*a));
                }
            }
        }
    }
    #[test]
    fn layouts_survive_saving_and_follow_the_nearest_edge() {
        let mut layout = Layout::default();
        let area = Rect::new(59., 0., 793., 393.);
        // Dragged next to the left edge: measured from the left.
        layout.place_at(Button::Jump, Vec2::new(120., 200.), area);
        let p = layout.places[Button::Jump.index()];
        assert_eq!(p.anchor, Anchor::Left);
        let back = layout.resolve(area)[Button::Jump.index()].1;
        assert!((back - Vec2::new(120., 200.)).length() < 0.01, "{back}");
        let decoded = Layout::decode(&layout.encode()).unwrap();
        for (a, b) in decoded.places.iter().zip(layout.places.iter()) {
            assert_eq!(a.anchor, b.anchor);
            assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3 && (a.r - b.r).abs() < 1e-3);
        }
        // Left-handed moves the stick to the right and the buttons left.
        let left = built_in(3);
        assert!(!left.stick_left);
        assert_eq!(left.places[0].anchor, Anchor::Left);
    }
    #[test]
    fn editing_a_built_in_preset_saves_it_as_a_custom_slot() {
        let mut layout = TouchLayout::default();
        layout.select(1);
        layout.begin_edit();
        layout.current.places[0].r = 0.15;
        layout.preset_customs_only_for_test();
        assert_eq!(layout.preset, BUILT_IN);
        assert_eq!(layout.customs[0].as_ref().unwrap().places[0].r, 0.15);
        // The shipped preset is unchanged.
        assert_ne!(built_in(1).places[0].r, 0.15);
    }
    impl TouchLayout {
        fn preset_customs_only_for_test(&mut self) {
            if self.preset >= BUILT_IN {
                self.customs[self.preset - BUILT_IN] = Some(self.current.clone());
            }
        }
    }
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
            .insert_resource(TouchControls { enabled: true, ..default() })
            .insert_resource(crate::game_menu::Frontend::playing_for_test())
            .add_systems(PreUpdate, inject.after(bevy::input::InputSystems));
        app.world_mut().spawn((
            Window { resolution: bevy::window::WindowResolution::new(2000, 1000), ..default() },
            PrimaryWindow,
        ));
        app
    }
    fn touch(app: &mut App, id: u64, phase: TouchPhase, at: Vec2) {
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        app.world_mut().write_message(TouchInput { phase, position: at, window, force: None, id });
    }
    #[test]
    fn touches_drive_the_stick_look_and_fire() {
        let mut app = app();
        app.update();
        // Stick: down on the left, drag up.
        touch(&mut app, 1, TouchPhase::Started, Vec2::new(300., 700.));
        // Fire button.
        let fire = layout(2000., 1000.)[0].1;
        touch(&mut app, 2, TouchPhase::Started, fire);
        // Look: free space on the right.
        touch(&mut app, 3, TouchPhase::Started, Vec2::new(1300., 300.));
        app.update();
        touch(&mut app, 1, TouchPhase::Moved, Vec2::new(300., 560.));
        touch(&mut app, 3, TouchPhase::Moved, Vec2::new(1320., 300.));
        app.update();
        let keys = app.world().resource::<ButtonInput<KeyCode>>();
        assert!(keys.pressed(KeyCode::KeyW) && !keys.pressed(KeyCode::ShiftLeft));
        assert!(app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
        assert!(app.world().resource::<AccumulatedMouseMotion>().delta.x > 0.);
        // Lifting every finger releases everything touch pressed.
        touch(&mut app, 1, TouchPhase::Ended, Vec2::new(300., 560.));
        touch(&mut app, 2, TouchPhase::Ended, fire);
        touch(&mut app, 3, TouchPhase::Ended, Vec2::new(1320., 300.));
        app.update();
        assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
        assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
    }
    #[test]
    fn dragging_a_held_fire_button_still_looks() {
        let mut app = app();
        app.update();
        let fire = layout(2000., 1000.)[0].1;
        touch(&mut app, 1, TouchPhase::Started, fire);
        app.update();
        touch(&mut app, 1, TouchPhase::Moved, fire + Vec2::new(30., 0.));
        app.update();
        assert!(app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left), "still firing");
        assert!(app.world().resource::<AccumulatedMouseMotion>().delta.x > 0., "the drag turns the view");
        touch(&mut app, 1, TouchPhase::Ended, fire + Vec2::new(30., 0.));
        app.update();
        assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
    }
    #[test]
    fn controls_wait_for_the_map_cutscene_to_finish() {
        let root = std::path::Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\Left 4 Dead 2");
        if !root.exists() {
            return;
        }
        let mut logic = crate::l4d2r_cinematic::MapLogic::load(root, &crate::test_lab::path(), true).unwrap();
        logic.active = true;
        let mut app = app();
        app.insert_resource(logic);
        app.update();
        let fire = layout(2000., 1000.)[0].1;
        touch(&mut app, 1, TouchPhase::Started, fire);
        app.update();
        assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
        assert!(!cutscene(None));
        touch(&mut app, 1, TouchPhase::Ended, fire);
        app.update();
        // Cutscene over: the same tap fires.
        app.world_mut().resource_mut::<crate::l4d2r_cinematic::MapLogic>().active = false;
        touch(&mut app, 2, TouchPhase::Started, fire);
        app.update();
        assert!(app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
    }
}
