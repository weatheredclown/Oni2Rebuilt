/*
 * debug_atdt.rs — On-screen ATDT progress bar (port of crStrikeProfile/
 * crAtkAnimCtrlBlock::Draw from rb/src/fight/fighter.cpp + attackdata.cpp).
 *
 * Renders a 2D bar showing the player's current attack animation progress
 * with the queue / no-queue / do windows colored.  Mirrors the legacy
 * red/yellow/green bar drawn in __DEV builds.  F9 toggles visibility.
 *
 * Color legend (matches the legacy palette):
 *   RED      base     — input rejected (no-queue zone, post-do tail)
 *   YELLOW   opp2 q   — input is captured into the queue but does not fire yet
 *   GREEN    opp2 do  — queued/fresh input fires the next attack now
 *   DK_GRN   opp2 dc  — critical sub-window of the do zone (lower half)
 *   ORANGE   opp3 q   — late-tail queue (only if queue_next_attack)
 *   DK_GRN   opp3 do  — late-tail do
 *   WHITE    cursor   — current animation phase
 *   ORANGE dot        — visible iff FsmRuntime queued_attack is set
 */
use bevy::prelude::*;

use crate::oni2_loader::Oni2AnimState;
use crate::player::components::Player;
use crate::statemachine::FsmRuntime;

const BAR_WIDTH: f32 = 360.0;
const BAR_HEIGHT: f32 = 18.0;
const TOP_OFFSET: f32 = 60.0;
const LEFT_OFFSET: f32 = 12.0;

#[derive(Resource)]
pub struct AtdtDebugVisible(pub bool);
impl Default for AtdtDebugVisible {
    fn default() -> Self {
        Self(false)
    }
}

#[derive(Component)]
struct AtdtDebugRoot;

#[derive(Component)]
struct AtdtBar;

#[derive(Component)]
struct AtdtLabel;

#[derive(Component, Clone, Copy)]
enum AtdtSeg {
    Opp2Queue,
    Opp2Do,
    Opp2QueueCrit,
    Opp2DoCrit,
    Opp3Queue,
    Opp3Do,
    PhaseCursor,
    QueuedDot,
}

pub struct AtdtDebugPlugin;

impl Plugin for AtdtDebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtdtDebugVisible>()
            .add_systems(Startup, setup_atdt_debug)
            .add_systems(Update, (toggle_atdt_debug, update_atdt_debug));
    }
}

// Color palette (matches the legacy MKRGB() constants).
fn col_red() -> Color {
    Color::srgb(0.55, 0.05, 0.05)
}
fn col_yellow() -> Color {
    Color::srgb(0.95, 0.85, 0.0)
}
fn col_dk_yellow() -> Color {
    Color::srgb(0.55, 0.45, 0.0)
}
fn col_green() -> Color {
    Color::srgb(0.0, 0.85, 0.0)
}
fn col_dk_green() -> Color {
    Color::srgb(0.0, 0.45, 0.0)
}
fn col_orange() -> Color {
    Color::srgb(1.0, 0.55, 0.0)
}

fn setup_atdt_debug(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(TOP_OFFSET),
                left: Val::Px(LEFT_OFFSET),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            GlobalZIndex(i32::MAX - 1),
            AtdtDebugRoot,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("ATDT: --"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                AtdtLabel,
            ));

            parent
                .spawn((
                    Node {
                        width: Val::Px(BAR_WIDTH),
                        height: Val::Px(BAR_HEIGHT),
                        position_type: PositionType::Relative,
                        ..default()
                    },
                    BackgroundColor(col_red()),
                    AtdtBar,
                ))
                .with_children(|bar| {
                    // Full-height segments (queue / do).
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(0.0),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT),
                            ..default()
                        },
                        BackgroundColor(col_yellow()),
                        AtdtSeg::Opp2Queue,
                    ));
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(0.0),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT),
                            ..default()
                        },
                        BackgroundColor(col_green()),
                        AtdtSeg::Opp2Do,
                    ));
                    // Critical sub-windows (lower half).
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(BAR_HEIGHT * 0.5),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT * 0.5),
                            ..default()
                        },
                        BackgroundColor(col_dk_yellow()),
                        AtdtSeg::Opp2QueueCrit,
                    ));
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(BAR_HEIGHT * 0.5),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT * 0.5),
                            ..default()
                        },
                        BackgroundColor(col_dk_green()),
                        AtdtSeg::Opp2DoCrit,
                    ));
                    // Opp3 strip (upper third).
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(0.0),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT / 3.0),
                            ..default()
                        },
                        BackgroundColor(col_orange()),
                        AtdtSeg::Opp3Queue,
                    ));
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(0.0),
                            left: Val::Px(0.0),
                            width: Val::Px(0.0),
                            height: Val::Px(BAR_HEIGHT / 3.0),
                            ..default()
                        },
                        BackgroundColor(col_dk_green()),
                        AtdtSeg::Opp3Do,
                    ));
                    // Phase cursor (drawn last so it sits on top).
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(-2.0),
                            left: Val::Px(0.0),
                            width: Val::Px(2.0),
                            height: Val::Px(BAR_HEIGHT + 4.0),
                            ..default()
                        },
                        BackgroundColor(Color::WHITE),
                        AtdtSeg::PhaseCursor,
                    ));
                    // Queued-input indicator (orange dot to the right of the bar).
                    bar.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(BAR_HEIGHT * 0.5 - 5.0),
                            left: Val::Px(BAR_WIDTH + 8.0),
                            width: Val::Px(10.0),
                            height: Val::Px(10.0),
                            display: Display::None,
                            ..default()
                        },
                        BackgroundColor(col_orange()),
                        AtdtSeg::QueuedDot,
                    ));
                });
        });
}

fn toggle_atdt_debug(keyboard: Res<ButtonInput<KeyCode>>, mut visible: ResMut<AtdtDebugVisible>) {
    if keyboard.just_pressed(KeyCode::F9) {
        visible.0 = !visible.0;
        info!(
            "ATDT debug bar: {}",
            if visible.0 { "ON" } else { "OFF" }
        );
    }
}

fn update_atdt_debug(
    visible: Res<AtdtDebugVisible>,
    player_q: Query<(&Oni2AnimState, &FsmRuntime), With<Player>>,
    mut root_q: Query<&mut Node, (With<AtdtDebugRoot>, Without<AtdtSeg>, Without<AtdtBar>)>,
    mut bar_q: Query<
        &mut Node,
        (
            With<AtdtBar>,
            Without<AtdtDebugRoot>,
            Without<AtdtSeg>,
        ),
    >,
    mut seg_q: Query<
        (&AtdtSeg, &mut Node),
        (Without<AtdtDebugRoot>, Without<AtdtBar>),
    >,
    mut label_q: Query<&mut Text, With<AtdtLabel>>,
) {
    let Ok(mut root_node) = root_q.single_mut() else {
        return;
    };
    if !visible.0 {
        root_node.display = Display::None;
        return;
    }
    root_node.display = Display::Flex;

    // Pull the player's current strike + phase + queued state.
    let (label_str, windows, phase, queued) = match player_q.single() {
        Ok((anim, runtime)) => {
            let strike = anim.anim.attack_data.as_ref().and_then(|a| a.strike.as_ref());
            let total = (anim.anim.num_frames as f32 - 1.0).max(1.0);
            let phase = (anim.current_time / total).clamp(0.0, 1.0);
            let queued = runtime.ctx.queued_attack || runtime.ctx.queued_attack_two;
            match strike {
                Some(s) => {
                    let label = format!(
                        "ATDT  phase={:.3}  qns={:.3} dst={:.3} dend={:.3} q3={:.3} d3={:.3} qNext={} q={}",
                        phase,
                        s.opp2_q_start,
                        s.opp2_do_start,
                        s.opp2_do_end,
                        s.opp3_q_start,
                        s.opp3_do_start,
                        if s.queue_next_attack { "Y" } else { "N" },
                        if queued { "Y" } else { "N" },
                    );
                    (
                        label,
                        Some(Windows {
                            opp2_q_start: s.opp2_q_start,
                            opp2_do_start: s.opp2_do_start,
                            opp2_q_crit_start: s.opp2_crit_start,
                            opp2_do_crit_start: s.opp2_do_crit_start,
                            opp2_do_end: s.opp2_do_end,
                            opp3_q_start: s.opp3_q_start,
                            opp3_do_start: s.opp3_do_start,
                            queue_next_attack: s.queue_next_attack,
                        }),
                        phase,
                        queued,
                    )
                }
                None => (
                    "ATDT  (no attack data on current anim)".to_string(),
                    None,
                    phase,
                    queued,
                ),
            }
        }
        Err(_) => ("ATDT  (no player)".to_string(), None, 0.0, false),
    };

    if let Ok(mut text) = label_q.single_mut() {
        *text = Text::new(label_str);
    }

    // Hide the bar entirely when there's no strike.
    if let Ok(mut bar_node) = bar_q.single_mut() {
        bar_node.display = if windows.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }

    let Some(w) = windows else {
        return;
    };

    // Update each segment's left/width to reflect the current windows, and
    // place the phase cursor + queued indicator.
    for (seg, mut node) in &mut seg_q {
        match seg {
            AtdtSeg::Opp2Queue => {
                let (l, r) = (w.opp2_q_start, w.opp2_do_start);
                set_seg_x(&mut node, l, r);
            }
            AtdtSeg::Opp2Do => {
                let (l, r) = (w.opp2_do_start, w.opp2_do_end);
                set_seg_x(&mut node, l, r);
            }
            AtdtSeg::Opp2QueueCrit => {
                let (l, r) = (w.opp2_q_crit_start, w.opp2_do_crit_start);
                set_seg_x(&mut node, l, r);
            }
            AtdtSeg::Opp2DoCrit => {
                let (l, r) = (w.opp2_do_crit_start, w.opp2_do_end);
                set_seg_x(&mut node, l, r);
            }
            AtdtSeg::Opp3Queue => {
                if w.queue_next_attack {
                    set_seg_x(&mut node, w.opp3_q_start, 1.0);
                } else {
                    node.width = Val::Px(0.0);
                }
            }
            AtdtSeg::Opp3Do => {
                if w.queue_next_attack {
                    set_seg_x(&mut node, w.opp3_do_start, 1.0);
                } else {
                    node.width = Val::Px(0.0);
                }
            }
            AtdtSeg::PhaseCursor => {
                node.left = Val::Px((phase * BAR_WIDTH).clamp(0.0, BAR_WIDTH - 2.0));
            }
            AtdtSeg::QueuedDot => {
                node.display = if queued { Display::Flex } else { Display::None };
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Windows {
    opp2_q_start: f32,
    opp2_do_start: f32,
    opp2_q_crit_start: f32,
    opp2_do_crit_start: f32,
    opp2_do_end: f32,
    opp3_q_start: f32,
    opp3_do_start: f32,
    queue_next_attack: bool,
}

fn set_seg_x(node: &mut Node, l: f32, r: f32) {
    let l = l.clamp(0.0, 1.0);
    let r = r.clamp(0.0, 1.0).max(l);
    node.left = Val::Px(l * BAR_WIDTH);
    node.width = Val::Px((r - l) * BAR_WIDTH);
}
