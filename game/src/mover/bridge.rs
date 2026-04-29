/*
 * mover/bridge.rs — per-tick translator from `LinearVelocity` (legacy
 * player/AI output) to Tnua's `TnuaBuiltinWalk` basis.
 *
 * Why route through `LinearVelocity` instead of changing player/AI to write a
 * new `IntendedVelocity` component: minimum-friction A/B. `player::systems`
 * and `ai::navigation` keep writing `vel.x` / `vel.z` exactly as today; this
 * system reads those values, builds the walk basis, and clears them so Avian
 * doesn't double-integrate (Tnua will own velocity once its forces apply).
 *
 * Y is left untouched — Tnua's float spring manages vertical position.
 */
use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use bevy_tnua::builtins::{TnuaBuiltinJump, TnuaBuiltinWalk};
use bevy_tnua::prelude::*;

use super::components::Oni2Scheme;

/// Reads `LinearVelocity` (set this frame by player/AI), packs xz into a
/// `TnuaBuiltinWalk` basis, then zeros vel.x/vel.z so Avian's solver doesn't
/// also pull the body laterally before Tnua's forces apply.
///
/// `TnuaBuiltinWalk` only carries per-frame intent (`desired_motion` and
/// `desired_forward`). Tuning like `float_height` / `max_slope` lives on the
/// `Oni2SchemeConfig` asset and is read by Tnua independently.
pub fn tnua_basis_from_linvel(
    mut query: Query<(
        &mut TnuaController<Oni2Scheme>,
        &mut LinearVelocity,
        Option<&Transform>,
    )>,
) {
    for (mut controller, mut velocity, transform_opt) in &mut query {
        controller.initiate_action_feeding();

        let desired_motion = Vec3::new(velocity.x, 0.0, velocity.z);

        controller.basis = TnuaBuiltinWalk {
            desired_motion,
            desired_forward: transform_opt
                .and_then(|tf| Dir3::new(tf.forward().as_vec3()).ok()),
        };

        // Hand off all velocity ownership to Tnua. The float spring + any
        // active action (jump, etc.) drive vertical motion; leftover y from
        // upstream writes (e.g. legacy `velocity.y = JUMP_IMPULSE`) would
        // fight the spring.
        velocity.x = 0.0;
        velocity.y = 0.0;
        velocity.z = 0.0;
    }
}

/// Reads `JumpImpulseMessage` (emitted by `player_movement_system` placeholder
/// branch and by the animator's `jump_impulse_emit_system` for data-driven
/// jumps) and converts it into a `TnuaBuiltinJump` action on the matching
/// entity. Counterpart of `animator::systems::jump_impulse_apply_system`,
/// which writes `LinearVelocity.y` for the Dynamic backend; both subscribe
/// to the same message but only one is active per backend.
///
/// Tnua 0.29 split jump tuning: `height` lives on `Oni2SchemeConfig.jump`
/// (consumed by Tnua via the config asset), while `TnuaBuiltinJump` itself
/// only carries per-invocation switches. To get variable jump heights per
/// message, the config asset would need to be updated in place before
/// firing — deferred to Phase 2.
pub fn tnua_jump_from_message(
    mut reader: MessageReader<crate::animator::JumpImpulseMessage>,
    mut controllers: Query<&mut TnuaController<Oni2Scheme>>,
) {
    for msg in reader.read() {
        let Ok(mut controller) = controllers.get_mut(msg.entity) else {
            continue;
        };
        controller.action(Oni2Scheme::Jump(TnuaBuiltinJump::default()));
    }
}
