//! Touch controls for the Android Inn Test (M8).
//!
//! Dylan's own button art (`assets/ui/`, his drawings) laid out as
//! the mobile-RPG scheme from
//! `references/mobile-rpg-button-layout.png`: virtual joystick far
//! left centre, right-thumb action cluster, compass top-centre,
//! quick slots bottom-centre, map/bag/menu top-right — and the
//! health/stamina/magicka bars in the original bottom row, from
//! the game's own `hudmenu.gfx` geometry (UI milestone 1).
//!
//! Input is read from Bevy's [`Touches`] resource and written into
//! [`TouchInputState`], which the player controller in [`player`]
//! reads alongside the keyboard/mouse. Hit-testing uses the same
//! layout constants as the UI spawn, so the touch targets and the
//! drawn buttons agree by construction — no UI-layout queries.
//!
//! Wired in this test: joystick move, drag-to-look on the right
//! half of the screen, jump (a real jump — M8.1 gravity), sprint
//! (3x speed, drains stamina), USE (M15: open/close the nearest
//! door), BAG (M17: open the inventory), MENU (UI milestone 1:
//! open the pause screen). Attack, sneak, shout, map and the
//! quick slots are drawn and show a press, but do nothing yet.
//! The compass strip rotates with the camera heading (M8.1) and
//! the stamina bar drains while sprinting and regens otherwise;
//! health and magicka stay full — there is no combat or magic to
//! drive them yet, and faking movement would be a lie. The bars
//! (UI milestone 1) are the original look from `hudmenu.gfx`: a
//! dark plate + light edge drawn in code, Health left, Stamina
//! centre, Magicka right, NO text labels — the original has
//! none (the HEALTH/MAGICKA/STAMINA labels were our own addition
//! and are gone). Dylan's EMPTY bar frames
//! (`ui/bar_frame_empty.png`, M11) are unused now — the file
//! stays in assets, like the old compass frame. The compass is
//! Dylan's EMPTY compass frame (`ui/compass_frame_empty.png`)
//! with the letters and ticks drawn by code on the sliding
//! strip, and the diamond marker above the frame pointing down
//! at the current heading.

// The plugin is only added on Android (`lib.rs`, `#[cfg(target_os
// = "android")]`), so on desktop this module would read as dead
// code. The allow is desktop-only: on Android, genuinely dead code
// in this module still warns.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use crate::player::{LookAngles, Stamina};
use crate::scene::Player;
use bevy::app::{App, Plugin, Startup, Update};
use bevy::asset::AssetServer;
use bevy::camera::visibility::Visibility;
use bevy::color::Color;
use bevy::ecs::prelude::*;
use bevy::input::touch::Touches;
use bevy::math::{Rect, Vec2};
use bevy::prelude::State;
use bevy::text::{FontSize, Justify, TextColor, TextFont, TextLayout};
use bevy::ui::widget::{ImageNode, Text};
use bevy::ui::{
    BackgroundColor, BorderColor, BorderRadius, FlexDirection, Node, Overflow, PositionType,
    UiRect, Val,
};
use bevy::window::{PrimaryWindow, Window};

/// What a touch button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    /// Real jump (Space on keyboard, M8.1 gravity).
    Jump,
    /// 3x movement speed while held.
    Sprint,
    /// M15: USE (E on keyboard) — open/close the nearest door.
    /// Edge-triggered like Jump (see `track_touches`).
    Use,
    /// Melee: attack with the right hand (mouse Left on
    /// desktop). Edge-triggered like Jump — holding it does
    /// not swing repeatedly (the swing cooldown gates repeats).
    /// Both hands swing the same equipped weapon: the
    /// inventory holds ONE equipped weapon, so there is no
    /// left/right-hand distinction yet.
    AttackRight,
    /// Melee: attack with the left hand. Same behavior as
    /// `AttackRight` until dual-wield exists.
    AttackLeft,
    /// Drawn, shows a press, does nothing yet.
    Inert,
}

/// Number of `ButtonAction` variants. The `held` array in
/// `update_touch_ui` is indexed by variant discriminant, so it
/// must be exactly this long — bump it when adding a variant.
/// (v0.4.12 crashed on launch because the melee milestone added
/// two variants and the array stayed at 4.)
const BUTTON_ACTION_COUNT: usize = 6;

/// A touch button that can be press-logged (the startup log's
/// `press:` lines — Dylan's 2026-11-17 order: count every button
/// press and what it did). The name is the log token. Covers
/// the action buttons, the top-right menu row, and sneak/shout
/// (both `ButtonAction::Inert` — told apart by their art, see
/// `inert_press_button`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PressButton {
    Jump,
    Sprint,
    Use,
    AttackRight,
    AttackLeft,
    Bag,
    Menu,
    Map,
    Sneak,
    Shout,
}

impl PressButton {
    pub fn name(self) -> &'static str {
        match self {
            PressButton::Jump => "jump",
            PressButton::Sprint => "sprint",
            PressButton::Use => "use",
            PressButton::AttackRight => "attack_right",
            PressButton::AttackLeft => "attack_left",
            PressButton::Bag => "bag",
            PressButton::Menu => "menu",
            PressButton::Map => "map",
            PressButton::Sneak => "sneak",
            PressButton::Shout => "shout",
        }
    }
}

/// Number of `PressButton` variants. The counts array in
/// `PressLog` is indexed by variant discriminant, so it must be
/// exactly this long — bump it when adding a variant (the same
/// rule as `BUTTON_ACTION_COUNT`).
const PRESS_BUTTON_COUNT: usize = 10;

/// The press log: per-button running counts + the pending
/// presses awaiting their outcome. One compact line per press
/// in the startup log: `press: use #12 -> talked to Embry`,
/// `press: attack_right #3 -> no target in range`, or bare
/// (`press: jump #5`) when the press has no outcome to report.
///
/// Two paths, by who knows the outcome:
/// * Immediate (`log_now`): jump/sprint/sneak/shout — no
///   consumer owns the outcome, so `track_touches` logs the
///   press the frame it happens.
/// * Pending (`record` + `claim`): use/attack/bag/menu/map —
///   `track_touches` records the press, and the system that
///   acts on it claims it and logs the line WITH the outcome
///   (dialogue_use, melee_attack, the menu-open observers).
///   A press nobody claims — a door opened, a pickup taken, or
///   nothing in range — is flushed bare by the NEXT frame's
///   `track_touches`, so every consumer has had its chance
///   regardless of system order (the consumers run before
///   `track_touches` in Update, so they claim a press the frame
///   after it is recorded). The bare line plus the adjacent
///   action lines ("picked up X", "door opened") say what
///   happened; a bare USE line with no action after it means
///   nothing was in range.
///
/// Keyboard/mouse presses are NOT counted here — this is the
/// touch buttons only (a keyboard-triggered action finds no
/// pending press, so `claim` returns `None` and nothing logs).
#[derive(Debug, Default, Resource)]
pub struct PressLog {
    counts: [u32; PRESS_BUTTON_COUNT],
    pending: Vec<(PressButton, u32)>,
    sprint_held: bool,
}

impl PressLog {
    /// Record a just-press: bump the button's count and queue it
    /// for the consumer to claim with its outcome. Returns the
    /// count (the `#N` in the log line).
    pub fn record(&mut self, button: PressButton) -> u32 {
        let count = &mut self.counts[button as usize];
        *count += 1;
        self.pending.push((button, *count));
        *count
    }

    /// Log a press immediately (no consumer owns the outcome).
    /// Returns the count.
    pub fn log_now(&mut self, button: PressButton, outcome: Option<&str>) -> u32 {
        let count = &mut self.counts[button as usize];
        *count += 1;
        crate::startup_log::log(&Self::format_line(button, *count, outcome));
        *count
    }

    /// Claim a pending press for `button` (the consumer acts on
    /// it): its count, or `None` when no press is pending
    /// (keyboard-triggered, already claimed, or the press was
    /// flushed). The consumer logs the line with the outcome
    /// via `format_line`.
    pub fn claim(&mut self, button: PressButton) -> Option<u32> {
        let index = self.pending.iter().position(|(b, _)| *b == button)?;
        Some(self.pending.remove(index).1)
    }

    /// Flush unclaimed pending presses as bare lines (the press
    /// did nothing a consumer logged). Called at the start of
    /// the next frame's `track_touches`.
    pub fn flush_unclaimed(&mut self) {
        for (button, count) in self.pending.drain(..) {
            crate::startup_log::log(&Self::format_line(button, count, None));
        }
    }

    /// One press line: `press: use #12 -> talked to Embry`, or
    /// `press: jump #5` with no outcome. Pure — unit-tested.
    pub fn format_line(button: PressButton, count: u32, outcome: Option<&str>) -> String {
        match outcome {
            Some(outcome) => format!("press: {} #{count} -> {outcome}", button.name()),
            None => format!("press: {} #{count}", button.name()),
        }
    }

    /// The sprint button's held state last frame (edge detection
    /// — sprint is a held button, not an edge-triggered flag).
    pub fn sprint_held(&self) -> bool {
        self.sprint_held
    }

    /// Record the sprint button's held state this frame.
    pub fn set_sprint_held(&mut self, held: bool) {
        self.sprint_held = held;
    }
}

/// Touch input for the player controller, rebuilt every frame by
/// [`track_touches`]. On desktop this resource is never created —
/// the player systems read it as `Option<Res<..>>`.
#[derive(Debug, Default, Resource)]
pub struct TouchInputState {
    /// Joystick vector: x = strafe right, y = forward. Length 0..1.
    pub move_vec: Vec2,
    /// Jump button pressed *this frame* (edge-triggered: holding
    /// the button does not bounce the player).
    pub jump_pressed: bool,
    /// USE button pressed *this frame* (M15, edge-triggered like
    /// jump: holding it does not toggle the door repeatedly).
    pub use_pressed: bool,
    /// BAG button pressed *this frame* (M17, edge-triggered like
    /// jump: holding it does not flicker the inventory open and
    /// shut).
    pub bag_pressed: bool,
    /// MENU button pressed *this frame* (UI milestone 1,
    /// edge-triggered like BAG: holding it does not flicker the
    /// pause screen open and shut).
    pub menu_pressed: bool,
    /// MAP button pressed *this frame* (UI milestone 2,
    /// edge-triggered like MENU: holding it does not flicker
    /// the character menu open and shut).
    pub map_pressed: bool,
    /// ATTACK (right hand) button pressed *this frame*
    /// (melee, edge-triggered like jump: holding it does not
    /// swing repeatedly — the swing cooldown gates repeats).
    pub attack_right_pressed: bool,
    /// ATTACK (left hand) button pressed *this frame* (melee,
    /// edge-triggered like the right hand).
    pub attack_left_pressed: bool,
    /// Sprint button held.
    pub sprint: bool,
    /// Look drag this frame, in logical pixels (same units and
    /// sign as mouse motion: +x turns right, +y looks down).
    pub look_delta: Vec2,
}

/// Compass geometry (M8.1, reworked for Dylan's EMPTY frame).
/// The visible window is the frame image, [`COMPASS_WIDTH_VH`]
/// wide, and shows [`COMPASS_WINDOW_DEGREES`] of heading; the
/// strip holds two full rotations so it wraps.
///
/// The frame is Dylan's EMPTY compass frame
/// (`ui/compass_frame_empty.png`, 2048x170) at its native aspect
/// (M11 rule: never squashed) at the old bar height, so the top
/// row — compass, menu buttons — reads as one HUD row (the bars
/// moved to the original bottom row in UI milestone 1).
/// Measured from the frame pixels: the top rail sits at
/// 16.5–22.4% of the height, the bottom rail at 73.5–78.8%, and
/// the knotwork diamonds span the outer 8.25% each side — so the
/// letters and ticks live in the clear band between the rails.
///
/// The marker is the diamond cropped from Dylan's marker image
/// (`ui/04_compass_diamond.png`, 41x58): rows 6..18 hold the
/// diamond itself. It sits ABOVE the frame, pointing down at the
/// current heading — the old layout rendered the whole marker
/// (diamond + big V chevron) at full frame height INSIDE the
/// frame, and the V covered the centred letter at every cardinal
/// heading, which is what "the compass still broke" looked like
/// on screen. The V chevron stays out; the diamond is the marker.
const COMPASS_HEIGHT_VH: f32 = 4.4;
const COMPASS_FRAME_ASPECT: f32 = 2048.0 / 170.0;
const COMPASS_WIDTH_VH: f32 = COMPASS_HEIGHT_VH * COMPASS_FRAME_ASPECT;
const COMPASS_WINDOW_DEGREES: f32 = 120.0;
/// Viewport-height units per degree of heading.
const COMPASS_VH_PER_DEGREE: f32 = COMPASS_WIDTH_VH / COMPASS_WINDOW_DEGREES;
/// One full rotation of the strip, in vh.
const COMPASS_ROTATION_VH: f32 = 360.0 * COMPASS_VH_PER_DEGREE;
/// Strip width: two rotations, for the wrap.
const COMPASS_STRIP_VH: f32 = 2.0 * COMPASS_ROTATION_VH;
/// Frame top, in vh (2.8% of the window height — the old top).
const COMPASS_TOP_VH: f32 = 2.8;
/// Marker crop rect in texture pixels: the diamond in
/// `04_compass_diamond.png` spans x 14..27, y 6..18 (the
/// horizontal line at rows 12–13 is the old frame's top rail
/// baked into the cut-out — it reads as the diamond's mount).
const COMPASS_MARKER_RECT: [f32; 4] = [14.0, 6.0, 27.0, 18.0];
/// Marker width (vh); height follows the crop's aspect.
const COMPASS_MARKER_WIDTH_VH: f32 = 1.8;
const COMPASS_MARKER_HEIGHT_VH: f32 = COMPASS_MARKER_WIDTH_VH
    * (COMPASS_MARKER_RECT[3] - COMPASS_MARKER_RECT[1])
    / (COMPASS_MARKER_RECT[2] - COMPASS_MARKER_RECT[0]);
/// Letter font size (px at 720p): 20px at the old 5.0vh frame
/// height, scaled to the new 4.4vh frame.
const COMPASS_FONT_PX: f32 = 18.0;
/// Letter node width (px): wide enough for a two-letter label
/// ("NE") at the compass font. The text is centred in the node
/// (`TextLayout::Center`), and the node is centred on its
/// heading position by a negative left margin — the old fixed
/// -12px margin over-shifted single letters left of the marker.
const COMPASS_LETTER_WIDTH_PX: f32 = 36.0;

/// Bar geometry (UI milestone 1): the original bars from the
/// game's `hudmenu.gfx` (see
/// `interface-extract/study/pause_hud_study.md`). Stage 1280x720;
/// vh here is stage px / 7.2 (the stage is 720 tall, so the bars
/// scale with the window height like the inventory's stage).
///
/// Frame (shape 725): 424x29, dark plate (35,31,32,153) + light
/// edge (187,189,191,153). Fill (shape 726): 370x21. The .swf's
/// static fill slots are adjacent with ~8px gaps — Health
/// 141-511, Stamina 519-888, Magicka 892-1262 (in-game color
/// order); the fill band is y 597-618, the frame band y 600-631.
/// The static placements are authoring scaffolding (the Meter
/// script repositions at runtime; the frames overlap 46px at the
/// static slots and the Magicka frame runs 9px past the right
/// edge), so the build uses the slot geometry as-is: frames
/// first, then fills, so no frame covers another bar's fill.
/// The gradient fill's stops are not decodable — the fill is a
/// two-tone approximation (base + a lighter top strip). The
/// overlay bitmap (shape 742) could not be identified — skipped.
///
/// NO text fields exist anywhere in the meter subtrees: the
/// original bars have NO labels. Dylan's EMPTY bar frames
/// (`ui/bar_frame_empty.png`) are unused now — the file stays in
/// assets, like the old compass frame.
const BAR_FRAME_W_VH: f32 = 424.0 / 7.2;
const BAR_FRAME_H_VH: f32 = 29.0 / 7.2;
const BAR_FILL_W_VH: f32 = 370.0 / 7.2;
const BAR_FILL_H_VH: f32 = 21.0 / 7.2;
/// Frame band y 600, fill band y 597 (the fill sits 3px above
/// the frame's top edge in the .swf — kept as-is).
const BAR_FRAME_TOP_VH: f32 = 600.0 / 7.2;
const BAR_FILL_TOP_VH: f32 = 597.0 / 7.2;
/// Fill slot left edges (stage px 141 / 519 / 892): Health,
/// Stamina, Magicka.
const BAR_FILL_LEFTS_VH: [f32; 3] = [141.0 / 7.2, 519.0 / 7.2, 892.0 / 7.2];
/// Frame left edges = fill left - 27px (stage px 114 / 492 /
/// 865). The Magicka frame runs 9px past the stage's right edge
/// — the window clips it, as the .swf itself does.
const BAR_FRAME_LEFTS_VH: [f32; 3] = [114.0 / 7.2, 492.0 / 7.2, 865.0 / 7.2];
/// Frame plate + edge, from the .swf's fill colors.
const BAR_PLATE: Color = Color::srgba(35.0 / 255.0, 31.0 / 255.0, 32.0 / 255.0, 153.0 / 255.0);
const BAR_EDGE: Color = Color::srgba(187.0 / 255.0, 189.0 / 255.0, 191.0 / 255.0, 153.0 / 255.0);
/// Fill colors, sampled from the centres of the old baked bars
/// (01/02/03) so the fills match the old look; the LIGHT
/// variants are the gradient approximation's bright top.
const HEALTH_FILL: Color = Color::srgb(0.76, 0.06, 0.01);
const HEALTH_FILL_LIGHT: Color = Color::srgb(0.95, 0.35, 0.20);
const MAGICKA_FILL: Color = Color::srgb(0.21, 0.58, 0.88);
const MAGICKA_FILL_LIGHT: Color = Color::srgb(0.45, 0.75, 1.0);
const STAMINA_FILL: Color = Color::srgb(0.10, 0.40, 0.15);
const STAMINA_FILL_LIGHT: Color = Color::srgb(0.30, 0.65, 0.30);

/// Fill width (vh) for a current/max fraction: the fraction of
/// the .swf fill width, clamped to [0, 1]. Pure — unit-tested.
fn bar_fill_width_vh(fraction: f32) -> f32 {
    fraction.clamp(0.0, 1.0) * BAR_FILL_W_VH
}

/// Heading (degrees clockwise from north) for a camera yaw. Yaw 0
/// faces north (Bevy -Z is Skyrim +Y); turning right decreases
/// yaw, so the heading is the negated yaw, wrapped to [0, 360).
/// Pure — unit-tested.
pub fn heading_degrees(yaw: f32) -> f32 {
    (-yaw.to_degrees()).rem_euclid(360.0)
}

/// Left offset of the compass strip (vh) for a heading: the strip
/// slides left as the heading turns right, so the current heading
/// stays centred. Pure — unit-tested.
pub fn compass_strip_offset_vh(heading_deg: f32) -> f32 {
    -(heading_deg.rem_euclid(360.0) * COMPASS_VH_PER_DEGREE)
}

/// Viewport position (vh from the window's left edge, relative to
/// the compass frame) of the letter for `letter_deg` when the
/// heading is `heading_deg`: the strip offset plus the letter's
/// strip position, wrapped to the nearest copy so the strip's
/// second rotation covers headings near 0/360. The current
/// heading's letter sits at the window centre
/// (`COMPASS_WIDTH_VH / 2`); a letter clockwise from the heading
/// (e.g. N at heading 350) sits right of centre. Pure —
/// unit-tested.
pub fn compass_letter_position_vh(letter_deg: f32, heading_deg: f32) -> f32 {
    let delta = (letter_deg - heading_deg).rem_euclid(360.0);
    let delta = if delta > 180.0 { delta - 360.0 } else { delta };
    COMPASS_WIDTH_VH / 2.0 + delta * COMPASS_VH_PER_DEGREE
}

/// One round action button: its art, its place on screen, what it
/// does. Centres are percent of window width/height, diameter is
/// percent of viewport height — the layout scales to any phone.
struct ButtonLayout {
    action: ButtonAction,
    image: &'static str,
    x_pct: f32,
    y_pct: f32,
    diameter_vh: f32,
}

/// Layout from the template (1920x1080), converted to %/vh.
const BUTTONS: &[ButtonLayout] = &[
    ButtonLayout {
        action: ButtonAction::Sprint,
        image: "ui/09_sprint.png",
        x_pct: 8.85,
        y_pct: 75.0,
        diameter_vh: 12.0,
    },
    ButtonLayout {
        action: ButtonAction::Jump,
        image: "ui/10_jump.png",
        x_pct: 78.1,
        y_pct: 60.2,
        diameter_vh: 13.0,
    },
    ButtonLayout {
        action: ButtonAction::Inert,
        image: "ui/11_sneak.png",
        x_pct: 88.5,
        y_pct: 51.9,
        diameter_vh: 11.1,
    },
    ButtonLayout {
        action: ButtonAction::Inert,
        image: "ui/12_shout.png",
        x_pct: 95.1,
        y_pct: 57.4,
        diameter_vh: 11.1,
    },
    ButtonLayout {
        // M15: USE opens/closes the nearest door (E on keyboard).
        // It was the fly-mode "descend" button; gravity killed
        // that, and it sat inert until interaction existed.
        action: ButtonAction::Use,
        image: "ui/13_use.png",
        x_pct: 68.75,
        y_pct: 72.2,
        diameter_vh: 12.0,
    },
    ButtonLayout {
        action: ButtonAction::AttackRight,
        image: "ui/14_attack_right_hand.png",
        x_pct: 89.6,
        y_pct: 74.1,
        diameter_vh: 20.4,
    },
    ButtonLayout {
        action: ButtonAction::AttackLeft,
        image: "ui/15_attack_left_hand.png",
        x_pct: 77.1,
        y_pct: 85.2,
        diameter_vh: 13.9,
    },
];

/// Joystick: centre (% of window) and base diameter (vh).
const JOYSTICK_X_PCT: f32 = 8.85;
const JOYSTICK_Y_PCT: f32 = 50.0;
const JOYSTICK_DIAMETER_VH: f32 = 26.0;
/// Knob diameter (vh): 112/350 of the base art.
const JOYSTICK_KNOB_VH: f32 = 8.3;

/// Marker for a touch button node (press tint target).
#[derive(Component)]
struct TouchButton {
    action: ButtonAction,
}

/// Marker for the joystick knob node (moved by the thumb).
#[derive(Component)]
struct JoystickKnob;

/// Marker for the scrolling compass strip node (M8.1).
#[derive(Component)]
struct CompassStrip;

/// Marker for the touch HUD root node: every HUD element
/// (bars, compass, joystick, buttons, quick slots, crosshair)
/// hangs under this one root, so one `Visibility` flip hides
/// or shows the whole HUD (v0.4.6 phone feedback: the HUD drew
/// over the loading screen during Loading/Spawning).
#[derive(Component)]
struct TouchHudRoot;

/// Marker for the Bevy HUD's DISPLAY elements (M4): the bar
/// frames + fills, the compass container, and the crosshair — the
/// parts the real Ruffle HUD replaces. The touch buttons,
/// joystick, and quick slots are INPUT, not display, and are NOT
/// marked: they stay visible (and tappable) under the Ruffle HUD.
/// The Ruffle plugin hides these nodes while the real HUD is
/// active; they stay visible when it is off or failed to build
/// (the Bevy HUD is the fallback). Visibility is inherited, so
/// hiding the compass container hides its marker/frame/strip and
/// hiding a bar fill hides its light strip.
#[derive(Component)]
pub struct BevyHudDisplay;

/// Whether the touch HUD is visible in a state with the given
/// menu state: InGame only, and hidden while any menu is open
/// (v0.4.13 phone feedback: the joystick and buttons drew OVER
/// the pause/inventory/dialogue screens). During Loading
/// (ESM parse) and Spawning (the chunked scene build) the
/// loading screen owns the display — the joystick, buttons,
/// bars and compass must not draw over it (Dylan's v0.4.6
/// screenshot: the joystick overlapped "Loading", SPRINT
/// overlapped "Parsing", the bars crossed the progress text).
/// Pure, so the unit test pins the rule directly.
pub fn touch_hud_visible(state: crate::AppState, menus_open: bool) -> bool {
    matches!(state, crate::AppState::InGame) && !menus_open
}

/// Which stat a bar fill node shows (M11): Dylan's empty bar
/// frames are the background and the code draws the fill — red
/// health, blue magicka, green stamina — with the width as the
/// current/max fraction of the fill track.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum BarFill {
    Health,
    Magicka,
    Stamina,
}

pub struct TouchControlsPlugin;

impl Plugin for TouchControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TouchInputState>()
            .init_resource::<PressLog>()
            .add_systems(Startup, spawn_touch_ui)
            .add_systems(
                Update,
                // Track first so the player systems read this
                // frame's input, not last frame's.
                (track_touches, update_touch_ui)
                    .chain()
                    .before(crate::player::mouse_look),
            )
            .add_systems(
                Update,
                // These read the player's new state (yaw after
                // mouse_look, stamina after the move system), so
                // they run after the player systems.
                (update_compass, update_bars).after(crate::player::keyboard_move),
            )
            // v0.4.6 phone feedback: the HUD is InGame-only —
            // hide it over the loading screen (Loading/Spawning).
            // Runs in every state (no run_if): the state is the
            // input, so the system must run to read it.
            .add_systems(Update, update_touch_hud_visibility);
    }
}

/// Centre of a button in logical pixels.
fn button_centre(layout: &ButtonLayout, size: Vec2) -> Vec2 {
    Vec2::new(layout.x_pct / 100.0 * size.x, layout.y_pct / 100.0 * size.y)
}

/// Hit radius of a button in logical pixels (15% larger than the
/// drawn button — thumbs are imprecise).
fn button_radius(layout: &ButtonLayout, size: Vec2) -> f32 {
    layout.diameter_vh / 100.0 * size.y / 2.0 * 1.15
}

/// Joystick centre + grab radius in logical pixels.
fn joystick_zone(size: Vec2) -> (Vec2, f32) {
    let centre = Vec2::new(
        JOYSTICK_X_PCT / 100.0 * size.x,
        JOYSTICK_Y_PCT / 100.0 * size.y,
    );
    let radius = JOYSTICK_DIAMETER_VH / 100.0 * size.y / 2.0 * 1.3;
    (centre, radius)
}

/// Which menu button (if any) a point is on: the top-right row
/// (map / bag / menu), in that order left to right. Mirrors the
/// spawn geometry in `spawn_touch_ui` — right 1%, top 2.8%, three
/// 7vh buttons with 1vh gaps — so the hit test and the drawing
/// can never drift apart without this function changing too.
/// Pure — unit-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuButton {
    Map,
    Bag,
    Menu,
}

const MENU_RIGHT_PCT: f32 = 1.0;
const MENU_TOP_PCT: f32 = 2.8;
const MENU_BUTTON_VH: f32 = 7.0;
const MENU_GAP_VH: f32 = 1.0;

fn menu_button_hit(point: Vec2, size: Vec2) -> Option<MenuButton> {
    let vh = size.y / 100.0;
    let row_width = 3.0 * MENU_BUTTON_VH * vh + 2.0 * MENU_GAP_VH * vh;
    let row_left = size.x * (1.0 - MENU_RIGHT_PCT / 100.0) - row_width;
    let row_top = size.y * MENU_TOP_PCT / 100.0;
    let button = MENU_BUTTON_VH * vh;
    if point.y < row_top || point.y > row_top + button {
        return None;
    }
    let x = point.x - row_left;
    if x < 0.0 || x > row_width {
        return None;
    }
    let step = button + MENU_GAP_VH * vh;
    let index = (x / step) as usize;
    // A point in a gap hits nothing.
    if x - index as f32 * step > button {
        return None;
    }
    match index {
        0 => Some(MenuButton::Map),
        1 => Some(MenuButton::Bag),
        2 => Some(MenuButton::Menu),
        _ => None,
    }
}

/// Which button (if any) a point is on.
fn hit_button(point: Vec2, size: Vec2) -> Option<ButtonAction> {
    BUTTONS.iter().find_map(|layout| {
        let centre = button_centre(layout, size);
        let radius = button_radius(layout, size);
        (point.distance(centre) <= radius).then_some(layout.action)
    })
}

/// Which inert (sneak/shout) button a point is on, if any.
/// Both are `ButtonAction::Inert` — told apart by their art
/// (the press log names them; the unit test pins the two
/// images so a rename breaks loudly, not silently).
fn inert_press_button(point: Vec2, size: Vec2) -> Option<PressButton> {
    BUTTONS
        .iter()
        .find(|layout| {
            layout.action == ButtonAction::Inert
                && point.distance(button_centre(layout, size))
                    <= button_radius(layout, size)
        })
        .and_then(|layout| match layout.image {
            "ui/11_sneak.png" => Some(PressButton::Sneak),
            "ui/12_shout.png" => Some(PressButton::Shout),
            _ => None,
        })
}

/// Joystick vector for a thumb at `point` with the stick centred at
/// `centre` and radius `radius`: x = right, y = forward (screen-up
/// is forward, so the y axis flips). Clamped to the unit circle.
fn joystick_vector(point: Vec2, centre: Vec2, radius: f32) -> Vec2 {
    let offset = point - centre;
    let clamped = if offset.length() > radius {
        offset.normalize() * radius
    } else {
        offset
    };
    Vec2::new(clamped.x, -clamped.y) / radius
}

/// Read the active touches and fill [`TouchInputState`].
///
/// Classification uses each touch's *start* position, so a finger
/// keeps its role even if it slides: joystick zone first, then a
/// button, then (right half) look drag. Stateless — recomputed from
/// [`Touches`] every frame, so there is no stale-touch bookkeeping
/// to get wrong.
fn track_touches(
    touches: Res<Touches>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut input: ResMut<TouchInputState>,
    mut press_log: Option<ResMut<PressLog>>,
) {
    // Flush last frame's unclaimed presses FIRST: every
    // consumer has had its chance by now (they run before this
    // system in Update, so they claim a press the frame after
    // it is recorded — the flush can never steal one).
    if let Some(log) = press_log.as_deref_mut() {
        log.flush_unclaimed();
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let size = window.size();
    *input = TouchInputState::default();
    let (stick_centre, stick_radius) = joystick_zone(size);

    // Lowest touch id wins each role, so two fingers in one zone
    // behave deterministically.
    let mut ordered: Vec<_> = touches.iter().collect();
    ordered.sort_by_key(|t| t.id());

    let mut stick_taken = false;
    let mut look_taken = false;
    for touch in ordered {
        let start = touch.start_position();
        if !stick_taken && start.distance(stick_centre) <= stick_radius {
            stick_taken = true;
            input.move_vec = joystick_vector(touch.position(), stick_centre, stick_radius / 1.3);
            continue;
        }
        if let Some(action) = hit_button(start, size) {
            match action {
                ButtonAction::Jump => {}
                ButtonAction::Sprint => input.sprint = true,
                ButtonAction::Use => {}
                ButtonAction::AttackRight => {}
                ButtonAction::AttackLeft => {}
                ButtonAction::Inert => {}
            }
            continue;
        }
        // A touch that starts on a menu button is that button's
        // touch — it must not also drag the camera (the row is on
        // the right half, where look drags live).
        if menu_button_hit(start, size).is_some() {
            continue;
        }
        if !look_taken && start.x > size.x / 2.0 {
            look_taken = true;
            input.look_delta += touch.delta();
        }
    }
    // Jump is edge-triggered (M8.1): only a touch that *started*
    // this frame on the jump button fires it — holding the button
    // does not bounce the player. USE is edge-triggered the same
    // way (M15): holding it does not toggle the door repeatedly.
    // BAG is edge-triggered too (M17): holding it does not flicker
    // the inventory open and shut. MENU is edge-triggered the
    // same way (UI milestone 1): holding it does not flicker the
    // pause screen. ATTACK R/L are edge-triggered the same way
    // (melee): holding them does not swing repeatedly.
    //
    // Press log: jump/sneak/shout log immediately (no consumer
    // owns their outcome); use/attack/bag/menu/map are recorded
    // pending — the consumer claims the press and logs the line
    // WITH the outcome (dialogue_use, melee_attack, the
    // menu-open observers). Sprint is held, not edge-triggered:
    // its rising edge logs below.
    if let Some(log) = press_log.as_deref_mut() {
        if input.sprint && !log.sprint_held() {
            log.log_now(PressButton::Sprint, Some("sprinting"));
        }
        log.set_sprint_held(input.sprint);
    }
    for touch in touches.iter_just_pressed() {
        let start = touch.start_position();
        if hit_button(start, size) == Some(ButtonAction::Jump) {
            input.jump_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.log_now(PressButton::Jump, None);
            }
        }
        if hit_button(start, size) == Some(ButtonAction::Use) {
            input.use_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::Use);
            }
        }
        if hit_button(start, size) == Some(ButtonAction::AttackRight) {
            input.attack_right_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::AttackRight);
            }
        }
        if hit_button(start, size) == Some(ButtonAction::AttackLeft) {
            input.attack_left_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::AttackLeft);
            }
        }
        if menu_button_hit(start, size) == Some(MenuButton::Bag) {
            input.bag_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::Bag);
            }
        }
        if menu_button_hit(start, size) == Some(MenuButton::Menu) {
            input.menu_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::Menu);
            }
        }
        if menu_button_hit(start, size) == Some(MenuButton::Map) {
            input.map_pressed = true;
            if let Some(log) = press_log.as_deref_mut() {
                log.record(PressButton::Map);
            }
        }
        if let Some(button) = inert_press_button(start, size) {
            if let Some(log) = press_log.as_deref_mut() {
                log.log_now(button, None);
            }
        }
    }
}

/// Slide the compass strip with the camera heading (M8.1) and keep
/// the bar fills in step with the player's stats (M11). Both read
/// this frame's player state.
fn update_compass(
    player: Query<&LookAngles, With<Player>>,
    mut strip: Query<&mut Node, With<CompassStrip>>,
) {
    let Ok(angles) = player.single() else {
        return;
    };
    let Ok(mut node) = strip.single_mut() else {
        return;
    };
    node.left = Val::Vh(compass_strip_offset_vh(heading_degrees(angles.yaw)));
}

/// Keep the three bar fills in step with the player's stats
/// (M11, reworked to the .swf geometry in UI milestone 1): fill
/// width is the current/max fraction of the .swf fill width.
/// Health and magicka stay full — there is no combat or magic to
/// drive them yet, and faking movement would be a lie.
fn update_bars(stamina: Res<Stamina>, mut fills: Query<(&BarFill, &mut Node)>) {
    let stamina_frac = (stamina.value / crate::player::STAMINA_MAX).clamp(0.0, 1.0);
    for (kind, mut node) in fills.iter_mut() {
        let fraction = match kind {
            BarFill::Health | BarFill::Magicka => 1.0,
            BarFill::Stamina => stamina_frac,
        };
        node.width = Val::Vh(bar_fill_width_vh(fraction));
    }
}

/// Show the touch HUD InGame, hide it otherwise (v0.4.6 phone
/// feedback, Bug 2) — and hide it while any menu is open
/// (v0.4.13: the buttons drew over the pause/inventory/
/// dialogue screens). One flip on the root: Bevy's visibility
/// is inherited, so every HUD child follows. Assigns only on
/// change — writing the same value every frame would mark the
/// component changed and churn the UI render. The loading
/// screen is a separate entity tree (lib.rs) and is untouched.
/// The menu resources are `Option` (the headless tests have no
/// menu plugins); absent means closed.
fn update_touch_hud_visibility(
    state: Res<State<crate::AppState>>,
    dialogue: Option<Res<crate::dialogue::DialogueOpen>>,
    inventory: Option<Res<crate::inventory::InventoryOpen>>,
    pause: Option<Res<crate::pause::PauseOpen>>,
    character: Option<Res<crate::character::CharacterMenuOpen>>,
    mut root: Query<&mut Visibility, With<TouchHudRoot>>,
) {
    let menus_open = dialogue.map(|o| o.0).unwrap_or(false)
        || inventory.map(|o| o.0).unwrap_or(false)
        || pause.map(|o| o.0).unwrap_or(false)
        || character.map(|o| o.0).unwrap_or(false);
    let want = if touch_hud_visible(*state.get(), menus_open) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for mut visibility in root.iter_mut() {
        if *visibility != want {
            *visibility = want;
        }
    }
}

/// Move the joystick knob with the thumb and tint held buttons.
fn update_touch_ui(
    touches: Res<Touches>,
    input: Res<TouchInputState>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut knob: Query<&mut Node, With<JoystickKnob>>,
    mut buttons: Query<(&TouchButton, &mut ImageNode)>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let size = window.size();

    // Knob: centred on the stick, offset by the joystick vector.
    if let Ok(mut node) = knob.single_mut() {
        let base_px = JOYSTICK_DIAMETER_VH / 100.0 * size.y;
        let knob_px = JOYSTICK_KNOB_VH / 100.0 * size.y;
        let travel = (base_px - knob_px) / 2.0 * 0.85;
        let rest = (base_px - knob_px) / 2.0;
        node.left = Val::Px(rest + input.move_vec.x * travel);
        node.top = Val::Px(rest - input.move_vec.y * travel);
    }

    // Press tint: any button with a live touch that started on it.
    // Indexed by ButtonAction discriminant — sized by the const.
    let mut held = [false; BUTTON_ACTION_COUNT];
    for touch in touches.iter() {
        if let Some(action) = hit_button(touch.start_position(), size) {
            held[action as usize] = true;
        }
    }
    for (button, mut image) in buttons.iter_mut() {
        image.color = if held[button.action as usize] {
            Color::srgb(0.65, 0.65, 0.65)
        } else {
            Color::WHITE
        };
    }
}

/// Spawn the whole touch UI: bars, compass, menu buttons, joystick,
/// action cluster, quick slots, crosshair.
fn spawn_touch_ui(mut commands: Commands, asset_server: Res<AssetServer>) {
    // v0.2.2: Startup marker — the touch UI is main-path-only, so
    // this line separates a Startup crash from an Update crash.
    crate::startup_log::log("touch UI spawned (Startup ran)");
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                ..Default::default()
            },
            TouchHudRoot,
        ))
        .with_children(|root| {
            // Bars, bottom row (UI milestone 1): the original
            // look from `hudmenu.gfx` — a dark plate + light edge
            // drawn in code, Health left, Stamina centre,
            // Magicka right, at the .swf's fill slots. NO labels:
            // the original has none. The frames spawn first and
            // the fills after, all flat under the root, so no
            // frame covers another bar's fill (the .swf's frames
            // overlap 46px at the static slots). Each fill is a
            // two-tone approximation of the .swf's vertical
            // gradient: the base color with a lighter top strip
            // (the gradient stops are not decodable). The dark
            // plate is the track — the empty part of a bar shows
            // the plate, not a hole.
            for left in BAR_FRAME_LEFTS_VH {
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Vh(left),
                        top: Val::Vh(BAR_FRAME_TOP_VH),
                        width: Val::Vh(BAR_FRAME_W_VH),
                        height: Val::Vh(BAR_FRAME_H_VH),
                        border: UiRect::all(Val::Px(1.0)),
                        ..Default::default()
                    },
                    BackgroundColor(BAR_PLATE),
                    BorderColor::all(BAR_EDGE),
                    BevyHudDisplay,
                ));
            }
            for (i, (kind, color, light)) in [
                (BarFill::Health, HEALTH_FILL, HEALTH_FILL_LIGHT),
                (BarFill::Stamina, STAMINA_FILL, STAMINA_FILL_LIGHT),
                (BarFill::Magicka, MAGICKA_FILL, MAGICKA_FILL_LIGHT),
            ]
            .into_iter()
            .enumerate()
            {
                // Full width at spawn, so the first frame never
                // shows an empty bar; `update_bars` sets the
                // width every frame after.
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Vh(BAR_FILL_LEFTS_VH[i]),
                        top: Val::Vh(BAR_FILL_TOP_VH),
                        width: Val::Vh(BAR_FILL_W_VH),
                        height: Val::Vh(BAR_FILL_H_VH),
                        ..Default::default()
                    },
                    BackgroundColor(color),
                    kind,
                    BevyHudDisplay,
                ))
                .with_children(|fill| {
                    fill.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Percent(100.0),
                            height: Val::Percent(40.0),
                            ..Default::default()
                        },
                        BackgroundColor(light),
                    ));
                });
            }

            // Compass, top-centre: Dylan's EMPTY frame
            // (`ui/compass_frame_empty.png`) with the letters and
            // ticks on a strip that slides with the camera
            // heading, clipped to the frame; the diamond marker
            // sits above the frame, pointing down at the current
            // heading. The container is marker + frame tall, so
            // the frame's top lands at the old 2.8% with the
            // marker clear above it — the marker must NOT sit
            // inside the frame (the old full-height marker covered
            // the centred letter).
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Vh(COMPASS_TOP_VH - COMPASS_MARKER_HEIGHT_VH),
                    width: Val::Vh(COMPASS_WIDTH_VH),
                    height: Val::Vh(COMPASS_MARKER_HEIGHT_VH + COMPASS_HEIGHT_VH),
                    margin: UiRect::left(Val::Vh(-COMPASS_WIDTH_VH / 2.0)),
                    ..Default::default()
                },
                BevyHudDisplay,
            ))
            .with_children(|compass| {
                // Fixed diamond at the centre, above the frame —
                // cropped from Dylan's marker image (see
                // COMPASS_MARKER_RECT). Its point touches the
                // frame's top rail.
                let mut marker = ImageNode::new(asset_server.load("ui/04_compass_diamond.png"));
                marker.rect = Some(Rect::new(
                    COMPASS_MARKER_RECT[0],
                    COMPASS_MARKER_RECT[1],
                    COMPASS_MARKER_RECT[2],
                    COMPASS_MARKER_RECT[3],
                ));
                compass.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(50.0),
                        top: Val::Vh(0.0),
                        width: Val::Vh(COMPASS_MARKER_WIDTH_VH),
                        height: Val::Vh(COMPASS_MARKER_HEIGHT_VH),
                        margin: UiRect::left(Val::Vh(-COMPASS_MARKER_WIDTH_VH / 2.0)),
                        ..Default::default()
                    },
                    marker,
                ));
                // The frame: the empty frame image, with the
                // sliding strip clipped to it.
                compass
                    .spawn(Node {
                        position_type: PositionType::Absolute,
                        left: Val::Vh(0.0),
                        top: Val::Vh(COMPASS_MARKER_HEIGHT_VH),
                        width: Val::Percent(100.0),
                        height: Val::Vh(COMPASS_HEIGHT_VH),
                        overflow: Overflow::clip(),
                        ..Default::default()
                    })
                    .with_children(|frame| {
                        // Dark backing behind the strip: the empty
                        // frame is transparent between the rails,
                        // and white letters wash out over a bright
                        // sky or sunlit wall without it. Spawned
                        // first so the frame image and strip render
                        // above it; the rails and knotwork are
                        // opaque enough to read over it. Absolute,
                        // not in-flow: as a flex item it would
                        // shrink to zero width against the frame
                        // image (which refuses to shrink below its
                        // content size) and never render.
                        frame.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Px(0.0),
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.4)),
                        ));
                        frame.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..Default::default()
                            },
                            ImageNode::new(asset_server.load("ui/compass_frame_empty.png")),
                        ));
                        // The strip: two full rotations, so a heading
                        // anywhere in [0, 360) has its letters on screen
                        // after the slide. Spawned after the frame so it
                        // renders above it.
                        let mut strip = frame.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Vh(0.0),
                                top: Val::Px(0.0),
                                width: Val::Vh(COMPASS_STRIP_VH),
                                height: Val::Percent(100.0),
                                ..Default::default()
                            },
                            CompassStrip,
                        ));
                        strip.with_children(|strip| {
                            // Cardinal + intercardinal letters, twice (two
                            // rotations). Each letter's node is centred on
                            // its heading's strip position (nudged half the
                            // window right so the current heading centres in
                            // the frame) and the text is centred in the node
                            // — one- and two-letter labels centre exactly.
                            const LETTERS: [(f32, &str); 8] = [
                                (0.0, "N"),
                                (45.0, "NE"),
                                (90.0, "E"),
                                (135.0, "SE"),
                                (180.0, "S"),
                                (225.0, "SW"),
                                (270.0, "W"),
                                (315.0, "NW"),
                            ];
                            for rotation in 0..2 {
                                for (deg, label) in LETTERS {
                                    let centre = rotation as f32 * COMPASS_ROTATION_VH
                                        + deg * COMPASS_VH_PER_DEGREE;
                                    strip.spawn((
                                        Text::new(label),
                                        TextFont {
                                            font_size: FontSize::Px(COMPASS_FONT_PX),
                                            ..Default::default()
                                        },
                                        TextColor(Color::WHITE),
                                        TextLayout::justify(Justify::Center),
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: Val::Vh(centre + COMPASS_WIDTH_VH / 2.0),
                                            top: Val::Percent(12.0),
                                            width: Val::Px(COMPASS_LETTER_WIDTH_PX),
                                            margin: UiRect::left(Val::Px(
                                                -COMPASS_LETTER_WIDTH_PX / 2.0,
                                            )),
                                            ..Default::default()
                                        },
                                    ));
                                }
                                // Minor ticks every 15°, majors at the letters.
                                for step in 0..24 {
                                    let deg = step as f32 * 15.0;
                                    let centre = rotation as f32 * COMPASS_ROTATION_VH
                                        + deg * COMPASS_VH_PER_DEGREE;
                                    let major = step % 3 == 0;
                                    strip.spawn((
                                        Node {
                                            position_type: PositionType::Absolute,
                                            left: Val::Vh(centre + COMPASS_WIDTH_VH / 2.0),
                                            top: Val::Percent(if major { 62.0 } else { 72.0 }),
                                            width: Val::Px(2.0),
                                            height: Val::Percent(if major { 30.0 } else { 20.0 }),
                                            ..Default::default()
                                        },
                                        BackgroundColor(Color::srgba(
                                            1.0,
                                            1.0,
                                            1.0,
                                            if major { 0.9 } else { 0.5 },
                                        )),
                                    ));
                                }
                            }
                        });
                    });
            });

            // Map / bag / menu, top-right row.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                right: Val::Percent(1.0),
                top: Val::Percent(2.8),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Vh(1.0),
                ..Default::default()
            })
            .with_children(|menu| {
                for image in ["ui/05_map.png", "ui/06_bag.png", "ui/07_menu.png"] {
                    menu.spawn((
                        Node {
                            width: Val::Vh(7.0),
                            height: Val::Vh(7.0),
                            ..Default::default()
                        },
                        ImageNode::new(asset_server.load(image)),
                    ));
                }
            });

            // Joystick base + knob (knob is a child, moved by
            // `update_touch_ui`).
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(JOYSTICK_X_PCT),
                top: Val::Percent(JOYSTICK_Y_PCT),
                width: Val::Vh(JOYSTICK_DIAMETER_VH),
                height: Val::Vh(JOYSTICK_DIAMETER_VH),
                margin: UiRect {
                    left: Val::Vh(-JOYSTICK_DIAMETER_VH / 2.0),
                    top: Val::Vh(-JOYSTICK_DIAMETER_VH / 2.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .with_children(|stick| {
                stick.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..Default::default()
                    },
                    ImageNode::new(asset_server.load("ui/08_joystick_base.png")),
                ));
                stick.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Vh(JOYSTICK_KNOB_VH),
                        height: Val::Vh(JOYSTICK_KNOB_VH),
                        ..Default::default()
                    },
                    ImageNode::new(asset_server.load("ui/08_joystick_knob.png")),
                    JoystickKnob,
                ));
            });

            // Action cluster.
            for layout in BUTTONS {
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Percent(layout.x_pct),
                        top: Val::Percent(layout.y_pct),
                        width: Val::Vh(layout.diameter_vh),
                        height: Val::Vh(layout.diameter_vh),
                        margin: UiRect {
                            left: Val::Vh(-layout.diameter_vh / 2.0),
                            top: Val::Vh(-layout.diameter_vh / 2.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ImageNode::new(asset_server.load(layout.image)),
                    TouchButton {
                        action: layout.action,
                    },
                ));
            }

            // Quick slots, bottom-centre.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                bottom: Val::Percent(2.0),
                width: Val::Vh(25.8),
                height: Val::Vh(9.0),
                margin: UiRect::left(Val::Vh(-12.9)),
                ..Default::default()
            })
            .with_children(|slots| {
                slots.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..Default::default()
                    },
                    ImageNode::new(asset_server.load("ui/19_quick_slots_header_frame.png")),
                ));
            });

            // Crosshair: small white dot, dead centre.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Vh(1.2),
                    height: Val::Vh(1.2),
                    margin: UiRect {
                        left: Val::Vh(-0.6),
                        top: Val::Vh(-0.6),
                        ..Default::default()
                    },
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    ..Default::default()
                },
                bevy::ui::BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
                BevyHudDisplay,
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joystick_vector_points_forward_up() {
        let centre = Vec2::new(100.0, 100.0);
        // Straight up on screen = full forward.
        let v = joystick_vector(Vec2::new(100.0, 50.0), centre, 50.0);
        assert!((v - Vec2::new(0.0, 1.0)).length() < 1e-6);
        // Straight right = full strafe.
        let v = joystick_vector(Vec2::new(150.0, 100.0), centre, 50.0);
        assert!((v - Vec2::new(1.0, 0.0)).length() < 1e-6);
        // Centre = zero.
        let v = joystick_vector(centre, centre, 50.0);
        assert_eq!(v, Vec2::ZERO);
    }

    #[test]
    fn joystick_vector_clamps_to_unit_circle() {
        let centre = Vec2::new(100.0, 100.0);
        let v = joystick_vector(Vec2::new(300.0, 100.0), centre, 50.0);
        assert!((v.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hit_button_finds_jump_and_misses_empty_space() {
        let size = Vec2::new(1920.0, 1080.0);
        // Jump's centre from the layout table.
        let jump = &BUTTONS[1];
        assert_eq!(jump.action, ButtonAction::Jump);
        let centre = button_centre(jump, size);
        assert_eq!(hit_button(centre, size), Some(ButtonAction::Jump));
        // Middle of the screen is no button.
        assert_eq!(hit_button(Vec2::new(960.0, 540.0), size), None);
    }

    #[test]
    fn button_action_discriminants_fit_the_held_array() {
        // v0.4.12 crashed on launch: Inert's discriminant (5)
        // exceeded the 4-slot held array after the melee milestone
        // added two variants. Every variant must index inside it.
        for action in [
            ButtonAction::Jump,
            ButtonAction::Sprint,
            ButtonAction::Use,
            ButtonAction::AttackRight,
            ButtonAction::AttackLeft,
            ButtonAction::Inert,
        ] {
            assert!(
                (action as usize) < BUTTON_ACTION_COUNT,
                "{action:?} does not fit the held array"
            );
        }
    }

    #[test]
    fn menu_button_hit_finds_each_button_and_the_gaps() {
        // 1000x800: 1vh = 8px, row 23vh = 184px wide, right edge
        // 990, so the row spans x 806..990, y 22.4..78.4.
        let size = Vec2::new(1000.0, 800.0);
        // Centres: map 834, bag 898, menu 954, all y 50.4.
        assert_eq!(
            menu_button_hit(Vec2::new(834.0, 50.4), size),
            Some(MenuButton::Map)
        );
        assert_eq!(
            menu_button_hit(Vec2::new(898.0, 50.4), size),
            Some(MenuButton::Bag)
        );
        assert_eq!(
            menu_button_hit(Vec2::new(954.0, 50.4), size),
            Some(MenuButton::Menu)
        );
        // The 1vh gap between map and bag hits nothing.
        assert_eq!(menu_button_hit(Vec2::new(866.0, 50.4), size), None);
        // Above the row and below it hit nothing.
        assert_eq!(menu_button_hit(Vec2::new(898.0, 10.0), size), None);
        assert_eq!(menu_button_hit(Vec2::new(898.0, 90.0), size), None);
        // Left of the row hits nothing.
        assert_eq!(menu_button_hit(Vec2::new(700.0, 50.4), size), None);
    }

    #[test]
    fn heading_from_yaw() {
        // Yaw 0 faces north.
        assert!((heading_degrees(0.0) - 0.0).abs() < 1e-6);
        // Turning right (yaw decreases) heads east.
        assert!((heading_degrees(-std::f32::consts::FRAC_PI_2) - 90.0).abs() < 1e-4);
        // Turning left heads west.
        assert!((heading_degrees(std::f32::consts::FRAC_PI_2) - 270.0).abs() < 1e-4);
        // Facing south: yaw ±π wraps to 180.
        assert!((heading_degrees(std::f32::consts::PI) - 180.0).abs() < 1e-4);
        // A full turn wraps back to north.
        assert!((heading_degrees(-2.0 * std::f32::consts::PI) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn bar_fill_width_scales_with_fraction() {
        assert!((bar_fill_width_vh(1.0) - BAR_FILL_W_VH).abs() < 1e-6);
        assert!((bar_fill_width_vh(0.5) - BAR_FILL_W_VH / 2.0).abs() < 1e-6);
        assert!((bar_fill_width_vh(0.0) - 0.0).abs() < 1e-6);
        // Out-of-range fractions clamp to the track ends.
        assert!((bar_fill_width_vh(1.5) - BAR_FILL_W_VH).abs() < 1e-6);
        assert!((bar_fill_width_vh(-0.5) - 0.0).abs() < 1e-6);
    }

    #[test]
    // The asserts below compare constants — clippy flags them,
    // but the test's job is to guard the geometry constants
    // against future edits (if someone moves a fill slot or
    // resizes a frame, this fails). That is the point.
    #[allow(clippy::assertions_on_constants)]
    fn bars_match_the_swf_geometry() {
        // Frame 424x29, fill 370x21 (hudmenu.gfx shapes 725/726).
        assert!((BAR_FRAME_W_VH - 424.0 / 7.2).abs() < 1e-4);
        assert!((BAR_FRAME_H_VH - 29.0 / 7.2).abs() < 1e-4);
        assert!((BAR_FILL_W_VH - 370.0 / 7.2).abs() < 1e-4);
        assert!((BAR_FILL_H_VH - 21.0 / 7.2).abs() < 1e-4);
        // Fill slots adjacent with small gaps (stage px 141-511,
        // 519-888, 892-1262 — the dump's slots are ±1px, so the
        // gaps are 8px and 3px; both are "adjacent").
        let fill_w_px = BAR_FILL_W_VH * 7.2;
        let lefts_px = [
            BAR_FILL_LEFTS_VH[0] * 7.2,
            BAR_FILL_LEFTS_VH[1] * 7.2,
            BAR_FILL_LEFTS_VH[2] * 7.2,
        ];
        let gap0 = lefts_px[1] - (lefts_px[0] + fill_w_px);
        let gap1 = lefts_px[2] - (lefts_px[1] + fill_w_px);
        assert!((0.0..=10.0).contains(&gap0), "gap0 = {gap0}");
        assert!((0.0..=10.0).contains(&gap1), "gap1 = {gap1}");
        // Frames are the fills expanded 27px each side.
        let frame_lefts_px = [
            BAR_FRAME_LEFTS_VH[0] * 7.2,
            BAR_FRAME_LEFTS_VH[1] * 7.2,
            BAR_FRAME_LEFTS_VH[2] * 7.2,
        ];
        for i in 0..3 {
            assert!((lefts_px[i] - frame_lefts_px[i] - 27.0).abs() < 1e-4);
        }
        // The row sits at the bottom of the stage (frame band y
        // 600-631 of 720) and the fills stay on-stage.
        assert!(BAR_FRAME_TOP_VH * 7.2 + BAR_FRAME_H_VH * 7.2 <= 720.0 + 1.0);
        assert!(lefts_px[2] + fill_w_px <= 1280.0 + 1.0);
    }

    #[test]
    fn compass_strip_offset_slides_left_with_heading() {
        // North: no slide.
        assert!((compass_strip_offset_vh(0.0) - 0.0).abs() < 1e-6);
        // East: slid left by 90° worth of strip.
        let east = compass_strip_offset_vh(90.0);
        assert!((east - (-90.0 * COMPASS_VH_PER_DEGREE)).abs() < 1e-6);
        // A full rotation wraps back to north (offset 0); just
        // before it, the strip is almost one rotation left — the
        // second copy of the letters lines up with the first.
        assert!((compass_strip_offset_vh(360.0) - 0.0).abs() < 1e-6);
        let almost = compass_strip_offset_vh(359.0);
        assert!((almost - (-359.0 * COMPASS_VH_PER_DEGREE)).abs() < 1e-6);
    }

    #[test]
    fn compass_letter_centred_at_heading() {
        let centre = COMPASS_WIDTH_VH / 2.0;
        // Each cardinal + intercardinal letter centres when the
        // heading is its own direction.
        for deg in [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
            let pos = compass_letter_position_vh(deg, deg);
            assert!((pos - centre).abs() < 1e-4, "{deg} not centred: {pos}");
        }
        // Neighbours: facing east, NE sits left of centre and SE
        // right of centre — the strip slides the right way.
        let ne = compass_letter_position_vh(45.0, 90.0);
        let se = compass_letter_position_vh(135.0, 90.0);
        assert!(ne < centre);
        assert!(se > centre);
        assert!((centre - ne - 45.0 * COMPASS_VH_PER_DEGREE).abs() < 1e-4);
        assert!((se - centre - 45.0 * COMPASS_VH_PER_DEGREE).abs() < 1e-4);
        // Wrap: at heading 350, N (0°) is 10° clockwise — right
        // of centre, via the strip's second rotation. At heading
        // 10, N is 10° counter-clockwise — left of centre.
        let n_at_350 = compass_letter_position_vh(0.0, 350.0);
        assert!((n_at_350 - centre - 10.0 * COMPASS_VH_PER_DEGREE).abs() < 1e-4);
        let n_at_10 = compass_letter_position_vh(0.0, 10.0);
        assert!((centre - n_at_10 - 10.0 * COMPASS_VH_PER_DEGREE).abs() < 1e-4);
    }

    #[test]
    // The asserts below compare constants — clippy flags them,
    // but the test's job is to guard the geometry constants
    // against future edits (if someone squashes the frame, this
    // fails). That is the point.
    #[allow(clippy::assertions_on_constants)]
    fn compass_frame_keeps_native_aspect() {
        // The frame is 2048x170; the compass must be exactly that
        // aspect at the bar height, never squashed.
        let aspect = COMPASS_WIDTH_VH / COMPASS_HEIGHT_VH;
        assert!((aspect - 2048.0 / 170.0).abs() < 1e-4);
        // The marker crop is the diamond itself (rows 6..18 of
        // the 58-tall image, cols 14..27 of 41), and its rendered
        // height follows the crop's aspect.
        assert!(COMPASS_MARKER_RECT[1] >= 0.0 && COMPASS_MARKER_RECT[3] <= 58.0);
        assert!(COMPASS_MARKER_RECT[0] >= 0.0 && COMPASS_MARKER_RECT[2] <= 41.0);
        let crop_aspect = (COMPASS_MARKER_RECT[3] - COMPASS_MARKER_RECT[1])
            / (COMPASS_MARKER_RECT[2] - COMPASS_MARKER_RECT[0]);
        assert!((COMPASS_MARKER_HEIGHT_VH / COMPASS_MARKER_WIDTH_VH - crop_aspect).abs() < 1e-4);
        // (The old bars-clearance assert is gone — the bars moved
        // to the original bottom row in UI milestone 1, so they
        // can never collide with the top-centre compass.)
    }

    #[test]
    fn touch_hud_visible_only_ingame() {
        // The v0.4.6 phone-feedback rule, all three states —
        // and the v0.4.13 rule: hidden while any menu is open.
        assert!(!touch_hud_visible(crate::AppState::Loading, false));
        assert!(!touch_hud_visible(crate::AppState::Spawning, false));
        assert!(touch_hud_visible(crate::AppState::InGame, false));
        assert!(!touch_hud_visible(crate::AppState::InGame, true));
        assert!(!touch_hud_visible(crate::AppState::Loading, true));
    }

    #[test]
    fn press_button_discriminants_fit_the_counts_array() {
        // The counts array is indexed by discriminant (the same
        // rule as BUTTON_ACTION_COUNT — the v0.4.12 lesson).
        for button in [
            PressButton::Jump,
            PressButton::Sprint,
            PressButton::Use,
            PressButton::AttackRight,
            PressButton::AttackLeft,
            PressButton::Bag,
            PressButton::Menu,
            PressButton::Map,
            PressButton::Sneak,
            PressButton::Shout,
        ] {
            assert!((button as usize) < PRESS_BUTTON_COUNT, "{button:?}");
        }
        assert_eq!(PRESS_BUTTON_COUNT, 10);
    }

    #[test]
    fn press_log_format_lines() {
        assert_eq!(
            PressLog::format_line(PressButton::Use, 12, Some("talked to Embry")),
            "press: use #12 -> talked to Embry"
        );
        assert_eq!(
            PressLog::format_line(PressButton::AttackRight, 3, Some("no target in range")),
            "press: attack_right #3 -> no target in range"
        );
        assert_eq!(
            PressLog::format_line(PressButton::Jump, 5, None),
            "press: jump #5"
        );
        assert_eq!(
            PressLog::format_line(PressButton::Bag, 2, Some("inventory opened")),
            "press: bag #2 -> inventory opened"
        );
    }

    #[test]
    fn press_log_record_claim_flush() {
        let mut log = PressLog::default();
        // Counts run per button, from 1.
        assert_eq!(log.record(PressButton::Use), 1);
        assert_eq!(log.record(PressButton::Use), 2);
        assert_eq!(log.record(PressButton::Jump), 1);
        // Claim takes the OLDEST pending press for the button
        // (FIFO — two USE presses claim in order).
        assert_eq!(log.claim(PressButton::Use), Some(1));
        assert_eq!(log.claim(PressButton::Use), Some(2));
        // Nothing pending: None (a keyboard-triggered action).
        assert_eq!(log.claim(PressButton::Use), None);
        assert_eq!(log.claim(PressButton::Bag), None);
        // log_now counts without queueing (nothing to claim).
        assert_eq!(log.log_now(PressButton::Jump, None), 2);
        assert_eq!(log.claim(PressButton::Jump), Some(1));
        assert_eq!(log.claim(PressButton::Jump), None);
        // Flush drains the rest (the startup logger is not
        // initialized in tests, so the lines drop silently).
        log.record(PressButton::Bag);
        log.flush_unclaimed();
        assert_eq!(log.claim(PressButton::Bag), None);
    }

    #[test]
    fn inert_press_button_names_sneak_and_shout() {
        // The two Inert layouts, told apart by their art — the
        // press log names them, so a layout/image change must
        // break this test, not silently mislabel presses.
        let size = Vec2::new(1920.0, 1080.0);
        let sneak = BUTTONS
            .iter()
            .find(|l| l.image == "ui/11_sneak.png")
            .expect("sneak layout");
        let shout = BUTTONS
            .iter()
            .find(|l| l.image == "ui/12_shout.png")
            .expect("shout layout");
        assert_eq!(
            inert_press_button(button_centre(sneak, size), size),
            Some(PressButton::Sneak)
        );
        assert_eq!(
            inert_press_button(button_centre(shout, size), size),
            Some(PressButton::Shout)
        );
        // The jump button is not inert.
        let jump = BUTTONS
            .iter()
            .find(|l| l.action == ButtonAction::Jump)
            .expect("jump layout");
        assert_eq!(
            inert_press_button(button_centre(jump, size), size),
            None
        );
    }

    #[test]
    fn touch_hud_visibility_follows_state_on_a_real_app() {
        // The gating system end to end on a headless app
        // (MinimalPlugins + StatesPlugin, plugins before
        // init_state — the v0.2.3 order). The screenshot path
        // cannot show this: it only shoots InGame frames, where
        // the HUD is correctly visible — so the app test is the
        // proof the HUD hides during Loading/Spawning. Only the
        // visibility system runs here (not the whole plugin:
        // spawn_touch_ui needs an AssetServer, track_touches
        // needs a window).
        use bevy::app::PluginGroup;
        use bevy::prelude::{NextState, State};
        use bevy::state::app::AppExtStates;
        let mut app = bevy::app::App::new();
        app.add_plugins(
            bevy::MinimalPlugins
                .build()
                .add(bevy::state::app::StatesPlugin),
        )
        .init_state::<crate::AppState>()
        .add_systems(Update, update_touch_hud_visibility);
        // The HUD root, as `spawn_touch_ui` spawns it (a Node
        // with the marker; the children don't matter — Bevy
        // inherits visibility down the tree).
        let root = app.world_mut().spawn((Node::default(), TouchHudRoot)).id();
        let visibility = |app: &bevy::app::App| {
            *app.world()
                .get::<Visibility>(root)
                .expect("HUD root has Visibility")
        };
        // Loading (the default state): hidden from the first
        // frame — Startup spawns the root before the first
        // Update, so no frame ever shows it during a load.
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);
        // Spawning: still hidden.
        set_state(&mut app, crate::AppState::Spawning);
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);
        // InGame: visible.
        set_state(&mut app, crate::AppState::InGame);
        app.update();
        assert_eq!(visibility(&app), Visibility::Visible);
        // Back to Loading (a door transition re-enters it):
        // hidden again.
        set_state(&mut app, crate::AppState::Loading);
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);
        // InGame with a menu open (v0.4.13): hidden — the
        // buttons must not draw over the dialogue screen.
        set_state(&mut app, crate::AppState::InGame);
        app.insert_resource(crate::dialogue::DialogueOpen(true));
        app.update();
        assert_eq!(visibility(&app), Visibility::Hidden);
        // Menu closed: visible again.
        app.world_mut()
            .resource_mut::<crate::dialogue::DialogueOpen>()
            .0 = false;
        app.update();
        assert_eq!(visibility(&app), Visibility::Visible);

        /// Drive the app until the state reports `want`
        /// (transitions apply on the next update, not
        /// immediately — poll, don't assume the frame).
        fn set_state(app: &mut bevy::app::App, want: crate::AppState) {
            app.world_mut()
                .resource_mut::<NextState<crate::AppState>>()
                .set(want);
            for _ in 0..5 {
                app.update();
                if *app.world().resource::<State<crate::AppState>>().get() == want {
                    return;
                }
            }
            panic!("state never reached {want:?}");
        }
    }
}
