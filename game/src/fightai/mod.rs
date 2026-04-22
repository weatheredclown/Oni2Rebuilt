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
                attack_runtime_update_system.run_if(in_state(crate::menu::AppState::InGame)),
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
    mut query: Query<(
        Entity,
        &mut crate::fightai::components::AttackRuntime,
        &crate::oni2_loader::animation::Oni2AnimLibrary,
        &mut crate::oni2_loader::animation::Oni2AnimState,
        &mut crate::combat::components::Fighter,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut runtime, anim_lib, mut anim_state, mut fighter) in &mut query {
        runtime.fsm.advance_clock(dt);
        runtime.ctx.dt = dt;
        runtime.ctx.anim_num_frames = anim_state.anim.num_frames as i32;
        runtime.ctx.anim_current_time = anim_state.current_time;
        runtime.ctx.anim_looping = anim_state.looping;

        // Split the two mutable borrows — `runtime.fsm.tick(&mut runtime.ctx)`
        // would alias `runtime` to itself.  Go through `as_mut`.
        let rt_mut = runtime.as_mut();
        let output = rt_mut.fsm.tick(&mut rt_mut.ctx);

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
