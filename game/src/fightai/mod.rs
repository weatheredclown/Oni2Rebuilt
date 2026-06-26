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
use crate::statemachine::drivers::parse_aifight_sm::parse_aifight_sm;
use crate::statemachine::drivers::squad::{SQUAD_ACTION_PARSER, SQUAD_EVENT_PARSER, SquadDriver};

pub mod components;
pub mod formation;
pub mod formation_data;
pub mod pattern;
pub mod poscheck;
pub mod position;
pub mod squad_leader;

pub use position::{FightResources, FightSlotState, ResourceOp};
pub use squad_leader::{
    leader_update_system, squad_leader_lifecycle_system, squad_member_sync_system,
    squad_update_system,
};

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
                match parse_aifight_sm::<FightDriver>(
                    &content,
                    FIGHT_EVENT_PARSER,
                    FIGHT_ACTION_PARSER,
                ) {
                    Ok(sm_data) => {
                        bevy::log::info!(
                            "fightai: Successfully loaded {} ({} states)",
                            filename,
                            sm_data.states.len()
                        );
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
                        match parse_aifight_sm::<FightDriver>(
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
/// `aiFightManager`.  The driver/parser vocabulary
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
                match parse_aifight_sm::<SquadDriver>(
                    &content,
                    SQUAD_EVENT_PARSER,
                    SQUAD_ACTION_PARSER,
                ) {
                    Ok(sm_data) => {
                        bevy::log::info!(
                            "fightai: Successfully loaded {} ({} states)",
                            filename,
                            sm_data.states.len()
                        );
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
            .init_resource::<squad_leader::SquadRegistry>()
            .init_resource::<formation_data::FormationLibrary>()
            .add_systems(
                Startup,
                (
                    load_fight_squad_singletons,
                    formation_data::load_formation_library_system,
                ),
            )
            .add_systems(
                Update,
                (
                    squad_leader_lifecycle_system,
                    // Group leaderless attackers by target into per-target
                    // squads, then fold any such squad into a leader's grunt
                    // squad when the leader engages that target.
                    squad_leader::attacker_squad_membership_system
                        .after(squad_leader_lifecycle_system),
                    squad_leader::leader_merge_system
                        .after(squad_leader::attacker_squad_membership_system),
                    // Rebuild Squad.members from SquadMember last, after all
                    // membership churn above has been applied.
                    squad_member_sync_system.after(squad_leader::leader_merge_system),
                    // Lay out each squad's circular formation (even slots +
                    // SortFighters assignment) once members are synced.
                    formation::formation_update_system.after(squad_member_sync_system),
                )
                    .run_if(in_state(crate::menu::AppState::InGame)),
            )
            .add_systems(
                FixedUpdate,
                (
                    leader_update_system,
                    squad_update_system.after(leader_update_system),
                    // Steer each leader toward its formation-tracking
                    // destination (writes LinearVelocity) after the squad
                    // state — circle_center is current by then.
                    squad_leader::leader_locomotion_system.after(squad_update_system),
                    // Resources offer pass clears stale offers and
                    // distributes new ones.  Must run BEFORE the FSM
                    // tick so `E_POSITION_OFFERED` / `E_COOKIE_OFFERED`
                    // see fresh offers this frame.
                    // Squad-level slot assignment (CalcMapping): distinct,
                    // angular-order positions per member before requests/offers.
                    pattern::squad_pattern_mapping_system.after(squad_update_system),
                    // Disable slots flanking active grapplers before offers go
                    // out (aiGrappler::UpdateGrapplePositions).
                    position::grapple_disable_positions_system
                        .after(pattern::squad_pattern_mapping_system),
                    position::fight_resources_offer_system
                        .after(position::grapple_disable_positions_system),
                    // Fight coordinator ticker reads FightSlotState
                    // (now populated by the offer pass above) into
                    // its FSM context, then collects requested
                    // FightActions into per-actor `pending_ops` queues.
                    fight_runtime_update_system.after(position::fight_resources_offer_system),
                    // Apply pass drains pending_ops and mutates other
                    // fighters' FightResources.  Must run AFTER the
                    // FSM tick.
                    position::fight_resource_apply_system.after(fight_runtime_update_system),
                    attack_runtime_update_system.after(fight_runtime_update_system),
                    // Throws basic strikes for actors with no attack table;
                    // reads the `fallback_strike_pending` flag set by the
                    // container system, so it must run after it.
                    fight_fallback_strike_system.after(fight_runtime_update_system),
                )
                    .run_if(in_state(crate::menu::AppState::InGame)),
            );
    }
}

/// Advance every actor's `AttackRuntime` once per FixedUpdate.  Mirrors the
/// legacy `aiAttackStateMachine::Update`: refresh the ctx with the current anim
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
            Option<&crate::animator::components::ActionPlayer>,
            Option<&GlobalTransform>,
            Option<&crate::fight::components::FighterState>,
            Option<&mut crate::fight::components::GrappleState>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
    transforms_q: Query<&GlobalTransform>,
    mut attack_fsm_cache: ResMut<AttackFsmCache>,
    mut start_events: MessageWriter<crate::fight::events::GrappleStartEvent>,
) {
    let dt = time.delta_secs();
    for (
        entity,
        mut runtime,
        anim_lib,
        mut anim_state,
        mut fighter,
        ai_fighter_opt,
        action_player_opt,
        self_tf_opt,
        fighter_state_opt,
        mut grapple_state_opt,
    ) in &mut query
    {
        // High-fidelity parity with aiFighter::UpdatePacketAttack:
        // Delay attack FSM ticks if the character is transitioning (e.g. crouching to standing).
        // This prevents attack animations from unexpectedly kicking in during stance transitions.
        if let Some(ap) = action_player_opt {
            if ap.is_transitioning() {
                continue;
            }
        }

        runtime.fsm.advance_clock(dt);
        runtime.ctx.dt = dt;
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames as i32;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;

        // Distance to current AI target — read by `ActionDistance::update`
        // (and any future range-gated action) to decide when MB/MA-style
        // "move to within X" actions complete.  Computed from world XZ;
        // None when we have no target or transforms aren't available.
        runtime.ctx.target_distance_current =
            ai_fighter_opt.and_then(|ai| ai.target).and_then(|tgt| {
                let self_pos = self_tf_opt?.translation();
                let tgt_pos = transforms_q.get(tgt).ok()?.translation();
                let dx = self_pos.x - tgt_pos.x;
                let dz = self_pos.z - tgt_pos.z;
                Some((dx * dx + dz * dz).sqrt())
            });

        let target = ai_fighter_opt.and_then(|ai| ai.target);
        let mut target_dist = 0.0;
        let mut target_facing_within_30 = false;
        let mut target_cross_y = 0.0;

        if let Some(tgt) = target {
            if let Ok(tgt_tf) = transforms_q.get(tgt) {
                if let Some(self_tf) = self_tf_opt {
                    let self_pos = self_tf.translation();
                    let tgt_pos = tgt_tf.translation();

                    // Flat direction to target on the XZ plane
                    let to_target_xz =
                        Vec3::new(tgt_pos.x - self_pos.x, 0.0, tgt_pos.z - self_pos.z)
                            .normalize_or_zero();
                    use crate::oni2_loader::utils::space::OniTransformExt;
                    let self_forward = *self_tf.oni_forward();
                    let self_forward_xz =
                        Vec3::new(self_forward.x, 0.0, self_forward.z).normalize_or_zero();

                    let angle = self_forward_xz.angle_between(to_target_xz).abs();
                    target_dist =
                        Vec3::new(tgt_pos.x - self_pos.x, 0.0, tgt_pos.z - self_pos.z).length();
                    target_facing_within_30 = angle <= 30.0_f32.to_radians();

                    // Cross product around Y axis
                    let cross = self_forward_xz.cross(to_target_xz);
                    target_cross_y = cross.y;
                }
            }
        }

        let is_grappling = fighter_state_opt
            .map(|fs| fs.is_grappling())
            .unwrap_or(false);
        let is_reacting = action_player_opt.is_some_and(|ap| ap.is_reacting());
        let last_frame = (anim_state.anim.num_frames as f32 - 1.0).max(0.0);
        let is_ready =
            !is_reacting && (anim_state.looping || anim_state.current_time >= last_frame);

        runtime.ctx.target = target;
        runtime.ctx.target_distance = target_dist;
        runtime.ctx.target_facing_within_30 = target_facing_within_30;
        runtime.ctx.target_cross_y = target_cross_y;
        runtime.ctx.is_grappling = is_grappling;
        runtime.ctx.is_ready = is_ready;

        // Restore original table when grapple ends
        if let Some(orig_table) = runtime.original_table.take() {
            if !is_grappling {
                let asset_base = crate::get_assets_path();
                if let Some(orig_data) = attack_fsm_cache.get_or_load(&orig_table, &asset_base) {
                    runtime.table_name = orig_table;
                    let mut fsm = crate::statemachine::core::SmRuntime::new(orig_data, 0);
                    fsm.entity = Some(entity);
                    runtime.fsm = fsm;
                    bevy::log::info!(
                        "attack_runtime_update_system: Grapple ended on {:?}, restoring attack table '{}'",
                        entity,
                        runtime.table_name
                    );
                } else {
                    bevy::log::error!(
                        "attack_runtime_update_system: Failed to restore original attack table '{}'",
                        orig_table
                    );
                }
            } else {
                runtime.original_table = Some(orig_table);
            }
        }

        // Split the two mutable borrows — `runtime.fsm.tick(&mut runtime.ctx)`
        // would alias `runtime` to itself.  Go through `as_mut`.
        let rt_mut = runtime.as_mut();

        rt_mut.tick_count = rt_mut.tick_count.wrapping_add(1);
        rt_mut.ctx.entity = Some(entity);

        let current_state = rt_mut.fsm.current_state;
        let got_cookie = rt_mut.ctx.got_cookie;

        if current_state != rt_mut.last_state_idx || got_cookie != rt_mut.last_cookie {
            let pre_state = rt_mut.fsm.data.state_name(current_state);
            let debug_str = crate::debug::debug_name(entity);
            bevy::log::info!(
                "AttackRuntime: {} [{:?}], state={}, got_cookie={}",
                debug_str,
                entity,
                pre_state,
                got_cookie
            );
            rt_mut.last_state_idx = current_state;
            rt_mut.last_cookie = got_cookie;
        }

        let mut output = rt_mut.fsm.tick(&mut rt_mut.ctx);

        // Handle grapple outputs
        if let Some((anim, table)) = output.grapple_start.take() {
            let asset_base = crate::get_assets_path();
            if let Some(new_data) = attack_fsm_cache.get_or_load(&table, &asset_base) {
                rt_mut.original_table = Some(rt_mut.table_name.clone());
                rt_mut.table_name = table;

                let mut fsm = crate::statemachine::core::SmRuntime::new(new_data, 0);
                fsm.entity = Some(entity);
                rt_mut.fsm = fsm;

                let target_ent = ai_fighter_opt.and_then(|ai| ai.target);
                if let Some(tgt) = target_ent {
                    let target_pos = transforms_q
                        .get(tgt)
                        .ok()
                        .map(|t| t.translation())
                        .unwrap_or(Vec3::ZERO);
                    start_events.write(crate::fight::events::GrappleStartEvent {
                        attacker: entity,
                        target: tgt,
                        target_pos,
                    });
                    bevy::log::info!(
                        "attack_runtime_update_system: Grapple start event on {:?} targeting {:?}",
                        entity,
                        tgt
                    );
                }
            } else {
                bevy::log::error!(
                    "attack_runtime_update_system: Failed to load grapple table '{}'",
                    table
                );
            }
        }

        if let Some(action) = output.grapple_action.take() {
            if let Some(ref mut gs) = grapple_state_opt {
                gs.pending_action = action;
            }
        }

        // Edge-trigger any output that came out of the .atk tick.  Lets us
        // see whether the AttackRuntime is actually producing a swing/block/
        // evade pulse or sitting silent despite got_cookie=true.
        if output.attack_anim.is_some()
            || output.block_anim.is_some()
            || output.evade_anim.is_some()
            || output.custom_anim.is_some()
            || output.start_following_distance.is_some()
        {
            let debug_str = crate::debug::debug_name(entity);
            let pre_state = rt_mut.fsm.data.state_name(rt_mut.fsm.current_state);
            bevy::log::info!(
                "AttackRuntime out {} [{:?}] state={} cookie={} \
                 attack={:?} block={:?} evade={:?} custom={:?} follow={:?}",
                debug_str,
                entity,
                pre_state,
                rt_mut.ctx.got_cookie,
                output.attack_anim.as_deref(),
                output.block_anim.as_deref(),
                output.evade_anim.as_ref().map(|(n, _)| n.as_str()),
                output.custom_anim.as_deref(),
                output.start_following_distance,
            );
        }

        // Suppress anim dispatch while the actor is in the react+getup
        // queue. The AttackRuntime FSM keeps ticking (so cookies and
        // state get reaped on schedule) but any swing/block/evade
        // pulse it produces is dropped — matches the legacy gates
        // (`CanStartAttack` / `CanStartBlock` return false while
        // `IsReacting()`) and prevents the queued getup from being
        // clobbered the moment the react reaches its last frame.
        if !is_reacting {
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
                crate::statemachine::runtime::do_evade(
                    anim_lib,
                    &mut anim_state,
                    anim_name,
                    *mirror,
                );
            }
            if let Some(anim_name) = &output.custom_anim {
                crate::statemachine::runtime::do_custom_anim(anim_lib, &mut anim_state, anim_name);
            }
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
/// legacy `aiFightStateMachine::Update` loop.
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
/// Inputs wired by the position/cookie coordinator
/// (`game/src/fightai/position.rs`):
///   • `has_position` ← `FightSlotState.current_position >= 0` and the
///     held slot's owner matches the current AI target
///   • `can_attack`   ← `has_position` (legacy `CanAttackFromHere`
///     literally returns true; left as a 1:1 alias here).  Will be
///     refined when range/heading checks land.
///   • `position_offered` ← `!FightSlotState.offered_positions.is_empty()`
///   • `cookie_offered`   ← `FightSlotState.cookie_offered_by ==
///                         Some(resource_target)`
///
/// Inputs STILL STUBBED:
///   • `prepare_next_attacker` — left at `false`.  Drives the cookie
///     hand-off in S_ATTACK: when true, the fight FSM releases the
///     cookie via `A_RELEASE_COOKIE` and steps to
///     S_PREPARE_NEXT_ATTACKER so a queued peer can swing.  Without it
///     a single AI fighter monopolizes the cookie forever (only
///     released externally on death / target switch / squad teardown),
///     so multi-attacker pressure on the same target serializes onto
///     whoever grabbed first.
///
///   Wiring sketch (legacy formula:
///   `!IsAttacking() || AttackStateMachine->IsDelayingCookie()`):
///     1. The "anim-ended" half is already in `ctx.attack_finished` —
///        set `prepare_next_attacker = ctx.attack_finished` and
///        combat will at least round-robin on swing end.
///     2. The "voluntary yield" half needs a new `delaying_cookie`
///        bool on `AttackCtx` plus an `.atk` action step that flips
///        it (legacy `aiAtkAction` subclasses set FLAG_DELAYED_COOKIE
///        on the attack state machine).  Read that flag out of the
///        attached `AttackRuntime.ctx` here.
///     3. (Done) `FIGHTMGR.CookieHogTime` is ported as a watchdog in
///        `fight_resources_offer_system` — it force-clears `cookie_holder`
///        when the holder has held longer than `COOKIE_HOG_TIME` while
///        others are queued (the legacy `aiAttackStateMachine` safety net).
///
///   See also `FightCtx::prepare_next_attacker` and
///   `FightEvent::PrepareNextAttacker` for the field/event docs.
///
/// Outputs: the FSM writes `FightAction` requests into
/// `FightOutput.requested_actions`.  We translate the coordinator
/// actions into `ResourceOp`s pushed onto `FightSlotState.pending_ops`,
/// drained next frame by `fight_resource_apply_system`.  `AtkAttack`
/// / `AtkIdle` / `Attack` also flip `AttackRuntime.ctx.got_cookie`,
/// which is how the attack FSM decides between cookie vs no-cookie
/// rows on its next tick.
pub fn fight_runtime_update_system(
    time: Res<Time>,
    mut query: Query<
        (
            Entity,
            &mut crate::fightai::components::FightRuntime,
            Option<&mut crate::fightai::components::AttackRuntime>,
            Option<&mut crate::ai::components::AiFighter>,
            Option<&crate::animator::components::ActionPlayer>,
            Option<&crate::fight::components::FighterState>,
            Option<&crate::oni2_loader::animation::Oni2AnimState>,
            Option<&mut crate::behavior::BehaviorRuntime>,
            Option<&mut crate::fightai::position::FightSlotState>,
            Option<&crate::fightai::components::FighterOrder>,
        ),
        Without<crate::oni2_loader::components::ActorAsleep>,
    >,
    target_health: Query<&crate::combat::components::Health>,
    target_transforms: Query<&GlobalTransform>,
    formation_slots: Query<&crate::fightai::formation::FormationSlot>,
) {
    let dt = time.delta_secs();
    for (
        entity,
        mut runtime,
        mut attack_rt_opt,
        mut ai_opt,
        action_player_opt,
        fs_opt,
        anim_opt,
        mut behavior_rt_opt,
        mut slot_state_opt,
        order_opt,
    ) in &mut query
    {
        if !runtime.ctx.init_logged {
            bevy::log::info!(
                "FightRuntime init[{:?}]: attack_rt={} slot_state={} behavior={} ai={} \
                 anim={} action_player={} fighter_state={} order={}",
                entity,
                attack_rt_opt.is_some(),
                slot_state_opt.is_some(),
                behavior_rt_opt.is_some(),
                ai_opt.is_some(),
                anim_opt.is_some(),
                action_player_opt.is_some(),
                fs_opt.is_some(),
                order_opt.is_some(),
            );
            runtime.ctx.init_logged = true;
        }

        // Heartbeat: if this entity has an AttackRuntime, peek its tick
        // counter and warn loudly if it's still 0 by the time the fight
        // FSM has progressed past S_STARTUP.  That indicates the
        // attack_runtime_update_system query isn't matching the entity
        // (most likely Oni2AnimLibrary or Fighter is missing — the two
        // strict requirements that fight_runtime_update_system doesn't
        // share).  One-shot per entity (gated on `warned_atk_silent`).
        if let Some(attack_rt) = attack_rt_opt.as_deref() {
            if attack_rt.tick_count == 0
                && runtime.fsm.current_state != 0
                && !runtime.ctx.warned_atk_silent
            {
                bevy::log::warn!(
                    "FightRuntime[{:?}]: AttackRuntime exists but its tick_count is still 0 — \
                     attack_runtime_update_system isn't matching this entity. \
                     Check Oni2AnimLibrary / Fighter presence.",
                    entity
                );
                runtime.ctx.warned_atk_silent = true;
            }
        }

        runtime.fsm.advance_clock(dt);

        // --- Populate context inputs ---
        let mut ai_target: Option<Entity> = ai_opt.as_ref().and_then(|a| a.target);

        // If order specifies a fight target and we don't have one, adopt it
        if let Some(fo) = order_opt {
            if fo.order == crate::statemachine::drivers::squad::SquadOrder::Fight {
                if ai_target.is_none() && fo.object.is_some() {
                    if let Some(ref mut ai) = ai_opt {
                        ai.target = fo.object;
                        ai_target = fo.object;
                    }
                }
            }
        }

        runtime.ctx.has_target = ai_target.is_some();
        runtime.ctx.is_reacting = action_player_opt.is_some_and(|ap| ap.is_reacting());
        runtime.ctx.target_killed = ai_target
            .and_then(|t| target_health.get(t).ok())
            .is_some_and(|h| h.current <= 0.0);

        // Position/cookie inputs from the coordinator.  We treat
        // `has_position` as "we hold a slot AROUND the current AI
        // target" — ResourceTarget on the slot state must match the
        // AI target the fight FSM is acting on, otherwise the slot
        // is stale.
        if let Some(slot) = slot_state_opt.as_deref() {
            runtime.ctx.has_position = slot.current_position >= 0
                && slot.resource_target == ai_target
                && ai_target.is_some();
            // Legacy `CanAttackFromHere` returns true unconditionally.
            // Mirror that until range/heading checks need adding.
            runtime.ctx.can_attack = runtime.ctx.has_position;
            runtime.ctx.position_offered = !slot.offered_positions.is_empty();
            runtime.ctx.cookie_offered = slot
                .cookie_offered_by
                .map(|by| Some(by) == ai_target)
                .unwrap_or(false);

            // Edge-triggered diagnostic: dump exactly why has_position
            // resolved the way it did.  Reads as
            //   "slot=<idx> tgt=<entity> ai_tgt=<entity> offers=<n> ops=<n>"
            // so a stuck S_STARTING_FIGHT shows whether the slot was
            // never claimed (current_position=-1), whether resource_target
            // diverged from the AI target, or whether the FSM is
            // emitting RequestPosition at all (pending_ops length).
            let dbg = format!(
                "slot_state[{:?}]: cur_pos={} res_tgt={:?} ai_tgt={:?} offers={} pending_ops={} -> has_position={}",
                entity,
                slot.current_position,
                slot.resource_target,
                ai_target,
                slot.offered_positions.len(),
                slot.pending_ops.len(),
                runtime.ctx.has_position,
            );
            if dbg != runtime.ctx.last_pos_dbg {
                bevy::log::info!("{}", dbg);
                runtime.ctx.last_pos_dbg = dbg;
            }
        } else {
            runtime.ctx.has_position = false;
            runtime.ctx.can_attack = false;
            runtime.ctx.position_offered = false;
            runtime.ctx.cookie_offered = false;
            let dbg = format!(
                "slot_state[{:?}]: <NO COMPONENT> ai_tgt={:?}",
                entity, ai_target
            );
            if dbg != runtime.ctx.last_pos_dbg {
                bevy::log::warn!("{}", dbg);
                runtime.ctx.last_pos_dbg = dbg;
            }
        }

        // Set `ctx.mode` mirroring `aiFightStateMachine::Update`.
        // If order is present, it directly dictates the mode (e.g. Idle -> idle, Retreat -> retreat, Formation -> join_formation).
        // If order is Fight, it uses the chase/attack logic.
        let mut target_mode = "idle";
        if let Some(fo) = order_opt {
            match fo.order {
                crate::statemachine::drivers::squad::SquadOrder::Idle => {
                    target_mode = "idle";
                }
                crate::statemachine::drivers::squad::SquadOrder::Fight => {
                    if runtime.ctx.has_position {
                        target_mode = "attack";
                    } else {
                        target_mode = "chase";
                    }
                }
                crate::statemachine::drivers::squad::SquadOrder::Retreat => {
                    target_mode = "retreat";
                }
                crate::statemachine::drivers::squad::SquadOrder::Formation => {
                    target_mode = "join_formation";
                }
            }
        } else {
            let scroni_locked = runtime.ctx.mode.eq_ignore_ascii_case("defend")
                || runtime.ctx.mode.eq_ignore_ascii_case("join_formation");
            if !scroni_locked {
                target_mode = if !runtime.ctx.has_target {
                    "idle"
                } else if runtime.ctx.has_position {
                    "attack"
                } else {
                    "chase"
                };
            } else {
                target_mode = &runtime.ctx.mode;
            }
        }

        if runtime.ctx.mode != target_mode {
            runtime.ctx.mode = target_mode.to_string();
        }

        // Stash the current behavior state name for dedup + the
        // MoveToPosition arm below (which gates on whether the actor
        // is already in GOTO_STATE / FIGHT_STATE).
        let behavior_state_name: Option<String> = behavior_rt_opt
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

        // This is the missing piece for round-robin combat among AI groups.
        // It drives E_PREPARE_NEXT_ATTACKER, which signals the FSM to yield
        // the combat cookie so another fighter can attack.
        //
        // Keyed purely off `attack_finished` (an anim-based proxy, so it works
        // with OR without an AttackRuntime): an actor relying on the
        // fallback-strike path yields the cookie when its basic strike anim
        // ends, exactly like one driven by an attack table.  An actor that
        // genuinely can't attack at all is caught by the COOKIE_HOG_TIME
        // watchdog rather than force-yielding here (which used to rob
        // no-AttackRuntime actors of any chance to swing).
        runtime.ctx.prepare_next_attacker = runtime.ctx.attack_finished;

        // Reset the per-tick fallback-strike request; the Attack action arm
        // below re-sets it when the FightDriver pulses an attack on an actor
        // that has no AttackRuntime.
        runtime.ctx.fallback_strike_pending = false;

        runtime.ctx.attacked = fs_opt.is_some_and(|fs| fs.in_invuln_phase);

        // --- Tick ---
        let rt_mut = runtime.as_mut();
        let current_state = rt_mut.fsm.current_state;

        if current_state != rt_mut.last_state_idx || rt_mut.ctx.mode != rt_mut.last_mode {
            bevy::log::info!(
                "FightRuntime: entity={:?}, state={}, mode='{}', has_target={}, behavior={}",
                entity,
                rt_mut.fsm.data.state_name(current_state),
                rt_mut.ctx.mode,
                rt_mut.ctx.has_target,
                behavior_state_name.as_deref().unwrap_or("None")
            );
            rt_mut.last_state_idx = current_state;
            rt_mut.last_mode = rt_mut.ctx.mode.clone();
        }

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
                // --- Cookie state → AttackRuntime + queue ops ---
                // GrabCookie / ReleaseCookie also flip the .atk
                // machine's `got_cookie` row selector.  Cheap to
                // re-write each tick (same bool value is a no-op).
                FightAction::GrabCookie => {
                    let had_rt = attack_rt_opt.is_some();
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        attack_rt.ctx.got_cookie = true;
                    }
                    if let Some(slot) = slot_state_opt.as_deref_mut() {
                        slot.pending_ops.push(position::ResourceOp::GrabCookie);
                    }
                    bevy::log::info!(
                        "FightRuntime: GrabCookie on {:?} attack_rt={} -> got_cookie=true",
                        entity,
                        had_rt
                    );
                }
                FightAction::ReleaseCookie => {
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        attack_rt.ctx.got_cookie = false;
                    }
                    if let Some(slot) = slot_state_opt.as_deref_mut() {
                        slot.pending_ops.push(position::ResourceOp::ReleaseCookie);
                    }
                }
                FightAction::RequestCookie => {
                    if let (Some(slot), Some(target)) = (slot_state_opt.as_deref_mut(), ai_target) {
                        slot.pending_ops.push(position::ResourceOp::RequestCookie {
                            target,
                            priority: 2,
                        });
                    }
                }
                FightAction::AtkAttack | FightAction::Attack => {
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        if !attack_rt.ctx.got_cookie {
                            bevy::log::info!(
                                "FightRuntime: {:?} on {:?} -> got_cookie=true",
                                action,
                                entity
                            );
                        }
                        attack_rt.ctx.got_cookie = true;
                    } else {
                        // No attack table → request a fallback basic strike.
                        // `fight_fallback_strike_system` (which carries the
                        // anim library + Fighter this system lacks) picks a
                        // random strike from the actor's AiAttackTable and
                        // plays it.  Port of `aiFighter::StartAttack`'s
                        // uninitialized-attack-state-machine fallback.
                        runtime.ctx.fallback_strike_pending = true;
                        if !runtime.ctx.warned_no_attack_rt {
                            bevy::log::debug!(
                                "FightRuntime: {:?} on {:?} has no AttackRuntime — \
                                 using fallback basic-strike path",
                                action,
                                entity
                            );
                            runtime.ctx.warned_no_attack_rt = true;
                        }
                    }
                }
                FightAction::AtkIdle => {
                    if let Some(attack_rt) = attack_rt_opt.as_deref_mut() {
                        if attack_rt.ctx.got_cookie {
                            bevy::log::info!(
                                "FightRuntime: AtkIdle on {:?} -> got_cookie=false",
                                entity
                            );
                        }
                        attack_rt.ctx.got_cookie = false;
                    }
                }

                // --- Position state ops (defender-side queues) ---
                // RequestPosition pushes self onto the target's slot
                // queue; the offer pass next frame will surface the
                // slot via E_POSITION_OFFERED if we win priority.
                FightAction::RequestPosition => {
                    if let (Some(slot), Some(target)) = (slot_state_opt.as_deref_mut(), ai_target) {
                        let assigned = slot.assigned_position;
                        slot.pending_ops
                            .push(position::ResourceOp::RequestPosition {
                                target,
                                slot: assigned,
                                priority: 2,
                            });
                    }
                }
                FightAction::GrabPosition => {
                    if let Some(slot) = slot_state_opt.as_deref_mut() {
                        slot.pending_ops.push(position::ResourceOp::GrabPosition);
                    }
                }
                FightAction::ReleasePosition => {
                    if let Some(slot) = slot_state_opt.as_deref_mut() {
                        slot.pending_ops.push(position::ResourceOp::ReleasePosition);
                    }
                }
                // UpgradePosition* — release current slot and re-request.
                // Directional bias is currently flat; once we surface
                // facing into FightCtx the apply system can pick
                // `slot = (relative_to_target_facing + offset) & 7`.
                FightAction::UpgradePosition
                | FightAction::UpgradePositionInFront
                | FightAction::UpgradePositionBehind
                | FightAction::UpgradePositionLeft
                | FightAction::UpgradePositionRight => {
                    if let (Some(slot), Some(target)) = (slot_state_opt.as_deref_mut(), ai_target) {
                        slot.pending_ops.push(position::ResourceOp::ReleasePosition);
                        slot.pending_ops
                            .push(position::ResourceOp::RequestPosition {
                                target,
                                slot: -1,
                                priority: 3,
                            });
                    }
                }

                // --- Movement bridge: drive GotoBehavior toward our
                //     held slot's world position.  Doesn't pre-empt an
                //     active GOTO_STATE/FIGHT_STATE — the Behavior FSM
                //     evaluates EFight before EGoto, and we'd just
                //     strobe.
                FightAction::MoveToPosition => {
                    let cur = behavior_state_name.as_deref().unwrap_or("");
                    if cur == "GOTO_STATE" || cur == "FIGHT_STATE" {
                        continue;
                    }
                    let Some(target_entity) = ai_target else {
                        continue;
                    };
                    let Ok(t_tf) = target_transforms.get(target_entity) else {
                        continue;
                    };
                    // Prefer the squad formation slot (even-spaced ring +
                    // SortFighters assignment) when this fighter has one — it
                    // supersedes the legacy 8-fixed-direction layout.  Fall
                    // back to OUR held resource slot, else just walk in
                    // (closest dir).
                    const ENGAGE_RADIUS: f32 = 2.0;
                    let dest = match formation_slots.get(entity) {
                        Ok(slot) if slot.valid => slot.center,
                        _ => {
                            let slot_idx = slot_state_opt
                                .as_deref()
                                .and_then(|s| {
                                    if s.current_position >= 0
                                        && s.resource_target == Some(target_entity)
                                    {
                                        Some(s.current_position as usize)
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(0);
                            position::slot_world_pos(t_tf.translation(), slot_idx, ENGAGE_RADIUS)
                        }
                    };
                    if let Some(rt) = behavior_rt_opt.as_deref_mut() {
                        rt.pending_params.target_point = Some(dest);
                        rt.pending_params.target_entity = Some(target_entity);
                        rt.pending_params.within = Some(0.5);
                        rt.ctx.requested_goto = true;
                    }
                }

                // --- Coordinator-only, no consumer yet ---
                FightAction::Idle
                | FightAction::Parry
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

/// Fallback basic-strike dispatcher for AI fighters with NO `AttackRuntime`
/// (their `.atk` attack table is missing or failed to load).  Mirrors the
/// `!AttackStateMachine->IsInitialized()` branch of `aiFighter::StartAttack`:
/// when such an actor's FightDriver wants to attack
/// (`FightCtx::fallback_strike_pending`, set by `fight_runtime_update_system`),
/// pick a random strike from its `AiAttackTable` — geometry-filtered against
/// the target when one exists (`ChooseRandomStrike(matrix, tgt, height)`),
/// else purely random — and play it.  Without this an actor with a broken
/// attack table just stands in S_ATTACK doing nothing.
///
/// Lives in a dedicated system (not `fight_runtime_update_system`) because the
/// swing needs the `Oni2AnimLibrary` + mutable `Fighter`/`Oni2AnimState` that
/// the container system deliberately doesn't hold.  Scoped to `Without<
/// AttackRuntime>` so attack-table fighters keep using their normal path.
pub fn fight_fallback_strike_system(
    mut query: Query<
        (
            Entity,
            &mut crate::fightai::components::FightRuntime,
            &crate::fight::ai_attack::AiAttackTable,
            &crate::oni2_loader::animation::Oni2AnimLibrary,
            &mut crate::oni2_loader::animation::Oni2AnimState,
            &mut crate::combat::components::Fighter,
            Option<&crate::ai::components::AiFighter>,
            &GlobalTransform,
        ),
        Without<crate::fightai::components::AttackRuntime>,
    >,
    target_transforms: Query<&GlobalTransform>,
) {
    // The legacy height gate reads `target->GetFighterData().GetHeight()`;
    // FighterType doesn't carry a height yet, so use a typical humanoid extent.
    // TODO: source the real per-type height once it's parsed.
    const DEFAULT_TARGET_HEIGHT: f32 = 2.0;

    let mut rng = rand::rng();

    for (entity, mut runtime, table, lib, mut anim_state, mut fighter, ai_opt, gt) in &mut query {
        if !runtime.ctx.fallback_strike_pending {
            continue;
        }
        runtime.ctx.fallback_strike_pending = false;

        // Don't restart a swing that's still playing — the engine only starts
        // a new strike between attacks.
        if crate::combat::locked_movement(&anim_state) {
            continue;
        }
        if table.attacks.is_empty() {
            continue;
        }

        let attacker_pos = gt.translation();
        let attacker_facing = fighter.facing;

        let chosen = match ai_opt.and_then(|a| a.target) {
            Some(target) => {
                let tgt_pos = target_transforms
                    .get(target)
                    .map(|t| t.translation())
                    .unwrap_or(attacker_pos);
                // Geometry-filtered overload: random among strikes that can
                // actually land.  If none are in range/aligned the actor is
                // mispositioned and doesn't swing (matches the legacy -1 path).
                table.choose_random_strike_for(
                    &mut rng,
                    attacker_pos,
                    attacker_facing,
                    tgt_pos,
                    DEFAULT_TARGET_HEIGHT,
                )
            }
            None => table.choose_random_strike(&mut rng),
        };

        if let Some(atk) = chosen {
            let name = atk.anim_name.clone();
            bevy::log::info!(
                "fight_fallback_strike: {:?} throwing basic strike '{}' (no attack table)",
                entity,
                name
            );
            crate::statemachine::runtime::do_attack(
                lib,
                &mut anim_state,
                &mut fighter,
                &name,
                0,
            );
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
