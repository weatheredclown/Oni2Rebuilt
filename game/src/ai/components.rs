/*
 * ai/components.rs — AI component types.
 *
 * `AiFighter`: per-entity marker for AI-controlled creatures.  Carries the
 *   current combat `target` (written by scripts via `SetAiTarget`/`TriggerFight`,
 *   read by the fight FSM) and `manual_target` to lock out auto-acquisition.
 *   Mirrors `aiFighter` in rb/src/aifight/fighter.h — the data surface (target,
 *   mode) lives here; behavior lives in the FSM runtimes (`AiFsmRuntime`,
 *   `FightRuntime`, `AttackRuntime`).
 *
 * `ActorFollower`, `ActorRetreating`: navigation glue used by ScrOni
 *   `FollowActor` and retreat-steering systems — independent of the fight FSM.
 */
use bevy::prelude::*;

#[derive(Component)]
pub struct ActorFollower {
    pub target: Entity,
    pub within: f32, // Dist to stop following
}

#[derive(Component)]
pub struct ActorRetreating {
    pub avoid_target: Option<Entity>,
}

#[derive(Component, Default)]
pub struct AiFighter {
    pub target: Option<Entity>,
    pub manual_target: bool,
}
