/*
 * animator/components.rs — ActionPlayer and related types.
 *
 * ActionPlayer is the Rust equivalent of crAnimActionPlayer — the high-level
 * character action state machine that drives jump, slide, crouch, fight stance,
 * draw weapon, fall, lie down, react, die, evade, ledge hang, play custom anim,
 * crane struggle, pickup, drop item, zipline, and transition actions.
 *
 * Each action has a reject list (flags that must NOT be set to start it) and an
 * override list (flags that get cleared when it starts).  SubState0 stores the
 * last substate requested per action, SubState1 tracks the current animation
 * substate (ACT_S1_*).
 */
use bevy::prelude::*;

// ---------------------------------------------------------------------------
// MainAction — the top-level action enum (mirrors crMainActionEnums)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum MainAction {
    #[default]
    None = 0,
    Jump = 1,
    Slide = 2,
    Crouch = 3,
    FightStance = 4,
    DrawWeapon = 5,
    Fall = 6,
    LieDown = 7,
    React = 8,
    Die = 9,
    Evade = 10,
    LedgeHang = 11,
    PlayCustomAnim = 12,
    CraneStruggle = 13,
    Pickup = 14,
    DropItem = 15,
    ZipLine = 16,
    Transition = 17,
}

pub const ACT_NUM_ACTIONS: usize = 18;

impl MainAction {
    pub fn from_index(i: usize) -> Option<Self> {
        use MainAction::*;
        match i {
            0 => Some(None),
            1 => Some(Jump),
            2 => Some(Slide),
            3 => Some(Crouch),
            4 => Some(FightStance),
            5 => Some(DrawWeapon),
            6 => Some(Fall),
            7 => Some(LieDown),
            8 => Some(React),
            9 => Some(Die),
            10 => Some(Evade),
            11 => Some(LedgeHang),
            12 => Some(PlayCustomAnim),
            13 => Some(CraneStruggle),
            14 => Some(Pickup),
            15 => Some(DropItem),
            16 => Some(ZipLine),
            17 => Some(Transition),
            _ => Option::None,
        }
    }
}

// ---------------------------------------------------------------------------
// SubState0 — adverbs passed into StartAction (per-action substate)
// Stored as i32 in ActionPlayer.sub_state_0[ACT_NUM_ACTIONS].
// -1 = ACT_S0_NONE.
// ---------------------------------------------------------------------------

pub mod sub_state_0 {
    pub const NONE: i32 = -1;

    // Jump substates
    pub const JUMP_UP: i32 = 0;
    pub const JUMP_FORWARD: i32 = 1;
    pub const JUMP_LEFT: i32 = 2;
    pub const JUMP_RIGHT: i32 = 3;
    pub const JUMP_BACK: i32 = 4;
    pub const JUMP_LEFT_EVADE: i32 = 5;
    pub const JUMP_RIGHT_EVADE: i32 = 6;
    pub const JUMP_BACK_EVADE: i32 = 7;
    pub const JUMP_SOMERSAULT: i32 = 8;
    pub const JUMP_WALL_COMPRESS: i32 = 9;
    pub const JUMP_WALL_SPRING: i32 = 10;

    // Draw weapon substates
    pub const DRAWWEAPON_HAS_TGT: i32 = 11;
    pub const DRAWWEAPON_NO_TGT: i32 = 12;
    pub const DRAWWEAPON_DISARM_FROM_GRAPPLE: i32 = 13;
    pub const DRAWWEAPON_FROM_GROUND: i32 = 14;

    // Ledge hang substates
    pub const LEDGEHANG_GRAB: i32 = 15;
    pub const LEDGEHANG_HOLD: i32 = 16;
    pub const LEDGEHANG_LEFT: i32 = 17;
    pub const LEDGEHANG_RIGHT: i32 = 18;
    pub const LEDGEHANG_CLAMBER: i32 = 19;
    pub const LEDGEHANG_DROP: i32 = 20;

    pub const CRANESTRUGGLE_NOCLEAR: i32 = 21;

    // Zipline substates
    pub const ZIPLINE_SLIDE: i32 = 22;
    pub const ZIPLINE_NAV: i32 = 23;
    pub const ZIPLINE_TURN: i32 = 24;

    // Transition substates
    pub const TRANSITION_WEAPON_DRAW: i32 = 25;
    pub const TRANSITION_CROUCH_START: i32 = 26;
    pub const TRANSITION_CROUCH_END: i32 = 27;
    pub const TRANSITION_FIGHTSTANCE_START: i32 = 28;
    pub const TRANSITION_FIGHTSTANCE_END: i32 = 29;

    pub const CROUCH_DURING_ATK: i32 = 30;
}

// ---------------------------------------------------------------------------
// SubState1 — current animation substate id (single value for the ActionPlayer)
// ---------------------------------------------------------------------------

pub mod sub_state_1 {
    pub const IDLE: i32 = -1;
    pub const JUMP_COMPRESS: i32 = 0;
    pub const JUMP_SOMERSAULT: i32 = 1;
    pub const JUMP_WALL_COMPRESS: i32 = 2;
    pub const JUMP_WALL_SPRING: i32 = 3;
    pub const JUMP_MAIN: i32 = 4;
    pub const JUMP_LAND: i32 = 5;
    pub const REACT: i32 = 6;
    pub const DIE: i32 = 7;
    pub const WEAPON_GET_FROM_HOLSTER: i32 = 8;
    pub const WEAPON_DROP_TO_HOLSTER: i32 = 9;
    pub const WEAPON_READY_TO_TARGET: i32 = 10;
    pub const WEAPON_ACTION: i32 = 11;
    pub const LEDGE_GRAB: i32 = 12;
    pub const LEDGE_HOLD: i32 = 13;
    pub const LEDGE_CLAMBER: i32 = 14;
    pub const LEDGE_STAND: i32 = 15;
    pub const LEDGE_DROP: i32 = 16;
    pub const CRANE_STRUGGLE: i32 = 17;
    pub const CUSTOM_ANIM: i32 = 18;
    pub const PICKUP: i32 = 19;
    pub const TRANSITION: i32 = 20;
    pub const EVADE: i32 = 21;
}

// ---------------------------------------------------------------------------
// Adverbs for EndAction()
// ---------------------------------------------------------------------------

pub mod end_adverb {
    pub const LIE_2CROUCH: i32 = 0;
    pub const LIE_2STAND: i32 = 1;
}

// ---------------------------------------------------------------------------
// Present-participle flags (ACT_FLAG_*)
// ---------------------------------------------------------------------------

pub mod action_flags {
    pub const CROUCHING: u32 = 0x0001;
    pub const FIGHTSTANCE: u32 = 0x0002;
    pub const JUMPING: u32 = 0x0004;
    pub const SLIDING: u32 = 0x0008;
    pub const DOUBLE_JUMPING: u32 = 0x0010;
    pub const LYINGDOWN: u32 = 0x0020;
    pub const WEAPON: u32 = 0x0040;
    pub const RUNNING_JUMP: u32 = 0x0080;
    pub const STANDING_JUMP: u32 = 0x0100;
    pub const LEDGEHANGING: u32 = 0x0200;
    pub const DEAD: u32 = 0x0800;
    pub const ZIPLINE: u32 = 0x1000;

    // Override lists — flags cleared when a given action starts
    pub const OVERRIDELIST_SLIDE: u32 = RUNNING_JUMP | STANDING_JUMP | JUMPING | DOUBLE_JUMPING;
    pub const OVERRIDELIST_FALL: u32 = SLIDING;
    pub const OVERRIDELIST_FIGHTSTANCE: u32 = 0;
    pub const OVERRIDELIST_JUMP: u32 = SLIDING;
    pub const OVERRIDELIST_STRUGGLE: u32 = RUNNING_JUMP | STANDING_JUMP | JUMPING | DOUBLE_JUMPING;
    pub const OVERRIDELIST_ZIPLINE: u32 =
        RUNNING_JUMP | STANDING_JUMP | JUMPING | DOUBLE_JUMPING | CROUCHING | SLIDING;

    // Reject lists — if any of these bits are set, the action is denied
    pub const REJECTLIST_FIGHTSTANCE: u32 = DEAD | WEAPON | FIGHTSTANCE | LEDGEHANGING | ZIPLINE;
    pub const REJECTLIST_DRAWWEAPON: u32 = DEAD | JUMPING | WEAPON | LEDGEHANGING | ZIPLINE;
    pub const REJECTLIST_CROUCH: u32 =
        DEAD | JUMPING | SLIDING | CROUCHING | LEDGEHANGING | ZIPLINE;
    pub const REJECTLIST_LIE: u32 = DEAD | JUMPING | SLIDING | LEDGEHANGING | ZIPLINE;
    pub const REJECTLIST_JUMP: u32 = DEAD | CROUCHING | DOUBLE_JUMPING | ZIPLINE;
    pub const REJECTLIST_REACT: u32 = JUMPING | LEDGEHANGING | ZIPLINE;
    pub const REJECTLIST_EVADE: u32 =
        DEAD | CROUCHING | LEDGEHANGING | DOUBLE_JUMPING | RUNNING_JUMP | STANDING_JUMP | ZIPLINE;
    pub const REJECTLIST_FALL: u32 = LEDGEHANGING | DEAD | ZIPLINE;
    pub const REJECTLIST_PICKUP: u32 = LEDGEHANGING | DEAD | JUMPING;

    // Clear lists — flags removed when ending a given action
    pub const CLEARLIST_JUMP: u32 = JUMPING | DOUBLE_JUMPING | RUNNING_JUMP | STANDING_JUMP;
}

// ---------------------------------------------------------------------------
// Pending event flags (PENDING_*)
// ---------------------------------------------------------------------------

pub mod pending_flags {
    pub const HOLSTER: u32 = 0x0001;
    pub const DRAW: u32 = 0x0002;
    pub const ARM_IK: u32 = 0x0004;
    pub const HOLSTER_ANIM: u32 = 0x0008;
    pub const CROUCH_PICKUP: u32 = 0x0010;
}

// ---------------------------------------------------------------------------
// ActionResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionResult {
    /// Action is missing one or more required assets (animation not in library).
    Failed,
    /// Action not allowed — reject-list flags are set.
    Denied,
    /// Action succeeded — animation started, flags updated.
    Succeeded,
}

// ---------------------------------------------------------------------------
// WeaponState — animator weapon-draw state (ANIMATOR_WS_*)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum WeaponState {
    /// This character cannot hold a weapon.
    CannotHold = 0,
    /// Weapon slot is invisible / not present.
    #[default]
    Invisible = 1,
    /// Weapon is holstered.
    Holstered = 2,
    /// Weapon is drawn and in hand.
    Drawn = 3,
}

// ---------------------------------------------------------------------------
// Head IK mode (mirrors animHeadIKMode)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum HeadIkMode {
    #[default]
    Disabled,
    TrackClosestActor,
    TrackActor {
        target: Entity,
    },
    TrackPosition {
        pos: Vec3,
    },
    Scan {
        range: f32,
        period: f32,
    },
    Set {
        azimuth: f32,
        incline: f32,
    },
}

// ---------------------------------------------------------------------------
// ActionPlayer component
// ---------------------------------------------------------------------------

/// Central character action state machine.  Mirrors crAnimActionPlayer.
///
/// Insert alongside an Oni2AnimState + Oni2AnimLibrary on any creature that
/// needs high-level action dispatch (jump, crouch, react, evade, etc.).
#[derive(Component, Debug, Clone)]
pub struct ActionPlayer {
    /// Present-participle flags (ACT_FLAG_*).
    pub flags: u32,
    /// Deferred events waiting for an animation substate (PENDING_*).
    pub pending_events: u32,

    /// MainAction that was most recently requested.
    pub last_action: MainAction,

    /// Current animation substate (ACT_S1_*).  Updated when a new animation starts.
    pub sub_state_1: i32,
    /// Previous-frame value — used by `sub_state_1_is_fresh()`.
    pub sub_state_1_last_frame: i32,
    /// Per-action substate (ACT_S0_*) — stored once per MainAction variant.
    pub sub_state_0: [i32; ACT_NUM_ACTIONS],

    /// Jump index deferred until the jump anim's main frame starts.  -1 = none.
    pub jump_index: i32,
    /// If true, re-enable creature collision detection once the react substate clears.
    pub reenable_collision_after_react: bool,
    /// Optional sound to play during the react animation.
    pub react_sound_name: Option<String>,
    pub react_sound_frame: i32,
    /// Entity being picked up (for ACT_PICKUP).
    pub pickup_item: Option<Entity>,
    /// Queue a jump once the wall-compress anim ends.
    pub jump_after_wall_compress: bool,

    /// Animator weapon-draw state (ANIMATOR_WS_*).
    pub weapon_state: WeaponState,
    /// Head IK mode — consumed by head_ik_mode_system.
    pub head_ik_mode: HeadIkMode,
    /// True if targeting-IK is permitted (set by ACT_S1_WEAPON_READY_TO_TARGET transitions).
    pub allow_targeting_ik: bool,

    /// Animator throttle override (0.0..1.0).  Forced throttle on the mover.
    pub animator_throttle: f32,
    /// Current body-tilt value (for bank/lean animation blends).
    pub animator_tilt: f32,
    /// Smoothed current-slope value (used to pick up/down slope variants).
    pub animator_slope: f32,

    pub jumps_remaining: u8,
    pub max_jumps: u8,
}

impl Default for ActionPlayer {
    fn default() -> Self {
        ActionPlayer {
            flags: 0,
            pending_events: 0,
            last_action: MainAction::None,
            sub_state_1: sub_state_1::IDLE,
            sub_state_1_last_frame: sub_state_1::IDLE,
            sub_state_0: [sub_state_0::NONE; ACT_NUM_ACTIONS],
            jump_index: -1,
            reenable_collision_after_react: false,
            react_sound_name: None,
            react_sound_frame: 0,
            pickup_item: None,
            jump_after_wall_compress: false,
            weapon_state: WeaponState::Invisible,
            head_ik_mode: HeadIkMode::Disabled,
            allow_targeting_ik: false,
            animator_throttle: 1.0,
            animator_tilt: 0.0,
            animator_slope: 0.0,
            jumps_remaining: 2,
            max_jumps: 2,
        }
    }
}

impl ActionPlayer {
    // --- Flag helpers ---

    pub fn set_flag(&mut self, f: u32, b: bool) {
        if b {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }
    pub fn check_flags(&self, f: u32) -> bool {
        (self.flags & f) == f
    }
    pub fn find_at_least_one_flag(&self, f: u32) -> bool {
        (self.flags & f) != 0
    }

    // --- Pending helpers ---

    pub fn set_pending(&mut self, f: u32) {
        self.pending_events |= f;
    }
    pub fn clear_pending(&mut self, f: u32) {
        self.pending_events &= !f;
    }
    pub fn is_pending(&self, f: u32) -> bool {
        (f & self.pending_events) == f
    }

    // --- Substate accessors ---

    pub fn get_sub_state_0(&self, a: MainAction) -> i32 {
        self.sub_state_0[a as usize]
    }
    pub fn set_sub_state_0(&mut self, a: MainAction, v: i32) {
        self.sub_state_0[a as usize] = v;
    }
    pub fn is_sub_state_0(&self, a: MainAction, v: i32) -> bool {
        self.get_sub_state_0(a) == v
    }
    pub fn is_sub_state_1(&self, v: i32) -> bool {
        self.sub_state_1 == v
    }
    pub fn sub_state_1_is_fresh(&self) -> bool {
        self.sub_state_1 != self.sub_state_1_last_frame
    }

    // --- Is* accessors (mirror crAnimActionPlayer::Is*) ---

    pub fn is_crouching(&self) -> bool {
        self.check_flags(action_flags::CROUCHING)
    }
    pub fn is_in_fight_stance(&self) -> bool {
        self.check_flags(action_flags::FIGHTSTANCE)
    }
    pub fn is_jumping(&self) -> bool {
        self.check_flags(action_flags::JUMPING) || self.sub_state_1 == sub_state_1::JUMP_LAND
    }
    pub fn is_jumping_running(&self) -> bool {
        self.check_flags(action_flags::RUNNING_JUMP)
    }
    pub fn is_jumping_standing(&self) -> bool {
        self.check_flags(action_flags::STANDING_JUMP)
    }
    pub fn is_jumping_forward(&self) -> bool {
        self.is_sub_state_0(MainAction::Jump, sub_state_0::JUMP_FORWARD)
            || self.is_sub_state_0(MainAction::Jump, sub_state_0::JUMP_SOMERSAULT)
    }
    pub fn is_jumping_backwards(&self) -> bool {
        self.is_sub_state_0(MainAction::Jump, sub_state_0::JUMP_BACK)
    }
    pub fn is_jumping_sideways(&self) -> bool {
        self.is_sub_state_0(MainAction::Jump, sub_state_0::JUMP_LEFT)
            || self.is_sub_state_0(MainAction::Jump, sub_state_0::JUMP_RIGHT)
    }
    pub fn is_somersaulting(&self) -> bool {
        self.check_flags(action_flags::DOUBLE_JUMPING)
            || self.sub_state_1 == sub_state_1::JUMP_SOMERSAULT
    }
    pub fn is_landing(&self) -> bool {
        self.sub_state_1 == sub_state_1::JUMP_LAND
    }
    pub fn is_compressing(&self) -> bool {
        self.sub_state_1 == sub_state_1::JUMP_COMPRESS
    }
    pub fn is_sliding(&self) -> bool {
        self.check_flags(action_flags::SLIDING)
    }
    pub fn is_lying_down(&self) -> bool {
        self.check_flags(action_flags::LYINGDOWN)
    }
    pub fn is_hanging(&self) -> bool {
        self.check_flags(action_flags::LEDGEHANGING)
    }
    pub fn is_reacting(&self) -> bool {
        self.sub_state_1 == sub_state_1::REACT || self.sub_state_1 == sub_state_1::DIE
    }
    pub fn is_playing_custom_anim(&self) -> bool {
        self.sub_state_1 == sub_state_1::CUSTOM_ANIM
    }
    pub fn is_transitioning(&self) -> bool {
        self.sub_state_1 == sub_state_1::TRANSITION
    }
    pub fn is_weapon_drawn(&self) -> bool {
        self.check_flags(action_flags::WEAPON)
    }
    pub fn is_evading(&self) -> bool {
        self.sub_state_1 == sub_state_1::EVADE
    }
    pub fn is_crane_struggling(&self) -> bool {
        self.sub_state_1 == sub_state_1::CRANE_STRUGGLE
    }
    pub fn is_dropping_from_ledge(&self) -> bool {
        self.sub_state_1 == sub_state_1::LEDGE_DROP
    }
    pub fn is_starting_crouch(&self) -> bool {
        self.is_sub_state_0(MainAction::Transition, sub_state_0::TRANSITION_CROUCH_START)
            && self.sub_state_1 == sub_state_1::TRANSITION
    }
    pub fn is_ending_crouch(&self) -> bool {
        self.is_sub_state_0(MainAction::Transition, sub_state_0::TRANSITION_CROUCH_END)
            && self.sub_state_1 == sub_state_1::TRANSITION
    }
    /// True if the current state is a translating animation that shouldn't be
    /// overridden by nav (react, custom anim, evade, transition).  Mirrors
    /// crAnimActionPlayer::IsDoingTranslateAction.
    pub fn is_doing_translate_action(&self) -> bool {
        self.is_reacting()
            || self.is_playing_custom_anim()
            || self.is_evading()
            || self.is_transitioning()
    }

    // --- Mutators invoked from systems ---

    /// Record that a new animation started: shift `sub_state_1` → `sub_state_1_last_frame`,
    /// then overwrite `sub_state_1` with the new id.  Called from action-start handlers
    /// and from the per-frame tick when the animation id changes.
    pub fn record_new_substate_1(&mut self, new_state_1: i32) {
        self.sub_state_1_last_frame = self.sub_state_1;
        self.sub_state_1 = new_state_1;
    }

    pub fn reset(&mut self) {
        *self = ActionPlayer::default();
    }
}

/// Marks an entity as currently hitched to a carrier — overhead claw,
/// crane, etc.  Mirrors the `m_PickupMtx != NULL` state of
/// `animElbowCraneHitchComponent` (rb/src/animator/elbowcranehitch.cpp).
/// While this component is present:
///   • the entity's transform is force-synced to `claw_translation +
///     claw_rotation` (minus `offset`) each frame,
///   • physics integration should be suppressed (handled by the consumer;
///     we store `saved_gravity_scale` so the sync system can restore it
///     on drop),
///   • the CRANESTRUGGLE action is expected to be playing (started by the
///     carrier; ends when the component is removed).
///
/// Inserted by `animator::systems::pickup_matrix_system` on receipt of
/// `SetPickupMatrixMessage`; removed by `drop_system` on `DropMessage`.
#[derive(Component, Debug, Clone, Copy)]
pub struct PickupHitched {
    /// Carrier's current world-space translation (updated every tick the
    /// carrier sends a fresh SetPickupMatrixMessage).
    pub claw_translation: bevy::prelude::Vec3,
    /// Carrier's current world-space rotation.
    pub claw_rotation: bevy::prelude::Quat,
    /// Local offset on the picked-up entity that the claw grabs onto —
    /// the claw's world position lands at `entity.pos + claw_rotation *
    /// offset`.  Defaults to ZERO if the carrier doesn't specify.  C++
    /// reference: `mat.d -= GetOffset()` in HandleDrop.
    pub offset: bevy::prelude::Vec3,
}

