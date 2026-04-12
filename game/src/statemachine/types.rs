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
}

/// Character body-state flags checked as Packet conditions.
/// Mirrors aiStateControlStruct::aiControlFlagEnum from the C++ source.
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
    // Synthetic weapon-type flags (from HAS_WEAPON:TYPE in .fsm Packet args)
    pub const HAS_WEAPON_PIPE: u32 = 0x80000;
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
}

/// A condition parsed from `Packet(ARG1, ARG2, ...)`.
/// All specified flags must be present in the packet for the condition to match.
#[derive(Clone, Debug, Default)]
pub struct PacketCondition {
    pub pad_flags: u64,
    pub ctrl_flags: u32,
    /// If Some(n), packet.class_hit must equal n
    pub class_hit: Option<i32>,
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
    DoAttack { anim_name: String, rotation_notches: i32 },
    DoBlock { anim_name: String },
    DoCounter,
    DoTriggerAtk,
    DoEvade { anim_name: String, mirror: bool },
    CtrlGrapple { action: String },
    /// Interrupt the current attack and start a new one.
    ChangeAttack { anim_name: String, rotation_notches: i32 },
    CustomAnim { anim_name: String },
    /// Inline-evaluate another state's rules without transitioning first.
    /// The u32 is the resolved state index.
    Check(usize),
    ResetTimer,
    SetDone,
    EnablePad,
    DisablePad,
    Display(String),
}

/// A single guarded rule: `if [!]event { actions; [goto state;] }`
#[derive(Clone, Debug)]
pub struct FsmRule {
    pub event: FsmEvent,
    /// When true the event result is inverted (`if !Event(...)`)
    pub negated: bool,
    pub actions: Vec<FsmAction>,
    /// Resolved index into FsmData::states, or None if no goto.
    pub goto_state: Option<usize>,
}

/// One state in the machine — a named list of rules evaluated top-to-bottom.
#[derive(Clone, Debug)]
pub struct FsmState {
    pub name: String,
    pub rules: Vec<FsmRule>,
}

/// Parsed, immutable FSM data shared (via Arc) across all entities using the same file.
#[derive(Clone, Debug)]
pub struct FsmData {
    pub states: Vec<FsmState>,
    pub state_index: std::collections::HashMap<String, usize>,
}

impl FsmData {
    /// Default entry state.
    pub fn initial_state_idx(&self) -> usize {
        self.state_index
            .get("ATTACK_START")
            .copied()
            .unwrap_or(0)
    }

    pub fn state_name(&self, idx: usize) -> &str {
        self.states
            .get(idx)
            .map(|s| s.name.as_str())
            .unwrap_or("<invalid>")
    }
}

/// Outputs produced by one FSM update tick that the game systems should act on.
#[derive(Clone, Debug, Default)]
pub struct FsmOutput {
    /// (animation_name, rotation_notches) — play this attack animation.
    pub attack_anim: Option<(String, i32)>,
    pub block_anim: Option<String>,
    pub evade_anim: Option<(String, bool)>,
    pub custom_anim: Option<String>,
    pub counter: bool,
    pub ctrl_grapple: Option<String>,
}
