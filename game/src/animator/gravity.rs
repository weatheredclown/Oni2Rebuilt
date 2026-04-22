/*
 * animator/gravity.rs — layered gravity modifiers + Avian sync system.
 *
 * Replaces the "impulse sets GravityScale, separate reset system clears it"
 * pattern that suffered from:
 *   • ordering fights between set and reset systems
 *   • ShapeCaster briefly reporting ground contact post-impulse triggering
 *     a spurious reset that clobbered the jump's gravity factor
 *   • multiple override sources (jump, cutscene freeze, slow-time) couldn't
 *     coexist — whichever reset last won
 *
 * New model:
 *   • Each entity that can have gravity overridden carries a
 *     `GravityModifiers(LayeredValue<f32>)` component, base `1.0`.
 *   • Contributors (JumpAction, cutscene scripts, etc.) `push("owner_key",
 *     factor)` when their effect starts and `remove("owner_key")` when
 *     their effect ends.  No one "resets" — removal is self-cleanup.
 *   • `gravity_sync_system` each tick reads the resolved `.current()`
 *     value and writes it into Avian's `GravityScale` component ONLY
 *     when it differs from the last-written value.
 *
 * Ordering hazards eliminated: since pushes and removes are explicit
 * operations on the stack, there's no window where the reset can fire
 * during an active override.  The velocity-gate hack is gone.
 */
use avian3d::prelude::*;
use bevy::prelude::*;

use crate::common::LayeredValue;

/// Per-entity stack of gravity overrides.  Base `1.0` == normal Avian
/// gravity.  Contributors push layers keyed by owner string.  A sync
/// system writes the resolved value into Avian's `GravityScale`.
///
/// Owner keys in use (keep this list canonical to avoid collisions):
///   • `"jump"`  — `JumpAction` while the jump arc is active
///
/// Additions welcome; just pick a key no one else uses.
#[derive(Component, Clone, Debug)]
pub struct GravityModifiers(pub LayeredValue<f32>);

impl Default for GravityModifiers {
    fn default() -> Self {
        GravityModifiers(LayeredValue::new(1.0))
    }
}

impl GravityModifiers {
    pub fn push(&mut self, key: impl Into<String>, factor: f32) {
        self.0.push(key, factor);
    }

    pub fn remove(&mut self, key: &str) -> Option<f32> {
        self.0.remove(key)
    }

    pub fn current(&self) -> f32 {
        *self.0.current()
    }

    pub fn has_layer(&self, key: &str) -> bool {
        self.0.has_layer(key)
    }
}

// ---------------------------------------------------------------------------
// gravity_sync_system
// ---------------------------------------------------------------------------

/// Read every `GravityModifiers` component and write its resolved value
/// into Avian's `GravityScale` when it changes.  Runs each tick in
/// FixedUpdate.
///
/// We only `insert` the new `GravityScale` when the value actually
/// differs from the current component (epsilon-compared to dodge
/// float-noise) so commands-flush cost stays near zero on the common
/// "nothing changed" path.
pub fn gravity_sync_system(
    mut commands: Commands,
    query: Query<(Entity, &GravityModifiers, &GravityScale)>,
) {
    for (entity, modifiers, current_scale) in &query {
        let target = modifiers.current();
        if (current_scale.0 - target).abs() > 0.001 {
            let layer_summary = if modifiers.0.layer_count() == 0 {
                String::from("[base only]")
            } else {
                // Dump every active layer's key for this sync — helps
                // diagnose "who pushed this?" bugs without needing to
                // grep through every push call site.
                let keys: Vec<&str> = modifiers.0.layers_iter().map(|(k, _)| k.as_str()).collect();
                format!("[{}]", keys.join(", "))
            };
            info!(
                "GRAVITY SYNC: entity {:?} {:.2} → {:.2} (base={:.2}, active layers: {} {})",
                entity,
                current_scale.0,
                target,
                modifiers.0.base(),
                modifiers.0.layer_count(),
                layer_summary,
            );
            commands.entity(entity).insert(GravityScale(target));
        }
    }
}

/// Attach `GravityModifiers::default()` to any entity that has a
/// `GravityScale` but no modifiers component yet.  One-shot per entity
/// via the `Without` filter.
///
/// Safety net, not the canonical path.  `AnimatorBundle` in
/// `animator/mod.rs` is the one-stop spawn for every animator-driven
/// entity and already includes `GravityModifiers`.  This system
/// catches stragglers — e.g. physics-only entities that gain a
/// `GravityScale` at runtime without going through `AnimatorBundle` —
/// so gravity-override pushes (jump, cutscene freeze, slow-time) have
/// somewhere to stack.  Prefer `AnimatorBundle` at spawn when you can,
/// since `commands.insert` here flushes at end-of-stage and the first
/// `JumpAction::on_enter` after the add would still see
/// `ctx.gravity == None`.
pub fn ensure_gravity_modifiers_component(
    mut commands: Commands,
    q: Query<Entity, (With<GravityScale>, Without<GravityModifiers>)>,
) {
    for entity in &q {
        commands.entity(entity).insert(GravityModifiers::default());
    }
}
