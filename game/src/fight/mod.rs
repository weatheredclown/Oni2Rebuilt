/*
 * fight/mod.rs — FightPlugin: high-fidelity crFighter system.
 *
 * Registers all fight components, events, and FixedUpdate systems.
 * Systems are ordered relative to the CombatPlugin pipeline:
 *   react_data_apply  → before hit_reaction (combat)
 *   fighter_state_update → after all combat systems
 *   All other fight systems run in sequence after that.
 */
pub mod ai_attack;
pub mod components;
pub mod events;
pub mod fx_table;
pub mod systems;

use bevy::prelude::*;

use crate::combat::components::{AttackClass, AttackStrength, AttackTarget};
use crate::combat::systems::{hit_detection_system, hit_reaction_system, injure_system};
use crate::fight::fx_table::{AttackFxTable, FxSet};
use crate::menu::AppState;
use crate::oni2_loader::parsers::effect::{EffectDef, StrikeFxDef};
use crate::oni2_loader::parsers::fxl;
use crate::oni2_loader::parsers::texture::load_texture;
use crate::oni2_loader::registries::{FxLibrary, load_global_registries};
use crate::vfs;

pub use ai_attack::AiAttackTableCache;
pub use components::FighterType;
pub use events::{
    ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent, GrappleEndEvent,
    GrappleStartEvent, SuperMeterAddEvent,
};
pub use fx_table::AttackFxRegistry;

pub struct FightPlugin;

impl Plugin for FightPlugin {
    fn build(&self, app: &mut App) {
        // Seed the FX registry with a minimal default table so hits produce
        // something visible/audible until per-fighter-type FX are loaded.
        let mut fx_registry = AttackFxRegistry::default();
        fx_table::register_default(&mut fx_registry);
        app.insert_resource(fx_registry);

        // AI attack table cache — holds one derived roster per fighter-type
        // name, populated lazily by build_ai_attack_tables_system.
        app.init_resource::<AiAttackTableCache>();

        // Populate FxLibrary + AttackFxRegistry from the `.fxl` files
        // shipped under `fx/`.  Must run AFTER load_global_registries so
        // FxLibrary is otherwise initialised (the `Settings/rb.fx` pass
        // doesn't touch our keys).  The hardcoded backstop runs first as
        // a fallback for the `_default_impact_strike` name registered in
        // `register_default`.
        app.add_systems(
            Startup,
            (
                setup_default_impact_strike.after(load_global_registries),
                load_impact_fxl.after(setup_default_impact_strike),
            ),
        );

        app
            // --- Messages ---
            .add_message::<GrappleStartEvent>()
            .add_message::<GrappleEndEvent>()
            .add_message::<BlockSuccessEvent>()
            .add_message::<BlockFailedEvent>()
            .add_message::<ApplyRotationNotchesEvent>()
            .add_message::<SuperMeterAddEvent>()
            // --- FixedUpdate systems ---
            .add_systems(
                FixedUpdate,
                (
                    // react_data_apply runs BEFORE hit_reaction_system so FighterState
                    // fields are populated before the animation starts.
                    systems::react_data_apply_system.before(hit_reaction_system),
                    // Block state-sync runs BEFORE hit_detection so cur_block /
                    // block_status / rate / hold-loop are current when the
                    // strike test inspects them. Also applies the hold-loop
                    // mid-point clamp every tick a block anim is playing.
                    systems::block_state_sync_system.before(hit_detection_system),
                    // Block success/failed responses run after hit_detection but before injure.
                    systems::block_success_system.after(injure_system),
                    systems::block_failed_system.after(injure_system),
                    // Grapple management runs each tick.
                    systems::grapple_tick_system,
                    systems::grapple_end_system,
                    // Rotation notches are applied after all state mutations are done.
                    systems::rotation_notches_system,
                    // React-data-driven post-reaction housekeeping.
                    systems::react_end_rotation_system,
                    // (knockdown_getup_system removed — getup is now
                    // queued as the second entry of the React action's
                    // AnimSchedule by action_start_system; the schedule
                    // tick auto-advances knockdown→getup atomically with
                    // the FSM's REACT state, mirroring legacy
                    // ActionStartReact's two-batch react+getup queue.)
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                FixedUpdate,
                (
                    // Per-frame timer bookkeeping (decay, flag transitions).
                    systems::fighter_state_update_system,
                    // Super meter.
                    systems::super_meter_system,
                    // Successive-attacks / combo escalation.
                    systems::successive_attacks_system,
                    // Attack spin during active frames.
                    systems::attack_spin_system,
                    // Hit ETA prediction (AboutToBeHit).
                    systems::hit_eta_system,
                    // Fight stance timer reset.
                    systems::fight_stance_timer_system,
                    // Track heading during attack for locked fighting targets
                    systems::update_fighter_strike_facing_system,
                    // Interpolate Fighter.facing toward
                    // FighterState.turn_final_target_dir each frame
                    // (crFighter::TurnLerper).
                    systems::fighter_turn_lerp_system,
                    // Push the defender across the react anim by their
                    // stashed react_distance.
                    systems::react_distance_apply_system,
                    // Face-after-run snap-toward-nearest-foe behaviour.
                    systems::face_after_run_system,
                    // Drop weapon when a grab-attack disarm crosses its
                    // configured anim phase.
                    systems::disarm_apply_system,
                    // AI disarm-intent decision.
                    systems::update_disarming_system,
                    // Fight stance entry/exit orchestration.  `sync` must
                    // run first so FighterState.FIGHT_MODE reflects the
                    // animator's FIGHTSTANCE flag before entry/exit read it.
                    systems::fight_stance_sync_system,
                    systems::fight_stance_entry_system.after(systems::fight_stance_sync_system),
                    systems::fight_stance_exit_system.after(systems::fight_stance_sync_system),
                    // Build AiAttackTable for any FighterType that doesn't
                    // have one yet.  One-shot per entity: the Without<...>
                    // filter means this is a no-op once every fighter has
                    // its table attached.
                    ai_attack::build_ai_attack_tables_system,
                )
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// Build a hardcoded `EffectDef::Strike` matching the `Punches_Body_Hard`
/// block from `fx/attack_impact.fxl` and insert it into `FxLibrary` under
/// `fx_table::DEFAULT_IMPACT_STRIKE_NAME`.  This is a stopgap until the
/// .fxl loader is wired up; once it is, this entry can be removed and the
/// fallback in `register_default` repointed at a real per-attack name.
fn setup_default_impact_strike(mut fx_lib: ResMut<FxLibrary>, mut images: ResMut<Assets<Image>>) {
    let key = fx_table::DEFAULT_IMPACT_STRIKE_NAME.to_lowercase();
    if fx_lib.effects.contains_key(&key) {
        return;
    }

    let texture_handle = load_texture("entity/strikefx", "bam", &mut images).map(|(h, _)| h);

    let def = StrikeFxDef {
        name: fx_table::DEFAULT_IMPACT_STRIKE_NAME.to_string(),
        exponent: 1.0,
        jumble: 0.0,
        spike_scale: 0.4,
        fade_start: 0.99,
        wave_count: 10,
        color1a: Color::srgb(1.0, 0.0, 0.0),
        color2a: Color::srgb(1.0, 0.15, 0.05),
        color3a: Color::srgb(1.0, 0.7, 0.2),
        color1b: Color::srgb(0.3, 0.7, 1.0),
        color2b: Color::srgb(0.1, 1.0, 0.5),
        color3b: Color::srgb(0.0, 0.3, 1.0),
        tile: Vec2::new(1.0, 1.0),
        slide_rate: Vec2::new(0.2, -1.25),
        scale_rate: 3.0,
        fade_duration: 0.2,
        duration: 0.35,
        position: Vec3::new(0.0, 1.5, 0.0),
        texture_name: "bam".to_string(),
        texture_handle,
    };

    info!(
        "fight: registered default impact strike '{}' (texture loaded: {})",
        def.name,
        def.texture_handle.is_some()
    );
    fx_lib.effects.insert(key, EffectDef::Strike(def));
}

/// Parse every shipped `.fxl` under `fx/` and turn each `FX_LIST` into a
/// matching pair: an `EffectDef::Strike` entry in `FxLibrary` (so
/// `handle_spawn_fx` can render it) and an `FxSet` in `AttackFxRegistry`
/// (so `combat::hit` dispatch can find it via (target, strength, class)).
///
/// Mapping rules for the file names ship in the asset pack:
/// `Punches_Body_Hard` → (Body, High, Punch), `Kicks_Face_Hard` →
/// (Head, High, Kick), etc.  Entries whose names don't match the impact
/// schema (e.g. `Exert_Punch_Hard_Konoko`, attack-windup grunts) get
/// registered as named FxSets but skipped from table-cell wiring; they
/// can be looked up by name later via `AttackFxRegistry::get_set`.
fn load_impact_fxl(
    mut fx_lib: ResMut<FxLibrary>,
    mut fx_registry: ResMut<AttackFxRegistry>,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
) {
    let files: &[(&str, &str)] = &[
        ("fx", "attack_impact.fxl"),
        ("fx", "block_impact.fxl"),
        ("fx", "attack_begin.fxl"),
    ];

    let mut all_entries = Vec::new();
    for (dir, file) in files {
        match vfs::read_to_string(dir, file) {
            Ok(content) => {
                let parsed = fxl::parse_fxl_content(&content, &asset_server, &mut images);
                info!(
                    "fight: loaded {} FX_LIST entries from {}/{}",
                    parsed.len(),
                    dir,
                    file
                );
                all_entries.extend(parsed);
            }
            Err(_) => warn!("fight: could not read {}/{}", dir, file),
        }
    }

    // Preserve the existing default-table fallback (the `_default_impact_strike`
    // FxSet wired up by `register_default`) so combos with no explicit cell
    // still render *something*.
    let prior_fallback = fx_registry
        .tables
        .get("default")
        .and_then(|t| t.fallback.clone());

    let mut default_table = AttackFxTable::new("default");
    default_table.fallback = prior_fallback;

    let mut wired_cells = 0usize;

    for entry in all_entries {
        // 1. STRIKE → FxLibrary (keyed by FX_LIST name, lowercased).
        if let Some(strike) = &entry.strike {
            // Clone-replace the def so the FxLibrary key matches the FxSet's
            // `impact_fx` name exactly (the StrikeFxDef inside was built by
            // `parse_effect` with `name = entry.name`, which is already what
            // we want; we just need it under the right map key).
            fx_lib
                .effects
                .insert(entry.name.to_lowercase(), EffectDef::Strike(strike.clone()));
        }

        // 2. Build & register the FxSet bundle.
        let mut fxset = FxSet::new(&entry.name);
        if entry.strike.is_some() {
            fxset = fxset.with_fx(&entry.name);
        }
        if let Some(audio) = &entry.sfx_audio_package {
            fxset = fxset.with_sound(audio);
        }
        let arc = fx_registry.register_set(fxset);

        // 3. Wire into the default table if the name parses as an impact combo.
        if let Some((target, strength, class)) = parse_attack_combo(&entry.name) {
            default_table.set(target, strength, class, arc);
            wired_cells += 1;
        }
    }

    info!(
        "fight: wired {} (target, strength, class) cells in default AttackFxTable",
        wired_cells
    );
    fx_registry.register_table(default_table);
}

/// Decode names like `Punches_Body_Hard` → `(Body, High, Punch)`.  Returns
/// `None` for entries that aren't impact bundles (e.g. attack-windup
/// vocalisations, block reactions) so they're skipped from table wiring.
fn parse_attack_combo(name: &str) -> Option<(AttackTarget, AttackStrength, AttackClass)> {
    let lower = name.to_lowercase();
    let class = if lower.starts_with("punches_") {
        AttackClass::Punch
    } else if lower.starts_with("kicks_") {
        AttackClass::Kick
    } else {
        return None;
    };
    let target = if lower.contains("_face_") {
        AttackTarget::Head
    } else if lower.contains("_body_") {
        AttackTarget::Body
    } else if lower.contains("_legs_") {
        AttackTarget::Legs
    } else {
        return None;
    };
    let strength = if lower.ends_with("_hard") || lower.ends_with("_super") {
        // No dedicated "Super" data in the shipped asset pack; treat
        // explicit `_super` suffixes as Super, default `_hard` to High.
        if lower.ends_with("_super") {
            AttackStrength::Super
        } else {
            AttackStrength::High
        }
    } else if lower.ends_with("_light") || lower.ends_with("_soft") || lower.ends_with("_med") {
        AttackStrength::Low
    } else {
        return None;
    };
    Some((target, strength, class))
}
