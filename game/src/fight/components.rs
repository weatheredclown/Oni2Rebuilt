/*
 * fight/components.rs — High-fidelity port of crFighter, ftFighterData, crGrab,
 * crBlockData, and related types from rb/src/fight/.
 *
 * FighterState   — per-entity mutable runtime (crFighter equivalent)
 * FighterType    — immutable per-type config (ftFighterData equivalent)
 * GrappleState   — active grapple holder state (crGrab runtime equivalent)
 * BlockDef       — one block definition (crBlockData equivalent)
 * BlockLibrary   — per-entity ordered list of BlockDefs
 * AnimControlBlock — ATDT timing windows for input queuing (crAtkAnimCtrlBlock)
 */
use bevy::prelude::*;

// ---------------------------------------------------------------------------
// Fighter flags (crFighterFlag)
// ---------------------------------------------------------------------------

pub mod fighter_flags {
    /// Was hit this frame (set by HIT_START, cleared next update).
    pub const HIT: u32 = 1 << 0;
    /// Just got hit — one-frame transition that latches HIT.
    pub const HIT_START: u32 = 1 << 1;
    /// An incoming attack is about to land (ETA from ATDT reactphase).
    pub const IMPENDING_HIT: u32 = 1 << 2;
    /// Impending-hit detected this frame (transition).
    pub const IH_START: u32 = 1 << 3;
    /// Face the attacker and drive the react animation.
    pub const FACE_AND_REACT: u32 = 1 << 4;
    /// Clear the strike target after the current attack ends.
    pub const CLEAR_STRIKETARGET: u32 = 1 << 5;
    /// Face the target after running to it (per-instance override of type default).
    pub const FACE_AFTER_RUN: u32 = 1 << 6;
    /// Has fired a weapon projectile this attack tick (multi-dir targeting guard).
    pub const HAS_FIRED: u32 = 1 << 7;
    /// Z-lock fight mode — pad commands processed in fight-stance context.
    pub const FIGHT_MODE: u32 = 1 << 8;
    /// Attacks that don't lock heading to strike target for their full duration.
    pub const CLEAR_ST_AFTER_FIRST_USE: u32 = 1 << 9;
    /// Grapple queued but not yet active.
    pub const GRAPPLE_PENDING: u32 = 1 << 10;
}

// ---------------------------------------------------------------------------
// Block status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockStatus {
    #[default]
    NotBlocking,
    /// Block is active but not yet tested against any incoming attack this frame.
    Untested,
    /// Block successfully deflected an attack this frame.
    Successful,
}

// ---------------------------------------------------------------------------
// Animation control block (crAtkAnimCtrlBlock)
// ---------------------------------------------------------------------------

/// Normalized [0..1] timing windows that govern when input can be queued and
/// when the CRITICAL_FRAME branch fires.  Populated from the active ATDT strike's
/// opp2_*/opp3_* fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimControlBlock {
    /// Phase at which the first-opportunity action must have ended.
    pub opp1_do_end: f32,
    /// Earliest phase at which input queuing is allowed.
    pub opp2_q_start: f32,
    /// Phase at which the CRITICAL_FRAME branch window opens (can fire immediately).
    pub opp2_do_start: f32,
    /// Phase at which the CRITICAL_FRAME condition becomes true.
    pub opp2_q_crit_start: f32,
    /// Phase at which queued input fires immediately.
    pub opp2_do_crit_start: f32,
    /// Phase at which the CRITICAL_FRAME window closes.
    pub opp2_do_end: f32,
    /// Third-opportunity queue-open phase.
    pub opp3_q_start: f32,
    /// Third-opportunity execute phase.
    pub opp3_do_start: f32,
}

impl AnimControlBlock {
    pub fn in_critical_frame(&self, phase: f32) -> bool {
        phase >= self.opp2_do_start && phase <= self.opp2_do_end
    }
    pub fn can_queue(&self, phase: f32) -> bool {
        phase >= self.opp2_q_start
    }
    pub fn can_start_immediately(&self, phase: f32) -> bool {
        phase >= self.opp2_do_start && phase <= self.opp2_do_end
    }
}

// ---------------------------------------------------------------------------
// FighterState — crFighter equivalent
// ---------------------------------------------------------------------------

/// Core per-entity fighter runtime state.
/// Add alongside Fighter for any entity that participates in the fight system
/// (player or AI).  Mirrors crFighter from rb/src/fight/fighter.h.
#[derive(Component)]
pub struct FighterState {
    // --- Flags ---
    /// Internal bitfield (see fighter_flags).
    pub flags: u32,

    // --- Block ---
    pub block_status: BlockStatus,
    /// Index of the active BlockDef in BlockLibrary (-1 = none).
    pub cur_block: i32,
    /// Is the block input button currently held?
    pub block_held: bool,
    /// Was a counter-attack already executed in this block window?
    pub block_counter_executed: bool,
    /// Absolute time (f64 seconds) when the current block started.
    pub block_start_time: f64,
    /// Consecutive hits received while blocking (for ComboCountBeforeCausingReact).
    pub block_combo_count: i32,

    // --- Targeting ---
    /// Current strike lock-on target.
    pub strike_target: Option<Entity>,
    /// Flags whether strike_target should be cleared on the next frame.
    pub clear_st_after_first_use: bool,
    /// Entity this fighter is grappling (attacker role).
    pub grapple_target: Option<Entity>,
    /// Entity that is grappling this fighter (victim role).
    pub grapple_attacker: Option<Entity>,

    // --- Super meter ---
    /// Super meter [0.0 = empty, 1.0 = full; can briefly exceed 1.0].
    pub super_meter: f32,

    // --- Reaction ---
    /// Pending/active reaction animation (animReactEnum; -1 = none).
    pub react_anim: i32,
    pub react_strength: crate::combat::components::AttackStrength,
    /// World-space knockback vector for this reaction.
    pub react_distance: Vec3,

    // --- Invulnerability phases (from ReactData) ---
    /// Normalized phase within the react animation when invulnerability begins.
    pub invulnerability_start_phase: f32,
    /// Normalized phase after which no further damage is accepted this reaction.
    pub no_react_start_phase: f32,
    /// True while we're inside the post-react invulnerability window.
    pub in_invuln_phase: bool,

    // --- Fight stance timer ---
    /// Counts down from FighterType.leave_fight_stance_delay; 0 → exit fight stance.
    pub leave_fight_stance_timer: f32,

    // --- Successive attacks / combo escalation ---
    /// How many hits in a row have landed on the same target.
    pub num_successive_attacks: i32,
    /// Position in the current combo chain.
    pub cur_combo_index: i32,
    /// Allow queuing the next attack (disabled by some grapple attacks).
    pub queue_attacks: bool,

    // --- Rotation tracking ---
    /// Radians of the last attack heading (for 20° strike-target clear check).
    pub last_attack_angle: f32,
    /// Turn interpolation lerp parameter [0..1].
    pub turn_lerper: f32,
    /// Face-and-react rotation notches queued for the rotation system.
    pub fr_notches: i32,
    /// Initial-rotation notches to apply at attack start.
    pub ir_notches: i32,
    /// Counter-rotation notches to apply after the attack animation ends.
    pub counter_rotation_notches: i32,

    // --- Blend-out parameters ---
    /// Normalized phase at which the current animation blends out.
    pub anim_blend_out_opp: f32,
    pub anim_blend_out_phase_length: f32,

    // --- Hit prediction / about-to-be-hit ---
    /// Seconds until the incoming strike lands (0.0 = not incoming).
    pub hit_eta_seconds: f32,
    /// World-space position of the entity about to strike us.
    pub incoming_attacker_pos: Vec3,
    /// Hit type of the incoming attack.
    pub incoming_attack_hit_type: u8,

    // --- Hit type history ---
    pub last_hit_type: i32,
    pub last_successive_react_hit_type: i32,

    // --- Active animation timing windows ---
    pub anim_control_block: Option<AnimControlBlock>,

    // --- Post-reaction ---
    /// Animation name to play immediately after a knockdown react completes.
    pub pending_getup_anim: Option<String>,
    /// End-rotation notches to apply once the current react finishes.
    pub pending_end_rotation_notches: i32,

    // --- Per-attack hit log (for HasHitClass) ---
    /// FighterType.creature_class values of every target the current/most-
    /// recent attack has landed on.  Mirrors `crAttack::TargetsHit`.
    /// `record_attack_hit` appends; `reset_attack_hits` clears at attack
    /// start.  Used by `has_hit_class` for combo-class predicates.
    pub attack_hit_classes: Vec<u32>,
}

impl Default for FighterState {
    fn default() -> Self {
        Self {
            flags: 0,
            block_status: BlockStatus::NotBlocking,
            cur_block: -1,
            block_held: false,
            block_counter_executed: false,
            block_start_time: 0.0,
            block_combo_count: 0,
            strike_target: None,
            clear_st_after_first_use: false,
            grapple_target: None,
            grapple_attacker: None,
            super_meter: 0.0,
            react_anim: -1,
            react_strength: crate::combat::components::AttackStrength::Low,
            react_distance: Vec3::ZERO,
            invulnerability_start_phase: 0.0,
            no_react_start_phase: 0.0,
            in_invuln_phase: false,
            leave_fight_stance_timer: 0.0,
            num_successive_attacks: 0,
            cur_combo_index: 0,
            queue_attacks: true,
            last_attack_angle: 0.0,
            turn_lerper: 0.0,
            fr_notches: 0,
            ir_notches: 0,
            counter_rotation_notches: 0,
            anim_blend_out_opp: 0.0,
            anim_blend_out_phase_length: 0.0,
            hit_eta_seconds: 0.0,
            incoming_attacker_pos: Vec3::ZERO,
            incoming_attack_hit_type: 0,
            last_hit_type: -1,
            last_successive_react_hit_type: -1,
            anim_control_block: None,
            pending_getup_anim: None,
            pending_end_rotation_notches: 0,
            attack_hit_classes: Vec::new(),
        }
    }
}

impl FighterState {
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }
    pub fn clear_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }
    pub fn set_flag_val(&mut self, flag: u32, val: bool) {
        if val {
            self.set_flag(flag);
        } else {
            self.clear_flag(flag);
        }
    }

    pub fn is_grappling(&self) -> bool {
        self.grapple_target.is_some()
    }
    pub fn is_being_grappled(&self) -> bool {
        self.grapple_attacker.is_some()
    }
    pub fn is_blocking(&self) -> bool {
        self.cur_block >= 0
    }
    pub fn in_fight_mode(&self) -> bool {
        self.has_flag(fighter_flags::FIGHT_MODE)
    }

    // -----------------------------------------------------------------------
    // C++ crFighter predicate ports
    //
    // The legacy IsAttacking / IsBlockQueued / HasHitClass had direct access
    // to the fighter's anim list.  In our ECS the equivalent state lives on
    // separate components, so each predicate takes only the data it actually
    // needs — keeps these callable from any system without per-call queries.
    // -----------------------------------------------------------------------

    /// True when an attack animation is actively playing at full speed and
    /// we're not currently in a block.  Mirrors `crFighter::IsAttacking`
    /// (rb/src/fight/fighter.cpp:542) — the C++ checks `channel.scale == 1.0
    /// && !paused && !IsBlocking()`; we apply the same gates against
    /// `Oni2AnimState`.  Caller passes the entity's active anim state.
    pub fn is_attacking(
        &self,
        anim: &crate::oni2_loader::animation::Oni2AnimState,
    ) -> bool {
        if self.is_blocking() {
            return false;
        }
        if anim.paused {
            return false;
        }
        if anim.anim.attack_data.is_none() {
            return false;
        }
        anim.speed_multiplier >= 0.999
    }

    /// True when the player has requested a block (button held) but the
    /// block animation hasn't yet started — i.e. the block is "queued"
    /// awaiting a permitting state.  Mirrors `crFighter::IsBlockQueued`
    /// (rb/src/fight/fighter.cpp:1037), adapted for our pipeline: the C++
    /// scans `AnimList` for a queued block record, but we don't push blocks
    /// onto `AnimSchedule`, so we infer "queued" from request-vs-active.
    pub fn is_block_queued(&self) -> bool {
        self.block_held && self.cur_block < 0
    }

    /// True iff the current/most-recent attack has landed on a target
    /// whose `FighterType.creature_class` includes `cls`.  Mirrors
    /// `crFighter::HasHitClass` (rb/src/fight/fighter.cpp:520) which
    /// forwards to `crAttack::HasHitClass` (attack.cpp:166) — that walks
    /// the attack's `TargetsHit` array and tests each target's class.
    /// Update via `record_attack_hit` (on hit landing) and clear via
    /// `reset_attack_hits` (on attack start).
    pub fn has_hit_class(&self, cls: u32) -> bool {
        self.attack_hit_classes.iter().any(|&c| c == cls)
    }

    /// Record a hit landing during the current attack — appends the target's
    /// creature_class to the per-attack hit log.  Idempotent: duplicate
    /// classes only land once.
    pub fn record_attack_hit(&mut self, target_class: u32) {
        if !self.attack_hit_classes.contains(&target_class) {
            self.attack_hit_classes.push(target_class);
        }
    }

    /// Clear the per-attack hit log.  Call when a new attack starts so the
    /// next `has_hit_class` query reflects only the new attack's targets.
    pub fn reset_attack_hits(&mut self) {
        self.attack_hit_classes.clear();
    }
}

#[cfg(test)]
mod predicate_tests {
    use super::*;

    #[test]
    fn block_queued_only_when_held_and_not_active() {
        let mut s = FighterState::default();
        assert!(!s.is_block_queued(), "default — not held, not blocking");
        s.block_held = true;
        assert!(s.is_block_queued(), "held, not started yet → queued");
        s.cur_block = 0;
        assert!(
            !s.is_block_queued(),
            "held but block now active → no longer queued"
        );
    }

    #[test]
    fn hit_class_log_is_idempotent() {
        let mut s = FighterState::default();
        assert!(!s.has_hit_class(7));
        s.record_attack_hit(7);
        s.record_attack_hit(7);
        s.record_attack_hit(11);
        assert!(s.has_hit_class(7));
        assert!(s.has_hit_class(11));
        assert_eq!(s.attack_hit_classes.len(), 2, "duplicates dropped");
        s.reset_attack_hits();
        assert!(!s.has_hit_class(7));
        assert!(s.attack_hit_classes.is_empty());
    }

    // is_attacking() takes Oni2AnimState which has many fields wired through
    // the asset pipeline; covering it here would pull in mock infrastructure
    // that isn't worth the test for this small predicate.  The body is
    // straight boolean composition over already-tested fields.
}

// Trailing brace to close the impl block; the test mod above moved it from
// its original position.  Keeping the impl block closed here preserves the
// original layout for downstream items in this file.
impl FighterState {

    /// Tick the per-frame state transitions:
    /// - HIT_START → HIT (set for one frame then clear)
    /// - IH_START → IMPENDING_HIT (set for one frame then clear)
    /// - Decay hit_eta_seconds by dt
    pub fn tick_frame_flags(&mut self, dt: f32) {
        // HIT_START latches HIT for one frame then clears both
        if self.has_flag(fighter_flags::HIT_START) {
            self.set_flag(fighter_flags::HIT);
            self.clear_flag(fighter_flags::HIT_START);
        } else {
            self.clear_flag(fighter_flags::HIT);
        }

        // IH_START latches IMPENDING_HIT for one frame then clears both
        if self.has_flag(fighter_flags::IH_START) {
            self.set_flag(fighter_flags::IMPENDING_HIT);
            self.clear_flag(fighter_flags::IH_START);
        } else {
            self.clear_flag(fighter_flags::IMPENDING_HIT);
        }

        if self.hit_eta_seconds > 0.0 {
            self.hit_eta_seconds = (self.hit_eta_seconds - dt).max(0.0);
        }
    }
}

// ---------------------------------------------------------------------------
// FighterType — ftFighterData equivalent
// ---------------------------------------------------------------------------

/// Immutable per-entity-type configuration loaded from .ft data files.
/// One instance is shared by all entities of the same character type.
#[derive(Component, Clone, Debug)]
pub struct FighterType {
    pub name: String,
    /// Seconds of inactivity before the entity exits fight stance.
    pub leave_fight_stance_delay: f32,
    /// Super meter empties from 1.0 to 0.0 in this many seconds.
    pub super_meter_decay: f32,
    /// A grapple auto-breaks after this many seconds (0.0 = infinite).
    pub grapple_break_time: f32,
    /// Consecutive Level-0 hits before reaction escalates.
    pub successive_level0_reacts: i32,
    /// Face the target after a running approach.
    pub face_after_run: bool,
    /// Seconds to run before facing after arrival.
    pub run_before_face: f32,
    /// Only attack entities that are actually enemies (faction check).
    pub only_fight_enemies: bool,
    /// Automatically enter fight stance when an enemy is nearby.
    pub auto_fight_stance: bool,
    /// Degrees of random noise applied to attack heading.
    pub strike_noise_range: f32,
    /// Creature-class bitmask for HasHitClass queries.
    pub creature_class: u32,
}

impl Default for FighterType {
    fn default() -> Self {
        Self {
            name: String::new(),
            leave_fight_stance_delay: 3.0,
            super_meter_decay: 10.0,
            grapple_break_time: 5.0,
            successive_level0_reacts: 3,
            face_after_run: true,
            run_before_face: 0.5,
            only_fight_enemies: true,
            auto_fight_stance: true,
            strike_noise_range: 5.0,
            creature_class: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// GrappleState — crGrab runtime state
// ---------------------------------------------------------------------------

pub mod grapple_flags {
    pub const BREAKING: u32 = 1 << 0;
    pub const BROKEN: u32 = 1 << 1;
    pub const BOUNDS_REMOVED: u32 = 1 << 2;
    pub const STARTED: u32 = 1 << 3;
    pub const INITIAL_GRAB_DONE: u32 = 1 << 4;
}

/// Commands a grapple holder can issue to the grapple state machine.
/// Mirrors GRAB_ACTION_* from rb/src/fight/grab.h.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrabAction {
    #[default]
    None,
    End,
    TurnLeft,
    TurnRight,
    MoveForward,
    MoveBackward,
    Shake,
    Throw,
    Die,
}

impl GrabAction {
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().trim_matches('"') {
            "END" => GrabAction::End,
            "TURN_LEFT" => GrabAction::TurnLeft,
            "TURN_RIGHT" => GrabAction::TurnRight,
            "MOVE_FORWARD" | "FORWARD" => GrabAction::MoveForward,
            "MOVE_BACKWARD" | "BACKWARD" => GrabAction::MoveBackward,
            "SHAKE" => GrabAction::Shake,
            "THROW" => GrabAction::Throw,
            "DIE" => GrabAction::Die,
            _ => GrabAction::None,
        }
    }
}

/// Per-entity grapple holder state.  Present only on the entity that initiated
/// the grapple.  Mirrors the runtime fields of crGrab.
#[derive(Component, Default)]
pub struct GrappleState {
    pub flags: u32,
    /// Resistance shake [0.0 = none, 1.0 = break threshold].
    pub shake_amt: f32,
    /// Seconds shake_amt has been >= 1.0 (must exceed shake_loose_time to break).
    pub full_shake_timer: f32,
    /// Holder's health at grab start (for damage-break threshold).
    pub grab_start_health: f32,
    /// Absolute time the grapple started (for grapple_break_time check).
    pub grab_start_time: f64,
    /// Holder-relative idle offset to position the target.
    pub idle_offset: Vec3,
    /// Navigation rotation notches pending from a Nav() call.
    pub nav_rotation_notches: i32,
    /// Time (seconds) for progressive shake to reach maximum.
    pub time_till_max_shake: f32,
    /// Seconds shake must stay at maximum before the grapple breaks.
    pub shake_loose_time: f32,
    /// Pending action dispatched this frame by the FSM CtrlGrapple output.
    pub pending_action: GrabAction,
}

impl GrappleState {
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }
    pub fn clear_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }

    pub fn is_breaking(&self) -> bool {
        self.has_flag(grapple_flags::BREAKING)
    }
    pub fn is_broken(&self) -> bool {
        self.has_flag(grapple_flags::BROKEN)
    }

    /// Apply one shake tick (called by SHAKE action).
    /// Returns true when the grapple should break.
    pub fn tick_shake(&mut self, dt: f32) -> bool {
        if self.is_breaking() || self.is_broken() {
            return false;
        }
        if self.time_till_max_shake > 0.0 {
            self.shake_amt = (self.shake_amt + dt / self.time_till_max_shake).min(1.0);
        }
        if self.shake_amt >= 1.0 {
            self.full_shake_timer += dt;
            if self.full_shake_timer >= self.shake_loose_time {
                return true;
            }
        }
        false
    }

    /// Check whether the grapple has timed out.
    pub fn is_timed_out(&self, now: f64, break_time: f32) -> bool {
        break_time > 0.0 && (now - self.grab_start_time) >= break_time as f64
    }
}

// ---------------------------------------------------------------------------
// BlockDef — crBlockData equivalent
// ---------------------------------------------------------------------------

/// One block definition for a character type.
/// Loaded from entity-type .blk files (or constructed at spawn time for simple cases).
#[derive(Debug, Clone, Default)]
pub struct BlockDef {
    /// animBlockEnum index (maps to an animation in the library).
    pub anim_index: i32,
    /// Center heading of the block arc, relative to entity facing (radians).
    pub heading_radians: f32,
    /// Half-arc of the block zone.  Full blockable arc = 2 × width_radians.
    pub width_radians: f32,
    /// Block is active only while the block animation phase is in [start_phase, end_phase).
    pub start_phase: f32,
    pub end_phase: f32,
    /// Playback rate for the block animation.
    pub rate: f32,
    /// Phase at which the hold-loop midpoint begins.
    pub anim_mid_point: f32,
    /// Phase at which the hold-loop midpoint ends.
    pub anim_mid_point_end: f32,
    /// Maximum seconds the block button can be held (0.0 = unlimited).
    pub max_hold_button: f32,
    /// Bitmask of hit types this block can deflect (0 = accept all).
    pub blockable_hit_types: u32,
    /// Consecutive hits-while-blocking before forcing a block-break react on
    /// the defender (combo pressure counter).
    pub combo_count_before_react: i32,
    /// Automatically execute the counter-attack on a successful block.
    pub auto_counter: bool,
    /// animReactEnum per hittype to play on the defender when the block fails.
    pub failed_block_react_anims: Vec<i32>,
    /// Counter-attack animation alias name on successful block.
    pub counter_atk: Option<String>,
    /// Override block animation name on success (instead of default block anim).
    pub successful_block_anim: Option<String>,
    /// animReactEnum to force on the *attacker* whose attack was blocked.
    pub block_reaction_on_attacker: i32,
    /// animBlockEnum to play as the secondary block animation on the defender.
    pub enemy_block_anim: i32,
}

impl BlockDef {
    /// True if this block can deflect a hit from `from_world_pos` at
    /// block-animation `phase` with `hit_type`.
    ///
    /// Mirrors crBlockData::CanBlock(headingFromEnemy, from, currentPhase, hitType).
    pub fn can_block(
        &self,
        defender_pos: Vec3,
        defender_facing: Vec3,
        from_world_pos: Vec3,
        phase: f32,
        hit_type: u8,
    ) -> bool {
        if phase < self.start_phase || phase >= self.end_phase {
            return false;
        }
        if self.blockable_hit_types != 0 && (self.blockable_hit_types & (1u32 << hit_type)) == 0 {
            return false;
        }
        // Angular check: attacker must be within block heading ± width_radians
        let diff_xz = Vec2::new(
            from_world_pos.x - defender_pos.x,
            from_world_pos.z - defender_pos.z,
        );
        let dist_xz = diff_xz.length();
        if dist_xz < 0.001 {
            return true; // point-blank, always blocks
        }
        let to_attacker = diff_xz / dist_xz;
        let facing_angle = defender_facing.z.atan2(defender_facing.x);
        let block_angle = facing_angle + self.heading_radians;
        let block_dir = Vec2::new(block_angle.cos(), block_angle.sin());
        let dot = block_dir.dot(to_attacker).clamp(-1.0, 1.0);
        dot.acos() <= self.width_radians
    }

    /// Returns the animReactEnum to play on the defender when a block fails
    /// for the given hit_type.
    pub fn failed_react_for(&self, hit_type: u8) -> i32 {
        self.failed_block_react_anims
            .get(hit_type as usize)
            .copied()
            .unwrap_or(0) // 0 = ANIMREACT_REGULAR
    }
}

// ---------------------------------------------------------------------------
// BlockLibrary
// ---------------------------------------------------------------------------

/// Per-entity ordered list of BlockDefs.
/// Indexed by the animBlockEnum value emitted by FSM DoBlock actions.
#[derive(Component, Default, Clone)]
pub struct BlockLibrary {
    pub blocks: Vec<BlockDef>,
}

impl BlockLibrary {
    pub fn get(&self, index: i32) -> Option<&BlockDef> {
        if index < 0 {
            return None;
        }
        self.blocks.get(index as usize)
    }

    /// Find the first BlockDef that can deflect this hit.
    /// Returns (index, &BlockDef).
    pub fn find_block(
        &self,
        defender_pos: Vec3,
        defender_facing: Vec3,
        from: Vec3,
        phase: f32,
        hit_type: u8,
    ) -> Option<(i32, &BlockDef)> {
        for (i, block) in self.blocks.iter().enumerate() {
            if block.can_block(defender_pos, defender_facing, from, phase, hit_type) {
                return Some((i as i32, block));
            }
        }
        None
    }
}
