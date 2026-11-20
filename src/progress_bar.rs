//! Retail `HudProgressBar` (scripts/hudlayout.res + resource/ui/hud/
//! ProgressBar.res): the centred bar shown while healing, defibrillating,
//! deploying an upgrade pack or reviving. Pills and adrenaline are quick
//! uses and show no bar. Units are the HUD's 480-tall proportional units.
use crate::game_state::{Match, Squad};
use bevy::{prelude::*, ui::Val};
use std::collections::HashMap;

fn vh(units: f32) -> Val {
    Val::Vh(units / 4.8)
}
#[derive(Component)]
pub struct ProgressRoot;
#[derive(Component)]
pub struct ProgressFill;
#[derive(Component)]
pub struct ProgressTitle;
#[derive(Component)]
pub struct ProgressSubtext;
#[derive(Component)]
pub struct ProgressIcon;

/// Full width of the "Bar" panel (ProgressBar.res).
const BAR_WIDE: f32 = 227.;

pub fn spawn(
    commands: &mut Commands,
    images: &HashMap<String, Handle<Image>>,
    font: &Handle<Font>,
) {
    let text_font = |size: f32| TextFont {
        font: bevy::text::FontSource::Handle(font.clone()),
        font_size: bevy::text::FontSize::Vh(size / 4.8),
        ..default()
    };
    let stretch = bevy::ui::widget::NodeImageMode::Stretch;
    let at = |x: f32, y: f32, w: f32, h: f32| Node {
        position_type: PositionType::Absolute,
        left: vh(x),
        top: vh(y),
        width: vh(w),
        height: vh(h),
        ..default()
    };
    // HudProgressBar: xpos c-114, ypos c10, 300x80, no background.
    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.),
                top: Val::Percent(50.),
                margin: UiRect {
                    left: vh(-114.),
                    top: vh(10.),
                    ..default()
                },
                width: vh(300.),
                height: vh(80.),
                ..default()
            },
            Visibility::Hidden,
            ProgressRoot,
        ))
        .id();
    let image = |name: &str| images.get(name).cloned().unwrap_or_default();
    // BackgroundImage: hud/progressbar_bg scaled to 272x68, drawColor 80 76 82.
    commands.spawn((
        ImageNode {
            color: Color::srgb_u8(80, 76, 82),
            image_mode: stretch.clone(),
            ..ImageNode::new(image("hud/progressbar_bg"))
        },
        at(0., 0., 272., 68.),
        ChildOf(root),
    ));
    // Bar: vgui/hud/progressbar_fill scaled to 227x35 at 34,24. Progress
    // draws the texture's left part (u 0..p) over the left p of the panel.
    commands.spawn((
        ImageNode {
            image_mode: stretch.clone(),
            rect: Some(Rect::new(0., 0., 0., 64.)),
            ..ImageNode::new(image("hud/progressbar_fill"))
        },
        at(34., 24., 0., 35.),
        ProgressFill,
        ChildOf(root),
    ));
    // AwardIcon: icon_healing (mod_textures.txt: vgui/hud/iconsheet
    // 128,320 64x64) at 9,10, 25x24.
    commands.spawn((
        ImageNode {
            rect: Some(Rect::new(128., 320., 192., 384.)),
            image_mode: stretch,
            ..ImageNode::new(image("hud/iconsheet"))
        },
        at(9., 10., 25., 24.),
        ProgressIcon,
        ChildOf(root),
    ));
    // BarLabel: 39,7 290x12, west, MenuTitle_DropShadow (Stubble bold 18,
    // drop shadow). Subtext: 39,28 300x30, west, PlayerDisplayName (14).
    // Source labels centre their text vertically in the label box.
    for (x, y, w, h, size, title) in [
        (39., 7., 290., 12., 18., true),
        (39., 28., 300., 30., 14., false),
    ] {
        let label = commands
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    overflow: Overflow::visible(),
                    ..at(x, y, w, h)
                },
                ChildOf(root),
            ))
            .id();
        let mut text = commands.spawn((
            Text::new(""),
            text_font(size),
            TextColor(Color::WHITE),
            bevy::text::LineHeight::RelativeToFont(1.),
            ChildOf(label),
        ));
        if title {
            text.insert((ProgressTitle, TextShadow::default()));
        } else {
            text.insert(ProgressSubtext);
        }
    }
}

/// What the bar shows: (title, subtext, progress 0..1).
pub fn content(
    state: &Match,
    squad: &Squad,
    name: impl Fn(usize) -> String,
) -> Option<(String, String, f32)> {
    if state.infected || state.dead {
        return None;
    }
    let progress = |remaining: f32, total: f32| (1. - remaining / total.max(1e-3)).clamp(0., 1.);
    if let Some((title, p)) = &state.hold_bar {
        return Some((title.clone(), String::new(), *p));
    }
    let target = || {
        state
            .medical_target
            .map_or(String::new(), |i| format!("Your target: {}", name(i)))
    };
    if state.action > 0. {
        let p = progress(state.action, state.action_total);
        return match state.medical_kind {
            1 => Some(("HEALING YOURSELF".into(), String::new(), p)),
            2 => Some(("HEALING TEAMMATE".into(), target(), p)),
            3 => Some(("USING DEFIBRILLATOR".into(), target(), p)),
            0 if state.slot == 4 => state.upgrade_pack.map(|kind| {
                (
                    match kind {
                        crate::upgrades::Kind::Incendiary => "DEPLOYING INCENDIARY AMMO",
                        crate::upgrades::Kind::Explosive => "DEPLOYING EXPLOSIVE AMMO",
                    }
                    .into(),
                    String::new(),
                    p,
                )
            }),
            // Carried use targets (carryables::Delivery): retail client
            // #L4D_progress_using_gas_can / _using_cola_bottles_alt1.
            10 => Some((
                if state.carried.as_deref() == Some("weapon_gascan") {
                    "POURING FUEL"
                } else {
                    "DELIVERING COLA BOTTLES"
                }
                .into(),
                String::new(),
                p,
            )),
            // Pills (5) and adrenaline (4) have no progress bar in retail.
            _ => None,
        };
    }
    // survivor_revive_duration 5, halved by adrenaline_revive_speedup.
    let revive = if state.adrenaline_time > 0. { 2.5 } else { 5. };
    if state.incap {
        if let Some((i, m)) = squad
            .0
            .iter()
            .enumerate()
            .find(|(_, m)| !m.state.incap && !m.state.dead && m.revive_time > 0.)
        {
            return Some((
                "BEING REVIVED".into(),
                format!("Your savior: {}", name(i)),
                (m.revive_time / revive).clamp(0., 1.),
            ));
        }
        return None;
    }
    // Only the survivor doing the reviving sees this bar, never bystanders.
    let i = state.reviving?;
    squad
        .0
        .get(i)
        .filter(|m| m.state.incap && m.revive_time > 0.)
        .map(|m| {
            (
                "REVIVING TEAMMATE".into(),
                format!("Your target: {}", name(i)),
                (m.revive_time / revive).clamp(0., 1.),
            )
        })
}

#[allow(clippy::type_complexity)]
pub fn update(
    state: Res<Match>,
    squad: Res<Squad>,
    session: Option<Res<crate::campaign_settings::SessionSettings>>,
    mut root: Query<&mut Visibility, With<ProgressRoot>>,
    mut fill: Query<(&mut Node, &mut ImageNode), With<ProgressFill>>,
    mut texts: Query<
        (&mut Text, Has<ProgressTitle>),
        Or<(With<ProgressTitle>, With<ProgressSubtext>)>,
    >,
    mut icons: Query<&mut Visibility, (With<ProgressIcon>, Without<ProgressRoot>)>,
) {
    // Retail draws the healing icon for medical uses only: delivering cola
    // or pouring fuel shows just the bar and its title.
    let icon = if state.medical_kind == 10 { Visibility::Hidden } else { Visibility::Inherited };
    for mut visibility in &mut icons {
        visibility.set_if_neq(icon);
    }
    let names: Vec<&'static str> = session
        .as_ref()
        .map(|s| {
            crate::campaign_settings::Survivor::roster(s.original)
                .into_iter()
                .filter(|v| *v != s.survivor)
                .map(|v| v.name())
                .collect()
        })
        .unwrap_or_default();
    let name = |i: usize| names.get(i).copied().unwrap_or("Survivor").to_string();
    let shown = content(&state, &squad, name);
    for mut visibility in &mut root {
        visibility.set_if_neq(if shown.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    let Some((title, subtext, progress)) = shown else {
        return;
    };
    for (mut node, mut image) in &mut fill {
        let width = vh(BAR_WIDE * progress);
        if node.width != width {
            node.width = width;
        }
        let rect = Some(Rect::new(0., 0., 512. * progress, 64.));
        if image.rect != rect {
            image.rect = rect;
        }
    }
    for (mut text, is_title) in &mut texts {
        let value = if is_title { &title } else { &subtext };
        if text.0 != *value {
            text.0 = value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn squad() -> Squad {
        Squad(vec![])
    }
    #[test]
    fn health_bar_draws_temporary_health_as_ticks_coloured_by_permanent_health() {
        use crate::game_ui::health_segment;
        let mut s = Match::new(&crate::game_state::tests_weapon());
        s.health = 40.;
        s.temporary = 50.;
        let solid = health_segment(&s, false);
        let ticks = health_segment(&s, true);
        assert_eq!(
            (solid.span, solid.texture),
            ((0., 0.4), "hud/healthbar_withglow_green")
        );
        assert_eq!(ticks.texture, "hud/healthbar_ticks_withglow_green");
        assert!((ticks.span.0 - 0.4).abs() < 1e-6 && (ticks.span.1 - 0.9).abs() < 1e-6);
        // Colour follows permanent health only: 20 + 70 buffer is red.
        s.health = 20.;
        s.temporary = 70.;
        assert_eq!(
            health_segment(&s, false).texture,
            "hud/healthbar_withglow_red"
        );
        assert_eq!(
            health_segment(&s, true).texture,
            "hud/healthbar_ticks_withglow_red"
        );
        s.health = 30.;
        assert_eq!(
            health_segment(&s, false).texture,
            "hud/healthbar_withglow_orange"
        );
        // The buffer never draws past full health.
        s.health = 80.;
        s.temporary = 50.;
        assert!((health_segment(&s, true).span.1 - 1.).abs() < 1e-6);
        s.black_white = true;
        assert_eq!(
            health_segment(&s, false).texture,
            "hud/healthbar_withglow_white"
        );
        // Incapacitated: tier 0 of the ticks set, the striped red bar over
        // bleed-out health out of 300; no buffer segment.
        s.incap = true;
        s.incap_health = 150.;
        let down = health_segment(&s, false);
        assert_eq!((down.span, down.texture), ((0., 0.5), "hud/healthbar_ticks_withglow_red"));
        assert_eq!(health_segment(&s, true).span, (0.5, 0.5));
    }
    #[test]
    fn revive_bar_only_for_the_survivor_doing_the_reviving() {
        let weapon = crate::game_state::tests_weapon();
        let mut s = Match::new(&weapon);
        let mut downed = crate::game_state::Companion {
            state: Match::new(&weapon),
            position: Vec3::ZERO,
            spawn: Vec3::ZERO,
            model: String::new(),
            vertical: 0.,
            cooldown: 0.,
            revive_time: 2.,
            aim: Vec3::ZERO,
        };
        downed.state.incap = true;
        let squad = Squad(vec![downed]);
        // A bot is reviving them while the player merely stands nearby.
        assert!(content(&s, &squad, |_| "Nick".into()).is_none());
        s.reviving = Some(0);
        let (title, sub, p) = content(&s, &squad, |_| "Nick".into()).unwrap();
        assert_eq!((title.as_str(), sub.as_str()), ("REVIVING TEAMMATE", "Your target: Nick"));
        assert!((p - 0.4).abs() < 1e-5);
    }
    #[test]
    fn retail_titles_and_no_bar_for_pills_or_adrenaline() {
        let mut s = Match::new(&crate::game_state::tests_weapon());
        assert!(content(&s, &squad(), |_| "Nick".into()).is_none());
        s.action = 2.5;
        s.action_total = 5.;
        s.medical_kind = 1;
        let (title, sub, p) = content(&s, &squad(), |_| "Nick".into()).unwrap();
        assert_eq!((title.as_str(), sub.as_str()), ("HEALING YOURSELF", ""));
        assert!((p - 0.5).abs() < 1e-5);
        s.medical_kind = 2;
        s.medical_target = Some(0);
        let (title, sub, _) = content(&s, &squad(), |_| "Nick".into()).unwrap();
        assert_eq!(
            (title.as_str(), sub.as_str()),
            ("HEALING TEAMMATE", "Your target: Nick")
        );
        s.medical_kind = 3;
        assert_eq!(
            content(&s, &squad(), |_| "Nick".into())
                .unwrap()
                .0,
            "USING DEFIBRILLATOR"
        );
        for kind in [4, 5] {
            s.medical_kind = kind;
            assert!(content(&s, &squad(), |_| "Nick".into()).is_none());
        }
        // Delivering the c1m2 cola and pouring gas show the use bar.
        s.medical_kind = 10;
        s.carried = Some("weapon_cola_bottles".into());
        let (title, _, p) = content(&s, &squad(), |_| "Nick".into()).unwrap();
        assert_eq!(title, "DELIVERING COLA BOTTLES");
        assert!((p - 0.5).abs() < 1e-5);
        s.carried = Some("weapon_gascan".into());
        assert_eq!(content(&s, &squad(), |_| "Nick".into()).unwrap().0, "POURING FUEL");
    }
}
