//! Fits the HUD and menus to the device: the safe area (notch, Dynamic
//! Island, rounded corners, home indicator), the player's HUD size and, on
//! touch screens, a larger menu.
//!
//! - The HUD container (game_ui::target_camera) is inset by the safe area,
//!   and each HUD element is scaled by `Interface::hud_scale` around the
//!   screen edge it hugs, so it grows inward instead of off screen.
//! - Menu pages are laid out on a 1280x720 canvas scaled to the screen
//!   height. Touch builds scale them up (phones) or down (screens narrower
//!   than 16:9, e.g. iPad), anchored at the left middle like the layout.
use bevy::{prelude::*, ui::ComputedNode};

/// Safe-area insets in logical pixels.
#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct SafeArea {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
impl SafeArea {
    /// The usable rectangle of a `size` screen.
    pub fn rect(&self, size: Vec2) -> Rect {
        Rect::new(self.left, self.top, (size.x - self.right).max(self.left + 1.), (size.y - self.bottom).max(self.top + 1.))
    }
}

/// Player interface settings, saved to runtime/interface.ini (with the touch
/// layouts, game_touch.rs).
#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct Interface {
    /// HUD element scale, 0.6..1.6.
    pub hud_scale: f32,
    /// Teammate health panels' offset from their retail place, as a
    /// fraction of the screen height (they move as one group).
    pub team_offset: Vec2,
    /// Touch look speed multiplier, 0.25..4.
    pub look_sensitivity: f32,
    /// iPhone render scale (UIKit contentScaleFactor), 1..3 in steps of
    /// 0.5. Lower is faster and uses less memory; the panel itself is 3x.
    pub render_scale: f32,
}
pub(crate) const DEFAULT_LOOK_SENSITIVITY: f32 = 1.5;
pub(crate) const DEFAULT_RENDER_SCALE: f32 = 2.;
impl Default for Interface {
    fn default() -> Self {
        Self {
            hud_scale: 1.,
            team_offset: Vec2::ZERO,
            look_sensitivity: DEFAULT_LOOK_SENSITIVITY,
            render_scale: DEFAULT_RENDER_SCALE,
        }
    }
}
pub(crate) const INTERFACE_FILE: &str = "runtime/interface.ini";
pub(crate) fn read_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(&format!("{key}=")).map(|v| v.trim().to_string()))
}
/// Replaces (or adds) `key=value` lines in the interface file.
pub(crate) fn write_values(values: &[(String, String)]) {
    let mut text = std::fs::read_to_string(INTERFACE_FILE).unwrap_or_default();
    for (key, value) in values {
        let line = format!("{key}={value}");
        let prefix = format!("{key}=");
        if let Some(start) = text.lines().position(|l| l.starts_with(&prefix)) {
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            lines[start] = line;
            text = lines.join("\n") + "\n";
        } else {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text += &line;
            text.push('\n');
        }
    }
    let _ = std::fs::create_dir_all("runtime");
    if let Err(e) = std::fs::write(INTERFACE_FILE, text) {
        eprintln!("Saving {INTERFACE_FILE}: {e}");
    }
}
impl Interface {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(INTERFACE_FILE).unwrap_or_default();
        let number = |key: &str| {
            read_value(&text, key)
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        Self {
            hud_scale: number("hud_scale").map_or(1., |v| v.clamp(0.6, 1.6)),
            team_offset: Vec2::new(
                number("team_offset_x").map_or(0., |v| v.clamp(-1., 2.)),
                number("team_offset_y").map_or(0., |v| v.clamp(-1., 1.)),
            ),
            look_sensitivity: number("look_sensitivity")
                .map_or(DEFAULT_LOOK_SENSITIVITY, |v| v.clamp(0.25, 4.)),
            render_scale: number("render_scale")
                .map_or(DEFAULT_RENDER_SCALE, |v| ((v * 2.).round() / 2.).clamp(1., 3.)),
        }
    }
    pub fn save(&self) {
        write_values(&[
            ("hud_scale".into(), format!("{:.2}", self.hud_scale)),
            ("team_offset_x".into(), format!("{:.4}", self.team_offset.x)),
            ("team_offset_y".into(), format!("{:.4}", self.team_offset.y)),
            ("look_sensitivity".into(), format!("{:.2}", self.look_sensitivity)),
            ("render_scale".into(), format!("{:.1}", self.render_scale)),
        ]);
    }
}

/// Touch builds (and `--touch`) fit the menu to the device.
pub(crate) fn touch_build() -> bool {
    crate::game::touch::enabled_by_default()
}

pub(crate) fn setup(mut commands: Commands) {
    commands.insert_resource(Interface::load());
    commands.insert_resource(crate::game::touch::TouchLayout::load());
    commands.init_resource::<SafeArea>();
}

#[cfg(target_os = "ios")]
unsafe extern "C" {
    fn l4d2r_safe_area(left: *mut f32, top: *mut f32, right: *mut f32, bottom: *mut f32);
    fn l4d2r_set_render_scale(scale: f32, width: *mut f32, height: *mut f32) -> f32;
}
/// iPhone render resolution: sets UIKit's contentScaleFactor on the game
/// view (ios/main.m). winit reports the new scale and size (its iOS layout
/// pass is patched in vendor/winit to use the view's scale, not the
/// screen's), so Bevy resizes the swapchain while layout and touches stay
/// in points.
#[cfg(target_os = "ios")]
pub(crate) fn apply_render_scale(
    _main: bevy::ecs::system::NonSendMarker,
    interface: Option<Res<Interface>>,
    windows: Query<&Window>,
    mut logged: Local<Option<(f32, u32, u32)>>,
) {
    let Some(interface) = interface else { return };
    let (mut width, mut height) = (0f32, 0f32);
    let scale = unsafe { l4d2r_set_render_scale(interface.render_scale, &mut width, &mut height) };
    if scale <= 0. {
        return;
    }
    for window in &windows {
        let state = (window.resolution.scale_factor(), window.physical_width(), window.physical_height());
        if *logged != Some(state) {
            eprintln!(
                "iOS render scale {scale} for {width}x{height} points; window {}x{} at scale {}",
                state.1, state.2, state.0
            );
            *logged = Some(state);
        }
    }
}
#[cfg(not(target_os = "ios"))]
pub(crate) fn apply_render_scale() {}
/// Reads UIKit's safe-area insets (main thread: UIKit is main-thread only).
#[cfg(target_os = "ios")]
pub(crate) fn read_safe_area(_main: bevy::ecs::system::NonSendMarker, mut safe: ResMut<SafeArea>) {
    let (mut l, mut t, mut r, mut b) = (0., 0., 0., 0.);
    unsafe { l4d2r_safe_area(&mut l, &mut t, &mut r, &mut b) };
    // Landscape: the home indicator inset is small and the HUD sits above it
    // already; keep a little of it so nothing touches the bar.
    let next = SafeArea { left: l, top: t, right: r, bottom: (b * 0.5).max(0.) };
    if *safe != next {
        *safe = next;
    }
}
#[cfg(not(target_os = "ios"))]
pub(crate) fn read_safe_area(mut safe: ResMut<SafeArea>) {
    // `--safe-area L T R B` previews a phone's insets on desktop.
    static ARG: std::sync::OnceLock<Option<SafeArea>> = std::sync::OnceLock::new();
    let forced = *ARG.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        let i = args.iter().position(|a| a == "--safe-area")?;
        let v: Vec<f32> = args.get(i + 1)?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        (v.len() == 4).then(|| SafeArea { left: v[0], top: v[1], right: v[2], bottom: v[3] })
    });
    if let Some(forced) = forced {
        if *safe != forced {
            *safe = forced;
        }
    }
}

/// Main-menu scale for a `screen` (logical pixels) with `usable` width
/// between the safe-area insets. The home page's content spans canvas x
/// 100..1150 and y 47..595 (720-line canvas, down to the iOS GAME FILES
/// row); phones scale it up to 1.3 as far as it fits, screens that are too
/// narrow scale it down.
pub(crate) fn home_scale(screen: Vec2, usable_width: f32, points_height: f32, touch: bool) -> f32 {
    if !touch || screen.y <= 0. {
        return 1.;
    }
    let k = screen.y / 720.;
    let wanted: f32 = if points_height <= 520. { 1.3 } else if points_height <= 700. { 1.15 } else { 1. };
    let fit_width = (usable_width - 30. * k) / (1050. * k);
    let fit_height = (360. - 8.) / 274.;
    wanted.min(fit_width).min(fit_height)
}
/// Phones: the 720-line canvas is only ~393 points tall, so menu rows of
/// 30-35 canvas units were 12-19 points high, well under a comfortable tap.
/// Pages whose content leaves room are scaled up, as far as their content
/// box allows, and centred across the safe area: (left, right, top, bottom)
/// in canvas units, measured from 2556x1179 `--touch --device-scale 3`
/// captures.
pub(crate) fn touch_content_box(page: crate::game_menu::Page, in_game: bool) -> Option<(f32, f32, f32, f32)> {
    use crate::game_menu::Page;
    match page {
        // The pause column, with its OPTIONS and CALL A VOTE flyouts.
        Page::Pause | Page::Vote => Some((150., 700., 190., 530.)),
        Page::Options if in_game => Some((150., 700., 190., 530.)),
        // Buttons, summary, player slots and chat down to START VOICE CHAT.
        Page::Lobby => Some((255., 1015., 62., 680.)),
        // Labels plus the value column ("Medium (models next map)").
        Page::Video => Some((290., 870., 12., 470.)),
        Page::Audio => Some((0., 1150., 12., 430.)),
        _ => None,
    }
}
/// Largest scale (capped at 1.5) that keeps a content box inside the usable
/// width and the screen height (less a small margin), centred vertically on
/// the box.
pub(crate) fn touch_box_scale(screen: Vec2, usable_width: f32, content: (f32, f32, f32, f32)) -> f32 {
    if screen.y <= 0. {
        return 1.;
    }
    let k = screen.y / 720.;
    let (left, right, top, bottom) = content;
    let fit_width = usable_width * 0.97 / ((right - left).max(1.) * k);
    // 12 canvas units clear of the top and bottom edges (the lobby's
    // START VOICE CHAT sat on the home indicator).
    let fit_height = (360. - 12.) / ((bottom - top) / 2.).max(1.);
    1.5f32.min(fit_width).min(fit_height).max(1.)
}
/// Other pages are drawn for a 16:9 canvas from the left: only screens
/// narrower than that (iPad) shrink them to fit.
pub(crate) fn page_scale(screen: Vec2, usable_width: f32, touch: bool) -> f32 {
    if !touch || screen.y <= 0. {
        return 1.;
    }
    (usable_width / (1280. * screen.y / 720.)).min(1.)
}

/// Where a box sits on screen: -1 hugs the left/top edge, 1 the right/bottom,
/// 0 is centred.
fn anchor(centre: f32, size: f32) -> f32 {
    if centre < size / 3. {
        -1.
    } else if centre > size * 2. / 3. {
        1.
    } else {
        0.
    }
}

/// A HUD node that clips at 100% HUD size. Clip rectangles ignore UI
/// transforms, so a scaled health bar cut its fill short inside the frame;
/// while the HUD is scaled these nodes draw unclipped.
#[derive(Component)]
pub(crate) struct ClipsUnscaled;
/// Turns HUD clipping off while scaled and back on at 100%.
pub(crate) fn hud_clipping(
    interface: Option<Res<Interface>>,
    containers: Query<Entity, (With<crate::game_ui::HudRoot>, Without<crate::l4d2r_cinematic::CinematicFade>)>,
    children: Query<&Children>,
    mut nodes: Query<(&mut Node, Has<ClipsUnscaled>)>,
    mut last: Local<Option<(f32, usize)>>,
    mut commands: Commands,
) {
    let scale = interface.map_or(1., |i| i.hud_scale);
    let scaled = (scale - 1.).abs() >= 1e-3;
    // Re-walk when the scale changes or the HUD gains nodes.
    let mut all = Vec::new();
    for root in &containers {
        all.extend(children.iter_descendants(root));
    }
    if *last == Some((scale, all.len())) {
        return;
    }
    *last = Some((scale, all.len()));
    for entity in all {
        let Ok((mut node, marked)) = nodes.get_mut(entity) else { continue };
        if scaled && node.overflow == Overflow::clip() {
            node.overflow = Overflow::visible();
            if !marked {
                commands.entity(entity).insert(ClipsUnscaled);
            }
        } else if !scaled && marked && node.overflow != Overflow::clip() {
            node.overflow = Overflow::clip();
        }
    }
}
/// Menu pages scaled up for touch (`touch_content_box`): clip rectangles
/// ignore UI transforms, so a scaled page's clipped boxes (the lobby chat)
/// cut their text in the wrong place. Those nodes draw unclipped while the
/// page is scaled, as the HUD does in `hud_clipping`.
pub(crate) fn menu_clipping(
    pages: Query<(Entity, Option<&UiTransform>), With<crate::game_menu::PageNode>>,
    children: Query<&Children>,
    mut nodes: Query<(&mut Node, Has<ClipsUnscaled>)>,
    mut commands: Commands,
) {
    for (page, transform) in &pages {
        let scaled = transform.is_some_and(|t| (t.scale.x - 1.).abs() >= 1e-3);
        for entity in children.iter_descendants(page) {
            let Ok((mut node, marked)) = nodes.get_mut(entity) else { continue };
            if scaled && node.overflow == Overflow::clip() {
                node.overflow = Overflow::visible();
                if !marked {
                    commands.entity(entity).insert(ClipsUnscaled);
                }
            } else if !scaled && marked && node.overflow != Overflow::clip() {
                node.overflow = Overflow::clip();
            }
        }
    }
}
/// UI roots, for the screen size when there is no window.
type Roots<'w, 's> = Query<'w, 's, &'static ComputedNode, Without<ChildOf>>;
/// The logical screen size: the window, or (captures have no window) the
/// largest laid-out UI root (the full-screen menu, HUD or touch overlay).
fn screen_size(windows: &Query<&Window>, roots: &Roots) -> Option<Vec2> {
    if let Some(w) = windows.iter().next() {
        return Some(Vec2::new(w.width(), w.height()));
    }
    roots
        .iter()
        .map(|n| n.size() * n.inverse_scale_factor())
        .max_by(|a, b| (a.x * a.y).total_cmp(&(b.x * b.y)))
        .filter(|s| s.x > 0. && s.y > 0.)
}
/// Insets the HUD container and scales HUD elements around their edge.
pub(crate) fn fit_hud(
    safe: Res<SafeArea>,
    interface: Option<Res<Interface>>,
    mut containers: Query<
        (&mut Node, &ComputedNode, &UiGlobalTransform, Option<&Children>),
        (With<crate::game_ui::HudRoot>, Without<crate::l4d2r_cinematic::CinematicFade>),
    >,
    mut elements: Query<(&ComputedNode, &UiGlobalTransform, Option<&mut UiTransform>), Without<crate::game_ui::HudRoot>>,
    windows: Query<&Window>,
    roots: Roots,
    mut commands: Commands,
) {
    let Some(screen) = screen_size(&windows, &roots) else { return };
    let scale = interface.map_or(1., |i| i.hud_scale);
    for (mut node, container, container_at, children) in &mut containers {
        let (left, top, right, bottom) = (Val::Px(safe.left), Val::Px(safe.top), Val::Px(safe.right), Val::Px(safe.bottom));
        if node.left != left || node.top != top || node.right != right || node.bottom != bottom || node.width != Val::Auto {
            node.left = left;
            node.top = top;
            node.right = right;
            node.bottom = bottom;
            node.width = Val::Auto;
            node.height = Val::Auto;
        }
        let Some(children) = children else { continue };
        // The whole HUD scales about the container edge (or middle) each
        // element hugs, so neighbouring panels keep their spacing:
        // an element's centre c moves to a + (c - a) * scale for its anchor
        // point a. A transform scales about the element's own centre, so the
        // translation is (scale - 1) * (c - a).
        let inverse = container.inverse_scale_factor();
        let bounds = Rect::from_center_size(container_at.translation * inverse, container.size() * inverse);
        for child in children.iter() {
            let Ok((computed, global, transform)) = elements.get_mut(child) else { continue };
            let size = computed.size() * computed.inverse_scale_factor();
            // Laid-out centre: undo this frame's own translation.
            let moved = transform.as_ref().map_or(Vec2::ZERO, |t| match (t.translation.x, t.translation.y) {
                (Val::Px(x), Val::Px(y)) => Vec2::new(x, y),
                _ => Vec2::ZERO,
            });
            let centre = global.translation * computed.inverse_scale_factor() - moved;
            // Full-screen layers (damage tint, crosshair holders) stay put.
            let full = size.x >= screen.x * 0.9 && size.y >= screen.y * 0.9;
            let wanted = if full || (scale - 1.).abs() < 1e-3 || bounds.width() <= 0. {
                UiTransform::IDENTITY
            } else {
                // Across: elements crossing (or within 8% of) the middle
                // line (crosshair, progress bars) scale about the middle; the
                // rest about their side's edge, so a row of panels scales as
                // a group. Down: thirds, so a column stacked around the
                // middle height (inventory slots) scales about the middle.
                let across = |c: f32, half: f32, lo: f32, hi: f32| {
                    let mid = (lo + hi) * 0.5;
                    if (c - half < mid && c + half > mid) || (c - mid).abs() < (hi - lo) * 0.08 {
                        mid
                    } else if c < mid {
                        lo
                    } else {
                        hi
                    }
                };
                let down = |c: f32, lo: f32, hi: f32| match anchor(c - lo, hi - lo) {
                    a if a < 0. => lo,
                    a if a > 0. => hi,
                    _ => (lo + hi) * 0.5,
                };
                let a = Vec2::new(
                    across(centre.x, size.x * 0.5, bounds.min.x, bounds.max.x),
                    down(centre.y, bounds.min.y, bounds.max.y),
                );
                let t = (centre - a) * (scale - 1.);
                UiTransform {
                    translation: Val2::px(t.x.round(), t.y.round()),
                    scale: Vec2::splat(scale),
                    ..default()
                }
            };
            match transform {
                Some(mut t) => {
                    if *t != wanted {
                        *t = wanted;
                    }
                }
                None => {
                    if wanted != UiTransform::IDENTITY {
                        commands.entity(child).insert(wanted);
                    }
                }
            }
        }
    }
}

/// Scales menu pages for touch screens and keeps them inside the safe area.
/// Transforms scale about a node's centre; the translations below place
/// canvas points (720-line units, `k` pixels each) where they belong.
#[allow(clippy::type_complexity)]
pub(crate) fn fit_menu(
    safe: Res<SafeArea>,
    windows: Query<&Window>,
    roots: Roots,
    mut pages: Query<(Entity, &crate::game_menu::PageNode, Option<&mut UiTransform>)>,
    mut tiles: Query<&mut Node, With<crate::game_menu::CarouselTile>>,
    mut commands: Commands,
    frontend: Option<Res<crate::game_menu::Frontend>>,
) {
    let Some(screen) = screen_size(&windows, &roots) else { return };
    let usable = safe.rect(screen);
    // `--device-scale N` previews a phone on desktop: N physical pixels per
    // point (iPhones are 3), so a 2556x1179 capture behaves like 852x393.
    static DEVICE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let device = *DEVICE.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        args.windows(2).find(|p| p[0] == "--device-scale").and_then(|p| p[1].parse().ok()).filter(|v: &f32| *v >= 1.).unwrap_or(1.)
    });
    let touch = touch_build();
    let k = screen.y / 720.;
    let centre = screen * 0.5;
    let home = home_scale(screen, usable.width(), screen.y / device, touch);
    let other = page_scale(screen, usable.width(), touch);
    // OPTIONS opened from the main menu: its flyout is drawn against the
    // home page's OPTIONS button, so it takes the home page's fit (it opened
    // above the scaled button on phones).
    let from_home = frontend.as_ref().is_some_and(|f| !f.started);
    for (entity, page, transform) in &mut pages {
        let home_fit = page.page() == crate::game_menu::Page::Home
            || (from_home && page.page() == crate::game_menu::Page::Options);
        let wanted = if home_fit {
            // Canvas x 100 sits 20 units inside the safe area; the content's
            // middle (y 321) sits at the screen's middle.
            let s = home;
            let tx = usable.min.x + 20. * k - centre.x - (100. * k - centre.x) * s;
            let ty = -(321. * k - centre.y) * s;
            if (s - 1.).abs() < 1e-3 && usable.min.x == 0. {
                UiTransform::IDENTITY
            } else {
                UiTransform { translation: Val2::px(tx, ty), scale: Vec2::splat(s), ..default() }
            }
        } else if let Some(content) = touch.then(|| touch_content_box(page.page(), !from_home)).flatten() {
            // Bigger tap targets: left edge at the safe-area inset, the
            // content box centred vertically.
            let s = touch_box_scale(screen, usable.width(), content);
            // Canvas x 0 lands at `origin`, putting the box's middle at the
            // middle of the safe area. The pause column (and its flyouts)
            // stays left-aligned at the safe-area edge, as in retail.
            use crate::game_menu::Page;
            let left_aligned = matches!(page.page(), Page::Pause | Page::Vote | Page::Options);
            let origin = if left_aligned {
                usable.min.x
            } else {
                usable.center().x - (content.0 + content.1) * 0.5 * k * s
            };
            let tx = origin - centre.x * (1. - s);
            let ty = -((content.2 + content.3) * 0.5 * k - centre.y) * s;
            UiTransform { translation: Val2::px(tx, ty), scale: Vec2::splat(s), ..default() }
        } else if (other - 1.).abs() < 1e-3 && usable.min.x == 0. {
            UiTransform::IDENTITY
        } else {
            // Canvas x 0 at the safe-area inset, vertically centred.
            let s = other;
            let tx = usable.min.x - centre.x * (1. - s);
            UiTransform { translation: Val2::px(tx, 0.), scale: Vec2::splat(s), ..default() }
        };
        match transform {
            Some(mut t) => {
                if *t != wanted {
                    *t = wanted;
                }
            }
            None => {
                if wanted != UiTransform::IDENTITY {
                    commands.entity(entity).insert(wanted);
                }
            }
        }
    }
    // Clip rectangles ignore UI transforms: a scaled home page clipped its
    // mode-tile labels. The clip only matters mid-slide, so drop it then.
    let overflow = if (home - 1.).abs() < 1e-3 { Overflow::clip() } else { Overflow::visible() };
    for mut tile in &mut tiles {
        if tile.overflow != overflow {
            tile.overflow = overflow;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menus_grow_on_phones_and_fit_narrow_screens() {
        // iPhone 15 Pro landscape (852 x 393 points, 59-point side insets).
        let screen = Vec2::new(852., 393.);
        let phone = home_scale(screen, 852. - 118., 393., true);
        let k = 393. / 720.;
        assert!(phone > 1.15, "{phone}");
        assert!(20. * k + 1050. * k * phone <= 852. - 118. - 10. * k + 0.5);
        assert_eq!(page_scale(screen, 852. - 118., true), 1.);
        // iPad 4:3: pages shrink to fit the width.
        let ipad = Vec2::new(1024., 768.);
        assert!((page_scale(ipad, 1024., true) - 0.75).abs() < 1e-3);
        assert!(home_scale(ipad, 1024., 768., true) < 1.);
        // Desktop is untouched.
        assert_eq!(home_scale(Vec2::new(1920., 1080.), 1920., 1080., false), 1.);
    }
    #[test]
    fn hud_elements_grow_away_from_their_edge() {
        assert_eq!(anchor(10., 1000.), -1.);
        assert_eq!(anchor(500., 1000.), 0.);
        assert_eq!(anchor(990., 1000.), 1.);
    }
    #[test]
    fn safe_area_rect_excludes_the_insets() {
        let r = SafeArea { left: 59., top: 0., right: 59., bottom: 10. }.rect(Vec2::new(852., 393.));
        assert_eq!((r.min, r.max), (Vec2::new(59., 0.), Vec2::new(793., 383.)));
    }
}
