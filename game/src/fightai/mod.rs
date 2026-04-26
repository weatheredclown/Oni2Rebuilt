/*
 * fightai/mod.rs — FightAI components and FSM caches.
 *
 * This module manages the state machines responsible for coordinating AI combat.
 * It provides cached, reusable parsings of `fight.fsm` (global fight coordination)
 * and `*.atk` (per-character attack routines) scripts, and attaches their runtime
 * contexts onto AI actors spawned with a FightAI component.
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use crate::statemachine::core::SmData;
use crate::statemachine::drivers::atk_parser::parse_atk;
use crate::statemachine::drivers::attack::AttackDriver;
use crate::statemachine::drivers::fight::{FIGHT_ACTION_PARSER, FIGHT_EVENT_PARSER, FightDriver};
use crate::statemachine::drivers::parse::parse_sm;
use crate::statemachine::drivers::squad::{SQUAD_ACTION_PARSER, SQUAD_EVENT_PARSER, SquadDriver};

pub mod components;

// ---------------------------------------------------------------------------
// Caches
// ---------------------------------------------------------------------------

/// Caches the singleton `fight.fsm` state machine data.
#[derive(Resource, Default)]
pub struct FightFsmCache {
    pub data: Option<Arc<SmData<FightDriver>>>,
}

impl FightFsmCache {
    /// Synchronously load `fight.fsm` from disk into the cache if not already loaded.
    pub fn get_or_load(&mut self, asset_base: &str) -> Option<Arc<SmData<FightDriver>>> {
        if let Some(data) = &self.data {
            return Some(Arc::clone(data));
        }

        let path = "statemachine";
        let filename = "fight.fsm";

        // Use vfs instead of direct filesystem access since files could be packed
        match crate::vfs::read_to_string(path, filename) {
            Ok(content) => {
                match parse_sm::<FightDriver>(&content, FIGHT_EVENT_PARSER, FIGHT_ACTION_PARSER) {
                    Ok(sm_data) => {
                        bevy::log::info!("fightai: Successfully loaded {}", filename);
                        let arc = Arc::new(sm_data);
                        self.data = Some(Arc::clone(&arc));
                        Some(arc)
                    }
                    Err(e) => {
                        bevy::log::error!("fightai: Failed to parse {}: {}", filename, e);
                        None
                    }
                }
            }
            Err(e) => {
                // To help debug missing files, try fallback direct reads around the workspace
                bevy::log::warn!(
                    "fightai: Failed to load {} from VFS (trying fallback... {})",
                    filename,
                    e
                );
                // Try reading directly from assets dir just in case
                let fb_path = format!("{}/statemachine/{}", asset_base, filename);
                match std::fs::read_to_string(&fb_path) {
                    Ok(content) => {
                        match parse_sm::<FightDriver>(
                            &content,
                            FIGHT_EVENT_PARSER,
                            FIGHT_ACTION_PARSER,
                        ) {
                            Ok(sm_data) => {
                                bevy::log::info!(
                                    "fightai: Successfully loaded {} (fallback)",
                                    filename
                                );
                                let arc = Arc::new(sm_data);
                                self.data = Some(Arc::clone(&arc));
                                Some(arc)
                            }
                            Err(e) => {
                                bevy::log::error!(
                                    "fightai: Failed to parse fallback {}: {}",
                                    filename,
                                    e
                                );
                                None
                            }
                        }
                    }
                    Err(e2) => {
                        bevy::log::error!("fightai: Failed to read FSM file {}: {}", filename, e2);
                        None
                    }
                }
            }
        }
    }
}

/// Caches the singleton `squad.fsm` state machine data.
///
/// Mirrors the legacy `aiSquadStateMachineData` singleton owned by
/// `aiFightManager` (rb/src/aifight/squad.cpp:783).  The driver/parser vocabulary
/// is currently a stub — loading succeeds but the parsed machine is effectively
/// empty until the Format-2 nested-brace parser + squad coordinator land.
#[derive(Resource, Default)]
pub struct SquadFsmCache {
    pub data: Option<Arc<SmData<SquadDriver>>>,
}

impl SquadFsmCache {
    /// Synchronously load `squad.fsm` from the VFS into the cache if not already loaded.
    pub fn get_or_load(&mut self) -> Option<Arc<SmData<SquadDriver>>> {
        if let Some(data) = &self.data {
            return Some(Arc::clone(data));
        }

        let filename = "squad.fsm";
        match crate::vfs::read_to_string("statemachine", filename) {
            Ok(content) => {
                match parse_sm::<SquadDriver>(&content, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER) {
                    Ok(sm_data) => {
                        bevy::log::info!("fightai: Successfully loaded {}", filename);
                        let arc = Arc::new(sm_data);
                        self.data = Some(Arc::clone(&arc));
                        Some(arc)
                    }
                    Err(e) => {
                        bevy::log::error!("fightai: Failed to parse {}: {}", filename, e);
                        None
                    }
                }
            }
            Err(e) => {
                bevy::log::warn!("fightai: Failed to load {} from VFS: {}", filename, e);
                None
            }
        }
    }
}

/// Caches parsed `.atk` state machines by filename.
#[derive(Resource, Default)]
pub struct AttackFsmCache {
    pub by_name: HashMap<String, Arc<SmData<AttackDriver>>>,
}

impl AttackFsmCache {
    /// Synchronously load a `.atk` file into the cache if not already loaded.
    pub fn get_or_load(
        &mut self,
        table_name: &str,
        asset_base: &str,
    ) -> Option<Arc<SmData<AttackDriver>>> {
        let filename = format!("{}.atk", table_name);

        if let Some(data) = self.by_name.get(&filename) {
            return Some(Arc::clone(data));
        }

        let path = "statemachine";

        let content = match crate::vfs::read_to_string(path, &filename) {
            Ok(c) => c,
            Err(_) => {
                // Fallback directory search
                let fb_path = format!("{}/statemachine/{}", asset_base, filename);
                match std::fs::read_to_string(&fb_path) {
                    Ok(c) => c,
                    Err(e) => {
                        bevy::log::error!("fightai: Failed to read {} from disk: {}", filename, e);
                        return None;
                    }
                }
            }
        };

        match parse_atk(&content) {
            Ok(sm_data) => {
                bevy::log::info!(
                    "fightai: Successfully loaded {} ({} states)",
                    filename,
                    sm_data.states.len()
                );
                let arc = Arc::new(sm_data);
                self.by_name.insert(filename.clone(), Arc::clone(&arc));
                Some(arc)
            }
            Err(e) => {
                bevy::log::error!("fightai: Failed to parse {}: {}", filename, e);
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin Configuration
// ---------------------------------------------------------------------------

pub struct FightAiPlugin;

impl Plugin for FightAiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FightFsmCache>()
            .init_resource::<AttackFsmCache>()
            .init_resource::<SquadFsmCache>()
            .add_systems(Startup, load_fight_squad_singletons)
            .add_systems(
                FixedUpdate,
                (
                    // Fight coordinator ticker must run BEFORE attack
                    // so any `AtkAttack`/`AtkIdle` requests translate
                    // into the next AttackRuntime tick's decisions.
                    // (Today it's a no-op because the coordinator
                    // isn't ported — see the comment on
                    // `fight_runtime_update_system`.)
                    fight_runtime_update_system,
                    attack_runtime_update_system.after(fight_runtime_update_system),
                )
                    .run_if(in_state(crate::menu::AppState::InGame)),
            );
    }
}

/// Advance every actor's `AttackRuntime` once per FixedUpdate.  Mirrors the
/// legacy `aiAttackStateMachine::Update` (rb/src/aifight/
/// attackstatemachine.cpp:318): refresh the ctx with the current anim
/// state (so `ActionAttack::update` can detect anim completion), tick the
/// SM, and drain any `attack_anim`/`block_anim`/etc. output into the
/// shared `do_attack` / `do_block` / … helpers — the same terminal DNA
/// the player's `fsm_update_system` funnels `DoAttack` through.
///
/// `ctx.got_cookie` is managed externally by `FightBehavior` (or, when
/// the fight coordinator lands, by the position/cookie system).  This
/// system only reads it via the FSM's `GotCookie` event.
pub fn attack_runtime_update_system(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &mut crate::fightai::components::AttackRuntime,
            &crate::oni2_loader::animation::Oni2AnimLibrary,
            &mut crate::oni2_loader::animation::Oni2AnimState,
            &mut crate::combat::components::Fighter,
            Option<&crate::ai::components::AiFighter>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, mut runtime, anim_lib, mut anim_state, mut fighter, ai_fighter_opt) in &mut query {
        runtime.fsm.advance_clock(dt);
        runtime.ctx.dt = dt;
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames as i32;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;

        // Split the two mutable borrows — `runtime.fsm.tick(&mut runtime.ctx)`
        // would alias `runtime` to itself.  Go through `as_mut`.
        let rt_mut = runtime.as_mut();
        let mut output = rt_mut.fsm.tick(&mut rt_mut.ctx);

        if let Some(anim_name) = &output.attack_anim {
            bevy::log::info!("AttackRuntime: DoAttack → '{}' on {:?}", anim_name, entity);
            crate::statemachine::runtime::do_attack(
                anim_lib,
                &mut anim_state,
                &mut fighter,
                anim_name,
                0,
            );
        }
        if let Some(anim_name) = &output.block_anim {
            crate::statemachine::runtime::do_block(anim_lib, &mut anim_state, anim_name);
        }
        if let Some((anim_name, mirror)) = &output.evade_anim {
            crate::statemachine::runtime::do_evade(anim_lib, &mut anim_state, anim_name, *mirror);
        }
        if let Some(anim_name) = &output.custom_anim {
            crate::statemachine::runtime::do_custom_anim(anim_lib, &mut anim_state, anim_name);
        }
        if let Some(target_distance) = output.start_following_distance.take() {
            if let Some(ai) = ai_fighter_opt {
                if let Some(target) = ai.target {
                    bevy::log::info!(
                        "AttackRuntime: Dispatching ActorFollower distance {:.2} on {:?}",
                        target_distance,
                        entity
                    );
                    commands
                        .entity(entity)
                        .insert(crate::ai::components::ActorFollower {
                            target,
                            within: target_distance,
                        });
                }
            }
        }
    }
}

/// Tick each actor's `FightRuntime` once per FixedUpdate.  Mirrors the
/// legacy `aiFightStateMachine::Update` loop (rb/src/aifight/
/// fightstatemachine.cpp).
///
/// Inputs wired today (from components that already exist):
///   • `has_target` ← `AiFighter.target.is_some()`
///   • `is_reacting` ← `ActionPlayer::is_reacting()`
///   • `target_killed` ← target's `Health.is_dead()` (checked against
///     the previously-known target)
///   • `attack_finished` ← `AttackRuntime.ctx.attack_finished`
///     (AttackDriver sets it when the inner anim completes)
///   • `attacked` ← `FighterState::in_invuln_phase` as a proxy for
///     "we were just hit this frame" — good enough to gate reaction
///     transitions until a proper hit-this-frame edge lands
///
/// Inputs STILL STUBBED (require the position/cookie/formation
/// coordinator, which isn't ported):
///   • `has_position`, `can_attack`, `position_offered`,
///     `cookie_offered`, `prepare_next_attacker`, `mode` — left at
///     the context's default values.
///
/// Outputs: the FSM writes `FightAction` requests into
/// `FightOutput.requested_actions`.  Most requests (Move/Grab/Release
/// position, Cookie handoff) have no runtime consumer yet — we log
/// them at trace level so you can watch the coordinator "think".
/// `AtkAttack` / `AtkIdle` / `Attack` DO have a real effect: they
/// flip `AttackRuntime.ctx.got_cookie`, which is how the attack FSM
/// decides between cookie vs no-cookie rows on its next tick.  That
/// path at least gives the fight coordinator influence over the
/// per-actor attack pattern even without the full position system.
pub fn fight_runtime_update_system(
    time: Res<Time>,
    mut query: Query<
        (
            Entity,
            &mut crate::fightai::components::FightRuntime,
            Option<&mut crate::fightai::components::AttackRuntime>,
            Option<&crate::ai::components::AiFighter>,
            Option<&crate::animator::components::ActionPlayer>,
            Option<&crate::fight::components::FighterState>,
            Option<&crate::oni2_loader::animation::Oni2AnimState>,
            Option<&mut crate::behavior::BehaviorRuntime>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
    target_health: Query<&crate::combat::components::Health>,
    target_transforms: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    for (
        entity,
        mut runtime,
        mut attack_rt_opt,
        ai_opt,
        action_player_opt,
        fs_opt,
        anim_opt,
        mut behavior_rt_opt,
    ) in &mut query
    {
        runtime.fsm.advance_clock(dt);

        // --- Populate context inputs ---
        runtime.ctx.has_target = ai_opt.and_then(|a| a.target).is_some();
        runtime.ctx.is_reacting = action_player_opt.is_some_and(|ap| ap.is_reacting());
        runtime.ctx.target_killed = ai_opt
            .and_then(|a| a.target)
            .and_then(|t| target_health.get(t).ok())
            .is_some_and(|h| h.current <= 0.0);

        // Set `ctx.mode` based on AI state — NOT the current Behavior
        // state.  The fight FSM's `S_STARTUP` only exits via
        // `E_MODE(...)` events, so without a sensible mode the FSM
        // parks forever and never logs a transition.
        //
        // Mode source precedence:
        //   1. If scroni has already set a mode (via SetAiTarget /
        //      TriggerFight), leave it alone — those commands are
        //      explicit script intent and shouldn't be clobbered.
        //   2. Otherwise, derive from `has_target`: target present →
        //      "fight"; no target → "idle".
        //
        // This used to mirror the BehaviorRuntime's current state,
        // but that created a feedback loop — scroni would set
        // mode="attack", we'd overwrite with "idle" (behavior still
        // in IDLE), fight FSM couldn't progress past S_STARTUP, which
        // kept behavior in IDLE, which kept us overwriting.
        let scroni_set_mode = !runtime.ctx.mode.is_empty()
            && !runtime.ctx.mode.eq_ignore_ascii_case("idle");
        if !scroni_set_mode {
            runtime.ctx.mode = if runtime.ctx.has_target {
                "fight".to_string()
            } else {
                "idle".to_string()
            };
        }

        // Stash the current behavior state name for dedup below.
        let behavior_state_name = behavior_rt_opt
            .as_deref()
            .map(|rt| rt.sm.data.state_name(rt.sm.current_state).to_string());
        // attack_finished: proxy = current attack anim has played to end
        // on a non-looping animation.  AttackRuntime doesn't publish
        // a completion flag yet (the inner AttackDriver knows, but
        // doesn't surface it back to the container).  Anim-end is a
        // close-enough proxy: once an attack animation ends, the FSM
        // is free to pick a next one.
        runtime.ctx.attack_finished = anim_opt.is_some_and(|a| {
            !a.looping
                && a.anim.num_frames > 1
                && a.current_time >= (a.anim.num_frames as f32 - 1.0)
        });
        runtime.ctx.attacked = fs_opt.is_some_and(|fs| fs.in_invuln_phase);

        // --- Tick ---
        let rt_mut = runtime.as_mut();
        let output = rt_mut.fsm.tick(&mut rt_mut.ctx);

        // --- Consume outputs ---
        // The Behavior layer (FightBehavior / GotoBehavior / RetreatBehavior,
        // driven by scroni `fight` / `goto` / `retreat` script commands)
        // is what actually moves the character in the world.  FightDriver
        // runs PARALLEL to that as the cookie/attack-tempo coordinator:
        // its job here is limited to flipping `AttackRuntime.ctx.got_cookie`
        // so the .atk machine picks between its cookie and no-cookie rows.
        //
        // We deliberately do NOT bridge MoveToPosition / RequestPosition /
        // ReleasePosition / Idle into `requested_goto` / `requested_retreat`
        // / `requested_idle`.  The fight FSM's fall-through rules pulse
        // those actions every tick; converting them to behavior-flag
        // pulses either caused strobing (when every tick re-entered the
        // same state) or stalls (when dedup skipped every pulse).  Until
        // a real position/cookie coordinator lands, let the Behavior layer
        // stand on its own and use FightDriver purely for cookie state.
        //
        // Suppressed actions are trace-logged so the fight FSM's ticks
        // remain visible.

        for action in &output.requested_actions {
            use crate::statemachine::drivers::fight::FightAction;
            match action {
                // --- Cookie state → AttackRuntime ---
                // These four drive the .atk machine's cookie vs no-cookie
                // row selection.  Cheap to re-write each tick (same bool
                // value is a no-op).
                FightAction::AtkAttack
                | FightAction::Attack
                | FightAction::GrabCookie => {
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        attack_rt.ctx.got_cookie = true;
                    }
                }
                FightAction::AtkIdle | FightAction::ReleaseCookie => {
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        attack_rt.ctx.got_cookie = false;
                    }
                }

                // --- Position → Goto ---
                // Fight FSM's S_MOVING / S_STARTING_CHASE fall-through
                // rules emit MoveToPosition / RequestPosition every tick.
                // We pulse `requested_goto` ONLY when the actor isn't
                // already in GOTO_STATE or FIGHT_STATE — the Behavior
                // FSM's BEHAVIOR_SWITCHES subroutine evaluates EFight
                // before EGoto, so a goto pulse won't displace an active
                // FightBehavior.  Without this bridge, AI never
                // approaches the target (FightBehavior deliberately
                // holds position until the attack FSM's distance-moves
                // take over, but those only fire once engagement is
                // already established).
                FightAction::MoveToPosition
                | FightAction::GrabPosition
                | FightAction::UpgradePosition
                | FightAction::UpgradePositionInFront
                | FightAction::UpgradePositionBehind
                | FightAction::UpgradePositionLeft
                | FightAction::UpgradePositionRight
                | FightAction::RequestPosition => {
                    let cur = behavior_state_name.as_deref().unwrap_or("");
                    if cur == "GOTO_STATE" || cur == "FIGHT_STATE" {
                        // Already moving or already engaged — don't
                        // re-pulse.  Dedup prevents the per-tick
                        // re-transition strobe; still lets us re-enter
                        // GOTO from IDLE when the last walk finished.
                        continue;
                    }
                    let target_entity = ai_opt.and_then(|a| a.target);
                    let Some(t_tf) =
                        target_entity.and_then(|t| target_transforms.get(t).ok())
                    else {
                        continue;
                    };
                    if let Some(rt) = behavior_rt_opt.as_deref_mut() {
                        // Ring position around the target — hash of the
                        // entity bits keeps different actors from
                        // piling up on the same point.
                        const ENGAGE_RADIUS: f32 = 2.0;
                        let bits = entity.to_bits();
                        let angle = ((bits & 0xFF) as f32) / 255.0
                            * std::f32::consts::TAU
                            + match action {
                                FightAction::UpgradePositionInFront => 0.0,
                                FightAction::UpgradePositionBehind => std::f32::consts::PI,
                                FightAction::UpgradePositionLeft => std::f32::consts::FRAC_PI_2,
                                FightAction::UpgradePositionRight => -std::f32::consts::FRAC_PI_2,
                                _ => 0.0,
                            };
                        let target_pos = t_tf.translation();
                        let offset =
                            Vec3::new(angle.cos(), 0.0, angle.sin()) * ENGAGE_RADIUS;
                        rt.pending_params.target_point = Some(target_pos + offset);
                        rt.pending_params.target_entity = target_entity;
                        rt.pending_params.within = Some(0.5);
                        rt.ctx.requested_goto = true;
                    }
                }

                // --- Coordinator-only, no consumer yet ---
                FightAction::ReleasePosition
                | FightAction::Parry
                | FightAction::Idle
                | FightAction::RequestCookie
                | FightAction::JoinFormation
                | FightAction::LeaveFormation
                | FightAction::ResetTimer => {
                    bevy::log::trace!(
                        "FightRuntime: {:?} on {:?} (coordinator-only, no consumer)",
                        action,
                        entity,
                    );
                }

                // --- Check / Display already handled inside apply_action ---
                FightAction::Check(_) | FightAction::Display(_) => {}
            }
        }
    }
}

/// Eagerly populate the singleton FSMs (`fight.fsm`, `squad.fsm`) at startup so
/// they're present as global objects before any AI spawns.  Mirrors the legacy
/// `aiFightManager::Init` path that constructs `aiFightStateMachineData` and
/// `aiSquadStateMachineData` once at boot.
fn load_fight_squad_singletons(
    mut fight_cache: ResMut<FightFsmCache>,
    mut squad_cache: ResMut<SquadFsmCache>,
) {
    let asset_base = crate::get_assets_path();
    let _ = fight_cache.get_or_load(asset_base);
    let _ = squad_cache.get_or_load();
}
