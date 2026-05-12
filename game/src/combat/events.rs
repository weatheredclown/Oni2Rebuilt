/*
 * combat/events.rs — Bevy Message types for combat communication.
 *
 * AttackMessage: attacker entity + attack class/strength.
 * DamageMessage: source, target, amount, reaction kind.
 * DeathMessage: entity that died.
 * AboutToBeHitMessage: early-warning for dodge/reaction systems.
 * HitReactionMessage: triggers animation FSM transition on the receiver.
 */
use bevy::prelude::*;

use super::components::{AttackClass, AttackStrength, AttackTarget, ReactionKind};

#[derive(Message)]
pub struct AttackMessage {
    pub attacker: Entity,
    pub class: AttackClass,
    pub strength: AttackStrength,
}

#[derive(Message)]
pub struct DamageMessage {
    pub attacker: Entity,
    pub target: Entity,
    pub damage: f32,
    pub was_blocked: bool,
    pub attack_class: AttackClass,
    pub attack_strength: AttackStrength,
}

#[derive(Message)]
pub struct DeathMessage {
    pub entity: Entity,
    pub killer: Entity,
}

#[derive(Message)]
pub struct AboutToBeHitMessage {
    pub target: Entity,
    pub eta: f32,
    pub hit_type: u8,
    pub from: Vec3,
    pub attacker: Entity,
}

#[derive(Message)]
pub struct HitReactionMessage {
    pub entity: Entity,
    pub kind: ReactionKind,
    pub direction: Vec3,
    /// animReactEnum integer from the attacker's ATDT reactanim0 field.
    /// Indexes into ANIMREACT_NAMES to pick the animation to play.
    pub react_enum: i32,
}

#[derive(Message, Clone)]
pub struct InjureMessage {
    pub target: Entity,
    pub attacker: Option<Entity>,
    pub damage: f32,
    pub hit_type: String,
    pub from: Option<Vec3>,
    pub play_react: bool,
    pub disable_creature_detect: bool,
    // Combat classification sourced from the attacker's ATDT file when
    // declared (authoritative).  All three are `None` for non-ATDT hits
    // (environmental damage, scripted injuries, etc.) — downstream FX
    // lookup skips in that case rather than picking a wrong default.
    pub attack_class: Option<AttackClass>,
    pub attack_strength: Option<AttackStrength>,
    pub attack_target: Option<AttackTarget>,
    // For handling animations resolving HitReaction logic
    pub strike_react_enum: Option<i32>,

    /// World-space displacement vector to push the defender across the
    /// react animation.  Already computed for the right react slot and
    /// translate mode (PUSH/SET) at hit time, so the downstream
    /// `react_distance_apply_system` only needs to divide by the react
    /// anim duration to get a constant per-frame velocity.  `None` for
    /// non-ATDT hits (env hazards, scripted injuries).  Mirrors the
    /// `translateDist` Vec3 built at rb/src/fight/strike.cpp:476-507 and
    /// stored via `SetReactDistance` (strike.cpp:513).
    pub react_distance: Option<Vec3>,
    /// True when the defender should snap-face the attacker as part of
    /// the hit reaction.  Mirrors `crStrikeReact::FaceWithReact`
    /// (rb/src/fight/attackdata.cpp:996) — default `false`; the legacy
    /// `sMakeTargetFace` (strike.cpp:235) only runs when this is set.
    pub face_with_react: bool,
    /// One-shot teleport destination for the defender when the strike's
    /// translate mode is `REACT_TRANSLATE_MODE_TELEPORT`.  When `Some`,
    /// the injure system snaps Transform.translation to this XZ position
    /// before the react plays.  Mirrors the `aMsgTeleport` broadcast at
    /// rb/src/fight/strike.cpp:500-502.
    pub teleport_to: Option<Vec3>,
}

#[derive(Message, Clone)]
pub struct StrikeConnectedEvent {
    pub attacker: Entity,
    pub target: Entity,
    pub headingnotlockedtotarget: bool,
}
