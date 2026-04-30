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
use crate::fight::events::{ApplyRotationNotchesEvent, BlockFailedEvent, BlockSuccessEvent};
use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::rct::ANIMREACT_NAMES;
use crate::projectile_system::SpawnProjectileEvent;
use crate::telemetry::bridge::TelemetryChannel;

use super::components::*;
use super::events::*;

const INVULN_DURATION: f64 = 0.2;
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
    )>,
    mut rotation_writer: MessageWriter<ApplyRotationNotchesEvent>,
) {
    for msg in reader.read() {
        let Ok((mut attack_state, anim_state, ap_opt)) = query.get_mut(msg.entity) else {
            continue;
        };
        let mut has_fire = false;

        // New animation started!
        if let Some(ref mut active) = attack_state.active_attack {
            // Apply end_rotation_notches from the PREVIOUS attack if any
            if active.end_rotation_notches != 0 {
                rotation_writer.write(ApplyRotationNotchesEvent {
                    entity: msg.entity,
                    notches: active.end_rotation_notches,
                });
            }
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
            if now < health.invulnerable_until {
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
                let _knockback_dir =
                    (target_tf.translation - attacker_tf.translation).normalize_or_zero();

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
        )) = query.get_mut(msg.target)
        else {
            continue;
        };

        if now < health.invulnerable_until {
            continue;
        }

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

        // Face and React
        if let Some(from_pos) = msg.from
            && let Some(ref mut fighter) = fighter_opt
            && msg.play_react
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
                if let Some(fx) = fx_registry.lookup(table_name, target, strength, class) {
                    fx.dispatch(&mut commands, hit_pos, Some(msg.target), msg.target);
                }
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

/// Checks for dead entities and emits DeathMessages.
pub fn death_system(
    query: Query<(Entity, &Health), Changed<Health>>,
    mut writer: MessageWriter<DeathMessage>,
) {
    for (entity, health) in &query {
        if health.current <= 0.0 {
            writer.write(DeathMessage {
                entity,
                killer: Entity::PLACEHOLDER,
            });
        }
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
    )>,
    materials: Query<&crate::oni2_loader::components::MaterialType>,
) {
    for (entity, mut anim_state, hits, mut snap_opt) in &mut query {
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
            // We use Y-up coordinates, but the model inherently faces local +Z
            // (so it looks backwards natively). We must rotate such that its
            // local +Z points ALONG fighter.facing.
            let target_rot = Quat::from_rotation_arc(Vec3::Z, fighter.facing.normalize());
            transform.rotation = target_rot;

            if let Some(mut phys_rot) = rot_opt {
                phys_rot.0 = target_rot;
            }
        }
    }
}
