/*
 * weapons/systems.rs — FixedUpdate systems for the weapon subsystem.
 *
 * fire_weapon_system       — handle FireWeaponMessage (pull trigger)
 * stop_firing_system       — handle StopFiringMessage (release trigger)
 * set_op_mode_system       — handle SetOpModeMessage
 * aim_system               — handle AimMessage, update Weapon.aim
 * ammo_system              — handle ReloadMessage / AddAmmoMessage
 * accuracy_system          — handle SetShooterAccuracyMessage
 *
 * weapon_update_system     — per-tick: aim resolution + charge-state progression
 *                            + projectile/effect scheduled launch (mirrors
 *                            weapWeapon::Update + weapTimeLine::Update).
 *
 * Produces SpawnProjectileEvent into the existing projectile_system and emits
 * WeaponFiredMessage / WeaponChargeChangedMessage for downstream listeners.
 */
use bevy::prelude::*;

use crate::fx_system::SpawnFx;
use crate::projectile_system::SpawnProjectileEvent;

use super::components::{
    AimTarget, Aimer, AmmoRegistry, LaunchCursor, TrigHoldMode, Weapon, weapon_flags,
};
use super::events::{
    AddAmmoMessage, AimMessage, FireWeaponMessage, ReloadMessage, SetOpModeMessage,
    SetShooterAccuracyMessage, StopFiringMessage, WeaponChargeChangedMessage, WeaponFiredMessage,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rotate a direction by Euler angles (pitch/yaw/roll in radians).  Matches
/// the ONI Matrix34::RotateEulers → direction transform used by muzzle offsets.
fn dir_from_eulers(base_dir: Vec3, eulers: Vec3) -> Vec3 {
    let rot = Quat::from_euler(EulerRot::XYZ, eulers.x, eulers.y, eulers.z);
    (rot * base_dir).normalize_or_zero()
}

/// Compute a muzzle transform given the parent (weapon) matrix and a muzzle
/// offset/euler pair.  Returns (world position, world direction).
fn muzzle_world_transform(
    parent_pos: Vec3,
    parent_rot: Quat,
    muzzle_offset: Vec3,
    muzzle_eulers: Vec3,
) -> (Vec3, Vec3) {
    let world_pos = parent_pos + parent_rot * muzzle_offset;
    let local_dir = dir_from_eulers(Vec3::NEG_Z, muzzle_eulers);
    let world_dir = (parent_rot * local_dir).normalize_or_zero();
    (world_pos, world_dir)
}

fn get_target_world_position(
    target_entity: Entity,
    global_transforms: &Query<&GlobalTransform>,
    target_comps: &Query<&crate::combat::components::TargetComponent>,
    anim_states: &Query<&crate::oni2_loader::animation::Oni2AnimState>,
) -> Vec3 {
    if let Ok(target_comp) = target_comps.get(target_entity) {
        let offset = target_comp.target_offset;

        if let Some(bone_idx) = target_comp.bone_index {
            if let Ok(anim_state) = anim_states.get(target_entity) {
                if let Some(&joint_entity) = anim_state.joint_entities.get(bone_idx) {
                    if let Ok(joint_gt) = global_transforms.get(joint_entity) {
                        return joint_gt.transform_point(offset);
                    }
                }
            }
        }

        if let Ok(actor_gt) = global_transforms.get(target_entity) {
            return actor_gt.transform_point(offset);
        }
    }

    if let Ok(actor_gt) = global_transforms.get(target_entity) {
        return actor_gt.translation();
    }

    Vec3::ZERO
}

pub fn resolve_target_bone_system(
    mut target_q: Query<(
        &mut crate::combat::components::TargetComponent,
        &crate::oni2_loader::animation::Oni2AnimState,
    )>,
) {
    for (mut target_comp, anim_state) in &mut target_q {
        if target_comp.bone_index.is_none() {
            if let Some(parent_bone_name) = &target_comp.parent_bone {
                if let Some(idx) = anim_state
                    .skeleton
                    .names
                    .iter()
                    .position(|name| name == parent_bone_name)
                {
                    target_comp.bone_index = Some(idx);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Aimers
// ---------------------------------------------------------------------------

/// Resolve a straight-line aim — the weapon points directly at the target.
fn simple_aim(weapon_pos: Vec3, target_pos: Vec3) -> Vec3 {
    (target_pos - weapon_pos).normalize_or_zero()
}

/// Ballistic aim — compute firing angle to hit `target_pos` with `initial_speed`
/// under gravity.  Returns the direction vector to aim the weapon.  When no
/// closed-form solution exists (out of range), falls back to simple aim.
fn ballistic_aim(
    weapon_pos: Vec3,
    target_pos: Vec3,
    target_velocity: Vec3,
    initial_speed: f32,
    gravity: f32,
) -> Vec3 {
    if initial_speed <= 0.0 || gravity <= 0.0 {
        return simple_aim(weapon_pos, target_pos);
    }

    // Lead the target: solve for time-of-flight to a leading point.
    // Approximate by iterating once:  t ≈ |Δ| / speed,  then aim at target + v·t.
    let mut diff = target_pos - weapon_pos;
    let t_approx = diff.length() / initial_speed;
    let lead_pos = target_pos + target_velocity * t_approx;
    diff = lead_pos - weapon_pos;

    let dx = Vec3::new(diff.x, 0.0, diff.z).length();
    let dy = diff.y;
    let v2 = initial_speed * initial_speed;

    // Quadratic solution (low arc).
    let root_arg = v2 * v2 - gravity * (gravity * dx * dx + 2.0 * dy * v2);
    if root_arg < 0.0 {
        return simple_aim(weapon_pos, target_pos);
    }
    let tan_theta = (v2 - root_arg.sqrt()) / (gravity * dx.max(1e-4));
    let theta = tan_theta.atan();

    let horiz = Vec3::new(diff.x, 0.0, diff.z).normalize_or_zero();

    (horiz * theta.cos() + Vec3::Y * theta.sin()).normalize_or_zero()
}

/// Apply a Weapon's `aim` field, computing a firing direction given the
/// weapon's world transform.  Needs the wielder's current `initial_speed` for
/// ballistic solutions — pulled from the active op-mode's first projectile.
fn resolve_aim(
    weapon_pos: Vec3,
    fallback_forward: Vec3,
    aim: &AimTarget,
    aimer: Aimer,
    projectile_speed: f32,
    target_positions: &bevy::ecs::entity::EntityHashMap<Vec3>,
) -> Vec3 {
    let mut tgt = match aim {
        AimTarget::None => return fallback_forward,
        AimTarget::Point(p) => *p,
        AimTarget::Actor { target, .. } | AimTarget::Miss { target, .. } => {
            match target_positions.get(target) {
                Some(p) => *p,
                None => return fallback_forward,
            }
        }
    };

    // AimToMiss: offset the aim point by `dist_away` meters in a
    // horizontal direction perpendicular to the weapon-to-target line,
    // so the shot passes cleanly beside the target.  Legacy semantics
    // — `dist_away` is an absolute miss distance, authored per
    // AttackData for "scare shots" and AI-miss behaviour.
    if let AimTarget::Miss { dist_away, .. } = aim
        && *dist_away > 0.0
    {
        let to_target = tgt - weapon_pos;
        let horiz = Vec3::new(to_target.x, 0.0, to_target.z);
        let perp = Vec3::new(-horiz.z, 0.0, horiz.x);
        if perp.length_squared() > 1e-6 {
            tgt += perp.normalize() * *dist_away;
        }
    }

    match (aim, aimer) {
        (_, Aimer::Simple) => simple_aim(weapon_pos, tgt),
        (
            AimTarget::Actor {
                target_velocity, ..
            },
            Aimer::Ballistic,
        ) => ballistic_aim(weapon_pos, tgt, *target_velocity, projectile_speed, 9.81),
        (
            AimTarget::Miss {
                target_velocity, ..
            },
            Aimer::Ballistic,
        ) => ballistic_aim(weapon_pos, tgt, *target_velocity, projectile_speed, 9.81),
        _ => ballistic_aim(weapon_pos, tgt, Vec3::ZERO, projectile_speed, 9.81),
    }
}

// ---------------------------------------------------------------------------
// Inbound message handlers
// ---------------------------------------------------------------------------

pub fn fire_weapon_system(
    mut reader: MessageReader<FireWeaponMessage>,
    mut query: Query<&mut Weapon>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();

    for msg in reader.read() {
        let Ok(mut w) = query.get_mut(msg.entity) else {
            continue;
        };

        // Chamber-time gate
        if w.fire_time > now {
            continue;
        }

        w.set_flag(weapon_flags::TRIGGER_PULLED);
        w.clear_flag(weapon_flags::TRIGGER_RELEASED_AFTER_FIRING);

        if w.is_firing() {
            // Already firing — let the update loop continue the cycle.  In
            // FirstAndCharge modes this also re-latches the charge timer.
            continue;
        }

        w.set_flag(weapon_flags::IS_FIRING | weapon_flags::FIRST_FRAME);
        w.firing_op_mode = Some(w.op_mode);
        w.firing_charge_state_index = None;
        w.charge_start_time = now;
        w.cursor = LaunchCursor::default();
    }
}

pub fn stop_firing_system(
    mut reader: MessageReader<StopFiringMessage>,
    mut query: Query<&mut Weapon>,
) {
    for msg in reader.read() {
        let Ok(mut w) = query.get_mut(msg.entity) else {
            continue;
        };
        w.clear_flag(weapon_flags::TRIGGER_PULLED);
        if w.is_firing() {
            w.set_flag(weapon_flags::TRIGGER_RELEASED_AFTER_FIRING);
        }
    }
}

pub fn set_op_mode_system(
    mut reader: MessageReader<SetOpModeMessage>,
    mut query: Query<&mut Weapon>,
) {
    for msg in reader.read() {
        let Ok(mut w) = query.get_mut(msg.entity) else {
            continue;
        };
        if msg.op_mode < w.ty.op_modes.len() {
            w.op_mode = msg.op_mode;
        }
    }
}

pub fn aim_system(mut reader: MessageReader<AimMessage>, mut query: Query<&mut Weapon>) {
    for msg in reader.read() {
        if let Ok(mut w) = query.get_mut(msg.entity) {
            w.aim = msg.target.clone();
        }
    }
}

pub fn ammo_system(
    mut reload_reader: MessageReader<ReloadMessage>,
    mut add_reader: MessageReader<AddAmmoMessage>,
    mut query: Query<&mut Weapon>,
    registry: Res<AmmoRegistry>,
) {
    for msg in reload_reader.read() {
        let Ok(mut w) = query.get_mut(msg.entity) else {
            continue;
        };
        let Some(ammo) = registry.get(&msg.ammo_type_name).cloned() else {
            warn!(
                "weapons: reload — unknown ammo type '{}'",
                msg.ammo_type_name
            );
            continue;
        };
        w.set_ammo(msg.slot, ammo, msg.amount);
    }
    for msg in add_reader.read() {
        let Ok(mut w) = query.get_mut(msg.entity) else {
            continue;
        };
        let Some(ammo) = registry.get(&msg.ammo_type_name).cloned() else {
            continue;
        };
        w.add_ammo(msg.slot, ammo, msg.amount);
    }
}

/// Body-turn component of aim IK.  Mirrors the "rotate character so
/// the aim is within the arm's reach" branch of
/// `animAnimatorComponent::TargetWithArm` at
/// the legacy aim IK path:
///
/// ```text
///     mv->ApplyWorldYRotation(extra);  // rotate body if arm can't cover
/// ```
///
/// Full arm / spine IK (the actual bone solver) still TODO — that
/// requires per-skeleton bone IDs and tuning data from .animatortype
/// files.  This system covers the body-turn contribution so the
/// character at least faces the aim target when firing at a wide
/// angle.
///
/// Uses `FighterState::start_turn_to` (fight/components.rs) which
/// feeds the existing `fighter_turn_lerp_system` — matches the legacy
/// turn-lerp rate of `15.0` rad/s.
pub fn weapon_aim_body_turn_system(
    weapon_q: Query<&Weapon>,
    transforms: Query<&GlobalTransform>,
    target_comps: Query<&crate::combat::components::TargetComponent>,
    anim_states: Query<&crate::oni2_loader::animation::Oni2AnimState>,
    mut fighter_q: Query<&mut crate::fight::components::FighterState>,
) {
    for weapon in &weapon_q {
        let target_pos = match &weapon.aim {
            AimTarget::None => continue,
            AimTarget::Point(p) => *p,
            AimTarget::Actor { target, .. } | AimTarget::Miss { target, .. } => {
                get_target_world_position(
                    *target,
                    &transforms,
                    &target_comps,
                    &anim_states,
                )
            }
        };
        // Owner is the wielder — turn their body to face the aim point
        // (XZ only — vertical aim is handled by spine IK in the C++,
        // deferred until bone-level IK lands).
        let Ok(owner_tf) = transforms.get(weapon.owner) else {
            continue;
        };
        let dir = target_pos - owner_tf.translation();
        let dir_xz = Vec3::new(dir.x, 0.0, dir.z);
        if dir_xz.length_squared() < 1e-4 {
            continue;
        }
        let Ok(mut fs) = fighter_q.get_mut(weapon.owner) else {
            continue;
        };
        fs.start_turn_to(dir_xz);
    }
}

pub fn accuracy_system(
    mut reader: MessageReader<SetShooterAccuracyMessage>,
    mut query: Query<&mut Weapon>,
) {
    for msg in reader.read() {
        if let Ok(mut w) = query.get_mut(msg.entity) {
            // Map 0..1 hit probability to a simple inaccuracy scalar.
            // 1.0 = perfect, 0.0 = ±PI/4 spread.
            let p = msg.hit_probability.clamp(0.0, 1.0);
            w.shooter_inaccuracy = (1.0 - p) * std::f32::consts::FRAC_PI_4;
        }
    }
}

// ---------------------------------------------------------------------------
// weapon_attachment_system
// ---------------------------------------------------------------------------

/// Parent each active weapon to its wielder's grip bone, with the
/// weapon's local `Transform` set to the per-mount grip offset /
/// rotation.  After this runs, Bevy's `TransformSystems::Propagate`
/// (later this same frame in `PostUpdate`) computes the weapon's
/// `GlobalTransform` AND its mesh-child's `GlobalTransform` from the
/// already-updated bone hierarchy — no race conditions, no manual
/// world-space math, no lag.
///
/// Why this hierarchy-based design (rather than the previous
/// "compute world pose each frame and overwrite weapon's
/// Transform/GlobalTransform"):
///
///   - The previous design treated the weapon as a top-level entity
///     and read the bone's `GlobalTransform` to compute the weapon's
///     world pose.  That meant we needed to run AFTER bone GTF was
///     propagated — i.e. after `TransformSystems::Propagate`.  But
///     the weapon entity itself owns a child mesh entity (see
///     `inventory::systems::add_weapon_system`), and writing only
///     the weapon's GTF leaves the mesh-child's GTF stale until next
///     frame's propagation.  Result: the visible mesh lagged by one
///     frame about half the time.
///   - By parenting the weapon to the bone with a fixed local
///     transform, propagation walks bone → weapon → mesh in one
///     pass.  Everything is consistent.
///
/// Runs in `Update` (well before `PostUpdate`'s propagation) — see
/// `WeaponPlugin::build`.  The grip bone choice depends on weapon
/// state (`Drawn` vs `Holstered`), which is set by the animator's
/// state machine in `FixedUpdate` and is therefore stable by the time
/// `Update` runs.
pub fn weapon_attachment_system(
    mut commands: Commands,
    mut weapon_query: Query<(Entity, &Weapon, &mut Transform, Option<&ChildOf>)>,
    wielder_query: Query<(
        Option<&crate::animator::components::ActionPlayer>,
        Option<&crate::inventory::components::ActorWeaponMounts>,
        Option<&crate::oni2_loader::animation::Oni2AnimState>,
    )>,
) {
    for (weapon_entity, weapon, mut weapon_tf, current_parent) in &mut weapon_query {
        let Ok((action_player, mounts, anim_state)) = wielder_query.get(weapon.owner) else {
            continue;
        };

        let weapon_state = action_player
            .map(|ap| ap.weapon_state)
            .unwrap_or(crate::animator::components::WeaponState::Drawn);

        let is_drawn = weapon_state == crate::animator::components::WeaponState::Drawn;

        let mut bone_idx = 11;
        let mut grip_offset = weapon.ty.grip_offset;
        let mut grip_rot = weapon.ty.grip_rot;

        if let Some(mounts) = mounts {
            if let Some((out_mount, away_mount)) = mounts.mounts.get(&weapon.ty.name.to_lowercase())
            {
                let mount = if is_drawn { out_mount } else { away_mount };
                bone_idx = mount.parent_bone;
                // Always take the mount's Bevy-space values when present.
                // (Quat::IDENTITY is a valid "no rotation" case and we
                // shouldn't fall back from it to weapon-type defaults.)
                grip_offset = mount.offset;
                grip_rot = mount.rot;
            } else if !is_drawn {
                bone_idx = 0;
            }
        }

        let Some(anim_state) = anim_state else {
            continue;
        };
        let Some(&joint_entity) = anim_state.joint_entities.get(bone_idx) else {
            continue;
        };

        // Re-parent only when the target bone changes (weapon
        // drawn ↔ holstered, or different mount picked).  Avoids
        // queuing a no-op `ChildOf` insert every frame.
        let needs_reparent = current_parent
            .map(|cof| cof.0 != joint_entity)
            .unwrap_or(true);
        if needs_reparent {
            commands.entity(weapon_entity).insert(ChildOf(joint_entity));
        }

        // Local transform = grip offset/rot in the BONE's frame.
        // grip_offset / grip_rot are already in Bevy space (converted
        // at parse time — see `parsers::actor_weap` / `parsers::weap`).
        weapon_tf.translation = grip_offset;
        weapon_tf.rotation = grip_rot;
        weapon_tf.scale = Vec3::ONE;
    }
}

// ---------------------------------------------------------------------------
// weapon_update_system
// ---------------------------------------------------------------------------

/// Per-tick update for every active Weapon.  This is the heart of the
/// subsystem — it mirrors `weapWeapon::Update` + `weapTimeLine::Update`:
///
///   1. Resolve aim (Simple / Ballistic) to a `firing_dir`.
///   2. If `TRIGGER_PULLED`, advance `cursor.elapsed` and pick the appropriate
///      charge state (`first_state` + `charge_states`).  Emit a
///      `WeaponChargeChangedMessage` on transitions.
///   3. Walk the active charge state's projectile + effect timeline; launch
///      everything whose `delay <= elapsed` and whose index isn't already in
///      `cursor.projectiles_fired` / `cursor.effects_spawned`.  Emit
///      `SpawnProjectileEvent` for projectiles and `SpawnFx` for effects.
///   4. When the cycle completes, re-latch `chamber_time` (+ `auto_fire_delay`
///      if auto-firing) and decide whether to stop firing or loop.
pub fn weapon_update_system(
    mut query: Query<(Entity, &mut Weapon, &GlobalTransform)>,
    global_transforms: Query<&GlobalTransform>,
    target_comps: Query<&crate::combat::components::TargetComponent>,
    anim_states: Query<&crate::oni2_loader::animation::Oni2AnimState>,
    time: Res<Time>,
    mut commands: Commands,
    mut fired_writer: MessageWriter<WeaponFiredMessage>,
    mut charge_writer: MessageWriter<WeaponChargeChangedMessage>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();

    // Pre-gather target positions for aim resolution.
    let mut target_positions = bevy::ecs::entity::EntityHashMap::default();
    for (_, w, _) in &query {
        match &w.aim {
            AimTarget::Actor { target, .. } | AimTarget::Miss { target, .. } => {
                let pos = get_target_world_position(
                    *target,
                    &global_transforms,
                    &target_comps,
                    &anim_states,
                );
                target_positions.insert(*target, pos);
            }
            _ => {}
        }
    }

    for (weapon_entity, mut weapon, weapon_gt) in &mut query {
        let (_, weapon_rot, weapon_pos) = weapon_gt.to_scale_rotation_translation();

        // --- 1. Aim resolution -------------------------------------------------
        let proj_speed = weapon
            .firing_mode()
            .and_then(|m| m.first_state.projectiles.first().map(|p| p.speed))
            .unwrap_or(20.0);
        let fallback_fwd = (weapon_rot * Vec3::NEG_Z).normalize_or_zero();
        let aimer = weapon
            .firing_mode()
            .map(|m| m.aimer)
            .unwrap_or(Aimer::Simple);

        weapon.firing_dir = resolve_aim(
            weapon_pos,
            fallback_fwd,
            &weapon.aim,
            aimer,
            proj_speed,
            &target_positions,
        );

        if !weapon.has_flag(weapon_flags::IS_FIRING) {
            continue;
        }

        // --- 2. Charge state selection ----------------------------------------
        let trigger_held = weapon.has_flag(weapon_flags::TRIGGER_PULLED);
        let charge_time = now - weapon.charge_start_time;

        // Determine the current charge state index for this frame.
        let (state_idx, state_ref_clone) = {
            let mode = match weapon.firing_mode() {
                Some(m) => m,
                None => {
                    weapon.clear_flag(weapon_flags::IS_FIRING);
                    continue;
                }
            };
            let effective_time = if !trigger_held { 0.0 } else { charge_time };
            // Find the charge state that matches (same lookup rule as
            // OperationalMode::find_state but we also want its numeric index).
            let mut idx: i32 = -1; // -1 = first_state
            let mut best_t = -1.0_f32;
            for (i, s) in mode.charge_states.iter().enumerate() {
                if s.min_charge_time <= effective_time && s.min_charge_time > best_t {
                    idx = i as i32;
                    best_t = s.min_charge_time;
                }
            }
            let st = if idx < 0 {
                mode.first_state.clone()
            } else {
                mode.charge_states[idx as usize].clone()
            };
            (idx, st)
        };

        if weapon.firing_charge_state_index != Some(state_idx) {
            weapon.firing_charge_state_index = Some(state_idx);
            weapon.cursor = LaunchCursor::default();
            charge_writer.write(WeaponChargeChangedMessage {
                entity: weapon_entity,
                charge_state_index: state_idx,
                charge_time,
            });
        }

        // --- 3. Advance cursor, launch projectiles + effects ------------------
        weapon.cursor.elapsed += dt;
        let elapsed = weapon.cursor.elapsed;

        // Muzzle frame — attached to the weapon's transform.  Subclasses may
        // later override this via a grip/bone socket.

        // --- 3a. Projectiles -------------------------------------------------
        for (pi_idx, proj) in state_ref_clone.projectiles.iter().enumerate() {
            let idx = pi_idx as u16;
            if weapon.cursor.projectiles_fired.contains(&idx) {
                continue;
            }
            if proj.delay > elapsed {
                continue;
            }

            // Ammo check (first matching slot).
            let mode = match weapon.firing_mode() {
                Some(m) => m,
                None => break,
            };
            let ammo_slot = mode.ammo_slot.max(0) as usize;
            let ammo_remaining = if ammo_slot < weapon.ammo_slots.len() {
                weapon.ammo_slots[ammo_slot].amount
            } else {
                0.0
            };
            if ammo_remaining < proj.ammo_use {
                // Out of ammo — still record as "fired" so we don't spin, and
                // skip the launch.
                weapon.cursor.projectiles_fired.push(idx);
                continue;
            }

            let (muzzle_pos, muzzle_dir_local) = muzzle_world_transform(
                weapon_pos,
                weapon_rot,
                proj.muzzle_offset,
                proj.muzzle_eulers,
            );

            // Since spine IK is not fully implemented yet, blending with the crosshair aim
            // direction causes bullets to shoot sideways out of the barrel if the player
            // aims up or down. Locking the firing direction to the muzzle's world orientation
            // ensures bullets always visually fire straight out of the gun.
            let aim_blend = muzzle_dir_local;

            // Apply shooter inaccuracy.  accX/accY are independent random angles in
            // [-inaccuracy, +inaccuracy] (radians).  accX rotates the
            // direction around world-Y (horizontal spread), accY tilts
            // it up/down.  The C++ also adds a muzzle-inherent
            // inaccuracy component from ComputeInaccuracy(target-range,
            // hit-prob) — we skip that for now since AccTargetRange /
            // AccTargetHitProb aren't plumbed through the ProjectileInfo
            // registry yet.
            let perturbed = if weapon.shooter_inaccuracy > 0.0 {
                let r = weapon.shooter_inaccuracy;
                // Random accX/accY in [-r, +r].
                let acc_x = (rand::random::<f32>() * 2.0 - 1.0) * r;
                let acc_y = (rand::random::<f32>() * 2.0 - 1.0) * r;
                // Horizontal rotation around Y: (x,z) spread perpendicular
                // to firing dir's XZ component.
                let mut d = Vec3::new(
                    aim_blend.x + acc_x * aim_blend.z,
                    aim_blend.y + acc_y,
                    aim_blend.z - acc_x * aim_blend.x,
                );
                if d.length_squared() > 1e-8 {
                    d = d.normalize();
                }
                d
            } else {
                aim_blend
            };

            commands.trigger(SpawnProjectileEvent {
                name: proj.projectile_name.clone(),
                position: muzzle_pos,
                velocity: perturbed * proj.speed,
                owner: weapon.owner,
                team: 0,
            });

            // Deduct ammo + bookkeeping.
            if ammo_slot < weapon.ammo_slots.len() {
                weapon.ammo_slots[ammo_slot].amount = (ammo_remaining - proj.ammo_use).max(0.0);
            }
            weapon.shots_fired += proj.ammo_use;
            weapon.cursor.projectiles_fired.push(idx);

            fired_writer.write(WeaponFiredMessage {
                entity: weapon_entity,
                owner: weapon.owner,
                projectile_name: proj.projectile_name.clone(),
                muzzle_position: muzzle_pos,
                muzzle_direction: perturbed,
                ammo_remaining: weapon.ammo_count(ammo_slot),
            });
        }

        // --- 3b. Effects -----------------------------------------------------
        for (fx_idx, fx) in state_ref_clone.firing_fx.iter().enumerate() {
            let idx = fx_idx as u16;
            if weapon.cursor.effects_spawned.contains(&idx) {
                continue;
            }
            if fx.delay > elapsed {
                continue;
            }
            let fx_pos = weapon_pos + weapon_rot * fx.offset;
            commands.trigger(SpawnFx {
                name: fx.fx_name.clone(),
                at: Some(fx_pos),
                parent: Some(weapon_entity),
                start_active: true,
            });
            weapon.cursor.effects_spawned.push(idx);
        }

        // --- 4. Cycle completion ---------------------------------------------
        let all_projectiles_done =
            weapon.cursor.projectiles_fired.len() == state_ref_clone.projectiles.len();
        let all_effects_done =
            weapon.cursor.effects_spawned.len() == state_ref_clone.firing_fx.len();
        if all_projectiles_done && all_effects_done {
            let (chamber, auto_delay, mode_auto) = weapon
                .firing_mode()
                .map(|m| (m.chamber_time, m.auto_fire_delay, m.auto_fire()))
                .unwrap_or((0.2, 0.0, false));

            weapon.fire_time = now + chamber + if mode_auto { auto_delay } else { 0.0 };

            let should_continue = match weapon
                .firing_mode()
                .map(|m| m.trig_hold_mode)
                .unwrap_or(TrigHoldMode::Single)
            {
                TrigHoldMode::Single => false,
                TrigHoldMode::Auto => trigger_held,
                TrigHoldMode::Charge | TrigHoldMode::FirstAndCharge => trigger_held,
            };

            if should_continue {
                // Loop — reset cursor, keep IS_FIRING, preserve charge_start_time
                // so the charge-state lookup picks the next tier.
                weapon.cursor = LaunchCursor::default();
            } else {
                weapon.clear_flag(weapon_flags::IS_FIRING | weapon_flags::FIRST_FRAME);
                weapon.firing_op_mode = None;
                weapon.firing_charge_state_index = None;
            }
        } else if weapon.has_flag(weapon_flags::FIRST_FRAME) {
            weapon.clear_flag(weapon_flags::FIRST_FRAME);
        }
    }
}
