/*
 * oni2_loader/crane_ik.rs — elbow-crane analytical 2-bone IK.
 *
 * Port of the legacy `animElbowCraneIKComponent`.  Drives a
 * five-bone EricArm-style crane skeleton (azimuth + shoulder +
 * elbow + forearm + paired clamp slides) so the claw chases a
 * world-space goal each tick.
 *
 * Pipeline
 *   1. Init (lazy): on first tick after spawn, find bone indices
 *      arm_01..arm_04 + clamp_01_{r,l}, sample joint limits and
 *      rest-pose offsets from the skeleton, cache them on the
 *      `ElbowCraneIK` component.
 *   2. Tick: drive the state machine; in active states ask
 *      `pos_to_angles` for the {azimuth, shoulder, elbow} triple
 *      that lands the claw at the goal (with `m_LastTarget`
 *      rate-limited approach controlling speed), derive wrist
 *      angle as PI/2 - (shoulder+elbow) to keep the claw vertical,
 *      and write per-joint local Transforms.
 *   3. Apply: each joint Transform is composed in actor-local
 *      (= skeleton-world-assuming-actor-at-origin) space so it
 *      slots in directly on top of whatever the animation system
 *      last wrote — without disturbing the parent actor's world
 *      transform.
 *
 * Coordinate handling
 *   The C++ math operates entirely in actor-local Oni-space.  We
 *   mirror that, then conjugate the final pose into Bevy space:
 *     • Y rotations pass through unchanged (Y axis is preserved
 *       by the Oni→Bevy 180°-Y conjugation).
 *     • X/Z rotations flip sign.
 *     • Position vectors negate their X and Z components.
 *
 *   The C++ writes `MakeRotateX(-angle)` (Oni rot_x(-a)); the Bevy
 *   equivalent is `Quat::from_rotation_x(a)`, hence the sign flips
 *   below.
 *
 * State machine
 *   `kIdle`, `kAcquisition`, `kAttached`, `kMovingtoTarget`,
 *   `kReleased` mirror the C++ enum directly.  The acquisition
 *   half (`PickupTarget`) and dropoff half (`DropoffAt`) are
 *   exposed as public methods; full ScrOni / behavior wiring lands
 *   in a follow-up alongside the Hitch component port.  The
 *   simpler `SetDesiredLocation` public method is plumbed today so
 *   level scripts (or debug overlays) can drive the crane.
 */
use bevy::prelude::*;
use std::f32::consts::PI;

use crate::oni2_loader::animation::Oni2AnimState;
use crate::oni2_loader::utils::space;

/// Companion component to [`ElbowCraneIK`] — declared on actors
/// that cranes can pick up.  Mirrors the C++
/// `animElbowCraneHitchComponent`.  `center` is the offset from
/// the actor origin to its hitch point (in Bevy space, already
/// coord-flipped at parse time); `extent` is the object width
/// the crane's claw will close around (legacy
/// `m_GrabRadius = extent / 2`).
///
/// The C++ keeps a transient `m_PickupMtx` pointer to the
/// crane's claw matrix while held; we instead drive the
/// follow-the-claw teleport from [`crane_carry_system`] reading
/// the crane's [`ElbowCraneIK::last_claw_world`] each tick.
#[derive(Component, Debug, Clone)]
pub struct ElbowCraneHitch {
    /// `Center` attribute in Bevy space.  Only the Y component is
    /// load-bearing in the legacy default code path
    /// (`#define USE_HITCH_Y` is always set); X/Z are preserved
    /// in case the non-USE_HITCH_Y branch is ever revived.
    pub center: Vec3,
    /// `Extent` attribute — full width of the object.  IK
    /// divides by 2 to drive the clamp closure.
    pub extent: f32,
    /// Crane currently holding this hitch, if any.  Set by the
    /// crane IK system on pickup, cleared on drop.
    pub held_by: Option<Entity>,
}

impl ElbowCraneHitch {
    pub fn new(center: Vec3, extent: f32) -> Self {
        Self {
            center,
            extent,
            held_by: None,
        }
    }
}

/// Bone indices into `Oni2Skeleton.joint_entities`.  Cached on first
/// tick so subsequent ticks skip the by-name lookup.
#[derive(Debug, Clone, Copy)]
pub struct CraneBones {
    pub azimuth: usize,  // arm_01
    pub shoulder: usize, // arm_02
    pub elbow: usize,    // arm_03
    pub forearm: usize,  // arm_04
    pub right_clamp: usize,
    pub left_clamp: usize,
}

/// Rest-pose measurements sampled once at init, matching the C++
/// `Init()` reads: bone lengths are pure Y offsets in EricArm.skel
/// so magnitude == |y| in either coordinate system.
#[derive(Debug, Clone)]
pub struct CraneBoneRest {
    /// `(arm_02.offset + arm_01.offset).magnitude` — distance from
    /// world origin (= root) up to the shoulder joint.
    pub base_length: f32,
    /// `arm_03.offset.magnitude` — shoulder→elbow distance.
    pub bicep_length: f32,
    /// `arm_04.offset.magnitude` — elbow→wrist (claw mount) distance.
    pub forearm_length: f32,
    /// Local offset of the right clamp bone at rest (Oni-space).
    /// Reused as the open-claw baseline; the X component is
    /// rewritten each tick to `±grab_radius` (closed) or the
    /// 0.157 m default (open).
    pub right_claw_origin_oni: Vec3,
    /// Mirror of [`right_claw_origin_oni`] for the left clamp.
    pub left_claw_origin_oni: Vec3,
    /// `arm_02` rotation X-channel min/max limit from the .skel, if present.
    pub shoulder_limit: Option<[f32; 2]>,
    /// `arm_03` rotation X-channel min/max limit from the .skel, if present.
    pub elbow_limit: Option<[f32; 2]>,
}

/// Top-level state machine.  Mirrors `m_State` in the C++.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CraneState {
    /// Crane is parked; no IK target.  After `Reset()` and after
    /// any pickup/dropoff "fails".
    #[default]
    Idle,
    /// Pickup behavior is running; track `target_world` until the
    /// claw arrives, then transition to `Attached`.
    Acquisition,
    /// Pickup completed; sitting on the target.  Bridge state
    /// between pickup and a follow-on dropoff command.
    Attached,
    /// Dropoff behavior is running; sub-state in
    /// [`CraneMovingState`] tracks the lift/translate/lower
    /// phases.
    MovingToTarget,
    /// Dropoff completed; the held actor was released and the
    /// claw-release timer expired.
    Released,
    /// External "go here" command (no held object, no
    /// acquisition/dropoff state machine).  Not in the C++; lets
    /// gameplay scripts position the claw directly without
    /// needing the full Hitch / Behavior plumbing.
    SeekDesiredLocation,
}

/// Sub-state of [`CraneState::MovingToTarget`].  Mirrors
/// `m_MovingState` in the C++.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CraneMovingState {
    #[default]
    Nothing,
    Lift,
    Translate,
    Lower,
    Releasing,
}

/// Crane IK component.  Attach alongside an `Oni2AnimState`
/// owning an EricArm-shaped skeleton.
///
/// `speed` is in degrees/sec (Oni convention from
/// `<ElbowCraneIK><Speed/></ElbowCraneIK>`); the rate-limit
/// approach uses it directly.  `clamp_offset` is the additional
/// distance from the wrist out to the claw centre, fed back into
/// the target before solving so the *claw* (not the wrist) hits
/// the goal.
#[derive(Component, Debug, Clone)]
pub struct ElbowCraneIK {
    pub speed_deg: f32,
    pub clamp_offset: f32,
    pub state: CraneState,
    pub moving_state: CraneMovingState,
    /// Solved azimuth, in radians, applied as Oni rot_y(-azm - PI/2).
    pub azimuth: f32,
    /// Solved shoulder X angle (positive = elbow back).
    pub shoulder_angle: f32,
    /// Solved elbow X angle.
    pub elbow_angle: f32,
    /// Rate-limited approached target in actor-local Oni-space.
    /// Mirrors `m_LastTarget`.
    pub last_target_local_oni: Vec3,
    /// The current goal in world-space Bevy coordinates.  Set by
    /// [`pickup_target`]/[`dropoff_at`]/[`set_desired_location`].
    pub goal_world: Vec3,
    /// Lift-phase intermediate goal (target + lift_height Y) used
    /// by the dropoff state machine.
    pub lift_world: Vec3,
    /// Dropoff destination — separate from `goal_world` because
    /// the dropoff state machine walks through several sub-goals
    /// (lift → translate → lower) all derived from it.
    pub dropoff_world: Vec3,
    /// Y-offset added during the lift / translate phases of the
    /// dropoff state machine.  C++ default 1.0.
    pub lift_height: f32,
    /// Half-width of the held object's hitch (drives clamp closure).
    pub grab_radius: f32,
    /// Carrying-object flag.  True from the moment pickup begins
    /// until `drop()` is called.  Gates the clamp-closure logic.
    pub carrying_object: bool,
    /// True until the IK pose has been re-anchored at the rest
    /// pose end-effector — the C++ resets this in `Reset()` and
    /// uses it as a one-shot "seed `m_LastTarget` from the
    /// skeleton at-rest pose" gate inside `Pos2Angles`.
    pub first_acquire: bool,
    /// Cached bone indices.  `None` until the first tick that
    /// finds the EricArm bones in the skeleton.
    pub bones: Option<CraneBones>,
    /// Cached rest-pose measurements, sampled alongside [`bones`].
    pub rest: Option<CraneBoneRest>,
    /// World-space target actor whose hitch we're holding.  Set by
    /// [`pickup_target_actor`] (which captures the Hitch's grab
    /// radius + center offset at the same time), cleared by
    /// [`drop`].  Used by [`crane_carry_system`] to teleport the
    /// held actor to follow the claw, and by `crane_ik_system` to
    /// re-fetch a moving target's world position each tick.
    pub held_entity: Option<Entity>,
    /// Hitch `Center` of the held entity in Bevy space — captured
    /// at pickup time.  Only the Y component is used by the
    /// follow-claw teleport (matches the legacy `USE_HITCH_Y` mode).
    pub held_hitch_center: Vec3,
    /// Most recently computed claw matrix in WORLD Bevy space.
    /// Written by `apply_ik_pose` each tick; read by
    /// `crane_carry_system` to drag the held actor along.
    pub last_claw_world_pos: Vec3,
    pub last_claw_world_rot: Quat,
    /// Previous tick's |claw - target|² in actor-local Oni space.
    /// Compared with the current squared distance to detect a
    /// stall — when the IK can't get any closer (target outside
    /// reach), the AtTarget check trips after a few repeats and
    /// the state machine advances anyway.  Mirrors `m_LastMag`.
    pub last_mag2: f32,
    /// Consecutive ticks the squared distance hasn't changed.
    /// Resets when it changes; when it climbs past 4 during a
    /// lift/lower sub-phase, the AtTarget check reports success
    /// to unstick the state machine.  Mirrors `m_RepeatCount`.
    pub repeat_count: u32,
    /// Seconds spent in [`CraneMovingState::Releasing`] since the
    /// actor was dropped.  When this reaches [`claw_time`] the
    /// dropoff completes (`state == Released`).  Mirrors the
    /// implicit `TIME.GetElapsedTime() - m_TimeStamp` check.
    pub release_timer: f32,
    /// How long the crane sits in `Releasing` after dropping
    /// before advancing to `Released` — `m_ClawTime` in the C++
    /// (default 1.0s).
    pub claw_time: f32,
    /// True while a pickup is in flight but the
    /// `Acquisition→Attached` transition hasn't fired
    /// `CRANESTRUGGLE` yet.  One-shot — mirrors the C++ pattern
    /// where `HandlePickupMatrix` (start struggle) is sent
    /// exactly once on arrival.
    pub struggle_pending: bool,
    /// True from pickup until `drop()`.  Triggers the
    /// `EndActionMessage(CraneStruggle)` + `DropMessage` on the
    /// transition out (end the carried-actor's struggle anim and
    /// teleport-fixup).  Distinct from `carrying_object` because
    /// the latter gates IK clamp-closure math.
    pub struggle_active: bool,
    /// One-shot gate: set to true after `try_init` has either
    /// succeeded (logs the init line) OR failed (logs the
    /// missing-bone warning).  Prevents per-tick log spam when a
    /// non-EricArm-shaped actor accidentally gets an
    /// `ElbowCraneIK` component — the warning fires once at
    /// startup, never again.
    pub init_logged: bool,
}

impl Default for ElbowCraneIK {
    fn default() -> Self {
        Self {
            speed_deg: 275.0,
            clamp_offset: 0.0,
            state: CraneState::Idle,
            moving_state: CraneMovingState::Nothing,
            azimuth: 0.0,
            shoulder_angle: 0.0,
            elbow_angle: 0.0,
            last_target_local_oni: Vec3::ZERO,
            goal_world: Vec3::ZERO,
            lift_world: Vec3::ZERO,
            dropoff_world: Vec3::new(100.0, 100.0, 100.0),
            lift_height: 1.0,
            grab_radius: 0.157,
            carrying_object: false,
            first_acquire: true,
            bones: None,
            rest: None,
            held_entity: None,
            held_hitch_center: Vec3::ZERO,
            last_claw_world_pos: Vec3::ZERO,
            last_claw_world_rot: Quat::IDENTITY,
            last_mag2: f32::INFINITY,
            repeat_count: 0,
            release_timer: 0.0,
            claw_time: 1.0,
            struggle_pending: false,
            struggle_active: false,
            init_logged: false,
        }
    }
}

impl ElbowCraneIK {
    /// Construct with the parsed `<ElbowCraneIK><Speed/></ElbowCraneIK>`
    /// (degrees/sec).  All other fields take their defaults.
    pub fn new(speed_deg: f32, clamp_offset: f32) -> Self {
        Self {
            speed_deg,
            clamp_offset,
            ..Self::default()
        }
    }

    /// Equivalent of `Reset()` in the C++.  Re-zero the solved
    /// angles, drop any held object marker, and re-seed the
    /// first-acquire flag so `pos_to_angles` snaps the approach
    /// target back onto the rest-pose end-effector.
    pub fn reset(&mut self) {
        self.azimuth = 0.0;
        self.shoulder_angle = 0.0;
        self.elbow_angle = 0.0;
        self.last_target_local_oni = Vec3::ZERO;
        self.first_acquire = true;
        self.carrying_object = false;
        self.state = CraneState::Idle;
        self.moving_state = CraneMovingState::Nothing;
        self.dropoff_world = Vec3::new(100.0, 100.0, 100.0);
        self.last_mag2 = f32::INFINITY;
        self.repeat_count = 0;
        self.release_timer = 0.0;
        self.struggle_pending = false;
        self.struggle_active = false;
    }

    /// Drive the claw straight to `goal_world` without engaging the
    /// pickup/dropoff state machine.  Useful for ScrOni-driven
    /// "park here" commands and for in-editor debug positioning.
    pub fn set_desired_location(&mut self, goal_world: Vec3) {
        self.goal_world = goal_world;
        self.state = CraneState::SeekDesiredLocation;
        self.moving_state = CraneMovingState::Nothing;
    }

    /// Bare-target form of [`pickup_target_actor`] for callers
    /// that don't have a holdable entity (debug / scripted
    /// "go-here-and-pretend").  Drives the IK to `world_pos` and
    /// sets carrying so the clamps close, but doesn't latch onto
    /// any actor — `held_entity` stays None.
    pub fn pickup_target(&mut self, world_pos: Vec3, grab_radius: f32) {
        self.goal_world = world_pos;
        self.grab_radius = grab_radius;
        self.carrying_object = true;
        self.state = CraneState::Acquisition;
        self.moving_state = CraneMovingState::Nothing;
        self.held_entity = None;
        self.held_hitch_center = Vec3::ZERO;
        self.last_mag2 = f32::INFINITY;
        self.repeat_count = 0;
        // No held_entity, so no struggle anim to drive.
        self.struggle_pending = false;
        self.struggle_active = false;
    }

    /// Equivalent of the C++ `PickupTarget(Target)` once the
    /// caller has resolved the Hitch.  `target` is the entity
    /// being grabbed; `target_world_pos` is its current world
    /// origin; `hitch_center` is the Bevy-space `Center` from
    /// the target's [`ElbowCraneHitch`]; `extent` is the hitch's
    /// `Extent` (full width).  Sets up acquisition + holds onto
    /// the target so the IK system can re-fetch its position each
    /// tick (moving targets stay tracked) and the carry system
    /// can drive the target's Transform once attached.
    pub fn pickup_target_actor(
        &mut self,
        target: Entity,
        target_world_pos: Vec3,
        hitch_center: Vec3,
        extent: f32,
    ) {
        // USE_HITCH_Y in the C++: only the Y component of the
        // hitch center is added when computing the IK goal.
        let mut goal = target_world_pos;
        goal.y += hitch_center.y;
        self.goal_world = goal;
        self.grab_radius = extent * 0.5;
        self.carrying_object = true;
        self.state = CraneState::Acquisition;
        self.moving_state = CraneMovingState::Nothing;
        self.held_entity = Some(target);
        self.held_hitch_center = hitch_center;
        self.last_mag2 = f32::INFINITY;
        self.repeat_count = 0;
        self.struggle_pending = true;
        self.struggle_active = true;
        info!(
            "[crane] pickup_target_actor: target={:?} target_pos={:?} hitch.center={:?} \
             extent={} -> goal_world={:?} grab_radius={} state=Acquisition",
            target, target_world_pos, hitch_center, extent, self.goal_world, self.grab_radius
        );
    }

    /// Equivalent of `DropoffAt(loc)`.  Begins the lift →
    /// translate → lower sequence.  Should only be called while
    /// the crane is in [`CraneState::Attached`]; calling it in
    /// other states still works but the IK will pop the claw to
    /// the lift point without an intermediate object snap.
    pub fn dropoff_at(&mut self, world_pos: Vec3) {
        self.state = CraneState::MovingToTarget;
        self.moving_state = CraneMovingState::Lift;
        self.dropoff_world = world_pos;
        // Clear stall tracking so the new sub-phase has a clean
        // slate (matches `m_RepeatCount = 0` resets in the C++
        // AtTarget when transitioning between modes).
        self.last_mag2 = f32::INFINITY;
        self.repeat_count = 0;
        info!(
            "[crane] dropoff_at: dropoff_world={:?} (lift_world cached at {:?}) \
             state=MovingToTarget moving_state=Lift",
            self.dropoff_world, self.lift_world
        );
    }

    /// Equivalent of `Drop()`.  Releases the carried object marker
    /// and returns to the idle state.  The IK chain stays where it
    /// is — only the carry flag and state enum reset.
    pub fn drop(&mut self) {
        self.carrying_object = false;
        self.state = CraneState::Idle;
        self.moving_state = CraneMovingState::Nothing;
        self.held_entity = None;
        self.held_hitch_center = Vec3::ZERO;
        // Struggle anim hooks are one-shot per pickup cycle.  The
        // crane_ik_system observes these BEFORE calling drop() to
        // emit the matching EndActionMessage; once drop() returns
        // they're cleared so a follow-up pickup starts cleanly.
        self.struggle_pending = false;
        self.struggle_active = false;
    }
}

/// Linear approach: move `current` toward `target` by at most
/// `rate * dt` per component.  Mirrors `Vector3::Approach` in the
/// C++ (the per-component clamp form, not the per-vector form).
fn approach_vec(current: &mut Vec3, target: Vec3, rate: f32, dt: f32) {
    let step = rate * dt;
    let delta = target - *current;
    let len = delta.length();
    if len <= step || len < 1e-6 {
        *current = target;
    } else {
        *current += delta * (step / len);
    }
}

fn find_bone(names: &[String], wanted: &str) -> Option<usize> {
    names.iter().position(|n| n.eq_ignore_ascii_case(wanted))
}

/// Resolve the EricArm bone indices and cache them along with the
/// rest-pose measurements.  Returns `false` if the skeleton
/// doesn't contain every bone we need (i.e. the actor isn't
/// actually using an EricArm skeleton); the caller should leave
/// the crane untouched in that case.
fn try_init(ik: &mut ElbowCraneIK, anim_state: &Oni2AnimState) -> bool {
    if ik.bones.is_some() && ik.rest.is_some() {
        return true;
    }
    let skel = &anim_state.skeleton;
    let azimuth = match find_bone(&skel.names, "arm_01") {
        Some(v) => v,
        None => return false,
    };
    let shoulder = match find_bone(&skel.names, "arm_02") {
        Some(v) => v,
        None => return false,
    };
    let elbow = match find_bone(&skel.names, "arm_03") {
        Some(v) => v,
        None => return false,
    };
    let forearm = match find_bone(&skel.names, "arm_04") {
        Some(v) => v,
        None => return false,
    };
    let right_clamp = match find_bone(&skel.names, "clamp_01_r") {
        Some(v) => v,
        None => return false,
    };
    let left_clamp = match find_bone(&skel.names, "clamp_01_l") {
        Some(v) => v,
        None => return false,
    };

    let azimuth_off = Vec3::from(skel.local_offsets[azimuth]);
    let shoulder_off = Vec3::from(skel.local_offsets[shoulder]);
    let elbow_off = Vec3::from(skel.local_offsets[elbow]);
    let forearm_off = Vec3::from(skel.local_offsets[forearm]);
    let right_claw_off = Vec3::from(skel.local_offsets[right_clamp]);
    let left_claw_off = Vec3::from(skel.local_offsets[left_clamp]);

    let base_length = (shoulder_off + azimuth_off).length();
    let bicep_length = elbow_off.length();
    let forearm_length = forearm_off.length();

    // Pull rotation limits from `skel.channels[shoulder/elbow]` if present
    let rest = CraneBoneRest {
        base_length,
        bicep_length,
        forearm_length,
        right_claw_origin_oni: right_claw_off,
        left_claw_origin_oni: left_claw_off,
        shoulder_limit: skel.channels.get(shoulder).and_then(|c| c.rot_x_limit),
        elbow_limit: skel.channels.get(elbow).and_then(|c| c.rot_x_limit),
    };

    ik.bones = Some(CraneBones {
        azimuth,
        shoulder,
        elbow,
        forearm,
        right_clamp,
        left_clamp,
    });
    ik.rest = Some(rest);
    info!(
        "[crane] init OK: bones=arm_01..arm_04+clamp_{{r,l}} base={:.3} bicep={:.3} \
         forearm={:.3} speed={}deg/s shoulder_lim={:?} elbow_lim={:?}",
        base_length,
        bicep_length,
        forearm_length,
        ik.speed_deg,
        ik.rest.as_ref().unwrap().shoulder_limit,
        ik.rest.as_ref().unwrap().elbow_limit,
    );
    ik.init_logged = true;
    true
}

/// Analytical 2-bone solver.  Identical math to the C++
/// `Pos2Angles`: project the target into the shoulder-rooted local
/// frame, derive azimuth via `atan2`, pull the target back by the
/// claw-offset, then law-of-cosines for elbow + shoulder.
///
/// Returns `Some((azimuth, shoulder, elbow))` when the target is
/// reachable; `None` (matching C++ `return false`) when the elbow
/// cosine would lie outside [-1, 1] — caller leaves the previous
/// pose in place.
///
/// `target_local_oni` is in actor-local Oni-space (X right, Y up,
/// Z forward).  Bone lengths feed in directly.
fn pos_to_angles(
    target_local_oni: Vec3,
    base_length: f32,
    bicep_length: f32,
    forearm_length: f32,
    clamp_offset: f32,
) -> Option<(f32, f32, f32)> {
    // Shoulder is at (0, base_length, 0) in actor-local Oni.
    let shoulder_joint = Vec3::new(0.0, base_length, 0.0);
    let mut shoulder_to_pos = target_local_oni - shoulder_joint;
    let azimuth = shoulder_to_pos.z.atan2(shoulder_to_pos.x);

    // Pull the target back along the claw-offset axis (the C++
    // does the same with `ClampOffset` so the *claw centre*
    // reaches the target rather than the wrist).
    let mut offset = Vec3::new(clamp_offset, 0.0, 0.0);
    // Rotate around Y by -azimuth (matches C++ `RotateY(-fAzimuth)`).
    let cos_a = (-azimuth).cos();
    let sin_a = (-azimuth).sin();
    let rotated = Vec3::new(
        offset.x * cos_a + offset.z * sin_a,
        offset.y,
        -offset.x * sin_a + offset.z * cos_a,
    );
    offset = rotated;
    shoulder_to_pos -= offset;

    let radius = shoulder_to_pos.length();
    let radius2 = radius * radius;
    let bicep2 = bicep_length * bicep_length;
    let forearm2 = forearm_length * forearm_length;

    let cos2 = (radius2 - (bicep2 + forearm2)) / (2.0 * bicep_length * forearm_length);
    if !(-1.0..=1.0).contains(&cos2) {
        return None;
    }
    let elbow = cos2.acos();

    let shoulder_a = ((forearm2 - (radius2 + bicep2)) / (-2.0 * radius * bicep_length)).acos();
    let run =
        (shoulder_to_pos.x * shoulder_to_pos.x + shoulder_to_pos.z * shoulder_to_pos.z).sqrt();
    let shoulder_b = shoulder_to_pos.y.atan2(run);
    let shoulder = (PI / 2.0 - shoulder_b) - shoulder_a;

    Some((azimuth, shoulder, elbow))
}

/// Convert an Oni-space quaternion to its Bevy-space equivalent
/// via 180°-Y conjugation: `(x, y, z, w) → (-x, y, -z, w)`.  This
/// is the rotation analog of `space::to_bevy_space_pos`
/// (`(x, y, z) → (-x, y, -z)`).
#[inline]
fn oni_quat_to_bevy(q: Quat) -> Quat {
    Quat::from_xyzw(-q.x, q.y, -q.z, q.w)
}

/// Compose the IK pose along the chain and write each joint's
/// actor-local Transform.  Joint Transforms in this codebase are
/// stored as actor-local skeleton-world poses (same convention as
/// `update_oni2_animation` writes); we follow suit so the
/// hierarchy composes correctly.
/// Apply the solved IK pose to the joint Transforms and return
/// the claw centre in actor-local Bevy space.  The caller composes
/// that with the actor's `GlobalTransform` to get the world claw
/// matrix for the carry system.
///
/// When `debug_skel` is `Some`, the six IK-controlled bones
/// (arm_01..arm_04, clamp_01_{r,l}) also have their positions
/// written into `Oni2DebugSkeleton.positions` so the F4 debug
/// overlay reflects the IK pose instead of the animation pose
/// that `update_oni2_animation` wrote earlier this frame.
#[must_use]
fn apply_ik_pose(
    bones: &CraneBones,
    rest: &CraneBoneRest,
    skel: &crate::oni2_loader::parsers::types::Oni2Skeleton,
    azimuth: f32,
    shoulder: f32,
    elbow: f32,
    carrying: bool,
    state: CraneState,
    grab_radius: f32,
    clamp_offset: f32,
    joint_entities: &[Entity],
    joint_tf_query: &mut Query<&mut Transform>,
    debug_skel: Option<&mut crate::oni2_loader::animation::Oni2DebugSkeleton>,
) -> (Vec3, Quat) {
    let arm01_offset = space::to_bevy_space_pos(Vec3::from(skel.local_offsets[bones.azimuth]));
    let arm02_offset = space::to_bevy_space_pos(Vec3::from(skel.local_offsets[bones.shoulder]));
    let arm03_offset = space::to_bevy_space_pos(Vec3::from(skel.local_offsets[bones.elbow]));
    let arm04_offset = space::to_bevy_space_pos(Vec3::from(skel.local_offsets[bones.forearm]));

    // Oni rotations from the C++.  Sign of every rotation argument
    // matches `MakeRotateY(-(...))` / `MakeRotateX(-(...))`.
    let oni_azimuth_rot = Quat::from_rotation_y(-azimuth - PI / 2.0);
    let oni_shoulder_rot = Quat::from_rotation_x(-shoulder);
    let oni_elbow_rot = Quat::from_rotation_x(-elbow);
    let wrist_angle = PI / 2.0 - (shoulder + elbow);
    let oni_wrist_rot = Quat::from_rotation_x(-wrist_angle);

    let bevy_azimuth_rot = oni_quat_to_bevy(oni_azimuth_rot);
    let bevy_shoulder_rot = oni_quat_to_bevy(oni_shoulder_rot);
    let bevy_elbow_rot = oni_quat_to_bevy(oni_elbow_rot);
    let bevy_wrist_rot = oni_quat_to_bevy(oni_wrist_rot);

    // Walk the chain in actor-local space.  Root assumed at
    // identity (it always is for an EricArm — the .skel's root
    // bone has zero offset and zero animated rotation).
    let arm01_pos = arm01_offset;
    let arm01_rot = bevy_azimuth_rot;

    let arm02_pos = arm01_pos + arm01_rot * arm02_offset;
    let arm02_rot = arm01_rot * bevy_shoulder_rot;

    let arm03_pos = arm02_pos + arm02_rot * arm03_offset;
    let arm03_rot = arm02_rot * bevy_elbow_rot;

    let arm04_pos = arm03_pos + arm03_rot * arm04_offset;
    let arm04_rot = arm03_rot * bevy_wrist_rot;

    if let Some(&ent) = joint_entities.get(bones.azimuth)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = arm01_pos;
        tf.rotation = arm01_rot;
    }
    if let Some(&ent) = joint_entities.get(bones.shoulder)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = arm02_pos;
        tf.rotation = arm02_rot;
    }
    if let Some(&ent) = joint_entities.get(bones.elbow)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = arm03_pos;
        tf.rotation = arm03_rot;
    }
    if let Some(&ent) = joint_entities.get(bones.forearm)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = arm04_pos;
        tf.rotation = arm04_rot;
    }

    // Clamps: only their X offset changes (closed when carrying
    // a real grabbed object, open otherwise).  The 0.157 m
    // half-width comes from the C++ `0.314f/2.0f` constant.
    let (right_x, left_x) = if carrying && state != CraneState::Acquisition {
        (grab_radius, -grab_radius)
    } else {
        (0.157, -0.157)
    };
    let mut right_off = rest.right_claw_origin_oni;
    let mut left_off = rest.left_claw_origin_oni;
    right_off.x = right_x;
    left_off.x = left_x;
    let right_bevy = space::to_bevy_space_pos(right_off);
    let left_bevy = space::to_bevy_space_pos(left_off);
    // Clamp bones are children of arm_04 in the rest skeleton;
    // their actor-local pose composes with the wrist.
    let right_pos = arm04_pos + arm04_rot * right_bevy;
    let left_pos = arm04_pos + arm04_rot * left_bevy;
    if let Some(&ent) = joint_entities.get(bones.right_clamp)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = right_pos;
        tf.rotation = arm04_rot;
    }
    if let Some(&ent) = joint_entities.get(bones.left_clamp)
        && let Ok(mut tf) = joint_tf_query.get_mut(ent)
    {
        tf.translation = left_pos;
        tf.rotation = arm04_rot;
    }

    // Claw matrix = wrist global * RotateLocalX(π/2) *
    // RotateLocalY(π); ClampOffset rides along the resulting Z
    // axis.  Matches the `m_ClawMtx` construction in the C++
    // MoveTo.  Under our 180°-Y conjugation, an Oni-space
    // RotateX(θ) becomes a Bevy RotateX(-θ) and a Y rotation
    // passes through unchanged, so the post-twist is
    // `RotX(-π/2) * RotY(π)`.  The `(0, 0, ClampOffset)` offset
    // in Oni becomes `(0, 0, -clamp_offset)` in Bevy.
    let claw_twist = Quat::from_rotation_x(-PI / 2.0) * Quat::from_rotation_y(PI);
    let claw_rot = arm04_rot * claw_twist;
    let clamp_offset_local = Vec3::new(0.0, 0.0, -clamp_offset);
    let claw_local = arm04_pos + claw_rot * clamp_offset_local;

    // Mirror the IK pose into `Oni2DebugSkeleton.positions` for
    // the six bones we control.  Without this, the F4 debug
    // overlay reads the snapshot `update_oni2_animation` cached
    // *before* we ran and shows the underlying animation pose
    // (rest, for an idle crane), making the IK invisible to the
    // debugger.  The remaining un-IK'd bones (root, plus
    // anything past the clamps in non-EricArm skeletons) keep
    // their animation-system positions.
    if let Some(ds) = debug_skel {
        let mut write = |idx: usize, pos: Vec3| {
            if let Some(slot) = ds.positions.get_mut(idx) {
                *slot = pos;
            }
        };
        write(bones.azimuth, arm01_pos);
        write(bones.shoulder, arm02_pos);
        write(bones.elbow, arm03_pos);
        write(bones.forearm, arm04_pos);
        write(bones.right_clamp, right_pos);
        write(bones.left_clamp, left_pos);
    }

    (claw_local, claw_rot)
}

/// Current claw centre in actor-local Oni-space, sampled from the
/// solved angles.  Used by the C++ `AtTarget` convergence test;
/// here it lets the state machine know when to advance phases.
fn claw_pos_local_oni(
    rest: &CraneBoneRest,
    azimuth: f32,
    shoulder: f32,
    elbow: f32,
    clamp_offset: f32,
) -> Vec3 {
    // Walk the chain in Oni-space rest pose, applying the solved
    // rotations.  Every bone offset is pure Y so we only need the
    // composed rotations to project the chain.
    let azimuth_rot = Quat::from_rotation_y(-azimuth - PI / 2.0);
    let shoulder_rot = Quat::from_rotation_x(-shoulder);
    let elbow_rot = Quat::from_rotation_x(-elbow);
    let wrist_angle = PI / 2.0 - (shoulder + elbow);
    let wrist_rot = Quat::from_rotation_x(-wrist_angle);

    let arm01 = Vec3::new(0.0, rest.base_length, 0.0); // azimuth pivot is at base height
    // Already (azimuth_rot rotates a Y-only offset → ZERO yaw effect on the next Y-only offset).
    let arm02_offset = Vec3::new(0.0, 0.0, 0.0); // shoulder is co-located with the azimuth height by `base_length`
    let _ = arm02_offset;
    let arm01_rot = azimuth_rot;
    let arm02_pos = arm01;
    let arm02_rot = arm01_rot * shoulder_rot;
    let arm03_pos = arm02_pos + arm02_rot * Vec3::new(0.0, rest.bicep_length, 0.0);
    let arm03_rot = arm02_rot * elbow_rot;
    let arm04_pos = arm03_pos + arm03_rot * Vec3::new(0.0, rest.forearm_length, 0.0);
    let arm04_rot = arm03_rot * wrist_rot;
    // Claw matrix = wrist * RotX(π/2) * RotY(π); ClampOffset
    // along the resulting Z.  Pure-Oni math here so the
    // convergence test compares apples-to-apples with
    // `local_oni` (which is also in Oni-space).
    let claw_rot = arm04_rot * Quat::from_rotation_x(PI / 2.0) * Quat::from_rotation_y(PI);
    arm04_pos + claw_rot * Vec3::new(0.0, 0.0, clamp_offset)
}

/// Driver system.  Runs after `update_oni2_animation` so the IK
/// rotations override whatever pose the animation system just
/// wrote.  Each tick:
///   1. Init bone indices on demand (lazy, harmless on already-
///      initialised crane).
///   2. Health gate — a dead crane drops and stops solving
///      (matches C++ HandleUpdate health early-out).
///   3. Pick a goal based on state machine; if no goal, leave the
///      animation pose alone.
///   4. Convert goal → actor-local Oni, run `pos_to_angles`,
///      advance the rate-limited approach toward the new pose.
///   5. Apply the result via `apply_ik_pose`.
///   6. Advance state machine when the claw reaches the goal.
pub fn crane_ik_system(
    time: Res<Time>,
    mut ik_query: Query<(
        Entity,
        &mut ElbowCraneIK,
        &Oni2AnimState,
        &GlobalTransform,
        Option<&crate::combat::components::Health>,
        Option<&mut crate::oni2_loader::animation::Oni2DebugSkeleton>,
        Option<&crate::scroni::vm::ScrOniScript>,
        Option<&crate::oni2_loader::components::ActorAsleep>,
    )>,
    mut joint_tf_query: Query<&mut Transform>,
    target_gtf_query: Query<&GlobalTransform, Without<ElbowCraneIK>>,
    mut start_action_writer: bevy::prelude::MessageWriter<
        crate::animator::events::StartActionMessage,
    >,
    mut end_action_writer: bevy::prelude::MessageWriter<crate::animator::events::EndActionMessage>,
    hitch_query: Query<(Entity, &ElbowCraneHitch, &GlobalTransform)>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for (
        entity,
        mut ik,
        anim_state,
        actor_gtf,
        health_opt,
        mut debug_skel_opt,
        scroni_opt,
        asleep_opt,
    ) in &mut ik_query
    {
        // Death gate — a dead crane releases what it's holding and
        // freezes.  Mirrors `if (!health->IsAlive()) Drop(); return;`
        // in the C++ HandleUpdate.
        if let Some(health) = health_opt
            && health.current <= 0.0
        {
            if ik.carrying_object {
                info!(
                    "[crane] crane died while carrying — dropping held={:?}",
                    ik.held_entity
                );
                // End the carried actor's struggle anim and let it
                // fall.  Matches the C++ Drop() path being followed
                // by the Hitch's drop teleport when the crane dies.
                if let Some(held) = ik.held_entity
                    && ik.struggle_active
                {
                    end_action_writer.write(crate::animator::events::EndActionMessage {
                        entity: held,
                        action: crate::animator::components::MainAction::CraneStruggle,
                        subaction: -1,
                    });
                }
                ik.drop();
            }
            continue;
        }

        let was_already_initd = ik.bones.is_some();
        if !try_init(&mut ik, anim_state) {
            if !ik.init_logged {
                warn!(
                    "[crane] init FAILED: actor has ElbowCraneIK but skeleton \
                     doesn't declare arm_01..arm_04 / clamp_01_{{r,l}} — IK will \
                     stay idle.  Actor's bones: {:?}",
                    anim_state.skeleton.names
                );
                ik.init_logged = true;
            }
            continue;
        }
        // One-shot post-init diagnostic dump: catalogues everything
        // that gates the crane's ability to be driven by a script.
        // If we never see this line, init never ran (no actor with
        // ElbowCraneIK + matching skeleton).  If we see it but never
        // see `[crane] ScrOni pickup issued`, the script side never
        // fires — the dump below tells you why (no script attached,
        // actor asleep, no hitchable targets in the world, etc).
        if !was_already_initd {
            let hitchable: Vec<(Entity, Vec3, f32)> = hitch_query
                .iter()
                .map(|(e, _h, gtf)| {
                    let p = gtf.translation();
                    let d = p.distance(actor_gtf.translation());
                    (e, p, d)
                })
                .collect();
            info!(
                "[crane] post-init context: actor={:?} pos={:?} \
                 has_scroni={} scroni_owner={:?} asleep={} health_present={} \
                 hitchable_actors_in_world={} (entries below)",
                entity,
                actor_gtf.translation(),
                scroni_opt.is_some(),
                scroni_opt.map(|s| s.exec.owner),
                asleep_opt.is_some(),
                health_opt.is_some(),
                hitchable.len(),
            );
            for (e, pos, dist) in &hitchable {
                info!(
                    "[crane]   hitchable: {:?} pos={:?} dist_from_crane={:.2}",
                    e, pos, dist
                );
            }
            if scroni_opt.is_none() {
                warn!(
                    "[crane] actor {:?} has ElbowCraneIK but NO ScrOniScript — \
                     no script can issue pickup/dropoff (legacy XML should declare \
                     `<ScrOni><Filename .../></ScrOni>` on the crane actor)",
                    entity
                );
            }
            if asleep_opt.is_some() {
                info!(
                    "[crane] actor {:?} is currently ActorAsleep (informational; \
                     layout_loader intentionally ignores updatestate=\"Asleep\")",
                    entity
                );
            }
        }
        let bones = ik.bones.expect("init succeeded");
        let rest = ik.rest.clone().expect("init succeeded");

        // While acquiring, re-fetch the held actor's world
        // position each tick so a moving target stays tracked
        // (the C++ caches a pointer into the target's matrix and
        // dereferences it every HandleUpdate).
        if ik.state == CraneState::Acquisition
            && let Some(held) = ik.held_entity
            && let Ok(target_gtf) = target_gtf_query.get(held)
        {
            let mut goal = target_gtf.translation();
            goal.y += ik.held_hitch_center.y;
            ik.goal_world = goal;
        }

        // Releasing sub-state runs a one-shot timer with no IK
        // motion — claws are open, actor has been dropped, we're
        // just waiting `claw_time` seconds before reporting
        // dropoff done.  Mirrors the
        //   if ((TIME.GetElapsedTime() - m_TimeStamp) >= m_ClawTime)
        // arm in the C++ HandleUpdate.
        if matches!(ik.state, CraneState::MovingToTarget)
            && matches!(ik.moving_state, CraneMovingState::Releasing)
        {
            ik.release_timer += dt;
            if ik.release_timer >= ik.claw_time {
                info!(
                    "[crane] state MovingToTarget/Releasing -> Released (claw_time={}s elapsed)",
                    ik.claw_time
                );
                ik.state = CraneState::Released;
                ik.moving_state = CraneMovingState::Nothing;
            }
            continue;
        }

        // Pick the active goal in WORLD space.  Returns `None`
        // when the state machine has no target this tick (idle or
        // between substates) — leave the pose alone.
        //
        // Translate / Lower include `held_hitch_center.y` so the
        // claw lands at the actor's hitch-point height above the
        // dropoff floor (matches `Target.y += m_TargetHitchOffset.y`
        // in the C++ kTranslate/kLower branches).  Without this,
        // the dropped actor's feet would clip into the floor by
        // the hitch height.
        let world_goal = match (ik.state, ik.moving_state) {
            (CraneState::SeekDesiredLocation, _) => Some(ik.goal_world),
            (CraneState::Acquisition, _) => Some(ik.goal_world),
            (CraneState::MovingToTarget, CraneMovingState::Lift) => Some(ik.lift_world),
            (CraneState::MovingToTarget, CraneMovingState::Translate) => {
                let mut t = ik.dropoff_world;
                t.y += ik.held_hitch_center.y + ik.lift_height;
                Some(t)
            }
            (CraneState::MovingToTarget, CraneMovingState::Lower) => {
                let mut t = ik.dropoff_world;
                t.y += ik.held_hitch_center.y;
                Some(t)
            }
            _ => None,
        };

        let Some(world_goal) = world_goal else {
            continue;
        };

        // World → actor-local Oni.  The actor's GlobalTransform is
        // in Bevy space; invert it to drop the world goal into
        // actor-local Bevy, then conjugate to Oni-space.
        let local_bevy = actor_gtf.affine().inverse().transform_point3(world_goal);
        let local_oni = space::to_bevy_space_pos(local_bevy);

        // First-acquire seed: snap the approach origin to the
        // current end-effector position so the first solve doesn't
        // jerk from `(0,0,0)`.
        if ik.first_acquire {
            ik.last_target_local_oni = claw_pos_local_oni(
                &rest,
                ik.azimuth,
                ik.shoulder_angle,
                ik.elbow_angle,
                ik.clamp_offset,
            );
            ik.first_acquire = false;
        }

        // Rate-limited approach (radians/sec).  C++ multiplies the
        // speed (deg/s) by DtoR before passing into `Approach` so
        // we match that here.
        let rate = ik.speed_deg * PI / 180.0;
        approach_vec(&mut ik.last_target_local_oni, local_oni, rate, dt);

        let solved = pos_to_angles(
            ik.last_target_local_oni,
            rest.base_length,
            rest.bicep_length,
            rest.forearm_length,
            ik.clamp_offset,
        );

        if let Some((az, sh, el)) = solved {
            // The .skel limits are on the angle actually written
            // to the bone (`MakeRotateX(-m_ShoulderAngle)` in the
            // legacy), so they constrain `-angle`.  Negate the
            // limit pair (and swap min/max since negation
            // reverses order) so we clamp the solver output —
            // which is always positive in practice — to the
            // equivalent positive-angle range.
            let sh = if let Some(lim) = rest.shoulder_limit {
                sh.clamp(-lim[1], -lim[0])
            } else {
                sh
            };
            let el = if let Some(lim) = rest.elbow_limit {
                el.clamp(-lim[1], -lim[0])
            } else {
                el
            };
            ik.azimuth = az;
            ik.shoulder_angle = sh;
            ik.elbow_angle = el;
        }
        // Note: when `solved` is None we keep the prior pose
        // (matches C++ behaviour of leaving the chain alone when
        // the target is out of reach).

        let (claw_local, claw_local_rot) = apply_ik_pose(
            &bones,
            &rest,
            &anim_state.skeleton,
            ik.azimuth,
            ik.shoulder_angle,
            ik.elbow_angle,
            ik.carrying_object,
            ik.state,
            ik.grab_radius,
            ik.clamp_offset,
            &anim_state.joint_entities,
            &mut joint_tf_query,
            debug_skel_opt.as_deref_mut(),
        );

        // Lift the actor-local claw matrix into world space so the
        // carry system (and any future debug overlay) can read it.
        let actor_affine = actor_gtf.affine();
        ik.last_claw_world_pos = actor_affine.transform_point3(claw_local);
        ik.last_claw_world_rot = actor_gtf.rotation() * claw_local_rot;

        // Convergence test — see if the solved claw is close
        // enough to the goal to advance the state machine.
        //
        // Also implements the C++ `m_LastMag` / `m_RepeatCount`
        // stall-protection: when the squared distance to the
        // target doesn't change across consecutive ticks (the
        // arm has plateaued because the goal is outside the IK
        // envelope), AtTarget reports success anyway after a few
        // repeats so the state machine can advance instead of
        // hanging forever.  The C++ only arms the stall counter
        // during a carrying lift/lower; we mirror that.
        let claw_local = claw_pos_local_oni(
            &rest,
            ik.azimuth,
            ik.shoulder_angle,
            ik.elbow_angle,
            ik.clamp_offset,
        );
        let mag2 = (claw_local - local_oni).length_squared();
        let mut at_target = mag2 < 0.1;
        let stall_armed = ik.carrying_object
            && matches!(
                ik.moving_state,
                CraneMovingState::Lift | CraneMovingState::Lower
            );
        if !stall_armed {
            ik.repeat_count = 0;
        }
        if !at_target && stall_armed {
            if (mag2 - ik.last_mag2).abs() < 1e-6 {
                ik.repeat_count = ik.repeat_count.saturating_add(1);
                if ik.repeat_count > 4 {
                    warn!(
                        "ElbowCraneIK: target outside reach, advancing state machine \
                         (mag2={:.4}, state={:?}/{:?})",
                        mag2, ik.state, ik.moving_state
                    );
                    at_target = true;
                }
            } else {
                ik.repeat_count = 0;
            }
        }
        ik.last_mag2 = mag2;

        if at_target {
            match (ik.state, ik.moving_state) {
                (CraneState::Acquisition, _) => {
                    info!(
                        "[crane] state Acquisition -> Attached (claw reached target, \
                         held={:?})",
                        ik.held_entity
                    );
                    ik.state = CraneState::Attached;
                    ik.lift_world = world_goal;
                    ik.lift_world.y += ik.lift_height;
                    // Start the held actor's struggle anim on
                    // arrival.  Mirrors the legacy `HandlePickupMatrix`
                    // sending `StartAction(ACT_CRANESTRUGGLE)` to the
                    // grabbed actor (see elbowcranehitch.cpp).
                    if ik.struggle_pending
                        && let Some(held) = ik.held_entity
                    {
                        info!(
                            "[crane] emitting StartActionMessage(CraneStruggle) for held={:?}",
                            held
                        );
                        start_action_writer.write(crate::animator::events::StartActionMessage {
                            entity: held,
                            action: crate::animator::components::MainAction::CraneStruggle,
                            substate: -1,
                        });
                    }
                    ik.struggle_pending = false;
                    // Stall tracking re-arms for the next phase.
                    ik.last_mag2 = f32::INFINITY;
                    ik.repeat_count = 0;
                }
                (CraneState::MovingToTarget, CraneMovingState::Lift) => {
                    info!("[crane] moving_state Lift -> Translate");
                    ik.moving_state = CraneMovingState::Translate;
                    ik.last_mag2 = f32::INFINITY;
                    ik.repeat_count = 0;
                }
                (CraneState::MovingToTarget, CraneMovingState::Translate) => {
                    info!("[crane] moving_state Translate -> Lower");
                    ik.moving_state = CraneMovingState::Lower;
                    ik.last_mag2 = f32::INFINITY;
                    ik.repeat_count = 0;
                }
                (CraneState::MovingToTarget, CraneMovingState::Lower) => {
                    info!(
                        "[crane] moving_state Lower -> Releasing (drop at {:?})",
                        ik.dropoff_world
                    );
                    let dropped_at = ik.dropoff_world;
                    let released = ik.held_entity;
                    let was_struggling = ik.struggle_active;
                    ik.drop();
                    // End the held actor's struggle anim and let
                    // the drop-fixup teleport snap it back to
                    // upright. `drop()` cleared `held_entity` so
                    // grab the cached value.
                    if let Some(held) = released
                        && was_struggling
                    {
                        end_action_writer.write(crate::animator::events::EndActionMessage {
                            entity: held,
                            action: crate::animator::components::MainAction::CraneStruggle,
                            subaction: -1,
                        });
                    }
                    // Re-enter the dropoff state machine in the
                    // `Releasing` sub-phase — the timer block at
                    // the top of the loop will advance us to
                    // `Released` after `claw_time` seconds.
                    ik.state = CraneState::MovingToTarget;
                    ik.moving_state = CraneMovingState::Releasing;
                    ik.release_timer = 0.0;
                    if released.is_some() {
                        ik.last_claw_world_pos = dropped_at;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Per-tick teleport of any held actor onto the crane's claw.
/// Mirrors the C++ `HandleUpdate` on the Hitch component:
/// `m_LocalMtx = *m_PickupMtx; m_LocalMtx.d.y -= Center.y;
/// Broadcast(aMsgTeleport)`.  We do it from the crane side
/// (rather than driving from the hitch's pointer-to-claw) so we
/// don't need cross-frame pointer plumbing.
///
/// Runs after `crane_ik_system` so `last_claw_world_pos` is
/// already up to date this frame.
pub fn crane_carry_system(
    crane_query: Query<&ElbowCraneIK>,
    hitch_query: Query<&ElbowCraneHitch>,
    mut held_tf_query: Query<&mut Transform>,
) {
    for ik in &crane_query {
        let Some(held) = ik.held_entity else {
            continue;
        };
        // Only follow once the IK has actually started solving —
        // skip until the chain has run at least one tick so we
        // don't teleport the actor to (0,0,0) for one frame.
        if ik.last_claw_world_pos == Vec3::ZERO && ik.last_claw_world_rot == Quat::IDENTITY {
            continue;
        }
        let center_y = hitch_query
            .get(held)
            .map(|h| h.center.y)
            .unwrap_or(ik.held_hitch_center.y);
        if let Ok(mut tf) = held_tf_query.get_mut(held) {
            let mut pos = ik.last_claw_world_pos;
            pos.y -= center_y;
            tf.translation = pos;
            // Preserve held actor's yaw — the C++ uses the full
            // claw matrix here, but for the cargo objects this
            // ships with that ends up tilting them visibly with
            // the wrist angle, which isn't what gameplay wants.
            // If a future asset needs full-matrix follow, gate
            // this on a Hitch flag.
        }
    }
}

/// Set `ElbowCraneHitch.held_by` based on each crane's current
/// `held_entity`.  Keeps the hitch's reciprocal back-reference
/// in sync so other systems (rendering hints, behaviour gates)
/// can ask the held actor "are you being held, and by whom?".
/// Cleared automatically when the crane drops.
pub fn crane_hitch_backlink_system(
    crane_query: Query<(Entity, &ElbowCraneIK)>,
    mut hitch_query: Query<(Entity, &mut ElbowCraneHitch)>,
) {
    // Build a held-by map from cranes first so each hitch sees a
    // single authoritative value (handles the unlikely case of
    // multiple cranes trying to grab the same object — last one
    // wins, matching the C++ where the last `HandlePickupMatrix`
    // sender overwrites `m_PickupMtx`).
    use std::collections::HashMap;
    let mut held_by: HashMap<Entity, Entity> = HashMap::new();
    for (crane_ent, ik) in &crane_query {
        if let Some(held) = ik.held_entity {
            held_by.insert(held, crane_ent);
        }
    }
    for (hitch_ent, mut hitch) in &mut hitch_query {
        hitch.held_by = held_by.get(&hitch_ent).copied();
    }
}
