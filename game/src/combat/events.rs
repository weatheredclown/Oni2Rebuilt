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

use super::components::{AttackClass, AttackStrength, ReactionKind};

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
