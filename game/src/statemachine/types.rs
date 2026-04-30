/*
 * statemachine/types.rs — FSM data types and pad/ctrl flag constants.
 *
 * FsmData: parsed state machine (states, transitions, conditions).
 * FsmState / FsmTransition / FsmCondition structs.
 * pad_flags / ctrl_flags / entity_flags: u64 bitmask constants matching
 * ONI2's PADCMD_* and CTRL_* enumerations used in transition rule evaluation.
 */
/// Radians per rotation notch (PI/4 = 45°). Matches NOTCHES2RADIANS in ONI2.
pub const NOTCH_RADIANS: f32 = std::f32::consts::FRAC_PI_4;

/// Bit flags for pad commands — one bit per logical button/direction.
/// u64 gives room for all ACK directional variants.
pub mod pad_flags {
    pub const PADCMD_ATTACK_HIGH: u64 = 1 << 0;
    pub const PADCMD_ATTACK_TWO_HIGH: u64 = 1 << 1;
    pub const PADCMD_BLOCK: u64 = 1 << 2;
    pub const PADCMD_SWEEP_FORWARD: u64 = 1 << 3;
    pub const PADCMD_GRAPPLE: u64 = 1 << 4;
    pub const PADCMD_GRAPPLE_ATK: u64 = 1 << 5;
    pub const PADCMD_GRAPPLE_ATK_2: u64 = 1 << 6;
    pub const PADCMD_END_GRAPPLE: u64 = 1 << 7;
    pub const PADCMD_LARIAT: u64 = 1 << 8;
    pub const PADCMD_WEAPON_LOCKON: u64 = 1 << 9;
    pub const PADCMD_WEAPON_FIRE_FORWARD: u64 = 1 << 10;
    pub const PADCMD_WEAPON_FIRE_LEFT: u64 = 1 << 11;
    pub const PADCMD_WEAPON_FIRE_RIGHT: u64 = 1 << 12;
    pub const PADCMD_WEAPON_FIRE_BACK: u64 = 1 << 13;
    pub const PADCMD_WEAPON_FIRE_BACK_LEFT: u64 = 1 << 14;
    pub const PADCMD_WEAPON_FIRE_BACK_RIGHT: u64 = 1 << 15;
    pub const PADCMD_WEAPON_FIRE_FWD_LEFT: u64 = 1 << 16;
    pub const PADCMD_WEAPON_FIRE_FWD_RIGHT: u64 = 1 << 17;
    pub const PADCMD_CHR_FWD: u64 = 1 << 18;
    pub const PADCMD_CHR_BAK: u64 = 1 << 19;
    pub const PADCMD_CHR_LFT: u64 = 1 << 20;
    pub const PADCMD_CHR_RGH: u64 = 1 << 21;
    pub const PADCMD_REDIRECT_FWD_BACK: u64 = 1 << 22;
    pub const PADCMD_REDIRECT_FWD_BACK_LEFT: u64 = 1 << 23;
    pub const PADCMD_REDIRECT_FWD_BACK_RIGHT: u64 = 1 << 24;
    // Directional attack 1 (attack button + movement direction)
    pub const ACK: u64 = 1 << 25; // back attack
    pub const ACK_LEFT: u64 = 1 << 26;
    pub const ACK_RIGHT: u64 = 1 << 27;
    pub const ACK_FORWARD_LEFT: u64 = 1 << 28;
    pub const ACK_FORWARD_RIGHT: u64 = 1 << 29;
    pub const ACK_BACKWARD_LEFT: u64 = 1 << 30;
    pub const ACK_BACKWARD_RIGHT: u64 = 1 << 31;
    // Directional attack 2 variants
    pub const ACK_TWO: u64 = 1 << 32;
    pub const ACK_TWO_LEFT: u64 = 1 << 33;
    pub const ACK_TWO_RIGHT: u64 = 1 << 34;
    pub const ACK_TWO_FORWARD_LEFT: u64 = 1 << 35;
    pub const ACK_TWO_FORWARD_RIGHT: u64 = 1 << 36;
    pub const ACK_TWO_BACKWARD_LEFT: u64 = 1 << 37;
    pub const ACK_TWO_BACKWARD_RIGHT: u64 = 1 << 38;
    /// Low-attack button (legacy `PADCMD_ATTACK_LOW` —
    /// rb/src/behavior/padmapper.h:43).  Used by `enemy.fsm`'s
    /// `if Packet(PADCMD_ATTACK_LOW)` rule to fire `ANIMATTACK_2`.
    /// MUST be present in `build_pad_table` (input.rs) — without it the
    /// parser silently drops the symbol and the rule's PacketCondition
    /// ends up all-zero, which used to make every empty-condition rule
    /// match every packet (every AI fired ANIMATTACK_2 on spawn).
    pub const PADCMD_ATTACK_LOW: u64 = 1 << 39;
    /// Generic action / "use object" button (legacy `PADCMD_ACTION` —
    /// rb/src/behavior/padmapper.h:66).  Referenced by `.fsm` rules.
    pub const PADCMD_ACTION: u64 = 1 << 40;
    /// Controller-shake input.  No legacy `PADCMD_SHAKE` exists (legacy
    /// uses analog-stick `pdAnalogShake` for this concept), but Rust-port
    /// `player.fsm` references the symbol — defined here so the parser
    /// resolves it to a unique bit instead of silently zeroing the
    /// PacketCondition.  Until something writes the bit, the rule is
    /// inert.
    pub const PADCMD_SHAKE: u64 = 1 << 41;
}

/// Character body-state flags checked as Packet conditions.
/// Character body-state flags checked as packet conditions during FSM evaluation.
pub mod ctrl_flags {
    pub const REACT: u32 = 0x00001;
    pub const STANDING: u32 = 0x00002;
    pub const WALKING: u32 = 0x00004;
    pub const RUNNING: u32 = 0x00008;
    pub const REDIRECT: u32 = 0x00010;
    pub const POSTHIT: u32 = 0x00020;
    pub const BLOCK_SUCCESS: u32 = 0x00040;
    pub const CRITICAL_FRAME: u32 = 0x00080;
    pub const JUMPING: u32 = 0x00100;
    pub const CROUCHING: u32 = 0x00200;
    pub const RUNNING_JUMP: u32 = 0x00400;
    pub const STANDING_JUMP: u32 = 0x00800;
    pub const WEAPON_DRAWN: u32 = 0x01000;
    pub const CAN_BLOCK: u32 = 0x02000;
    pub const HITTARGET: u32 = 0x04000;
    pub const FIGHT_MODE: u32 = 0x08000;
    pub const ENEMY_IN_SLICE: u32 = 0x10000;
    pub const STARTING_CROUCH: u32 = 0x20000;
    pub const ENDING_CROUCH: u32 = 0x40000;
}

/// Entity state flags used in Me() / Target() events.
pub mod entity_flags {
    pub const GRAPPLING: u32 = 0x0001;
    pub const GETTING_GRAPPLED: u32 = 0x0002;
}

/// Input packet passed to the FSM every update tick.
/// Built from the logical InputState plus physics/combat state.
#[derive(Clone, Debug, Default)]
pub struct FsmPacket {
    /// Pad command bits (logical buttons pressed this frame)
    pub pad_flags: u64,
    /// Control/body-state flags (RUNNING, JUMPING, FIGHT_MODE, etc.)
    pub ctrl_flags: u32,
    /// Attack class that landed on the player this frame (-1 = none)
    pub class_hit: i32,
    /// Self entity state flags for Me() events
    pub me_flags: u32,
    /// Target entity state flags for Target() events
    pub target_flags: u32,
    /// Weapon name currently held (for dynamic HAS_WEAPON:* checks)
    pub has_weapon: Option<String>,
}

/// A condition parsed from `Packet(ARG1, ARG2, ...)`.
/// All specified flags must be present in the packet for the condition to match.
#[derive(Clone, Debug, Default)]
pub struct PacketCondition {
    pub pad_flags: u64,
    pub ctrl_flags: u32,
    pub not_pad_flags: u64,
    pub not_ctrl_flags: u32,
    /// If Some(n), packet.class_hit must equal n
    pub class_hit: Option<i32>,
    pub has_weapon_name: Option<String>,
    pub not_has_weapon_name: Option<String>,
}

/// An event/condition in a rule's guard.
#[derive(Clone, Debug)]
pub enum FsmEvent {
    Packet(PacketCondition),
    /// Current attack/block animation has finished playing.
    Timeout,
    /// Check self entity state (grappling, etc.).
    Me(u32),
    /// Check target entity state.
    Target(u32),
    /// Always true.
    True,
    /// True with probability p each tick.
    Random(f32),
    /// True if at least t seconds have elapsed since last ResetTimer.
    Timer(f32),
    /// Current custom animation has finished.
    CustomAnimDone,
}

/// An action executed when a rule fires.
#[derive(Clone, Debug)]
pub enum FsmAction {
    DoAttack {
        anim_name: String,
        rotation_notches: i32,
    },
    DoBlock {
        anim_name: String,
    },
    DoCounter,
    DoTriggerAtk,
    DoEvade {
        anim_name: String,
        mirror: bool,
    },
    CtrlGrapple {
        action: String,
    },
    /// Interrupt the current attack and start a new one.
    ChangeAttack {
        anim_name: String,
        rotation_notches: i32,
    },
    CustomAnim {
        anim_name: String,
    },
    /// Inline-evaluate another state's rules without transitioning first.
    /// The u32 is the resolved state index.
    Check(usize),
    ResetTimer,
    SetDone,
    EnablePad,
    DisablePad,
    Display(String),
}

/// Outputs produced by one FSM update tick that the game systems should act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FsmOutput {
    /// (animation_name, rotation_notches) — play this attack animation.
    pub attack_anim: Option<(String, i32)>,
    pub block_anim: Option<String>,
    pub evade_anim: Option<(String, bool)>,
    pub custom_anim: Option<String>,
    pub counter: bool,
    pub ctrl_grapple: Option<String>,
}
