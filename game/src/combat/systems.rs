/*
 * combat/systems.rs — all combat FixedUpdate systems.
 *
 * ground_detection_system: ShapeCaster ground check → Fighter.is_grounded.
 * attack_sync_system: drives AttackState frames from Oni2AnimState ATDT data.
 * hit_detection_system: cylinder-slice overlap test for active strike frames.
 * about_to_be_hit_system: predictive warning for targets about to be struck.
 * hit_reaction_system: applies knockback impulse and posts HitReactionMessage.
 * combo_tracking_system: counts consecutive hits and classifies combo strength.
 * death_system / death_cleanup_system / death_timer_system: entity removal.
 * telemetry_combat_system: forwards DamageMessage to the telemetry channel.
 */
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::ecs::relationship::Relationship;
use rb_shared::events::CombatEvent;

use crate::fight::components::{BlockLibrary, BlockStatus, FighterState};
use crate::fight::events::{BlockFailedEvent, BlockSuccessEvent};
use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::rct::ANIMREACT_NAMES;
use crate::projectile_system::SpawnProjectileEvent;
use crate::telemetry::bridge::TelemetryChannel;

use super::components::*;
use super::events::*;


/// Height of the physics capsule center above ground (capsule_half_height + snap_buffer).
/// Must match the value used in spawn.rs / NeedsGroundSnap.
const CAPSULE_CENTER_HEIGHT: f32 = 1.1;
/// Half-height of the capsule cylinder section (exclusive of hemisphere caps).
const CAPSULE_HALF_HEIGHT: f32 = 1.0;

/// Syncs the AttackState with the current running animation. Clears collision lists when a new animation plays.
///
/// Consumes `AnimStartedMessage` events (edge-triggered, ordering-immune)
/// rather than polling a shared bool — this is the migration away from
/// the `anim_just_started` shared-state pattern that required a late
/// `Last`-scheduled reset and was vulnerable to schedule mismatches.
pub fn attack_sync_system(
    mut reader: MessageReader<crate::animator::AnimStartedMessage>,
    mut query: Query<(
        &mut AttackState,
        &crate::oni2_loader::animation::Oni2AnimState,
        Option<&mut crate::animator::components::ActionPlayer>,
        Option<&Fighter>,
    )>,
) {
    for msg in reader.read() {
        let Ok((mut attack_state, anim_state, ap_opt, fighter_opt)) = query.get_mut(msg.entity)
        else {
            continue;
        };
        let mut has_fire = false;

        // New animation started — log entry and (re)seed ActiveAttack.
        //
        // Note: `end_rotation_notches` is NO LONGER applied here.  It
        // used to fire at the start of the NEXT animation, which left a
        // one-frame visual gap where the new anim was already rendering
        // at the un-rotated orientation.  The rotation is now applied
        // by `apply_end_rotation_on_anim_end_system` via
        // `AnimEndedMessage`, which fires the tick the PREVIOUS anim's
        // phase advances past its last frame — *before* any system
        // gets to switch to the next anim.  This system's only
        // remaining job here is bookkeeping: clearing per-attack state
        // and seeding the new attack's data.
        let new_anim_name = anim_state
            .current_anim_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        let new_strike_slice = anim_state
            .anim
            .attack_data
            .as_ref()
            .and_then(|d| d.strike.as_ref())
            .map(|s| {
                (
                    s.slicestartradians,
                    s.sliceendradians,
                    s.stop_track_frame,
                    s.spin,
                    s.end_rotation_notches,
                )
            });
        let cur_facing = fighter_opt.as_ref().map(|f| (f.facing.x, f.facing.z));
        info!(
            "STRIKE_DEBUG: anim-started entity={:?} new_anim='{}' cur_facing={:?} new_strike={:?}",
            msg.entity, new_anim_name, cur_facing, new_strike_slice,
        );

        if let Some(ref mut active) = attack_state.active_attack {
            // Reset per-attack state.  end_rotation_notches MAY still
            // be non-zero here as a fallback for anims that ended
            // without `update_oni2_animation` running its end-edge
            // (e.g. attack interrupted by react / death / scripted
            // anim swap before the phase advanced past the end).  Move
            // the stale value into the new active so that the next
            // anim-end path can still drain it, but DO NOT apply it —
            // visual continuity is best preserved by skipping the
            // rotation on an interrupted attack.
            active.end_rotation_notches = 0;
            active.hit_entities.clear();
            active.has_fired_projectile = false;

            // Grab ATDT end rotation notches for the newly started animation
            if let Some(data) = &anim_state.anim.attack_data {
                if let Some(strike) = &data.strike {
                    active.end_rotation_notches = strike.end_rotation_notches;
                    has_fire = strike.fire;
                }
            }
        } else {
            let mut new_active = ActiveAttack::default();
            if let Some(data) = &anim_state.anim.attack_data
                && let Some(strike) = &data.strike
            {
                new_active.end_rotation_notches = strike.end_rotation_notches;
                has_fire = strike.fire;
            }
            attack_state.active_attack = Some(new_active);
        }

        if has_fire {
            if let Some(mut ap) = ap_opt {
                ap.weapon_state = crate::animator::components::WeaponState::Drawn;
            }
        }
    }
}

/// Apply the post-attack `end_rotation_notches` rotation the tick the
/// attack animation crosses past its last frame — emitted by
/// `update_oni2_animation` as `AnimEndedMessage`.
///
/// Critical timing: runs in `FixedUpdate` immediately AFTER
/// `update_oni2_animation` (so it sees this tick's edge events) and
/// BEFORE any system that would react to the anim ending by switching
/// to a new anim (FSM tick / action_player tick / etc.).  This places
/// the Fighter / Transform rotation BEFORE the next-anim's first
/// rendered frame, eliminating the one-frame "flash to old facing"
/// glitch.  Mirrors the C++ `crStrike::End()` call site
/// (rb/src/fight/strike.cpp:217-233): the rotation is part of the
/// outgoing strike's end, not the incoming anim's start.
///
/// We mutate Fighter.facing AND Transform.rotation (+ avian Rotation)
/// inline using the same formula `fighter_rotation_sync_system` uses
/// — that system runs later in FixedUpdate but the inline write keeps
/// the Transform consistent for any system reading it before then.
pub fn apply_end_rotation_on_anim_end_system(
    mut reader: MessageReader<crate::animator::AnimEndedMessage>,
    mut query: Query<(
        &mut AttackState,
        Option<&mut Fighter>,
        Option<&mut Transform>,
        Option<&mut avian3d::prelude::Rotation>,
    )>,
) {
    for msg in reader.read() {
        let Ok((mut attack_state, fighter_opt, transform_opt, avian_rot_opt)) =
            query.get_mut(msg.entity)
        else {
            continue;
        };
        let Some(ref mut active) = attack_state.active_attack else {
            continue;
        };
        let notches = active.end_rotation_notches;
        if notches == 0 {
            continue;
        }
        // Drain so we don't double-apply if anim-start fires later.
        active.end_rotation_notches = 0;

        let radians = notches as f32 * crate::fight::systems::NOTCH_RADIANS;
        let rotation = Quat::from_rotation_y(radians);

        if let Some(mut fighter) = fighter_opt {
            let pre = fighter.facing;
            fighter.facing = rotation * fighter.facing;
            info!(
                "STRIKE_DEBUG: end-rotation-notches-fire entity={:?} notches={} radians={:.3} pre_facing=({:.2},{:.2}) post_facing=({:.2},{:.2}) (on-anim-end)",
                msg.entity,
                notches,
                radians,
                pre.x, pre.z,
                fighter.facing.x, fighter.facing.z,
            );

            // Mirror onto Transform + avian Rotation now so any reader
            // that hits Transform before fighter_rotation_sync_system
            // (which runs later this same FixedUpdate) sees the new
            // rotation.  Formula matches `fighter_rotation_sync_system`.
            if fighter.facing.length_squared() > 0.001 {
                let dir = fighter.facing.normalize();
                if let Some(mut transform) = transform_opt {
                    transform.look_to(dir, Vec3::Y);
                    transform.rotate_y(std::f32::consts::PI);
                    if let Some(mut phys_rot) = avian_rot_opt {
                        phys_rot.0 = transform.rotation;
                    }
                } else if let Some(mut phys_rot) = avian_rot_opt {
                    let mut tmp = Transform::IDENTITY;
                    tmp.look_to(dir, Vec3::Y);
                    tmp.rotate_y(std::f32::consts::PI);
                    phys_rot.0 = tmp.rotation;
                }
            }
        } else {
            // No Fighter — rotate Transform directly.
            if let Some(mut transform) = transform_opt {
                transform.rotation = rotation * transform.rotation;
                if let Some(mut phys_rot) = avian_rot_opt {
                    phys_rot.0 = transform.rotation;
                }
            }
            info!(
                "STRIKE_DEBUG: end-rotation-notches-fire entity={:?} notches={} radians={:.3} (no Fighter, Transform-only) (on-anim-end)",
                msg.entity, notches, radians,
            );
        }
    }
}

/// Cylinder-slice overlap hit detection reading from `.atdt` files embedded in Oni2AnimState.
/// Also checks FighterState + BlockLibrary to handle block interception:
///   - Blocked hit → emits BlockSuccessEvent / BlockFailedEvent, zero damage
///   - Unblocked hit → InjureMessage as normal
pub fn hit_detection_system(
    mut commands: Commands,
    mut attackers: Query<(
        Entity,
        &Transform,
        &Fighter,
        &mut AttackState,
        &Oni2AnimState,
    )>,
    mut targets: Query<(
        Entity,
        &Transform,
        &mut Health,
        &Fighter,
        Option<&ReactLibrary>,
        Option<&FighterState>,
        Option<&BlockLibrary>,
        Option<&Oni2AnimState>,
    )>,
    time: Res<Time>,
    mut damage_writer: MessageWriter<DamageMessage>,
    mut injure_writer: MessageWriter<InjureMessage>,
    mut block_success_writer: MessageWriter<BlockSuccessEvent>,
    _block_failed_writer: MessageWriter<BlockFailedEvent>,
    mut strike_connected_writer: MessageWriter<StrikeConnectedEvent>,
    query_inventory: Query<&crate::inventory::components::Inventory>,
    query_weapon: Query<&crate::weapons::components::Weapon>,
    query_global_transform: Query<&GlobalTransform>,
) {
    let now = time.elapsed_secs_f64();

    for (attacker_entity, attacker_tf, attacker_fighter, mut attack_state, anim_state) in
        &mut attackers
    {
        let Some(attack_data) = &anim_state.anim.attack_data else {
            continue;
        };
        let Some(strike) = &attack_data.strike else {
            continue;
        };

        if anim_state.anim.num_frames <= 1 {
            continue;
        }

        let frame = anim_state.current_time;
        let is_active = if strike.frameduration > 0.0 {
            frame >= strike.framenum && frame <= strike.framenum + strike.frameduration
        } else {
            true
        };
        if !is_active {
            continue;
        }

        let phase = if anim_state.anim.num_frames > 1 {
            frame / (anim_state.anim.num_frames as f32 - 1.0).max(1.0)
        } else {
            0.0
        };

        // --- Weapon Fire ---
        if strike.fire {
            // Check if we haven't already fired this attack cycle
            let active_attack = attack_state
                .active_attack
                .get_or_insert_with(ActiveAttack::default);
            if !active_attack.has_fired_projectile {
                let mut weapon_name_to_fire = "default".to_string();
                let mut muzzle_bevy_pos = None;
                let mut muzzle_bevy_dir: Option<Vec3> = None;

                if let Ok(inv) = query_inventory.get(attacker_entity) {
                    if let Some(weap_ent) = inv.current_weapon_entity() {
                        if let Ok(weap_tf) = query_global_transform.get(weap_ent) {
                            if let Ok(weap) = query_weapon.get(weap_ent) {
                                if !weap.ty.op_modes.is_empty() && !weap.ty.op_modes[0].first_state.projectiles.is_empty() {
                                    weapon_name_to_fire = weap.ty.op_modes[0].first_state.projectiles[0].projectile_name.clone();

                                    // Spawn at the weapon entity's world origin
                                    // (the grip point on the hand) — skipping the
                                    // un-converted Oni2-space muzzle_offset for
                                    // now.  Direction comes from the weapon's
                                    // OWN world forward so side-shots, high/low
                                    // shots, etc. actually match the animated
                                    // gun pose.  The legacy mesh loader negates
                                    // X and Z on vertex positions
                                    // (oni2_loader/parsers/mesh.rs:106), which
                                    // effectively rotates the weapon model 180°
                                    // around Y — so what was +Z (Oni2 barrel
                                    // forward) becomes -Z in Bevy's mesh-local
                                    // frame, and `weapon_rot * Vec3::NEG_Z` is
                                    // the intuitive "forward".  Empirically
                                    // that fires backwards; the mesh must be
                                    // imported with the barrel along +Z in the
                                    // weapon entity's local frame (likely the
                                    // grip_rot/bone basis eats an extra flip).
                                    // Use +Z — bullets now match the visible
                                    // gun direction, including side-shots.
                                    muzzle_bevy_pos = Some(weap_tf.translation());
                                    let weapon_rot = weap_tf.compute_transform().rotation;
                                    muzzle_bevy_dir =
                                        Some((weapon_rot * Vec3::Z).normalize_or_zero());
                                }
                            }
                        }
                    }
                }

                let legacy_spawn_pos = attacker_tf.translation + Vec3::Y * 1.5;
                let actual_spawn_pos = muzzle_bevy_pos.unwrap_or(legacy_spawn_pos);
                let actual_spawn_dir = muzzle_bevy_dir
                    .unwrap_or_else(|| attacker_fighter.facing.normalize_or_zero());

                commands.trigger(SpawnProjectileEvent {
                    name: weapon_name_to_fire,
                    position: actual_spawn_pos,
                    velocity: actual_spawn_dir * 30.0,
                    owner: attacker_entity,
                    team: 0, // Fallback team since team system isn't ported
                });
                active_attack.has_fired_projectile = true;
            }
            continue; // Fire strikes don't process melee swept cylinders
        }

        let attacker_forward = attacker_fighter.facing;
        let active_attack = attack_state
            .active_attack
            .get_or_insert_with(ActiveAttack::default);

        let attacker_feet_y = attacker_tf.translation.y - CAPSULE_CENTER_HEIGHT;
        let attacker_base = Vec3::new(attacker_tf.translation.x, attacker_feet_y, attacker_tf.translation.z);

        let wedge = crate::combat::hitbox::EvaluatedWedge::evaluate(
            strike,
            attacker_base,
            attacker_forward,
            frame,
            anim_state.anim.num_frames as f32,
        );

        for (
            target_entity,
            target_tf,
            health,
            target_fighter,
            _react_lib,
            target_fs_opt,
            block_lib_opt,
            target_anim_opt,
        ) in &mut targets
        {
            if target_entity == attacker_entity {
                continue;
            }
            if active_attack.hit_entities.contains(&target_entity) {
                continue;
            }


            // Check FighterState phase-based invulnerability (crReactData.does_not_take_damage_after_phase)
            if let Some(fs) = target_fs_opt {
                if fs.in_invuln_phase {
                    continue;
                }
                // no_react_start_phase: if set, damage only accepted before this phase
                if fs.no_react_start_phase > 0.0
                    && let Some(tanim) = target_anim_opt
                    && tanim.anim.num_frames > 1
                {
                    let tphase = tanim.current_time / (tanim.anim.num_frames as f32 - 1.0).max(1.0);
                    if tphase >= fs.no_react_start_phase {
                        continue;
                    }
                }
            }

            let target_lo_y = target_tf.translation.y - CAPSULE_HALF_HEIGHT;
            let target_hi_y = target_tf.translation.y + CAPSULE_HALF_HEIGHT;

            if !wedge.contains_target(target_tf.translation, target_lo_y, target_hi_y) {
                continue;
            }

            // Wedge check done above; hit confirmed!

            // ── Hit confirmed ─────────────────────────────────────────────

            active_attack.hit_entities.push(target_entity);

            let react_enum = strike.reactanim[0];
            // Prefer ATDT-declared classification; fall back to hittype
            // heuristic when a given ATDT hasn't been authored with the
            // classes set.  The three ATDT fields are optional — we keep
            // the fallback so attacks still land, but once every ATDT in
            // the game has its classes filled in, the hittype fallback
            // can be removed and `attack_class` just reads the ATDT.
            let atdt_attack_class = attack_data.attack_class;
            let atdt_strength = attack_data.strength_class;
            let atdt_target = attack_data.target_class;
            let attack_class = atdt_attack_class.unwrap_or(match strike.hittype {
                0..=2 => AttackClass::Punch,
                3..=5 => AttackClass::Kick,
                6..=8 => AttackClass::Grab,
                _ => AttackClass::Punch,
            });
            let attack_strength = atdt_strength.unwrap_or(AttackStrength::High);

            // ── Block check ───────────────────────────────────────────────
            // Determine current block animation phase on the target.
            let block_result = block_lib_opt.and_then(|block_lib| {
                let fs = target_fs_opt?;
                if !fs.is_blocking() || fs.block_status == BlockStatus::NotBlocking {
                    return None;
                }
                // Compute the current phase of the target's block animation.
                let bphase = if let Some(tanim) = target_anim_opt {
                    if tanim.anim.num_frames > 1 {
                        tanim.current_time / (tanim.anim.num_frames as f32 - 1.0).max(1.0)
                    } else {
                        0.5 // assume mid-block if no frame data
                    }
                } else {
                    0.5
                };
                block_lib.find_block(
                    target_tf.translation,
                    target_fighter.facing,
                    attacker_tf.translation,
                    bphase,
                    strike.hittype,
                )
            });

            if let Some((block_idx, block_def)) = block_result {
                // ── Blocked ──────────────────────────────────────────────
                block_success_writer.write(BlockSuccessEvent {
                    attacker: attacker_entity,
                    blocker: target_entity,
                    block_index: block_idx,
                    counter_atk: block_def.counter_atk.clone(),
                    enemy_block_anim: block_def.enemy_block_anim,
                    block_reaction_on_attacker: block_def.block_reaction_on_attacker,
                    block_combo_count: target_fs_opt.map(|fs| fs.block_combo_count).unwrap_or(0),
                    combo_count_before_react: block_def.combo_count_before_react,
                });

                // Zero-damage DamageMessage so combo/telemetry still fire
                damage_writer.write(DamageMessage {
                    attacker: attacker_entity,
                    target: target_entity,
                    damage: 0.0,
                    was_blocked: true,
                    attack_class,
                    attack_strength: AttackStrength::Low,
                });

                info!(
                    "Blocked [{:?} -> {:?}]: block_idx={} hittype={}",
                    attacker_entity, target_entity, block_idx, strike.hittype
                );
            } else {
                // ── Not blocked ───────────────────────────────────────────
                let damage = attack_data.damage;

                // ── React-slot resolution + position-lock fields ──────────
                // Pick the first slot `i` where current phase ≤ reactphase[i];
                // fall back to slot 0 if none match.  Mirrors strike.cpp:311-314.
                let slot = {
                    let mut found = 0usize;
                    let mut any = false;
                    for i in 0..4 {
                        if phase <= strike.reactphase[i] {
                            found = i;
                            any = true;
                            break;
                        }
                    }
                    if any { found } else { 0 }
                };

                let face_with_react = strike.face_with_react[slot];
                let mode = strike.set_distance_mode[slot];
                let cfg_dist = strike.reactdistance[slot];

                // Flat XZ direction from attacker to target.
                let mut from_atk = target_tf.translation - attacker_tf.translation;
                from_atk.y = 0.0;
                let flat_dist = from_atk.length();
                let from_dir = if flat_dist > 1e-4 {
                    from_atk / flat_dist
                } else {
                    attacker_fighter.facing
                };

                use crate::oni2_loader::parsers::atdt::{
                    REACT_TRANSLATE_MODE_PUSH, REACT_TRANSLATE_MODE_SET,
                    REACT_TRANSLATE_MODE_TELEPORT,
                };
                let mut react_distance: Option<Vec3> = None;
                let mut teleport_to: Option<Vec3> = None;

                match mode {
                    REACT_TRANSLATE_MODE_SET => {
                        // Push so the final distance from attacker = cfg_dist.
                        // strike.cpp:479-481: dist-=flatDistToTarg; fallthrough into PUSH.
                        let dist = cfg_dist - flat_dist;
                        react_distance = Some(from_dir * dist);
                    }
                    REACT_TRANSLATE_MODE_PUSH => {
                        // strike.cpp:482-483: translate = from * cfg_dist.
                        react_distance = Some(from_dir * cfg_dist);
                    }
                    REACT_TRANSLATE_MODE_TELEPORT => {
                        // strike.cpp:485-502: teleport target to (attacker pos +
                        // rotated -Z distVec).  The slice-heading rotation
                        // requires per-strike state we don't track yet; for
                        // the common straight-ahead case, "in front of
                        // attacker at cfg_dist" matches the legacy intent.
                        let dest = attacker_tf.translation + attacker_fighter.facing * cfg_dist;
                        teleport_to = Some(Vec3::new(dest.x, target_tf.translation.y, dest.z));
                    }
                    _ => {
                        // Unknown mode: fall back to PUSH semantics.
                        react_distance = Some(from_dir * cfg_dist);
                    }
                }

                injure_writer.write(InjureMessage {
                    target: target_entity,
                    attacker: Some(attacker_entity),
                    damage,
                    hit_type: "strike".to_string(),
                    from: Some(attacker_tf.translation),
                    play_react: true,
                    disable_creature_detect: false,
                    attack_class: Some(attack_class),
                    attack_strength: Some(attack_strength),
                    attack_target: atdt_target,
                    strike_react_enum: Some(react_enum),
                    react_distance,
                    face_with_react,
                    teleport_to,
                });

                damage_writer.write(DamageMessage {
                    attacker: attacker_entity,
                    target: target_entity,
                    damage,
                    was_blocked: false,
                    attack_class,
                    attack_strength,
                });

                strike_connected_writer.write(StrikeConnectedEvent {
                    attacker: attacker_entity,
                    target: target_entity,
                    headingnotlockedtotarget: strike.headingnotlockedtotarget,
                });

                info!(
                    "Hit [{:?} -> {:?}]: damage={:.1} | react={} ({})",
                    attacker_entity,
                    target_entity,
                    damage,
                    react_enum,
                    ANIMREACT_NAMES
                        .get(react_enum.max(0) as usize)
                        .unwrap_or(&"?")
                );
            }
        }
    }
}

pub fn process_strike_connections_system(
    mut events: MessageReader<StrikeConnectedEvent>,
    mut fs_query: Query<&mut FighterState>,
) {
    for ev in events.read() {
        if let Ok(mut fs) = fs_query.get_mut(ev.attacker) {
            info!(
                "STRIKE_DEBUG: lock-set attacker={:?} target={:?} clear_st_after_first_use={} (headingnotlockedtotarget)",
                ev.attacker, ev.target, ev.headingnotlockedtotarget,
            );
            fs.strike_target = Some(ev.target);
            fs.clear_st_after_first_use = ev.headingnotlockedtotarget;
            // Dedup-add the hit target to this frame's pending list
            // (mirrors AddTargetPending in rb/src/fight/strike.cpp:630).
            // The list is cleared each frame in fighter_state_update_system,
            // so entries here are short-lived and cheap.
            fs.add_target_pending(ev.target);
        }
    }
}

/// About-to-be-hit system: reads messages and sets component on target.
/// Ticks down existing warnings and clears expired ones.
pub fn about_to_be_hit_system(
    mut reader: MessageReader<AboutToBeHitMessage>,
    mut query: Query<&mut AboutToBeHit>,
    time: Res<Time>,
) {
    // Tick down existing warnings
    for mut about in &mut query {
        if let Some(ref mut data) = about.active {
            data.eta -= time.delta_secs();
            if data.eta <= 0.0 {
                about.active = None;
            }
        }
    }

    // Apply new warnings
    for msg in reader.read() {
        if let Ok(mut about) = query.get_mut(msg.target) {
            about.active = Some(AboutToBeHitData {
                eta: msg.eta,
                hit_type: msg.hit_type,
                from: msg.from,
                attacker: msg.attacker,
            });
        }
    }
}

/// Applies generic injury logic (from strikes, explosions, hazards)
pub fn injure_system(
    mut events: MessageReader<InjureMessage>,
    mut query: Query<(
        &mut Health,
        Option<&mut Fighter>,
        Option<&mut Transform>,
        Option<&HitReaction>,
        Option<&Oni2AnimState>,
        Option<&crate::fight::FighterType>,
        Option<&mut FighterState>,
    )>,
    time: Res<Time>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
    mut commands: Commands,
    fx_registry: Res<crate::fight::AttackFxRegistry>,
) {
    let now = time.elapsed_secs_f64();

    for msg in events.read() {
        let Ok((
            mut health,
            mut fighter_opt,
            mut transform_opt,
            hit_reaction_opt,
            _anim_state_opt,
            fighter_type_opt,
            mut fighter_state_opt,
        )) = query.get_mut(msg.target)
        else {
            continue;
        };



        let is_env_hazard = msg.hit_type.eq_ignore_ascii_case("environmentalhazard");

        if is_env_hazard {
            // "not allowed to do environmental damage until you've finished reacting"
            if let Some(hr) = hit_reaction_opt
                && hr.active.is_some()
            {
                continue;
            }
        }

        let damage = if is_env_hazard {
            msg.damage * time.delta_secs()
        } else {
            msg.damage
        };

        let last_hp = health.current;
        health.current = (health.current - damage).max(0.0);

        let mut reacting_to_hit_from_behind = false;
        let mut hit_from_behind = false;

        if let Some(ref mut fighter) = fighter_opt {
            fighter.facing = fighter.facing; // Trigger mut access

            if let Some(from_pos) = msg.from
                && let Some(tf) = &transform_opt
            {
                let mut diff = from_pos - tf.translation;
                diff.y = 0.0;
                if diff.length_squared() > 0.001 {
                    let to_attack = diff.normalize_or_zero();
                    let my_forward = fighter.facing;
                    let angle = my_forward.dot(to_attack).clamp(-1.0, 1.0).acos();
                    if angle > 100.0f32.to_radians() {
                        hit_from_behind = true;
                    }
                }
            }

            if fighter.throttle > 0.65 && hit_from_behind {
                // If running and hit from behind, substitute override
                let react_strength = msg.attack_strength.unwrap_or(AttackStrength::Low);
                match react_strength {
                    AttackStrength::Low => {
                        // CR_ATTACK_STRENGTH_LOW -> ANIMREACT_FROMBACK_RUN_SFT
                        if msg.play_react {
                            reacting_to_hit_from_behind = true;
                            // 10 = ANIMREACT_FROMBACK_RUN_SFT
                        }
                    }
                    AttackStrength::High | AttackStrength::Super => {
                        if msg.play_react {
                            reacting_to_hit_from_behind = true;
                            // 11 = ANIMREACT_FROMBACK_RUN_HRD
                        }
                    }
                }
            }
        }

        // Face and React.
        //
        // C++ `sMakeTargetFace` (rb/src/fight/strike.cpp:235) snaps the
        // defender's yaw to face the attacker before the react plays.
        // Strict-fidelity gate is `strikeReact.FaceWithReact`
        // (strike.cpp:460-472), default false (attackdata.cpp:996),
        // because the legacy engine relies on the *attacker's*
        // per-frame `StrikeTarget` rotation (fighter.cpp:1507-1532) AND
        // the *defender's* AI face-tracking behaviour to keep the
        // bodies aligned even without an explicit snap.  We don't have
        // AI face-tracking ported yet, so honour the `face_with_react`
        // opt-in **and** snap unconditionally for any play-react path —
        // otherwise nothing aligns the defender during the react and
        // shipped ATDTs (which all set `facewithreact*=0`) drift apart.
        // TODO: remove the unconditional fallback once AI face-tracking
        // is in place; at that point only the legacy opt-in should snap.
        let do_face_snap = msg.play_react;
        let _strict_face = msg.face_with_react; // kept for future strict-fidelity mode
        if do_face_snap
            && let Some(from_pos) = msg.from
            && let Some(ref mut fighter) = fighter_opt
            && let Some(ref mut tf) = transform_opt
        {
            let mut to_attacker = (from_pos - tf.translation).normalize_or_zero();
            to_attacker.y = 0.0;
            if to_attacker.length_squared() > 0.001 {
                if reacting_to_hit_from_behind {
                    to_attacker = -to_attacker;
                }
                fighter.facing = to_attacker;
            }
        }

        // Teleport mode — strike.cpp:485-502 broadcasts an aMsgTeleport;
        // here we just snap the defender's transform.  Done before the
        // react anim begins so the anim plays at the destination.
        if msg.play_react
            && let Some(dest) = msg.teleport_to
            && let Some(ref mut tf) = transform_opt
        {
            tf.translation.x = dest.x;
            tf.translation.z = dest.z;
        }

        // Stash the per-react displacement vector for
        // `react_distance_apply_system` to consume over the react anim.
        // Mirrors `SetReactDistance(translateDist)` at strike.cpp:513.
        if msg.play_react
            && let Some(ref mut fs) = fighter_state_opt
        {
            fs.react_distance = msg.react_distance.unwrap_or(Vec3::ZERO);
        }

        if health.current <= 0.0 && last_hp > 0.0 {
            // Handled mostly by death_system, but could dispatch custom death anim here
        }

        if msg.play_react {
            let mut react_enum = msg.strike_react_enum.unwrap_or(0); // 0 = Generic Flinch
            if reacting_to_hit_from_behind {
                let react_strength = msg.attack_strength.unwrap_or(AttackStrength::Low);
                react_enum = match react_strength {
                    AttackStrength::Low => 10,
                    _ => 11,
                };
            }

            let kind = match react_enum {
                4 => ReactionKind::Knockdown,
                46 | 47 => ReactionKind::Knockback,
                _ => ReactionKind::Flinch,
            };

            let knockback_dir = if let (Some(from_pos), Some(tf)) = (msg.from, &transform_opt) {
                (tf.translation - from_pos).normalize_or_zero()
            } else {
                Vec3::X
            };

            reaction_writer.write(HitReactionMessage {
                entity: msg.target,
                kind,
                direction: knockback_dir,
                react_enum,
            });

            // Queue scream sound! (routed through fx_system's audio dispatch).
            commands.trigger(crate::fx_system::PlaySound {
                script_entity: msg.target,  // using the struck entity as originator
                actor: None,                // implicit on same entity
                name: "scream".to_string(), // fallback label if actor is missing an exact mapping
            });

            // FX-table impact lookup.  Target's FighterType (if present)
            // picks the per-character table; fall back to "default".
            // All three classifications (target/strength/class) must be
            // present — when any is `None` we skip, matching the legacy
            // "THE FOLLOWING ATDT FILES NEED TO HAVE TargetClass,
            // StrengthClass, and/or AttackClass SETUP" diagnostic
            // (rb/src/fight/attackdata.cpp:258).
            if let (Some(class), Some(strength), Some(target)) =
                (msg.attack_class, msg.attack_strength, msg.attack_target)
            {
                let table_name: &str = fighter_type_opt
                    .map(|ft| ft.name.as_str())
                    .unwrap_or("default");
                let hit_pos = transform_opt.as_ref().map(|tf| tf.translation);
                let lookup = fx_registry.lookup(table_name, target, strength, class);
                info!(
                    "combat: hit dispatch table='{}' target={:?} strength={:?} class={:?} hit_pos={:?} found={}",
                    table_name,
                    target,
                    strength,
                    class,
                    hit_pos,
                    lookup.is_some()
                );
                if let Some(fx) = lookup {
                    fx.dispatch(&mut commands, hit_pos, Some(msg.target), msg.target);
                }
            } else {
                info!(
                    "combat: hit dispatch SKIPPED — missing classification (class={:?} strength={:?} target={:?})",
                    msg.attack_class, msg.attack_strength, msg.attack_target
                );
            }

            // Spawn the legacy HealthIndicator
            if let Some(tf) = transform_opt.as_ref() {
                commands.trigger(crate::fx_system::SpawnFx {
                    name: "HealthIndicator".to_string(),
                    at: Some(tf.translation + Vec3::new(0.0, 2.0, 0.0)),
                    parent: Some(msg.target),
                    start_active: true,
                });
            }
        }
    }
}

/// Hit reaction system: applies and ticks hit reactions.
/// Plays the correct ANIMREACT_* animation via the entity's Oni2AnimLibrary,
/// and clears the reaction when the animation finishes (timer fallback if no anim).
pub fn hit_reaction_system(
    mut reader: MessageReader<HitReactionMessage>,
    mut query: Query<(
        &mut HitReaction,
        &mut LinearVelocity,
        Option<&mut Oni2AnimState>,
        Option<&Oni2AnimLibrary>,
    )>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();

    // Tick existing reactions — clear when react anim finishes, or fallback timer.
    for (mut reaction, _vel, anim_state_opt, _lib) in &mut query {
        let Some(ref mut active) = reaction.active else {
            continue;
        };

        let mut done = false;
        if let Some(ref anim_state) = anim_state_opt {
            // If the currently playing anim matches what we triggered and it has reached its
            // last frame (non-looping), the react is complete.
            if active.react_anim_id != 0
                && anim_state.current_anim_id == Some(AnimId(active.react_anim_id))
                && !anim_state.looping
            {
                let last = (anim_state.anim.num_frames as f32 - 1.0).max(0.0);
                if anim_state.current_time >= last {
                    done = true;
                }
            }
        }

        if !done {
            active.elapsed += dt;
            if active.elapsed >= active.duration {
                done = true;
            }
        }

        if done {
            reaction.active = None;
        }
    }

    // Apply new reactions: play animation + physics impulse.
    for msg in reader.read() {
        let Ok((mut reaction, mut velocity, mut anim_state_opt, lib_opt)) =
            query.get_mut(msg.entity)
        else {
            continue;
        };

        let mut active = ActiveReaction::new(msg.kind, msg.direction, msg.react_enum);

        // Play the ANIMREACT_* animation and record its id for completion detection.
        if let (Some(ref mut anim_state), Some(lib)) = (anim_state_opt.as_mut(), lib_opt)
            && msg.react_enum >= 0
            && let Some(&anim_name) = ANIMREACT_NAMES.get(msg.react_enum as usize)
        {
            if lib.play(anim_name, anim_state) {
                active.react_anim_id = AnimId::new(anim_name).0;
            } else {
                warn!("hit_reaction: react anim '{}' not in library", anim_name);
            }
        }

        reaction.active = Some(active);

        // Physics impulse
        let knockback_dir = Vec3::new(msg.direction.x, 0.0, msg.direction.z).normalize_or_zero();
        match msg.kind {
            ReactionKind::Knockback => {
                let impulse = knockback_dir * 8.0 + Vec3::Y * 2.0;
                velocity.x += impulse.x;
                velocity.y += impulse.y;
                velocity.z += impulse.z;
            }
            ReactionKind::Knockdown => {
                let impulse = knockback_dir * 12.0 + Vec3::Y * 4.0;
                velocity.x += impulse.x;
                velocity.y += impulse.y;
                velocity.z += impulse.z;
            }
            ReactionKind::Flinch => {
                let impulse = knockback_dir * 3.0;
                velocity.x += impulse.x;
                velocity.z += impulse.z;
            }
        }
    }
}

/// Increments combo counter on successive hits within the combo window.
pub fn combo_tracking_system(
    mut reader: MessageReader<DamageMessage>,
    mut combo_query: Query<&mut ComboTracker>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for msg in reader.read() {
        let Ok(mut combo) = combo_query.get_mut(msg.attacker) else {
            continue;
        };

        if now - combo.last_hit_time <= combo.combo_window {
            combo.hit_count += 1;
        } else {
            combo.hit_count = 1;
        }
        combo.last_hit_time = now;
    }
}

/// Checks for dead entities and emits DeathMessages plus a one-shot
/// `PlayDieMessage` that drives the death animation through the action
/// dispatcher.  The die_enum chosen mirrors legacy `ActionStartDie`
/// (rb/src/animator/action.cpp:1325-1326): the killing blow's react if
/// it has `CanBeDieAnimation = true`, otherwise `ANIMDIE_GENERAL`.
pub fn death_system(
    health_query: Query<(Entity, &Health), Changed<Health>>,
    fighter_query: Query<(Option<&FighterState>, Option<&ReactLibrary>)>,
    mut death_writer: MessageWriter<DeathMessage>,
    mut die_writer: MessageWriter<crate::animator::PlayDieMessage>,
) {
    for (entity, health) in &health_query {
        if health.current > 0.0 {
            continue;
        }
        death_writer.write(DeathMessage {
            entity,
            killer: Entity::PLACEHOLDER,
        });

        // Pick the death anim enum.  Without a ReactLibrary or last-react
        // record we still fall back to ANIMDIE_GENERAL — the action
        // dispatcher will simply attempt the alias and skip if missing.
        let (fs_opt, lib_opt) = fighter_query.get(entity).unwrap_or((None, None));
        let killing_react = fs_opt.map(|fs| fs.react_anim).unwrap_or(-1);
        let die_enum = lib_opt
            .map(|lib| lib.pick_die_enum(killing_react))
            .unwrap_or(crate::oni2_loader::parsers::rct::ANIMDIE_GENERAL_INDEX);
        die_writer.write(crate::animator::PlayDieMessage { entity, die_enum });
    }
}

/// Reads DeathMessages and triggers despawns or delayed death timers based on DestroyOnDeath.
pub fn death_cleanup_system(
    mut commands: Commands,
    mut reader: MessageReader<DeathMessage>,
    query: Query<Option<&DestroyOnDeath>>,
) {
    for msg in reader.read() {
        if let Ok(dod) = query.get(msg.entity) {
            if let Some(delay) = dod {
                if delay.0 <= 0.0 {
                    commands.entity(msg.entity).try_despawn();
                } else {
                    commands
                        .entity(msg.entity)
                        .insert(DeathSequenceTimer(Timer::from_seconds(
                            delay.0,
                            TimerMode::Once,
                        )));
                }
            } else {
                info!(
                    "Entity {} has no DestroyOnDeath component, despawning immediately",
                    msg.entity
                );
                commands.entity(msg.entity).try_despawn();
            }
        }
    }
}

/// Ticks death sequence timers and despawns entities when they finish.
pub fn death_timer_system(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut DeathSequenceTimer)>,
) {
    for (entity, mut timer) in &mut query {
        if timer.0.tick(time.delta()).just_finished() {
            info!("Entity {} death timer finished, despawning", entity);
            commands.entity(entity).try_despawn();
        }
    }
}

/// Sends combat events to the telemetry channel.
pub fn telemetry_combat_system(
    mut damage_reader: MessageReader<DamageMessage>,
    mut death_reader: MessageReader<DeathMessage>,
    fighter_ids: Query<(&FighterId, &Transform)>,
    combo_query: Query<&ComboTracker>,
    channel: Res<TelemetryChannel>,
) {
    for msg in damage_reader.read() {
        let attacker_id = fighter_ids
            .get(msg.attacker)
            .map(|(id, _)| id.0)
            .unwrap_or(uuid::Uuid::nil());
        let (target_id, pos) = fighter_ids
            .get(msg.target)
            .map(|(id, tf)| (id.0, tf.translation))
            .unwrap_or((uuid::Uuid::nil(), Vec3::ZERO));
        let combo_count = combo_query
            .get(msg.attacker)
            .map(|c| c.hit_count)
            .unwrap_or(0);

        let event = CombatEvent::damage(
            attacker_id,
            target_id,
            msg.damage,
            msg.was_blocked,
            combo_count,
            msg.attack_class.name(),
            [pos.x, pos.y, pos.z],
        );
        let _ = channel.sender.send(event);
    }

    for msg in death_reader.read() {
        let (target_id, pos) = fighter_ids
            .get(msg.entity)
            .map(|(id, tf)| (id.0, tf.translation))
            .unwrap_or((uuid::Uuid::nil(), Vec3::ZERO));
        let killer_id = fighter_ids
            .get(msg.killer)
            .map(|(id, _)| id.0)
            .unwrap_or(uuid::Uuid::nil());

        let event = CombatEvent::death(target_id, killer_id, [pos.x, pos.y, pos.z]);
        let _ = channel.sender.send(event);
    }
}

pub fn ground_detection_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &mut crate::oni2_loader::animation::Oni2AnimState,
        &ShapeHits,
        Option<&mut crate::oni2_loader::spawn::JustGroundSnapped>,
        Option<&crate::oni2_loader::spawn::NeedsGroundSnap>,
    )>,
    materials: Query<&crate::oni2_loader::components::MaterialType>,
) {
    for (entity, mut anim_state, hits, mut snap_opt, needs_snap_opt) in &mut query {
        if let Some(mut snap) = snap_opt {
            if snap.0 > 0 {
                snap.0 -= 1;
                anim_state.is_grounded = true;
                anim_state.material_stood_on = None; // Avoid legacy material until physics hits
                continue;
            } else {
                commands.entity(entity).remove::<crate::oni2_loader::spawn::JustGroundSnapped>();
            }
        }

        if needs_snap_opt.is_some() {
            anim_state.is_grounded = true;
            anim_state.material_stood_on = None;
            continue;
        }

        anim_state.is_grounded = !hits.is_empty();
        if let Some(first_hit) = hits.first() {
            if let Ok(material_type) = materials.get(first_hit.entity) {
                anim_state.material_stood_on = Some(material_type.0.clone());
            } else {
                anim_state.material_stood_on = None;
            }
        } else {
            anim_state.material_stood_on = None;
        }
    }
}

// ---------------------------------------------------------------------------
// fighter_rotation_sync_system
// ---------------------------------------------------------------------------

/// Central authority for rotating characters. Ensures that `Fighter.facing` is the
/// single source of truth for the entity's Y-axis orientation, applying it to both
/// the visual `Transform` and Avian's physics `Rotation` component to prevent the
/// physics engine from reverting rotation changes made in FixedUpdate.
pub fn fighter_rotation_sync_system(
    mut query: Query<(
        Entity,
        &crate::combat::components::Fighter,
        &mut Transform,
        Option<&mut avian3d::prelude::Rotation>,
    )>,
) {
    for (_entity, fighter, mut transform, rot_opt) in &mut query {
        if fighter.facing.length_squared() > 0.001 {
            // Bevy's look_to aligns -Z with the target direction.
            // Oni2 models inherently face local +Z. We must rotate 180 degrees
            // to get the mesh's face (+Z) pointing along fighter.facing.
            let dir = fighter.facing.normalize();
            transform.look_to(dir, Vec3::Y);
            transform.rotate_y(std::f32::consts::PI);
            
            if let Some(mut phys_rot) = rot_opt {
                phys_rot.0 = transform.rotation;
            }
        }
    }
}
