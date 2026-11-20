/*
 * sizzle/mod.rs — SizzlePlugin: replay rb's sizzle-reel shots in Rust.
 *
 * The C++ engine records each reel shot with tools/sizzle/shoot.sh
 * (`-fixedfps 60 -noui -nohud -cinecam .. -padscript .. -padaim ..`).  These
 * flags give the Rust build the same controls, so the two can be compared
 * frame for frame (tools/sizzle_shot.sh):
 *
 *   --cinecam <file>   keyframed cinematic camera   (cinecam.rs)
 *   --padscript <file> scripted controller input    (player/padscript.rs)
 *   --padaim <file>    stick toward named actors    (player/padscript.rs)
 *   --fixedfps <n>     every frame advances game time by exactly 1/n s
 *                      (rb: timeManager::Update, age/src/data/timemgr.cpp —
 *                      m_UnwarpedSeconds = 1/m_FixedFps), however slowly the
 *                      frame actually renders
 *   --nohud / --noui   hide the in-game UI (all root UI nodes)
 */
pub mod cinecam;

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::system::SystemParam;
use bevy::light::SimulationLightSystems;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::winit::{UpdateMode, WinitSettings};

use crate::camera::components::CameraController;
use crate::camera::polar;
use crate::menu::AppState;

/// Value following `flag` on the command line.
pub fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

pub fn has_flag(flag: &str) -> bool {
    std::env::args().any(|a| a == flag)
}

/// Creature roots: rb's actor matrix sits at the feet, the Rust creature
/// Transform at its physics capsule's centre (oni2_loader/spawn.rs: a capsule
/// standing on the ground, tagged `CreatureRenderOffset`).  Drop such a
/// transform by the capsule's lower half so it reads like rb's
/// `GetMatrix().d`.  Everything else (vehicles, props, triggers) is already
/// at its layout origin.
fn actor_root(
    g: &GlobalTransform,
    creature: Option<&crate::oni2_loader::spawn::CreatureRenderOffset>,
    collider: Option<&avian3d::prelude::Collider>,
) -> GlobalTransform {
    use avian3d::prelude::SimpleCollider;
    match (creature, collider) {
        (Some(_), Some(c)) => {
            let below = -c.aabb(Vec3::ZERO, Quat::IDENTITY).min.y;
            let (scale, rot, pos) = g.to_scale_rotation_translation();
            GlobalTransform::from(Transform {
                translation: pos - Vec3::Y * below,
                rotation: rot,
                scale,
            })
        }
        _ => *g,
    }
}

type ActorData = (
    Entity,
    &'static Name,
    &'static GlobalTransform,
    Option<&'static ChildOf>,
    Option<&'static crate::oni2_loader::spawn::CreatureRenderOffset>,
    Option<&'static avian3d::prelude::Collider>,
);

/// Finds the player and named layout actors (`actor_SzBrawler1`, ...), the
/// Rust stand-in for `rbGuid(name)` / `rbGuid::GetPlayer()`, as rb-style
/// root transforms (see `actor_root`).  Never the camera, so it can sit
/// beside a camera `&mut Transform` query.
#[derive(SystemParam)]
pub struct ActorLookup<'w, 's> {
    players: Query<
        'w,
        's,
        (
            &'static GlobalTransform,
            Option<&'static crate::oni2_loader::spawn::CreatureRenderOffset>,
            Option<&'static avian3d::prelude::Collider>,
        ),
        (
            With<crate::player::components::Player>,
            Without<CameraController>,
        ),
    >,
    named: Query<'w, 's, ActorData, Without<CameraController>>,
    cache: Local<'s, RefCell<HashMap<String, Entity>>>,
}

impl ActorLookup<'_, '_> {
    pub fn player(&self) -> Option<GlobalTransform> {
        let (g, cr, co) = self.players.iter().next()?;
        Some(actor_root(g, cr, co))
    }

    pub fn named(&self, name: &str) -> Option<GlobalTransform> {
        if let Some(&e) = self.cache.borrow().get(name)
            && let Ok((_, _, g, _, cr, co)) = self.named.get(e)
        {
            return Some(actor_root(g, cr, co));
        }
        // Prefer an exact, root-level match (bone/collider children are
        // named "<actor>::<part>", so they never collide).
        let mut best: Option<(Entity, GlobalTransform, u8)> = None;
        for (e, n, g, parent, cr, co) in &self.named {
            let rank = match (n.as_str() == name, n.as_str().eq_ignore_ascii_case(name)) {
                (true, _) => 2,
                (false, true) => 1,
                _ => continue,
            } * 2
                + u8::from(parent.is_none());
            if best.is_none_or(|(_, _, r)| rank > r) {
                best = Some((e, actor_root(g, cr, co), rank));
            }
        }
        let (e, g, _) = best?;
        self.cache.borrow_mut().insert(name.to_string(), e);
        Some(g)
    }
}

pub struct SizzlePlugin;

impl Plugin for SizzlePlugin {
    fn build(&self, app: &mut App) {
        if let Some(fps) = arg_value("--fixedfps").and_then(|v| v.parse::<u32>().ok())
            && fps > 0
        {
            info!("[fixedfps] every frame advances game time by 1/{fps} s");
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / fps as f64,
            )))
            // An unattended capture keeps running at full speed in the
            // background (the default throttles unfocused windows to 2 Hz).
            .insert_resource(WinitSettings {
                focused_mode: UpdateMode::Continuous,
                unfocused_mode: UpdateMode::Continuous,
            });
        }

        if has_flag("--nohud") || has_flag("--noui") {
            app.add_systems(Update, hide_ui.run_if(in_state(AppState::InGame)));
        }

        if let Some(cine) = arg_value("--cinecam").and_then(|p| cinecam::CineCam::load(&p)) {
            app.insert_resource(cine)
                .add_systems(
                    // After PreUpdate, where the prepass takes the camera's
                    // pose as motion blur's "previous view" (that must be
                    // last frame's cine view, not the game pose), and before
                    // FixedUpdate / Update gameplay.
                    RunFixedMainLoop,
                    cinecam::cine_restore_game_view
                        .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
                )
                .add_systems(
                    PostUpdate,
                    (
                        cinecam::cine_note_game_shake
                            .after(crate::camera::systems::update_camera_channel)
                            .before(polar::apply_camera_transform),
                        cinecam::cine_apply
                            .after(polar::apply_camera_transform)
                            .before(VisibilitySystems::UpdateFrusta)
                            .before(SimulationLightSystems::UpdateDirectionalLightCascades)
                            .run_if(in_state(AppState::InGame)),
                    ),
                );
        }
    }
}

/// `--nohud`: keep every root UI node hidden (rb: -noui -nohud).
fn hide_ui(mut nodes: Query<&mut Visibility, (With<Node>, Without<ChildOf>)>) {
    for mut v in &mut nodes {
        if *v != Visibility::Hidden {
            *v = Visibility::Hidden;
        }
    }
}
