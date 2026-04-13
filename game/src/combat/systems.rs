use avian3d::prelude::*;
use bevy::prelude::*;
use rb_shared::events::CombatEvent;

use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::oni2_loader::parsers::rct::ANIMREACT_NAMES;
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
pub fn attack_sync_system(
    mut query: Query<(&mut AttackState, &crate::oni2_loader::animation::Oni2AnimState)>,
) {
    for (mut attack_state, anim_state) in &mut query {
        if anim_state.current_anim_id != anim_state.previous_anim_id {
            // New animation started! Clear the hit list.
            if let Some(ref mut active) = attack_state.active_attack {
                active.hit_entities.clear();
            } else {
                attack_state.active_attack = Some(ActiveAttack::default());
            }
        }
    }
}

/// Cylinder-slice overlap hit detection reading from `.atdt` files embedded in Oni2AnimState.
pub fn hit_detection_system(
    mut attackers: Query<(Entity, &Transform, &Fighter, &mut AttackState, &crate::oni2_loader::animation::Oni2AnimState)>,
    mut targets: Query<(Entity, &Transform, &mut Health, &Fighter, Option<&ReactLibrary>)>,
    time: Res<Time>,
    mut damage_writer: MessageWriter<DamageMessage>,
    mut reaction_writer: MessageWriter<HitReactionMessage>,
) {
    let now = time.elapsed_secs_f64();

    for (attacker_entity, attacker_tf, attacker_fighter, mut attack_state, anim_state) in &mut attackers {
        let Some(attack_data) = &anim_state.anim.attack_data else {
            continue;
        };
        let Some(strike) = &attack_data.strike else {
            continue;
        };

        if anim_state.anim.num_frames <= 1 {
            continue;
        }

        // Hit window: framenum..framenum+frameduration are raw frame numbers matching current_time.
        // (minradiusframe/maxradiusframe describe the disk-radius ramp-up, not the hit window.)
        let frame = anim_state.current_time;
        let is_active = if strike.frameduration > 0.0 {
            frame >= strike.framenum && frame <= strike.framenum + strike.frameduration
        } else {
            // No explicit hit window — active throughout the animation.
            true
        };
        if !is_active {
            continue;
        }

        // Fighter.facing is the canonical world-space facing direction for both player and AI.
        // (player sets it from transform.forward(), AI sets it from dir_to_target)
        let attacker_forward = attacker_fighter.facing;

        // Get or insert active attack (cleared by attack_sync_system on new anims)
        let active_attack = attack_state.active_attack.get_or_insert_with(ActiveAttack::default);

        for (target_entity, target_tf, mut health, target_fighter, react_lib) in &mut targets {
            if target_entity == attacker_entity {
                continue;
            }

            if active_attack.hit_entities.contains(&target_entity) {
                continue;
            }

            if now < health.invulnerable_until {
                continue;
            }

            let diff = target_tf.translation - attacker_tf.translation;

            // XZ distance check
            let dist_sq = diff.x * diff.x + diff.z * diff.z;
            if dist_sq > strike.reactdiskradius * strike.reactdiskradius {
                debug!(
                    "hit miss: dist {:.2} > radius {:.2}",
                    dist_sq.sqrt(), strike.reactdiskradius
                );
                continue;
            }

            // Height check: target's capsule Y extent must overlap the disk's world-space Y band.
            // Disk is at attacker's feet + reactdiskheight, ±reactdiskheighttolerance.
            // Matches rb attackdata.cpp TestHit: [ReactDiskHeight ± Tolerance + chrY] vs [tgtLoY, tgtHiY].
            let attacker_feet_y = attacker_tf.translation.y - CAPSULE_CENTER_HEIGHT;
            let disk_min_y = attacker_feet_y + strike.reactdiskheight - strike.reactdiskheighttolerance;
            let disk_max_y = attacker_feet_y + strike.reactdiskheight + strike.reactdiskheighttolerance;
            let target_lo_y = target_tf.translation.y - CAPSULE_HALF_HEIGHT;
            let target_hi_y = target_tf.translation.y + CAPSULE_HALF_HEIGHT;

            if target_lo_y > disk_max_y || target_hi_y < disk_min_y {
                debug!(
                    "hit miss: target Y [{:.2}, {:.2}] misses disk Y [{:.2}, {:.2}]",
                    target_lo_y, target_hi_y, disk_min_y, disk_max_y
                );
                continue;
            }

            // Slice angular check
            let dir_to_target = Vec3::new(diff.x, 0.0, diff.z).normalize_or_zero();

            // sliceheadingradiansb offsets the slice center relative to the attacker's facing.
            // Match the debug renderer: Oni negates the Y rotation so positive angles swing left (-X).
            let slice_heading = Quat::from_rotation_y(-strike.sliceheadingradiansb) * attacker_forward;
            let slice_heading_xz = Vec3::new(slice_heading.x, 0.0, slice_heading.z).normalize_or_zero();

            let dot = slice_heading_xz.dot(dir_to_target);
            let angle = dot.clamp(-1.0, 1.0).acos();
            let cross_y = slice_heading_xz.cross(dir_to_target).y;
            // If cross_y > 0, target is to the right (+X), which represents a negative angle.
            // If cross_y < 0, target is to the left (-X), which represents a positive angle.
            let signed_angle = if cross_y > 0.0 { -angle } else { angle };

            if signed_angle < strike.slicestartradians || signed_angle > strike.sliceendradians {
                debug!(
                    "hit miss: angle {:.3}rad outside [{:.3}, {:.3}]",
                    signed_angle, strike.slicestartradians, strike.sliceendradians
                );
                continue;
            }

            // Hit confirmed!
            active_attack.hit_entities.push(target_entity);

            let damage = attack_data.damage;
            let was_blocked = false;

            // Resolve react data from the target's ReactLibrary using reactanim0
            // (reactanim[0] is the primary reaction; higher indices are alternatives for different states)
            let react_enum = strike.reactanim[0];
            let react = react_lib.and_then(|lib| lib.get(react_enum));

            if !was_blocked {
                health.current = (health.current - damage).max(0.0);

                // Invulnerability: use react's InvulnerabilityStartPhase if available,
                // scaled to a time estimate (react anim duration unknown here — use INVULN_DURATION as floor).
                // TODO: once react anims are played via Oni2AnimState, derive duration from num_frames.
                let invuln_secs = if let Some(rct) = react {
                    // invulnerability_start_phase is 0..1 of the react anim; use flat minimum for now
                    INVULN_DURATION.max(rct.invulnerability_start_phase as f64 * 0.5)
                } else {
                    INVULN_DURATION
                };
                health.invulnerable_until = now + invuln_secs;

                let knockback_dir = (target_tf.translation - attacker_tf.translation).normalize_or_zero();

                // Classify reaction kind from react enum (knockdown vs flinch vs standard)
                let kind = match react_enum {
                    4 => ReactionKind::Knockdown,   // ANIMREACT_KNOCKDOWN
                    46 | 47 => ReactionKind::Knockback, // ANIMREACT_WALL / WALL_GETUP
                    _ => ReactionKind::Flinch,
                };

                reaction_writer.write(HitReactionMessage {
                    entity: target_entity,
                    kind,
                    direction: knockback_dir,
                    react_enum,
                });

                info!(
                    "Hit [{:?} -> {:?}]: damage={:.1} | remaining_health={:.1} | react={} ({})",
                    attacker_entity,
                    target_entity,
                    damage,
                    health.current,
                    react_enum,
                    crate::oni2_loader::parsers::rct::ANIMREACT_NAMES
                        .get(react_enum.max(0) as usize)
                        .unwrap_or(&"?")
                );
            }

            // Map hittype to AttackClass for the event
            let attack_class = match strike.hittype {
                0 | 1 | 2 => AttackClass::Punch,
                3 | 4 | 5 => AttackClass::Kick,
                6 | 7 | 8 => AttackClass::Grab,
                _ => AttackClass::Punch,
            };

            damage_writer.write(DamageMessage {
                attacker: attacker_entity,
                target: target_entity,
                damage: if was_blocked { 0.0 } else { damage },
                was_blocked,
                attack_class,
                attack_strength: AttackStrength::High,
            });
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
        let Some(ref mut active) = reaction.active else { continue };

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
        if let (Some(ref mut anim_state), Some(lib)) = (anim_state_opt.as_mut(), lib_opt) {
            if msg.react_enum >= 0 {
                if let Some(&anim_name) = ANIMREACT_NAMES.get(msg.react_enum as usize) {
                    if lib.play(anim_name, anim_state) {
                        active.react_anim_id = AnimId::new(anim_name).0;
                    } else {
                        warn!("hit_reaction: react anim '{}' not in library", anim_name);
                    }
                }
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
                    commands.entity(msg.entity).insert(DeathSequenceTimer(Timer::from_seconds(delay.0, TimerMode::Once)));
                }
            } else {
                info!("Entity {} has no DestroyOnDeath component, despawning immediately", msg.entity);
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

/// Updates Fighter.is_grounded based on ShapeCaster ground detection.
pub fn ground_detection_system(mut query: Query<(&mut Fighter, &ShapeHits)>) {
    for (mut fighter, hits) in &mut query {
        fighter.is_grounded = !hits.is_empty();
    }
}
