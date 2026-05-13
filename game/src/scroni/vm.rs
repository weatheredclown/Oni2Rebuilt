/*
 * scroni/vm.rs — ScrOni virtual machine.
 *
 * Core execution engine for ONI2 .oni scripts.  Manages named scripts loaded
 * from the VFS, thread scheduling (fork/kill), blocking actions (sleep, move,
 * camera), variable scopes, and message passing between threads.
 * scroni_tick_system: advances all active ScriptExec instances each frame.
 * update_broadcast_triggers / checkpoint_trigger_system: fire scripts on world events.
 * update_screen_fade_system / apply_shader_locals_system: render-state helpers.
 * ScroniTextState / ScrOniSysEvent: shared state and the event type dispatched
 * to system_bindings for world-mutation side effects.
 */
use std::collections::HashMap;

use bevy::prelude::*;

use crate::oni2_loader::utils::space;

use super::ast::*;
use super::compiler::Compiler;

/// Component representing local dynamic script-driven parameters bound to materials.
#[derive(Component, Default, Debug, Clone)]
pub struct ShaderLocals {
    pub locals: HashMap<String, f32>,
}

/// Marker component ensuring we only unique-clone a material handle once per actor.
#[derive(Component)]
pub struct ClonedShaderLocalMaterial;

/// Runtime value on the VM stack / in variables.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i32),
    Float(f32),
    String(String),
    Vector(Vec3),
    Actor(Entity),
    ActorList(Vec<Entity>, usize),
    None,
}

impl Value {
    pub fn default_for_type(var_type: &VarType) -> Self {
        match var_type {
            VarType::Integer => Value::Int(0),
            VarType::Float => Value::Float(0.0),
            VarType::Vector => Value::Vector(Vec3::ZERO),
            VarType::String => Value::String(String::new()),
            VarType::Timer => Value::Float(0.0),
            VarType::Label => Value::String(String::new()),
            VarType::ActorList => Value::ActorList(Vec::new(), 0),
            VarType::Child => Value::Int(0),
        }
    }

    pub fn as_float(&self) -> f32 {
        match self {
            Value::Int(i) => *i as f32,
            Value::Float(f) => *f,
            _ => 0.0,
        }
    }

    pub fn as_int(&self) -> i32 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i32,
            _ => 0,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::None => false,
            _ => true,
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Vector(v) => format!("({}, {}, {})", v.x, v.y, v.z),
            Value::ActorList(l, idx) => format!("ActorList[len={} idx={}]", l.len(), idx),
            Value::Actor(act) => format!("{} ({:?})", crate::debug::debug_name(*act), act),
            Value::None => "##UNLOGGABLE##".to_string(),
        }
    }

    pub fn as_vec3(&self) -> Vec3 {
        match self {
            Value::Vector(v) => *v,
            _ => Vec3::ZERO,
        }
    }
}

/// Execution state of a single script instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecState {
    /// Running normally.
    Running,
    /// Yielded for this frame (exit instruction).
    Yielded,
    /// Yielded control immediately to enter a newly pushed inner loop block.
    PushLoop,
    /// Script completed (done instruction or fell off end of sequence).
    Done,
    /// Waiting for a blocking behavior to complete.
    Blocked,
    /// Script aborted the sequence (home, switch, etc.). Unwinds loops.
    AbortSequence,
}

/// A message sent between scripts.
#[derive(Debug, Clone)]
pub struct ScriptMessage {
    pub msg: String,
    pub from: Entity,
    pub to: Entity,
    pub args: Vec<Value>,
    pub is_action: bool,
}

/// Represents a blocking command that the VM has issued and is waiting on.
#[derive(Debug, Clone)]
pub enum BlockingAction {
    Idle {
        end_time: f64,
    },
    GotoCurvePhase {
        target: f32,
        seconds: f32,
    },
    GotoCurveKnot {
        knot: i32,
        seconds: f32,
    },
    GotoCurveLerp {
        target: f32,
        seconds: f32,
    },
    Face {
        target: Value,
        seconds: Option<f32>,
    },
    GotoPoint {
        target: Value,
        within: Option<f32>,
        speed: Option<f32>,
        duration: Option<f32>,
    },
    PlayAnimation {
        name: String,
        hold: bool,
        loop_anim: bool,
        rate: Option<f32>,
        duration: Option<f32>,
    },
    Fight,
    Shoot,
    Patrol(Value),
    Follow(Value),
    Attack(Value),
    /// Waiting for the actor's BehaviorRuntime to finish the specified
    /// behavior kind.  Resolved by an `EndBehaviorMessage` whose
    /// `(entity, kind)` matches.  Used by the ported `goto` path so the
    /// script thread parks until GotoBehavior returns Finished.
    ///
    /// `deadline` mirrors the C++ `BehaviorDoneOrTimeout` instruction
    /// — when set, the wait
    /// also resolves once `now >= deadline`, even if the behavior is
    /// still running.  Timeout resolutions clear `blocking_failed`
    /// (timeout != failure, matching the legacy `IsFailing=false` path).
    /// `None` = wait indefinitely; preserves the historic behavior of
    /// `goto` and bare `takecover`.
    WaitingForBehavior {
        kind: crate::statemachine::drivers::behavior::BehaviorKind,
        deadline: Option<f64>,
    },
    /// Internal: waiting for CurveFollower to reach its target phase.
    /// Set by the bridge system after configuring the CurveFollower from a GotoCurvePhase.
    WaitingForCurve,
    /// Internal: waiting for a non-looping animation to finish playing.
    WaitingForAnimation,
    WaitingForPath,
}

/// Resolution decision for a `WaitingForBehavior` thread.  Pulled out of
/// the bridge's tick loop so the timeout/end-behavior arbitration can be
/// unit-tested without standing up a Bevy app.  Inputs:
///
///   * `entity` — the actor owning the script's BehaviorRuntime.
///   * `kind` — the behavior the thread is waiting on.
///   * `deadline` — optional absolute script-clock time after which the
///     wait gives up regardless of behavior state (set by `takecover for
///     <secs>` and friends).
///   * `now` — current script-clock time (`Time::elapsed_secs_f64`).
///   * `ended_behaviors` — `(entity, kind) → failed` map drained from
///     this tick's `EndBehaviorMessage` stream.
///
/// Returns `Some(failed)` when the wait should resolve this tick, `None`
/// when it should keep waiting.  `failed=true` propagates to the
/// thread's `blocking_failed` flag; `failed=false` (including timeout
/// resolutions) clears it.
pub fn resolve_behavior_wait(
    entity: Entity,
    kind: crate::statemachine::drivers::behavior::BehaviorKind,
    deadline: Option<f64>,
    now: f64,
    ended_behaviors: &std::collections::HashMap<
        (
            Entity,
            crate::statemachine::drivers::behavior::BehaviorKind,
        ),
        bool,
    >,
) -> Option<bool> {
    if let Some(&failed) = ended_behaviors.get(&(entity, kind)) {
        Some(failed)
    } else if let Some(d) = deadline
        && now >= d
    {
        // Timeout — matches the C++ DoBehaviorDoneOrTimeout path that
        // treats elapsed-deadline as a non-failing resolution.
        Some(false)
    } else {
        None
    }
}

/// Dead-code on this enum is escalated to a hard error: every variant
/// must be constructed somewhere (i.e. produced by an ops handler from a
/// `Stmt`). When the linter flags a variant as unused, that variant is an
/// orphan — observer plumbing without a parser/ops bridge — and the
/// script-level command silently does nothing. The fix is either to add
/// the missing `Stmt::X` arm in `scroni/ops/`, or, if the variant is
/// genuinely WIP, to mark it `#[allow(dead_code)]` with a comment.
#[deny(dead_code)]
#[derive(Debug, Clone)]
pub enum SysRequest {
    SetFullScreenColor {
        color: Vec3,
        duration: f32,
    },
    ControlHead {
        actor: Entity,
        task: crate::oni2_loader::components::ControlHeadTask,
    },
    TextureMovie {
        target_name: String,
        action: super::ast::TextureMovieAction,
        arg: Value,
    },
    Spawn {
        script: String,
        assign_to: Option<String>,
        at: Option<Vec3>,
        name: Option<String>,
    },
    Teleport {
        target: Entity,
        to: Option<Vec3>,
        face: Option<f32>,
    },
    SetFaction {
        actor: Entity,
        faction: String,
    },
    Retreat {
        actor: Entity,
        target: Option<Entity>,
    },
    /// `takecover [for <duration>]` — kicks the actor's BehaviorRuntime
    /// into TAKECOVER, which finds a POINT_COVER node and walks to it.
    /// Port of `bhMsgSetBehavior(kBehaviorTakeCover)`.
    /// `duration` is parsed but unused by the
    /// runtime today (no timed-cover behavior).
    TakeCover {
        actor: Entity,
        // Parsed but unused by the runtime today (no timed-cover
        // behavior); kept on the SysRequest so the wire format matches
        // the legacy parser.
        #[allow(dead_code)]
        duration: Option<f32>,
    },
    CameraSetPackage(String),
    CameraReset,
    /// `cameramode (script|game) [time <seconds>]` — mode name + optional
    /// transition duration.  `None` means snap-switch with no fade.
    CameraMode(String, Option<f32>),
    CameraSetFOV(f32, f32), // Target FOV, Duration
    CameraShake,
    CameraFollowActor(Entity),
    CameraTrackActor(Entity),
    CameraTrackPoint(Vec3),
    CameraMoveToActor(Entity, f32), // Target, Duration
    CameraMoveToPoint(Vec3, f32),
    // `cameramovealongrail` — token + observer + ScrOniSysEvent are wired,
    // but no `Stmt::CameraMoveAlongRail` parser arm exists yet. Marked
    // allow-dead until the parser side is implemented.
    #[allow(dead_code)]
    CameraMoveAlongRail(String, f32),
    /// `makeprojectile <name> direction <vec> speed <num> [at <expr>]`
    /// — converted to a `SpawnProjectileEvent` in `system_bindings`.
    /// `direction` is the unit travel vector and `speed` the magnitude
    /// (units/sec); we combine them into the `velocity` field that the
    /// projectile system expects.
    MakeProjectile {
        script_entity: Entity,
        name: String,
        direction: Vec3,
        speed: f32,
        at: Option<Vec3>,
    },
    /// Leave the frontend and run a level.  `None` for `level` means
    /// re-run the current layout (matches legacy
    /// `GAMEDATA.GetCurrentLayoutIndex()` fallback).  `save_point` defaults to 0.
    RunGame {
        level: Option<i32>,
        save_point: i32,
    },
    DrawText(String),
    At(f32, f32),
    MakeFx {
        script_entity: Entity,
        name: String,
        at: Option<Vec3>,
    },
    SendAction {
        action: String,
        target: Entity,
        component: String,
    },
    SetLightIntensity {
        light: String,
        intensity: f32,
    },
    SetShaderLocal {
        name: String,
        val: f32,
    },
    SetUpdateState {
        target: String,
        state: String,
    },
    SetAiTarget {
        actor: Entity,
        target: Entity,
    },
    TriggerFight {
        actor: Entity,
        target: Option<Entity>,
    },
    FollowActor {
        actor: Entity,
        target: Entity,
    },
    UsePad(Entity),
    PlayAmbientSound(
        i32,
        String,
        Option<f32>,
        Option<f32>,
        Option<(f32, f32, f32)>,
        Option<(f32, f32, f32)>,
    ),
    AmbientSoundStop(i32),
    /// Stop every running ambient sound on the script's owner (matches the
    /// `AmbientSound all Stop` shorthand used by e.g. M03 LevelMgr.oni:1517).
    AmbientSoundStopAll,
    AmbientSoundVolumeRamp(i32, f32, f32),
    AmbientSoundPitchRamp(i32, f32, f32),
    PlaySound(Option<String>, String),
    Hit {
        target: Entity,
        hit_type: String,
        damage: f32,
    },
    MakeExplosion {
        name: String,
        orientation: [f32; 3],
        at: [f32; 3],
    },
    Destroy(Entity),
    PlayerTaskBegin {
        timeout: Option<f32>,
    },
    PlayerTaskSuccessful,
    PlayerTaskFailure,
}

#[derive(Component)]
pub struct ActiveAmbientSound {
    pub handle: i32,
}

#[derive(Component)]
pub struct AudioVolumeRamp {
    pub start_vol: f32,
    pub end_vol: f32,
    pub duration: f32,
    pub elapsed: f32,
}

#[derive(Component)]
pub struct AudioPitchRamp {
    pub start_pitch: f32,
    pub end_pitch: f32,
    pub duration: f32,
    pub elapsed: f32,
}

#[derive(Event, Debug, Clone)]
pub enum ScrOniSysEvent {
    SetFullScreenColor {
        color: Vec3,
        duration: f32,
    },
    ControlHead {
        actor: Entity,
        task: crate::oni2_loader::components::ControlHeadTask,
    },
    TextureMovie {
        script_entity: Entity,
        target_name: String,
        action: super::ast::TextureMovieAction,
        arg: Value,
    },
    Spawn {
        script_entity: Entity,
        script: String,
        assign_to: Option<String>,
        at: Option<Vec3>,
        name: Option<String>,
    },
    // PlaySound moved to `fx_system::PlaySound`.  Scripts that request sound
    // playback still use `SysRequest::PlaySound` — the VM fan-out below
    // translates that into the FX-layer event.
    Teleport {
        script_entity: Entity,
        target: Entity,
        to: Option<Vec3>,
        face: Option<f32>,
    },
    CameraSetPackage(String),
    CameraReset,
    /// `cameramode (script|game) [time <seconds>]` — mode name + optional
    /// transition duration.  `None` means snap-switch with no fade.
    CameraMode(String, Option<f32>),
    CameraSetFOV(f32, f32), // Target FOV, Duration
    CameraShake,
    CameraFollowActor(Entity),
    CameraTrackActor(Entity),
    CameraTrackPoint(Vec3),
    CameraMoveToActor(Entity, f32),
    CameraMoveToPoint(Vec3, f32),
    CameraMoveAlongRail(String, f32),
    /// `makeprojectile` event — see SysRequest variant for semantics.
    MakeProjectile {
        script_entity: Entity,
        name: String,
        direction: Vec3,
        speed: f32,
        at: Option<Vec3>,
    },
    /// Leave the frontend and run a level — `None` for `level`
    /// re-runs the current layout, otherwise the int is interpreted
    /// as an index into `FrontendLevelList.entries`.
    RunGame {
        level: Option<i32>,
        save_point: i32,
    },
    DrawText(String),
    At(f32, f32),
    MakeFx {
        script_entity: Entity,
        name: String,
        at: Option<Vec3>,
    },
    SendAction {
        action: String,
        target: Entity,
        component: String,
    },
    SetLightIntensity {
        script_entity: Entity,
        light: String,
        intensity: f32,
    },
    SetShaderLocal {
        script_entity: Entity,
        name: String,
        val: f32,
    },
    SetUpdateState {
        target: String,
        state: String,
    },
    SetAiTarget {
        actor: Entity,
        target: Entity,
    },
    TriggerFight {
        actor: Entity,
        target: Option<Entity>,
    },
    FollowActor {
        actor: Entity,
        target: Entity,
    },
    UsePad {
        script_entity: Entity,
    },
    PlayAmbientSound {
        script_entity: Entity,
        handle: i32,
        name: String,
        volume: Option<f32>,
        pitch: Option<f32>,
        volume_ramp: Option<(f32, f32, f32)>,
        pitch_ramp: Option<(f32, f32, f32)>,
    },
    AmbientSoundStop {
        script_entity: Entity,
        handle: i32,
    },
    AmbientSoundStopAll,
    AmbientSoundVolumeRamp {
        script_entity: Entity,
        handle: i32,
        target_vol: f32,
        duration: f32,
    },
    AmbientSoundPitchRamp {
        script_entity: Entity,
        handle: i32,
        target_pitch: f32,
        duration: f32,
    },
    MakeExplosion {
        script_entity: Entity,
        name: String,
        orientation: [f32; 3],
        at: [f32; 3],
    },
    Destroy(Entity),
    PlayerTaskBegin {
        timeout: Option<f32>,
    },
    PlayerTaskSuccessful,
    PlayerTaskFailure,
}

#[derive(Debug, Clone)]
pub struct CallFrame {
    pub script: ScriptDef,
    pub variables: HashMap<String, Value>,
    pub seq_pc: usize,
    pub loop_stack: Vec<LoopState>,
}

#[derive(Debug, Clone)]
pub struct ScrOniThread {
    pub thread_id: u32,
    pub parent_thread_id: Option<u32>,
    pub script: ScriptDef,
    pub variables: HashMap<String, Value>,
    pub state: ExecState,
    pub seq_pc: usize,
    pub loop_stack: Vec<LoopState>,
    pub call_stack: Vec<CallFrame>,
    pub blocking: Option<BlockingAction>,
    pub in_whenever: bool,
    pub whenever_timers: HashMap<String, f64>,
    /// Latched output of the most recently resolved blocking command.
    /// Set by the bridge when a `WaitingForBehavior` resolution carries
    /// `failed=true` (TakeCover with no cover spot, future Goto with no
    /// path, etc.); cleared back to false on a successful resolution.
    /// Read by the `blockingcommandfailed` query in `eval_expr`, which
    /// scripts use in retry loops:
    ///
    ///     takecover
    ///     do while blockingcommandfailed begin
    ///         retreat for 0.5
    ///         takecover
    ///     end
    ///
    /// Persists across non-blocking statements until the next blocking
    /// command resolves and overwrites it.
    pub blocking_failed: bool,
}

impl ScrOniThread {
    pub fn new(thread_id: u32, parent_thread_id: Option<u32>, script: ScriptDef) -> Self {
        let mut variables = HashMap::new();
        init_variables(&mut variables, &script.variables);

        Self {
            thread_id,
            parent_thread_id,
            script,
            variables,
            state: ExecState::Running,
            seq_pc: 0,
            loop_stack: Vec::new(),
            call_stack: Vec::new(),
            blocking: None,
            in_whenever: false,
            whenever_timers: HashMap::new(),
            blocking_failed: false,
        }
    }
}

pub fn hash_name(s: &str) -> i32 {
    // We must use a fully deterministic hash across identical executions, not DefaultHasher
    // which generates random seeds via RandomState internally each iteration.
    let mut hash: u32 = 2166136261;
    for byte in s.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    (hash % 100000) as i32
}

pub fn eval_constant(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::IntLit(i) => Some(Value::Int(*i)),
        Expr::FloatLit(f) => Some(Value::Float(*f)),
        Expr::StringLit(s) => Some(Value::String(s.clone())),
        Expr::Call { name, args } if name.to_lowercase() == "guid" && args.len() == 1 => {
            if let Expr::StringLit(ref s) = args[0] {
                Some(Value::Int(hash_name(s)))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn init_variables(
    variables: &mut HashMap<String, Value>,
    decls: &[crate::scroni::ast::VarDecl],
) {
    variables.clear();
    for var in decls {
        if var.is_parent {
            continue;
        }
        let mut val = Value::default_for_type(&var.var_type);
        if let Some(ref expr) = var.initializer
            && let Some(c) = eval_constant(expr)
        {
            val = c;
        }
        variables.insert(var.name.clone(), val);
    }
}

/// Execution context for a script block. Holds the main thread and all concurrent child threads.
pub struct ScriptExec {
    pub main_thread: ScrOniThread,
    pub child_threads: Vec<ScrOniThread>,
    pub next_thread_id: u32,

    pub available_scripts: HashMap<String, ScriptDef>,
    pub message_queue: Vec<ScriptMessage>,
    /// Outgoing message queue.
    pub outgoing_messages: Vec<ScriptMessage>,
    /// Requests to the ECS system to perform engine-level actions.
    pub sys_requests: Vec<SysRequest>,
    /// The entity this script is attached to.
    pub owner: Entity,
    /// Currently active light selected by scripts (SetLightParameter).
    pub current_light: Option<String>,
    /// Whether this script is actively evaluating ticks. Modifiable via SetUpdateState natively.
    pub active: bool,
    /// Number of frames this script has been alive. Used to delay first tick protecting hierarchy initialization natively.
    pub ticks_alive: u32,
}

pub struct ScroniContext<'a, 'w_e, 's_e, 'w_t, 's_t> {
    pub all_entities:
        &'a Query<'w_e, 's_e, (Entity, &'static GlobalTransform, Option<&'static Name>)>,
    pub triggers: &'a Query<'w_t, 's_t, &'static BroadcastTrigger>,
    pub player: Option<Entity>,
    pub current_checkpoint: i32,
    pub layout_dir: String,
    /// Per-entity status string: "alive", "dead", "fighting". Built each frame.
    pub actor_statuses: &'a std::collections::HashMap<Entity, &'static str>,
    /// Optional line-of-sight checker: (from, to, exclude_a, exclude_b) -> bool.
    pub line_of_sight: Option<&'a dyn Fn(Vec3, Vec3, Entity, Entity) -> bool>,
    pub is_enemy: Option<&'a dyn Fn(Entity, Entity) -> bool>,
    pub get_perception_radius: Option<&'a dyn Fn(Entity) -> f32>,
    pub get_perception_fov: Option<&'a dyn Fn(Entity) -> f32>,
    pub get_actor_health: Option<&'a dyn Fn(Entity) -> f32>,
    /// `GetUIItemValue(<page>, <item>) -> f32`.  Resolves the
    /// numeric value of a frontend UI item — for `LevelList` this
    /// is the selected row index.  Mirrors `DoGetUIItemValue`
    /// (`DoGetUIItemValue`).  `None` when the
    /// frontend isn't loaded (no-frontend builds, in-game scripts).
    pub get_ui_item_value: Option<&'a dyn Fn(&str, &str) -> f32>,
}

impl<'a, 'w_e, 's_e, 'w_t, 's_t> ScroniContext<'a, 'w_e, 's_e, 'w_t, 's_t> {
    pub fn resolve_targets(&self, val: &Value) -> Vec<Entity> {
        let mut targets = Vec::new();
        match val {
            Value::Actor(act) => {
                if self.all_entities.get(*act).is_ok() {
                    targets.push(*act);
                }
            }
            Value::Int(guid) => {
                for (e, _, name_opt) in self.all_entities.iter() {
                    if let Some(n) = name_opt
                        && hash_name(n.as_str()) == *guid
                    {
                        targets.push(e);
                    }
                }
            }
            Value::ActorList(acts, _) => {
                for act in acts {
                    if self.all_entities.get(*act).is_ok() {
                        targets.push(*act);
                    }
                }
            }
            _ => {}
        }
        targets
    }
}

#[derive(Component, Default)]
pub struct BroadcastTrigger {
    pub radius: f32,
    pub inside: std::collections::HashSet<Entity>,
    pub just_entered: std::collections::HashSet<Entity>,
    pub just_exited: std::collections::HashSet<Entity>,
    pub world_center: Vec3,
}

pub fn update_broadcast_triggers(
    mut triggers: Query<(Entity, &mut BroadcastTrigger, &GlobalTransform)>,
    targets: Query<(Entity, &GlobalTransform)>,
) {
    for (trigger_ent, mut trigger, trigger_tf) in &mut triggers {
        let center = trigger_tf.translation();
        trigger.world_center = center;

        let r_sq = trigger.radius * trigger.radius;

        let mut currently_inside = std::collections::HashSet::new();

        for (target_ent, target_tf) in &targets {
            if target_ent == trigger_ent {
                continue;
            }

            // Use rigorous spherical checks natively (this mathematically guarantees parity with `find range N` script logic natively mapping distance from center to center)
            if target_tf.translation().distance_squared(center) <= r_sq {
                currently_inside.insert(target_ent);
            }
        }

        trigger.just_entered.clear();
        trigger.just_exited.clear();

        for ent in &currently_inside {
            if !trigger.inside.contains(ent) {
                trigger.just_entered.insert(*ent);
            }
        }

        let old_inside = trigger.inside.clone();
        for ent in &old_inside {
            if !currently_inside.contains(ent) {
                trigger.just_exited.insert(*ent);
            }
        }

        trigger.inside = currently_inside;
    }
}

#[derive(Debug, Clone)]
pub enum LoopState {
    Forever {
        body: Vec<Stmt>,
        pc: usize,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
        pc: usize,
    },
    NTimes {
        remaining: i32,
        body: Vec<Stmt>,
        pc: usize,
    },
    ForSeconds {
        end_time: f64,
        body: Vec<Stmt>,
        pc: usize,
    },
    Block {
        stmts: Vec<Stmt>,
        pc: usize,
    },
}

impl ScriptExec {
    pub fn new(script: ScriptDef, owner: Entity) -> Self {
        Self {
            main_thread: ScrOniThread::new(0, None, script),
            child_threads: Vec::new(),
            next_thread_id: 1,
            available_scripts: HashMap::new(),
            message_queue: Vec::new(),
            outgoing_messages: Vec::new(),
            sys_requests: Vec::new(),
            owner,
            current_light: None,
            active: true,
            ticks_alive: 0,
        }
    }

    /// Dynamically load and cache a script from file if not available in current execution context natively.
    pub fn resolve_script(&mut self, script_name: &str, ctx: &ScroniContext) -> Option<ScriptDef> {
        if let Some(new_script) = self.available_scripts.get(script_name).cloned() {
            return Some(new_script);
        }

        // Try to parse cross-file reference, e.g. "$scavenger:Scv_Uber_Main" or "routines:GetUniqueRandomList"
        if let Some(colon) = script_name.find(':') {
            let filename = if script_name.starts_with('$') {
                &script_name[1..colon]
            } else {
                &script_name[..colon]
            };
            let target_script = &script_name[colon + 1..];

            let script_fname = format!(
                "{}.oni",
                filename.trim_end_matches(".xml").trim_end_matches(".oni")
            );

            let mut paths_to_try = Vec::new();
            let mut push_path = |path: String| {
                if path.is_empty() {
                    return;
                }
                if !paths_to_try
                    .iter()
                    .any(|p: &String| p.eq_ignore_ascii_case(&path))
                {
                    paths_to_try.push(path);
                }
            };

            let layout_dir = ctx.layout_dir.trim_end_matches('/');
            if script_name.starts_with('$') {
                if !layout_dir.is_empty() {
                    push_path(format!("{}/Scripts", layout_dir));
                    push_path(format!("{}/scripts", layout_dir));
                }
                push_path("Scripts".to_string());
                push_path("scripts".to_string());
            } else {
                push_path("Scripts".to_string());
                push_path("scripts".to_string());
                if !layout_dir.is_empty() {
                    push_path(format!("{}/Scripts", layout_dir));
                    push_path(format!("{}/scripts", layout_dir));
                }
            }

            for dir in &paths_to_try {
                if crate::vfs::exists(dir, &script_fname) {
                    match load_script_file(dir, &script_fname) {
                        Ok(file) => {
                            for s in &file.scripts {
                                self.available_scripts.insert(s.name.clone(), s.clone());
                            }
                            return self.available_scripts.get(target_script).cloned();
                        }
                        Err(e) => {
                            warn!(
                                "[ScrOni][{}] resolve_script: Error loading {}:\n{}",
                                self.main_thread.script.name, script_fname, e
                            );
                        }
                    }
                }
            }

            warn!(
                "[ScrOni][{}] resolve_script: Failed to load cross-reference script file {}",
                self.main_thread.script.name, script_fname
            );
        }
        None
    }

    pub fn all_threads_mut(&mut self) -> impl Iterator<Item = &mut ScrOniThread> {
        std::iter::once(&mut self.main_thread).chain(self.child_threads.iter_mut())
    }

    /// True when this script's `tick()` would do nothing observable this
    /// frame — every thread is sleeping on `Idle { end_time }` with the
    /// wake-up still in the future, AND the script has no `whenever` block
    /// that could fire on world state.  In that case the caller can
    /// `continue` past the tick and just call `advance_idle_skip` to
    /// keep timer variables decrementing.
    ///
    /// Important boundaries:
    ///   • If ANY thread has a non-Idle blocking action (Goto / Face /
    ///     WaitingForBehavior / etc.) we MUST run tick — those resolve
    ///     externally and the script needs to react when the resolver
    ///     systems clear them.
    ///   • If the script has a whenever block, we MUST run tick — whenever
    ///     polls world state (player position, packets, etc.) and missing
    ///     a tick changes script behavior.
    ///   • A `safety_margin` of one frame's worth of time keeps the unblock
    ///     responsive — we wake up right around `end_time`, not after it.
    pub fn can_skip_for_idle(&self, now: f64, safety_margin: f64) -> bool {
        if !self.active {
            // Inactive scripts are already cheap (`tick` early-returns).
            // Skipping here saves nothing — keep the tick path so any
            // future re-activation logic still runs.
            return false;
        }
        if self.main_thread.script.whenever.is_some() {
            return false;
        }
        let threads_idle = std::iter::once(&self.main_thread)
            .chain(self.child_threads.iter())
            .all(|t| match &t.blocking {
                Some(BlockingAction::Idle { end_time }) => now + safety_margin < *end_time,
                _ => false,
            });
        threads_idle
    }

    /// Decrement timer-typed variables on every thread by `delta_time`.
    /// Mirrors the timer-decrement loop at the top of `tick()`; called
    /// from the host system when `can_skip_for_idle` lets us bypass the
    /// full tick this frame so timers don't fall behind wall-clock time.
    pub fn advance_idle_skip(&mut self, delta_time: f32) {
        for thread in std::iter::once(&mut self.main_thread).chain(self.child_threads.iter_mut()) {
            if thread.state == ExecState::Done {
                continue;
            }
            for var_decl in &thread.script.variables {
                if var_decl.var_type == VarType::Timer
                    && !var_decl.is_parent
                    && let Some(val) = thread.variables.get(&var_decl.name)
                {
                    let new_val = match val {
                        Value::Float(f) if *f > 0.0 => Some((*f - delta_time).max(0.0)),
                        Value::Int(i) if (*i as f32) > 0.0 => {
                            Some(((*i as f32) - delta_time).max(0.0))
                        }
                        _ => None,
                    };
                    if let Some(nv) = new_val {
                        thread.variables.insert(var_decl.name.clone(), Value::Float(nv));
                    }
                }
            }
        }
    }

    pub fn get_thread(&self, tid: u32) -> &ScrOniThread {
        if tid == 0 {
            &self.main_thread
        } else {
            self.child_threads
                .iter()
                .find(|t| t.thread_id == tid)
                .unwrap()
        }
    }

    pub fn get_thread_mut(&mut self, tid: u32) -> &mut ScrOniThread {
        if tid == 0 {
            &mut self.main_thread
        } else {
            self.child_threads
                .iter_mut()
                .find(|t| t.thread_id == tid)
                .unwrap()
        }
    }

    pub fn get_var(&self, tid: u32, name: &str) -> Value {
        let thread = self.get_thread(tid);
        if let Some(v) = thread.variables.get(name) {
            return v.clone();
        }
        for frame in thread.call_stack.iter().rev() {
            if let Some(v) = frame.variables.get(name) {
                return v.clone();
            }
        }
        if let Some(pid) = thread.parent_thread_id {
            return self.get_var(pid, name);
        }
        Value::None
    }

    pub fn set_var(&mut self, tid: u32, name: String, val: Value) {
        if self.get_thread(tid).variables.contains_key(&name) {
            self.get_thread_mut(tid).variables.insert(name, val);
            return;
        }

        let mut found_in_call_stack = false;
        {
            let thread = self.get_thread_mut(tid);
            for frame in thread.call_stack.iter_mut().rev() {
                if frame.variables.contains_key(&name) {
                    frame.variables.insert(name.clone(), val.clone());
                    found_in_call_stack = true;
                    break;
                }
            }
        }
        if found_in_call_stack {
            return;
        }

        let mut current = self.get_thread(tid).parent_thread_id;
        while let Some(pid) = current {
            if self.get_thread(pid).variables.contains_key(&name) {
                self.get_thread_mut(pid).variables.insert(name, val);
                return;
            }
            current = self.get_thread(pid).parent_thread_id;
        }

        self.get_thread_mut(tid).variables.insert(name, val);
    }

    /// Execute one frame's worth of the script. Returns the execution state.
    /// Tick the script executor. Process messages, then the main thread and child threads.
    pub fn tick(
        &mut self,
        current_time: f64,
        delta_time: f32,
        ctx: &mut ScroniContext,
    ) -> ExecState {
        if !self.active {
            return ExecState::Blocked;
        }
        let _span = bevy::log::info_span!("scroni::tick").entered();

        // Degrade timer variables (only local ones to avoid double-decrement on inherited variables)
        for thread in std::iter::once(&mut self.main_thread).chain(self.child_threads.iter_mut()) {
            if thread.state == ExecState::Done {
                continue;
            }
            for var_decl in &thread.script.variables {
                if var_decl.var_type == VarType::Timer
                    && !var_decl.is_parent
                    && let Some(val) = thread.variables.get(&var_decl.name)
                {
                    match val {
                        Value::Float(f) => {
                            if *f > 0.0 {
                                let mut new_val = *f - delta_time;
                                if new_val < 0.0 {
                                    new_val = 0.0;
                                }
                                thread
                                    .variables
                                    .insert(var_decl.name.clone(), Value::Float(new_val));
                            }
                        }
                        Value::Int(i) => {
                            let f = *i as f32;
                            if f > 0.0 {
                                let mut new_val = f - delta_time;
                                if new_val < 0.0 {
                                    new_val = 0.0;
                                }
                                thread
                                    .variables
                                    .insert(var_decl.name.clone(), Value::Float(new_val));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Execute main thread
        self.tick_thread(0, current_time, ctx);

        let mut i = 0;
        while i < self.child_threads.len() {
            let tid = self.child_threads[i].thread_id;
            self.tick_thread(tid, current_time, ctx);

            if self.child_threads[i].state == ExecState::Done {
                self.child_threads.remove(i);
            } else {
                i += 1;
            }
        }

        self.main_thread.state
    }

    fn tick_thread(&mut self, tid: u32, now: f64, ctx: &mut ScroniContext) {
        let _span = bevy::log::info_span!("scroni::tick_thread").entered();
        // Run whenever block (non-blocking, runs every frame)
        // This must run even if the main sequence is blocked or done!
        let whenever = self.get_thread(tid).script.whenever.clone();
        if let Some(ref whenever_stmts) = whenever {
            // Save state before whenever runs
            let pre_state = self.get_thread(tid).state;
            let pre_blocking = self.get_thread(tid).blocking.clone();

            // Force state to running temporarily for whenever
            if pre_state != ExecState::Running {
                self.get_thread_mut(tid).state = ExecState::Running;
            }

            self.get_thread_mut(tid).in_whenever = true;
            for stmt in whenever_stmts {
                self.exec_stmt(tid, stmt, now, ctx);
                let current_state = self.get_thread(tid).state;
                if current_state == ExecState::Yielded {
                    // ScrOni 'exit' effectively breaks out of the current tick's whenever loop silently.
                    self.get_thread_mut(tid).state = ExecState::Running;
                    break;
                } else if current_state == ExecState::Blocked {
                    warn!(
                        "[ScrOni][{}] Whenever block attempted to block (state: {:?})",
                        self.get_thread(tid).script.name,
                        current_state
                    );
                    self.get_thread_mut(tid).state = ExecState::Running;
                    break;
                } else if current_state == ExecState::PushLoop {
                    warn!(
                        "[ScrOni][{}] Whenever block pushed a loop, this is not fully supported and drops to sequence.",
                        self.get_thread(tid).script.name
                    );
                    self.get_thread_mut(tid).state = ExecState::Running;
                }
            }
            self.get_thread_mut(tid).in_whenever = false;

            // If the whenever block switched scripts or exited, we should keep that state
            // Otherwise, restore the state of the sequence
            let post_state = self.get_thread(tid).state;
            if post_state == ExecState::Running || post_state == ExecState::Yielded {
                self.get_thread_mut(tid).state = pre_state;
                self.get_thread_mut(tid).blocking = pre_blocking;
            }
        }

        let state = self.get_thread(tid).state;
        if state == ExecState::Done {
            return;
        }

        // Check if blocking action completed
        if let Some(ref action) = self.get_thread(tid).blocking.clone() {
            match action {
                BlockingAction::Idle { end_time } => {
                    if now >= *end_time {
                        self.get_thread_mut(tid).blocking = None;
                    } else {
                        self.get_thread_mut(tid).state = ExecState::Blocked;
                        return;
                    }
                }
                // Other blocking actions are resolved externally by the game systems
                _ => {
                    self.get_thread_mut(tid).state = ExecState::Blocked;
                    return;
                }
            }
        }

        if self.get_thread(tid).state != ExecState::Running {
            self.get_thread_mut(tid).state = ExecState::Running;
        }

        // Run sequence block from current PC
        self.run_sequence(tid, now, ctx);
    }

    fn run_sequence(&mut self, tid: u32, now: f64, ctx: &mut ScroniContext) {
        let _span = bevy::log::info_span!("scroni::run_sequence").entered();
        let mut instruction_count = 0;
        let max_instructions = 10000;

        loop {
            // If we're inside a loop, continue that loop
            while !self.get_thread(tid).loop_stack.is_empty() {
                if self.get_thread(tid).state != ExecState::Running {
                    return;
                }

                let (active, should_pop) =
                    self.step_top_loop(tid, &mut instruction_count, max_instructions, now, ctx);

                if should_pop {
                    self.get_thread_mut(tid).loop_stack.pop();
                }

                if self.get_thread(tid).state == ExecState::PushLoop {
                    self.get_thread_mut(tid).state = ExecState::Running;
                    continue; // Re-evaluate loop stack, top is now the new inner loop!
                }

                if active {
                    return;
                }
            }

            if self.get_thread(tid).state != ExecState::Running {
                return;
            }

            let mut broke_for_loop = false;

            // Continue sequence from PC
            while self.get_thread(tid).state == ExecState::Running {
                if instruction_count >= max_instructions {
                    warn!(
                        "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                        self.get_thread(tid).script.name,
                        max_instructions
                    );
                    self.get_thread_mut(tid).state = ExecState::Yielded;
                    return;
                }
                instruction_count += 1;

                let seq_pc = self.get_thread(tid).seq_pc;
                let len = self.get_thread(tid).script.sequence.len();
                if seq_pc < len {
                    let stmt = self.get_thread(tid).script.sequence[seq_pc].clone();
                    self.get_thread_mut(tid).seq_pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).state == ExecState::PushLoop {
                        self.get_thread_mut(tid).state = ExecState::Running;
                        broke_for_loop = true;
                        break;
                    }
                } else {
                    // Fell off end of sequence
                    if self.get_thread(tid).loop_stack.is_empty() {
                        let frame = self.get_thread_mut(tid).call_stack.pop();
                        if let Some(frame) = frame {
                            let t = self.get_thread_mut(tid);
                            t.script = frame.script;
                            t.variables = frame.variables;
                            t.seq_pc = frame.seq_pc;
                            t.loop_stack = frame.loop_stack;

                            // Break out of the native sequence loop to correctly
                            // re-evaluate the restored loop_stack from the top!
                            broke_for_loop = true;
                            break;
                        } else {
                            self.get_thread_mut(tid).state = ExecState::Done;
                            return;
                        }
                    } else {
                        return; // Should not happen, but break to be safe
                    }
                }
            }

            if !broke_for_loop {
                break; // If we didn't break to push a loop, the sequence is done or yielded, so end run_sequence
            }
        }
    }

    /// Checks the current thread state after executing loop statements and returns early tuple
    /// if the state requires a standard loop control flow break/yield/abort.
    fn check_loop_state(&mut self, tid: u32) -> Option<(bool, bool)> {
        match self.get_thread(tid).state {
            ExecState::PushLoop => Some((true, true)),
            ExecState::AbortSequence => Some((true, false)),
            ExecState::Done => Some((false, false)),
            ExecState::Yielded => {
                // Return active=true to bubble out of step_loop, push_back=true to keep the loop on stack
                Some((true, true))
            }
            _ => None,
        }
    }

    /// Step the top loop on the loop stack. Returns (still_active, should_pop).
    fn step_top_loop(
        &mut self,
        tid: u32,
        ins_count: &mut usize,
        max_ins: usize,
        now: f64,
        ctx: &mut ScroniContext,
    ) -> (bool, bool) {
        let top_idx = self.get_thread(tid).loop_stack.len() - 1;
        let mut ls = self.get_thread(tid).loop_stack[top_idx].clone();

        match &mut ls {
            LoopState::Forever { body, pc } => {
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame conditionally, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, false);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Forever {
                        body: body.clone(),
                        pc: *pc,
                    };
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).loop_stack.len() > top_idx + 1 {
                        return (false, false);
                    }
                }
                if let Some(res) = self.check_loop_state(tid) {
                    let (active, push_back) = res;
                    return (active, !push_back);
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0; // restart loop
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Forever {
                        body: body.clone(),
                        pc: *pc,
                    };
                    return (false, false);
                }
                (true, false) // blocked — keep loop
            }
            LoopState::While {
                condition,
                body,
                pc,
            } => {
                let cond = condition.clone();
                let cond_val = self.eval_expr(tid, &cond, now, ctx);
                if !cond_val.as_bool() {
                    return (false, true); // loop done
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, false);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::While {
                        condition: condition.clone(),
                        body: body.clone(),
                        pc: *pc,
                    };
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).loop_stack.len() > top_idx + 1 {
                        return (false, false);
                    }
                }
                if let Some(res) = self.check_loop_state(tid) {
                    let (active, push_back) = res;
                    return (active, !push_back);
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::While {
                        condition: condition.clone(),
                        body: body.clone(),
                        pc: *pc,
                    };
                    return (false, false);
                }
                (true, false)
            }
            LoopState::NTimes {
                remaining,
                body,
                pc,
            } => {
                if *remaining <= 0 {
                    return (false, true);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, false);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::NTimes {
                        remaining: *remaining,
                        body: body.clone(),
                        pc: *pc,
                    };
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).loop_stack.len() > top_idx + 1 {
                        return (false, false);
                    }
                }
                if let Some(res) = self.check_loop_state(tid) {
                    let (active, push_back) = res;
                    return (active, !push_back);
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *remaining -= 1;
                    *pc = 0;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::NTimes {
                        remaining: *remaining,
                        body: body.clone(),
                        pc: *pc,
                    };
                    let still_active = *remaining > 0;
                    return (false, !still_active);
                }
                (true, false)
            }
            LoopState::ForSeconds { end_time, body, pc } => {
                if now >= *end_time {
                    return (false, true);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, false);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::ForSeconds {
                        end_time: *end_time,
                        body: body.clone(),
                        pc: *pc,
                    };
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).loop_stack.len() > top_idx + 1 {
                        return (false, false);
                    }
                }
                if let Some(res) = self.check_loop_state(tid) {
                    let (active, push_back) = res;
                    return (active, !push_back);
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::ForSeconds {
                        end_time: *end_time,
                        body: body.clone(),
                        pc: *pc,
                    };
                    let keep = true;
                    return (keep, false);
                }
                (true, false)
            }
            LoopState::Block { stmts, pc } => {
                while *pc < stmts.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, false);
                    }
                    *ins_count += 1;

                    let stmt = stmts[*pc].clone();
                    *pc += 1;
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Block {
                        stmts: stmts.clone(),
                        pc: *pc,
                    };
                    self.exec_stmt(tid, &stmt, now, ctx);

                    if self.get_thread(tid).loop_stack.len() > top_idx + 1 {
                        return (false, false);
                    }
                }
                if let Some(res) = self.check_loop_state(tid) {
                    let (active, push_back) = res;
                    return (active, !push_back);
                }
                if *pc >= stmts.len() {
                    return (false, true); // block done
                }
                (true, false)
            }
        }
    }

    pub fn exec_stmt(&mut self, tid: u32, stmt: &Stmt, now: f64, ctx: &mut ScroniContext) {
        if self.get_thread(tid).state != ExecState::Running {
            return;
        }

        let mut ops_ctx = crate::scroni::ops::OpsCtx {
            exec: self,
            tid,
            now,
            ctx,
        };
        let handled = match stmt {
            stmt if crate::scroni::ops::core::exec(&mut ops_ctx, stmt) => true,
            stmt if crate::scroni::ops::movement::exec(&mut ops_ctx, stmt) => true,
            stmt if crate::scroni::ops::combat::exec(&mut ops_ctx, stmt) => true,
            stmt if crate::scroni::ops::camera::exec(&mut ops_ctx, stmt) => true,
            stmt if crate::scroni::ops::audio::exec(&mut ops_ctx, stmt) => true,
            stmt if crate::scroni::ops::animation::exec(&mut ops_ctx, stmt) => true,
            _ => false,
        };

        if !handled {
            // warn!("Unhandled Stmt: {:?}", stmt);
        }
    }

    pub fn flatten_to_block(&self, stmt: &Stmt) -> Vec<Stmt> {
        match stmt {
            Stmt::Block(stmts) => stmts.clone(),
            other => vec![other.clone()],
        }
    }

    // ---- Expression evaluation ----

    pub fn eval_expr(&mut self, tid: u32, expr: &Expr, now: f64, ctx: &mut ScroniContext) -> Value {
        match expr {
            Expr::IntLit(i) => Value::Int(*i),
            Expr::FloatLit(f) => Value::Float(*f),
            Expr::StringLit(s) => Value::String(s.clone()),
            Expr::List(exprs) => {
                let mut ents = Vec::new();
                for e in exprs {
                    let v = self.eval_expr(tid, e, now, ctx);
                    ents.extend(ctx.resolve_targets(&v));
                }
                Value::ActorList(ents, 0)
            }
            Expr::Var(name) => {
                if name.eq_ignore_ascii_case("facing") {
                    let mut facing_deg = 0.0;
                    if let Ok((_ent, tf, _)) = ctx.all_entities.get(self.owner) {
                        facing_deg = space::to_oni2_space_rot(tf.compute_transform().rotation).y;
                    }
                    Value::Float(facing_deg)
                } else {
                    self.get_var(tid, name)
                }
            }
            Expr::Me => Value::Actor(self.owner),
            Expr::Player => {
                if let Some(p) = ctx.player {
                    Value::Actor(p)
                } else {
                    Value::None
                }
            }
            Expr::Paren(inner) => self.eval_expr(tid, inner, now, ctx),
            Expr::Not(inner) => {
                let v = self.eval_expr(tid, inner, now, ctx);
                Value::Int(if v.as_bool() { 0 } else { 1 })
            }
            Expr::Negate(inner) => {
                let v = self.eval_expr(tid, inner, now, ctx);
                match v {
                    Value::Int(i) => Value::Int(-i),
                    Value::Float(f) => Value::Float(-f),
                    _ => Value::Int(0),
                }
            }
            Expr::VectorLit(x_expr, y_expr, z_expr) => {
                let x = self.eval_expr(tid, x_expr, now, ctx).as_float();
                let y = self.eval_expr(tid, y_expr, now, ctx).as_float();
                let z = self.eval_expr(tid, z_expr, now, ctx).as_float();
                Value::Vector(Vec3::new(x, y, z))
            }
            Expr::FieldAccess { base, field } => {
                let base_val = self.eval_expr(tid, base, now, ctx);
                match base_val {
                    Value::Vector(v) => match field.as_str() {
                        "x" | "X" => Value::Float(v.x),
                        "y" | "Y" => Value::Float(v.y),
                        "z" | "Z" => Value::Float(v.z),
                        _ => Value::None,
                    },
                    _ => Value::None,
                }
            }
            Expr::BinOp { op, left, right } => {
                let l = self.eval_expr(tid, left, now, ctx);
                let r = self.eval_expr(tid, right, now, ctx);
                eval_binop(*op, &l, &r, ctx)
            }
            Expr::Call { name, args } => {
                match name.as_str() {
                    "clock" => Value::Float(now as f32),
                    "blockingcommandfailed" => {
                        // Latched on this thread by the bridge whenever a
                        // `WaitingForBehavior` resolves; succeeds → false,
                        // fails → true.  Persists across non-blocking
                        // statements so retry loops can read it after
                        // intervening logic.
                        let failed = self.get_thread(tid).blocking_failed;
                        Value::Int(if failed { 1 } else { 0 })
                    }
                    "sin" => {
                        let val = args
                            .first()
                            .map_or(0.0, |e| self.eval_expr(tid, e, now, ctx).as_float());
                        Value::Float(val.to_radians().sin())
                    }
                    "cos" => {
                        let val = args
                            .first()
                            .map_or(0.0, |e| self.eval_expr(tid, e, now, ctx).as_float());
                        Value::Float(val.to_radians().cos())
                    }
                    "makestring" => {
                        let mut s = String::new();
                        for arg in args {
                            s.push_str(&self.eval_expr(tid, arg, now, ctx).as_string());
                        }
                        Value::String(s)
                    }
                    "random" => Value::Int(rand::random::<i32>().abs() % 100),
                    "randomrange" => {
                        let min = args
                            .first()
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_int())
                            .unwrap_or(0);
                        let max = args
                            .get(1)
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_int())
                            .unwrap_or(100);
                        if max > min {
                            Value::Int(
                                min + (rand::random::<i32>().unsigned_abs() as i32
                                    % (max - min + 1)),
                            )
                        } else {
                            Value::Int(min)
                        }
                    }
                    "randomrangefloat" => {
                        let min = args
                            .first()
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_float())
                            .unwrap_or(0.0);
                        let max = args
                            .get(1)
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_float())
                            .unwrap_or(1.0);
                        let r: f32 = rand::random();
                        Value::Float(min + r * (max - min))
                    }
                    "guid" => {
                        if let Some(e) = args.first() {
                            let val = self.eval_expr(tid, e, now, ctx);
                            return Value::Int(hash_name(&val.as_string()));
                        }
                        Value::None
                    }
                    "getuiitemvalue" => {
                        // `GetUIItemValue(<pageName>, <itemName>) -> float`.
                        // Mirrors DoGetUIItemValue: pop itemName then
                        // pageName, look up the page+item in the UI
                        // manager, return `item->GetValue()` as float.
                        // For our LevelList that's the selected row index.
                        let page = args
                            .first()
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_string())
                            .unwrap_or_default();
                        let item = args
                            .get(1)
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_string())
                            .unwrap_or_default();
                        let v = ctx
                            .get_ui_item_value
                            .map(|f| f(&page, &item))
                            .unwrap_or(0.0);
                        Value::Float(v)
                    }
                    "exists" => {
                        let target = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(t) = target {
                            let ents = ctx.resolve_targets(&t);
                            for ent in ents {
                                if ctx.actor_statuses.get(&ent).copied() != Some("dead")
                                {
                                    return Value::Int(1);
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "status" => {
                        let target = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(t) = target {
                            let ents = ctx.resolve_targets(&t);
                            if let Some(&ent) = ents.first() {
                                let s = ctx
                                    .actor_statuses
                                    .get(&ent)
                                    .copied()
                                    .unwrap_or("dead");
                                return Value::String(s.to_string());
                            }
                        }
                        Value::String("dead".to_string())
                    }
                    "alive" => Value::String("alive".to_string()),
                    "dead" => Value::String("dead".to_string()),
                    "health" => {
                        let target = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(t) = target {
                            let ents = ctx.resolve_targets(&t);
                            if let Some(&ent) = ents.first() {
                                if let Some(get_health) = ctx.get_actor_health {
                                    return Value::Float(get_health(ent));
                                }
                            }
                        }
                        Value::Float(0.0)
                    }
                    "damage" => Value::Int(0), // Placeholder until injury tracking is wired
                    "fighting" => Value::String("fighting".to_string()),
                    "notdying" => Value::String("notdying".to_string()),
                    "knockeddown" => Value::String("knockeddown".to_string()),
                    "attacking" => Value::String("attacking".to_string()),
                    // Faction-relation keywords used in `status X is enemy`-
                    // style predicates (grunt2.oni:296).  Evaluator returns
                    // the literal back as a string so the comparison path
                    // has something to match against — real faction-aware
                    // status querying (returning "enemy" from status(X) when
                    // X is hostile to the script owner) needs coordinated
                    // changes in `status` and `actor_statuses`; until then
                    // scripts that AND "alive, enemy" will short-circuit
                    // false on the enemy leg.  Still parses and runs without
                    // panics — that's the immediate bar.
                    "enemy" => Value::String("enemy".to_string()),
                    "friendly" => Value::String("friendly".to_string()),
                    "location" => {
                        let target = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Actor(act)) = target
                            && let Ok((_, tf, _)) = ctx.all_entities.get(act)
                        {
                            let p = tf.translation();
                            return Value::Vector(space::to_oni2_space_pos(p));
                        }
                        Value::None
                    }
                    "direction" => {
                        let arg1 = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let resolve_pos = |val: Value| -> Option<Vec3> {
                            match val {
                                Value::Vector(v) => Some(space::to_bevy_space_pos(v)),
                                Value::Actor(act) => {
                                    if let Ok((_, tf, _)) = ctx.all_entities.get(act) {
                                        Some(tf.translation())
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            }
                        };
                        let target_pos = arg1.and_then(resolve_pos);
                        if let Some(tpos) = target_pos {
                            if let Ok((_, my_tf, _)) = ctx.all_entities.get(self.owner) {
                                let diff = tpos - my_tf.translation();
                                let mut dir = diff.normalize_or_zero();
                                dir.y = 0.0;
                                if dir.length_squared() > 0.001 {
                                    dir = dir.normalize();
                                    // Fighter.facing computes rotation by Quat::from_rotation_arc(Vec3::Z, dir).
                                    // To get the Oni yaw, we do the same and use the space conversion utility.
                                    let q = Quat::from_rotation_arc(Vec3::Z, dir);
                                    let oni_rot = space::to_oni2_space_rot_rad(q);
                                    return Value::Float(oni_rot.y.to_degrees());
                                }
                            }
                        }
                        Value::None
                    }
                    "distance" => {
                        let arg1 = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let arg2 = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));

                        let resolve_pos = |val: Value| -> Option<Vec3> {
                            match val {
                                Value::Vector(v) => Some(v),
                                Value::Actor(act) => {
                                    if let Ok((_, tf, _)) = ctx.all_entities.get(act) {
                                        let p = tf.translation();
                                        Some(space::to_oni2_space_pos(p))
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            }
                        };

                        let mut p1 = arg1.and_then(resolve_pos);
                        let mut p2 = arg2.and_then(resolve_pos);

                        if p1.is_some()
                            && p2.is_none()
                            && let Ok((_, my_tf, _)) = ctx.all_entities.get(self.owner)
                        {
                            p2 = p1;
                            let my_p = my_tf.translation();
                            p1 = Some(space::to_oni2_space_pos(my_p));
                        }

                        if let (Some(a), Some(b)) = (p1, p2) {
                            return Value::Float(a.distance(b));
                        }
                        Value::Float(99999.0)
                    }
                    "trigger" => {
                        if let Some(event_expr) = args.first() {
                            let event_name = match event_expr {
                                Expr::Var(n) => n.clone(),
                                Expr::StringLit(s) => s.clone(),
                                _ => self.eval_expr(tid, event_expr, now, ctx).as_string(),
                            };

                            if let Ok(trigger) = ctx.triggers.get(self.owner) {
                                if event_name.eq_ignore_ascii_case("playerenter") {
                                    if let Some(p) = ctx.player
                                        && trigger.just_entered.contains(&p)
                                    {
                                        return Value::Int(1);
                                    }
                                } else if event_name.eq_ignore_ascii_case("playerexit") {
                                    if let Some(p) = ctx.player
                                        && trigger.just_exited.contains(&p)
                                    {
                                        return Value::Int(1);
                                    }
                                } else if event_name.eq_ignore_ascii_case("playerinside") {
                                    if let Some(p) = ctx.player
                                        && trigger.inside.contains(&p)
                                    {
                                        return Value::Int(1);
                                    }
                                } else if event_name.eq_ignore_ascii_case("playeroutside") {
                                    if let Some(p) = ctx.player
                                        && !trigger.inside.contains(&p)
                                    {
                                        return Value::Int(1);
                                    }
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "triggerentered" => {
                        let trig_ent = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                            && let Ok(trigger) = ctx.triggers.get(t)
                            && trigger.just_entered.contains(&e)
                        {
                            return Value::Int(1);
                        }
                        Value::Int(0)
                    }
                    "triggerexited" => {
                        let trig_ent = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                            && let Ok(trigger) = ctx.triggers.get(t)
                            && trigger.just_exited.contains(&e)
                        {
                            return Value::Int(1);
                        }
                        Value::Int(0)
                    }
                    "triggerinside" => {
                        let trig_ent = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                            && let Ok(trigger) = ctx.triggers.get(t)
                            && trigger.inside.contains(&e)
                        {
                            return Value::Int(1);
                        }
                        Value::Int(0)
                    }
                    "receivemessage" => {
                        if let Some(msg_expr) = args.first() {
                            let target_msg = self.eval_expr(tid, msg_expr, now, ctx).as_string();
                            if let Some(idx) = self
                                .message_queue
                                .iter()
                                .position(|m| !m.is_action && m.msg == target_msg)
                            {
                                info!(
                                    "[ScrOni][{}] Received message '{}' (from {:?})",
                                    self.get_thread(tid).script.name,
                                    target_msg,
                                    self.message_queue[idx].from
                                );
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "receiveaction" => {
                        if let Some(msg_expr) = args.first() {
                            let target_msg = self.eval_expr(tid, msg_expr, now, ctx).as_string();
                            if let Some(idx) = self
                                .message_queue
                                .iter()
                                .position(|m| m.is_action && m.msg == target_msg)
                            {
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        } else if let Some(idx) =
                            self.message_queue.iter().position(|m| m.is_action)
                        {
                            self.message_queue.remove(idx);
                            return Value::Int(1);
                        }
                        Value::Int(0)
                    }
                    "getcheckpointindex" => Value::Int(ctx.current_checkpoint),
                    "first" => {
                        if let Some(Expr::Var(list_name)) = args.first()
                            && let Value::ActorList(entities, _) = self.get_var(tid, list_name)
                        {
                            let updated = entities.clone();
                            if let Some(&first_ent) = updated.first() {
                                self.set_var(tid, list_name.clone(), Value::ActorList(updated, 1));
                                return Value::Actor(first_ent);
                            } else {
                                self.set_var(tid, list_name.clone(), Value::ActorList(updated, 0));
                                return Value::None;
                            }
                        }
                        Value::None
                    }
                    "playambientsound" => {
                        let n = args.first().map_or(String::new(), |e| {
                            self.eval_expr(tid, e, now, ctx).as_string()
                        });
                        let mut v = None;
                        let mut p = None;
                        let mut vr = None;
                        let mut pr = None;
                        let mut i = 1;
                        while i < args.len() {
                            if let Expr::StringLit(m) = &args[i] {
                                if m == "volumeramp" && i + 3 < args.len() {
                                    vr = Some((
                                        self.eval_expr(tid, &args[i + 1], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i + 2], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i + 3], now, ctx).as_float(),
                                    ));
                                    i += 4;
                                    continue;
                                } else if m == "pitchramp" && i + 3 < args.len() {
                                    pr = Some((
                                        self.eval_expr(tid, &args[i + 1], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i + 2], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i + 3], now, ctx).as_float(),
                                    ));
                                    i += 4;
                                    continue;
                                }
                            }
                            if i == 1 {
                                v = Some(self.eval_expr(tid, &args[i], now, ctx).as_float());
                                i += 1;
                            } else if i == 2 {
                                p = Some(self.eval_expr(tid, &args[i], now, ctx).as_float());
                                i += 1;
                            } else {
                                i += 1;
                            }
                        }

                        let handle = rand::random::<i32>().abs();
                        self.sys_requests
                            .push(SysRequest::PlayAmbientSound(handle, n, v, p, vr, pr));
                        Value::Int(handle)
                    }
                    "size" => {
                        if let Some(Expr::Var(list_name)) = args.first()
                            && let Value::ActorList(entities, _) = self.get_var(tid, list_name)
                        {
                            return Value::Int(entities.len() as i32);
                        }
                        Value::Int(0)
                    }
                    "next" => {
                        if let Some(Expr::Var(list_name)) = args.first()
                            && let Value::ActorList(entities, idx) = self.get_var(tid, list_name)
                        {
                            let updated = entities.clone();
                            let current_idx = idx;
                            if current_idx < updated.len() {
                                let ent = updated[current_idx];
                                self.set_var(
                                    tid,
                                    list_name.clone(),
                                    Value::ActorList(updated, current_idx + 1),
                                );
                                return Value::Actor(ent);
                            } else {
                                self.set_var(
                                    tid,
                                    list_name.clone(),
                                    Value::ActorList(updated, current_idx),
                                );
                                return Value::None;
                            }
                        }
                        Value::None
                    }
                    "isdone" => {
                        let target_var = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Int(target_tid)) = target_var {
                            // Check if child thread still exists
                            let mut exists_and_running = false;
                            for ct in &self.child_threads {
                                if ct.thread_id == target_tid as u32 {
                                    exists_and_running = ct.state != ExecState::Done;
                                    break;
                                }
                            }
                            return Value::Int(if exists_and_running { 0 } else { 1 });
                        }
                        Value::Int(1)
                    }
                    "ishome" => {
                        // `ishome <childvar>` — true iff the child thread's
                        // PC has been rewound to its home script's entry
                        // (seq_pc=0, empty call_stack, empty loop_stack).
                        // Matches the legacy semantics where `childhome`
                        // resets the thread and `ishome` asks "has it
                        // settled back to the starting point?".
                        let target_var = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Int(target_tid)) = target_var
                            && target_tid > 0
                        {
                            for ct in &self.child_threads {
                                if ct.thread_id == target_tid as u32 {
                                    // "home" means the thread is executing its base script
                                    // (not deep in a childstack call).
                                    let at_home = ct.call_stack.is_empty();
                                    return Value::Int(if at_home { 1 } else { 0 });
                                }
                            }
                        }
                        // Missing child → treat as "home" (nothing running).
                        Value::Int(1)
                    }
                    "status_is" => {
                        // `status_is(actor, state)` — one predicate leg of a
                        // `status X is A, B, ...` list (see compiler's
                        // parse_comparison desugar).  Mirrors the legacy
                        // `ParseStatusList`,
                        // which AND's per-state predicates.  Returns 1/0.
                        let actor_val = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let state_val = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        let state = state_val
                            .as_ref()
                            .map(|v| v.as_string())
                            .unwrap_or_default();
                        if let Some(val) = actor_val {
                            let ents = ctx.resolve_targets(&val);
                            if let Some(&ent) = ents.first() {
                                let cur = ctx
                                    .actor_statuses
                                    .get(&ent)
                                    .copied()
                                    .unwrap_or("");
                                let matches = if state.eq_ignore_ascii_case("alive") {
                                    !cur.is_empty() && cur != "dead"
                                } else if state.eq_ignore_ascii_case("dead") {
                                    cur == "dead" || cur.is_empty()
                                } else if state.eq_ignore_ascii_case("fighting") {
                                    cur == "fighting"
                                } else if state.eq_ignore_ascii_case("notdying") {
                                    cur != "dying" && cur != "dead"
                                } else if state.eq_ignore_ascii_case("player") {
                                    ctx.player == Some(ent)
                                } else if state.eq_ignore_ascii_case("enemy") {
                                    ent != self.owner
                                } else if state.eq_ignore_ascii_case("friendly") {
                                    ent == self.owner
                                } else {
                                    false
                                };
                                return Value::Int(if matches { 1 } else { 0 });
                            }
                        }
                        Value::Int(0)
                    }
                    "navpoint" | "path" => {
                        // navpoint("Name") or path("Name") -> return the name as a string so that
                        // the goto resolver can look it up in nav.names or path configs.
                        if let Some(e) = args.first() {
                            Value::String(self.eval_expr(tid, e, now, ctx).as_string())
                        } else {
                            Value::None
                        }
                    }
                    "lineofsight" => {
                        // lineofsight(actor_a, actor_b) -> bool (1 if unobstructed)
                        let a = args.first().map(|e| self.eval_expr(tid, e, now, ctx));
                        let b = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        let resolve_pos = |val: &Value| -> Option<(Vec3, Entity)> {
                            match val {
                                Value::Actor(ent) => ctx
                                    .all_entities
                                    .get(*ent)
                                    .ok()
                                    .map(|(e, tf, _)| (tf.translation(), e)),
                                _ => None,
                            }
                        };
                        if let (Some(av), Some(bv)) = (a.as_ref(), b.as_ref())
                            && let (Some((pos_a, ent_a)), Some((pos_b, ent_b))) =
                                (resolve_pos(av), resolve_pos(bv))
                            && let Some(los_fn) = ctx.line_of_sight
                        {
                            return Value::Int(if los_fn(pos_a, pos_b, ent_a, ent_b) {
                                1
                            } else {
                                0
                            });
                        }
                        Value::Int(0)
                    }
                    _ => Value::None,
                }
            }
            Expr::Exists(expr_opt) => {
                let val = if let Some(expr) = expr_opt {
                    self.eval_expr(tid, expr, now, ctx)
                } else {
                    Value::None
                };
                let ents = ctx.resolve_targets(&val);
                let alive = ents
                    .iter()
                    .any(|ent| ctx.actor_statuses.get(ent).copied() != Some("dead"));
                Value::Int(if alive { 1 } else { 0 })
            }
        }
    }

    // Helper to unblock a thread
    pub fn clear_blocking(&mut self, tid: u32) {
        if let Some(t) = self
            .child_threads
            .iter_mut()
            .find(|t| t.thread_id == tid)
            .or({
                if tid == 0 {
                    Some(&mut self.main_thread)
                } else {
                    None
                }
            })
        {
            t.blocking = None;
            if t.state == ExecState::Blocked {
                t.state = ExecState::Running;
            }
        }
    }
}

fn eval_binop(op: BinOp, l: &Value, r: &Value, ctx: &ScroniContext) -> Value {
    match op {
        BinOp::Mod => match (l, r) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    Value::Int(0)
                } else {
                    Value::Int(a % b)
                }
            }
            _ => {
                let rv = r.as_float();
                if rv == 0.0 {
                    Value::Float(0.0)
                } else {
                    Value::Float(l.as_float() % rv)
                }
            }
        },
        BinOp::Add => match (l, r) {
            (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
            (Value::Vector(a), Value::Vector(b)) => Value::Vector(*a + *b),
            _ => Value::Float(l.as_float() + r.as_float()),
        },
        BinOp::Sub => match (l, r) {
            (Value::Int(a), Value::Int(b)) => Value::Int(a - b),
            (Value::Vector(a), Value::Vector(b)) => Value::Vector(*a - *b),
            _ => Value::Float(l.as_float() - r.as_float()),
        },
        BinOp::Mul => match (l, r) {
            (Value::Int(a), Value::Int(b)) => Value::Int(a * b),
            _ => Value::Float(l.as_float() * r.as_float()),
        },
        BinOp::Div => {
            let rv = r.as_float();
            if rv == 0.0 {
                Value::Float(0.0)
            } else {
                Value::Float(l.as_float() / rv)
            }
        }
        BinOp::Equal => {
            if let (Value::Actor(e1), Value::Actor(e2)) = (l, r) {
                return Value::Int(if e1 == e2 { 1 } else { 0 });
            }
            if let (Value::Int(guid), Value::Actor(ent)) | (Value::Actor(ent), Value::Int(guid)) =
                (l, r)
            {
                if *guid == 0 {
                    return Value::Int(0);
                }
                let mut matched = false;
                if let Ok((_, _, Some(name))) = ctx.all_entities.get(*ent)
                    && hash_name(name.as_str()) == *guid
                {
                    matched = true;
                }
                return Value::Int(if matched { 1 } else { 0 });
            }
            if let (Value::Actor(_), Value::None) | (Value::None, Value::Actor(_)) = (l, r) {
                return Value::Int(0);
            }
            if let (Value::String(s1), Value::String(s2)) = (l, r) {
                return Value::Int(if s1.to_lowercase() == s2.to_lowercase() {
                    1
                } else {
                    0
                });
            }
            if let (Value::None, Value::Int(0)) | (Value::Int(0), Value::None) = (l, r) {
                return Value::Int(1);
            }
            if matches!(l, Value::Actor(_)) && matches!(r, Value::Int(0)) {
                return Value::Int(0);
            }
            if matches!(l, Value::Int(0)) && matches!(r, Value::Actor(_)) {
                return Value::Int(0);
            }
            Value::Int(if (l.as_float() - r.as_float()).abs() < f32::EPSILON {
                1
            } else {
                0
            })
        }
        BinOp::NotEqual => {
            let eq_val = eval_binop(BinOp::Equal, l, r, ctx);
            Value::Int(if eq_val.as_bool() { 0 } else { 1 })
        }
        BinOp::Less => Value::Int(if l.as_float() < r.as_float() { 1 } else { 0 }),
        BinOp::LessOrEqual => Value::Int(if l.as_float() <= r.as_float() { 1 } else { 0 }),
        BinOp::Greater => Value::Int(if l.as_float() > r.as_float() { 1 } else { 0 }),
        BinOp::GreaterOrEqual => Value::Int(if l.as_float() >= r.as_float() { 1 } else { 0 }),
        BinOp::And => Value::Int(if l.as_bool() && r.as_bool() { 1 } else { 0 }),
        BinOp::Or => Value::Int(if l.as_bool() || r.as_bool() { 1 } else { 0 }),
        BinOp::Dot => match (l, r) {
            (Value::Vector(a), Value::Vector(b)) => Value::Float(a.dot(*b)),
            _ => Value::Float(0.0),
        },
        BinOp::Cross => match (l, r) {
            (Value::Vector(a), Value::Vector(b)) => Value::Vector(a.cross(*b)),
            _ => Value::Vector(Vec3::ZERO),
        },
    }
}

/// Bevy component wrapping a ScrOni script executor.
#[derive(Component)]
pub struct ScrOniScript {
    pub exec: ScriptExec,
}

/// Bevy system: tick all ScrOni scripts each frame.
pub fn scroni_tick_system(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &mut ScrOniScript,
        &GlobalTransform,
        Option<&crate::ai::navigation::ActorPathfollower>,
        Option<&crate::ai::components::ActorRetreating>,
    )>,
    all_entities: Query<(Entity, &'static GlobalTransform, Option<&'static Name>)>,
    triggers: Query<&'static BroadcastTrigger>,
    time: Res<Time>,
    player_query: Query<Entity, With<crate::player::components::Player>>,
    current_checkpoint: Res<crate::oni2_loader::components::CurrentCheckpointIndex>,
    health_query: Query<(
        Entity,
        &mut crate::combat::components::Health,
        Option<&crate::ai::components::AiFighter>,
    )>,
    spatial_query: avian3d::prelude::SpatialQuery,
    mut injure_writer: MessageWriter<crate::combat::events::InjureMessage>,
    mut behavior_runtime_query: Query<&mut crate::behavior::BehaviorRuntime>,
    mut end_behavior_reader: MessageReader<crate::behavior::EndBehaviorMessage>,
    faction_query: Query<(Entity, &crate::combat::faction::Faction)>,
    mut fighter_query: Query<&mut crate::combat::components::Fighter>,
    opt_res: (
        Option<Res<crate::oni2_loader::environment::LayoutContext>>,
        Option<Res<crate::ai::navigation::NavGraph>>,
        Option<Res<crate::oni2_loader::environment::LayoutPaths>>,
        Option<Res<crate::combat::faction::FactionManager>>,
        Option<Res<crate::frontend::runtime::FrontendLevelList>>,
    ),
) {
    let (layout_context, nav_graph_opt, layout_paths, faction_manager_opt, frontend_levels) =
        opt_res;
    let now = time.elapsed_secs_f64();
    let delta_time = time.delta_secs();

    // Drain behavior-completion events so threads blocked on
    // `WaitingForBehavior { kind }` can resolve this tick.  Map keyed on
    // (entity, kind) → failed; messages are one-shot so re-reading the
    // reader in the resolve loop below would miss them.  The `failed`
    // bit propagates into the resolved thread's `blocking_failed` so
    // `blockingcommandfailed` reads true after a TakeCover that couldn't
    // find cover (etc.).
    let ended_behaviors: std::collections::HashMap<
        (
            Entity,
            crate::statemachine::drivers::behavior::BehaviorKind,
        ),
        bool,
    > = end_behavior_reader
        .read()
        .map(|m| ((m.entity, m.kind), m.failed))
        .collect();
    let mut all_messages = Vec::new();
    let player_ent = player_query.iter().next();

    let _build_status_span = bevy::log::info_span!("scroni::build_actor_statuses").entered();
    let mut actor_statuses: std::collections::HashMap<Entity, &'static str> = std::collections::HashMap::new();
    for (ent, health, ai_opt) in health_query.iter() {
        let status = if health.current <= 0.0 {
            "dead"
        } else if let Some(ai) = ai_opt {
            // "fighting" means the actor has a current combat target.  Mirrors
            // the legacy `aiFighter::GetMode() != M_IDLE` check — engaged vs
            // not — without binding the scripting interface to any specific
            // FSM state name.  Once the fight coordinator is wired, target
            // assignment will flow from the fight FSM itself.
            if ai.target.is_some() {
                "fighting"
            } else {
                "alive"
            }
        } else {
            "alive"
        };
        actor_statuses.insert(ent, status);
    }
    drop(_build_status_span);

    let _scripts_span = bevy::log::info_span!("scroni::script_loop").entered();
    // Wake-up margin for the Idle-skip optimization.  Two frames at 60fps
    // (~33 ms) — small enough that the wake feels instant, large enough
    // that we always tick at least once before `end_time` actually passes.
    const IDLE_SKIP_MARGIN_S: f64 = 0.033;
    for (entity, mut script, transform, pathfollower_opt, _retreating_opt) in &mut query {
        if script.exec.ticks_alive == 0 {
            script.exec.ticks_alive += 1;
            continue;
        }
        script.exec.ticks_alive += 1;

        // Skip the entire per-script body when every thread is sleeping
        // on `Idle { end_time }` with the wake-up still in the future and
        // the script has no `whenever` block — there's nothing for tick
        // to do this frame.  We still advance any timer variables so they
        // don't drift behind wall-clock, then continue past closure setup,
        // ScroniContext construction, blocking-action collection, and
        // sysreq drain (all of which would be no-ops on a sleeping
        // script).  See `ScriptExec::can_skip_for_idle` for the full
        // safety contract.
        if script.exec.can_skip_for_idle(now, IDLE_SKIP_MARGIN_S) {
            let _skip_span = bevy::log::info_span!("scroni::skip_idle").entered();
            script.exec.advance_idle_skip(delta_time);
            continue;
        }

        let los_checker = |from: Vec3, to: Vec3, exc_a: Entity, exc_b: Entity| -> bool {
            let delta = to - from;
            let dist = delta.length();
            if dist < 0.01 {
                return true;
            }
            let Ok(dir) = Dir3::new(delta / dist) else {
                return true;
            };
            let filter =
                avian3d::prelude::SpatialQueryFilter::from_excluded_entities([exc_a, exc_b]);
            spatial_query
                .cast_ray(from, dir, dist * 0.99, true, &filter)
                .is_none()
        };
        let los_ref: &dyn Fn(Vec3, Vec3, Entity, Entity) -> bool = &los_checker;

        let is_enemy = |a: Entity, b: Entity| -> bool {
            if let Ok((_, f_a)) = faction_query.get(a) {
                if let Ok((_, f_b)) = faction_query.get(b) {
                    if let Some(fm) = faction_manager_opt.as_ref() {
                        return fm.get_status(&f_a.0, &f_b.0)
                            == crate::combat::faction::FactionStatus::Enemy;
                    }
                }
            }
            a != b
        };
        let is_enemy_ref: &dyn Fn(Entity, Entity) -> bool = &is_enemy;

        let get_rad = |e: Entity| -> f32 {
            if let Ok((_, _, Some(ai))) = health_query.get(e) {
                ai.perception_radius()
            } else {
                30.0
            }
        };
        let get_rad_ref: &dyn Fn(Entity) -> f32 = &get_rad;

        let get_fov = |e: Entity| -> f32 {
            if let Ok((_, _, Some(ai))) = health_query.get(e) {
                ai.perception_fov()
            } else {
                45.0_f32.to_radians()
            }
        };
        let get_fov_ref: &dyn Fn(Entity) -> f32 = &get_fov;

        // GetUIItemValue resolver — currently the only "values" we
        // have are LevelList row indices on the frontend.  Future UI
        // items (sliders, etc.) extend this match.  Returns 0.0 for
        // unknown page/item or when no frontend is loaded.
        let get_ui_item_value = |page: &str, item: &str| -> f32 {
            let Some(ref levels) = frontend_levels else {
                return 0.0;
            };
            match (page, item) {
                ("Choose_Level", "LevelList") => levels.selected as f32,
                // The Save_Point sub-page has no separate state yet;
                // returning 0 matches the legacy "no save data" path.
                ("Choose_Level_Save_Point", "LevelSavePointList") => 0.0,
                _ => {
                    warn!(
                        "scroni GetUIItemValue: unknown page/item ('{}','{}')",
                        page, item
                    );
                    0.0
                }
            }
        };
        let get_ui_item_value_ref: &dyn Fn(&str, &str) -> f32 = &get_ui_item_value;

        let get_health = |e: Entity| -> f32 {
            if let Ok((_, health, _)) = health_query.get(e) {
                health.current
            } else {
                0.0
            }
        };
        let get_health_ref: &dyn Fn(Entity) -> f32 = &get_health;

        let mut ctx = ScroniContext {
            all_entities: &all_entities,
            triggers: &triggers,
            player: player_ent,
            current_checkpoint: current_checkpoint.0,
            layout_dir: layout_context
                .as_ref()
                .map(|c| c.layout_dir.clone())
                .unwrap_or_default(),
            actor_statuses: &actor_statuses,
            line_of_sight: Some(los_ref),
            is_enemy: Some(is_enemy_ref),
            get_perception_radius: Some(get_rad_ref),
            get_perception_fov: Some(get_fov_ref),
            get_actor_health: Some(get_health_ref),
            get_ui_item_value: Some(get_ui_item_value_ref),
        };
        script.exec.tick(now, delta_time, &mut ctx);

        let is_done = script.exec.main_thread.state == ExecState::Done;

        let mut gotos_to_resolve = Vec::new();
        let mut patrols_to_resolve = Vec::new();
        let mut waiting_for_path = Vec::new();
        let mut waiting_for_behavior = Vec::new();
        let mut faces_to_resolve = Vec::new();
        for t in script.exec.all_threads_mut() {
            if let Some(BlockingAction::GotoPoint {
                target,
                within,
                speed,
                duration,
            }) = t.blocking.clone()
            {
                gotos_to_resolve.push((t.thread_id, target, within, speed, duration));
            } else if let Some(BlockingAction::Patrol(path_val)) = t.blocking.clone() {
                patrols_to_resolve.push((t.thread_id, path_val));
            } else if let Some(BlockingAction::WaitingForPath) = t.blocking.clone() {
                waiting_for_path.push(t.thread_id);
            } else if let Some(BlockingAction::WaitingForBehavior { kind, deadline }) =
                t.blocking.clone()
            {
                waiting_for_behavior.push((t.thread_id, kind, deadline));
            } else if let Some(BlockingAction::Face { target, seconds }) = t.blocking.clone() {
                faces_to_resolve.push((t.thread_id, target, seconds));
            }
        }

        for tid in waiting_for_path {
            if pathfollower_opt.is_none() {
                script.exec.clear_blocking(tid);
                script.exec.tick_thread(tid, now, &mut ctx);
            }
        }

        // Resolve `WaitingForBehavior` threads whose behavior finished this
        // tick (EndBehaviorMessage was drained into `ended_behaviors` above).
        // Entity + kind must both match — a different behavior finishing on
        // the same actor doesn't unblock a script waiting on Goto.  The
        // `failed` bit is latched onto the thread before clearing the
        // blocking action, so the surrounding script's next read of
        // `blockingcommandfailed` reflects this resolution's outcome.
        //
        // Timeout path: when `deadline` is set and `now >= deadline`, the
        // wait resolves even if the behavior is still running.  Mirrors
        // `DoBehaviorDoneOrTimeout` — timeout != failure, so
        // `blocking_failed` is cleared.
        // The behavior keeps ticking on the actor; the script just stops
        // listening (its EndBehaviorMessage will arrive later with no
        // thread parked on it, and is harmlessly dropped).
        for (tid, kind, deadline) in waiting_for_behavior {
            if let Some(failed) =
                resolve_behavior_wait(entity, kind, deadline, now, &ended_behaviors)
            {
                script.exec.get_thread_mut(tid).blocking_failed = failed;
                script.exec.clear_blocking(tid);
                script.exec.tick_thread(tid, now, &mut ctx);
            }
        }

        for (tid, path_val) in patrols_to_resolve {
            let path_name = path_val.as_string();
            let waypoints = layout_paths
                .as_ref()
                .and_then(|lp| {
                    lp.curves
                        .iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(&path_name))
                })
                .map(|(_, pts)| pts.clone());
            if let Some(pts) = waypoints {
                if let Ok(mut e_cmd) = commands.get_entity(entity) {
                    e_cmd.insert(crate::ai::navigation::ActorPathfollower {
                        path: pts,
                        current_wp: 0,
                        speed_throttle: 1.0,
                        within: None,
                    });
                }
                script.exec.get_thread_mut(tid).blocking = Some(BlockingAction::WaitingForPath);
            } else {
                warn!("patrol: path '{}' not found in LayoutPaths", path_name);
                script.exec.clear_blocking(tid);
                script.exec.tick_thread(tid, now, &mut ctx);
            }
        }

        for (tid, target, _seconds) in faces_to_resolve {
            let target_bevy_pos = match target {
                Value::Vector(v) => Some(space::to_bevy_space_pos(v)),
                Value::Actor(act) => {
                    if let Ok((_, tf, _)) = all_entities.get(act) {
                        Some(tf.translation())
                    } else {
                        None
                    }
                }
                Value::Float(f) => {
                    // It's an angle in degrees (Oni Yaw).
                    let rads = f.to_radians();
                    // Use standard space utilities to get a Bevy rotation quaternion.
                    // The Oni Euler rotations are passed as [pitch, yaw, roll], so yaw is Y.
                    let oni_ypr = Vec3::new(0.0, rads, 0.0);
                    let q = space::to_bevy_space_rot_rad(oni_ypr);
                    // The fighter rotation system uses Quat::from_rotation_arc(Vec3::Z, fighter.facing)
                    // so facing should just be the local Z vector of this quaternion.
                    let facing = q * Vec3::Z;
                    Some(transform.translation() + facing)
                }
                _ => None,
            };

            if let Some(tgt_pos) = target_bevy_pos {
                let mut dir = (tgt_pos - transform.translation()).normalize_or_zero();
                dir.y = 0.0; // Keep facing planar
                if dir.length_squared() > 0.001 {
                    dir = dir.normalize();
                    // Send an action to smoothly or instantly rotate the character.
                    // Right now, instantly update fighter.facing.
                    // We dispatch a behavior request via BehaviorRuntime if possible.
                    if let Ok(mut rt) = behavior_runtime_query.get_mut(entity) {
                        rt.pending_params.target_point = Some(tgt_pos);
                        // We do not have a dedicated Face behavior yet, so we could just fake it
                        // by forcing the fighter component's facing directly:
                    }
                    if let Ok(mut fighter) = fighter_query.get_mut(entity) {
                        fighter.facing = dir;
                    }
                }
            }
            script.exec.clear_blocking(tid);
            script.exec.tick_thread(tid, now, &mut ctx);
        }

        for (tid, target, within, speed, _duration) in gotos_to_resolve {
            let path = match &target {
                Value::String(s) => nav_graph_opt.as_ref().and_then(|nav| {
                    nav.find_path(transform.translation(), s)
                        .or_else(|| nav.names.get(s).map(|&idx| vec![nav.points[idx]]))
                }),
                Value::Vector(v) => {
                    let pos = space::to_bevy_space_pos(v);
                    nav_graph_opt
                        .as_ref()
                        .and_then(|nav| nav.find_path_to_point(transform.translation(), pos))
                        .or_else(|| Some(vec![pos]))
                }
                Value::Actor(act) => {
                    if let Ok((_, tf, _)) = all_entities.get(*act) {
                        let pos = tf.translation();
                        nav_graph_opt
                            .as_ref()
                            .and_then(|nav| nav.find_path_to_point(transform.translation(), pos))
                            .or_else(|| Some(vec![pos]))
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(path) = path {
                // Route the path through the new BehaviorRuntime pipeline.
                // GotoBehavior consumes `pending_params.path` in its on_enter
                // and walks the chain, writing velocity each FixedUpdate tick.
                // The thread parks on WaitingForBehavior until the GotoBehavior
                // fires `EndBehaviorMessage { kind: Goto }` on finish.
                if let Ok(mut rt) = behavior_runtime_query.get_mut(entity) {
                    rt.pending_params.path = path;
                    rt.pending_params.within = within;
                    rt.pending_params.speed_throttle = speed;
                    rt.ctx.requested_goto = true;
                    script.exec.get_thread_mut(tid).blocking =
                        Some(BlockingAction::WaitingForBehavior {
                            kind: crate::statemachine::drivers::behavior::BehaviorKind::Goto,
                            deadline: None,
                        });
                    continue;
                } else {
                    warn!(
                        "scroni goto: entity {:?} has no BehaviorRuntime — falling back to \
                         unblocking without navigation",
                        entity
                    );
                }
            }

            // If we get here, pathfinding failed OR the actor has no
            // BehaviorRuntime.  Clear the block so the script doesn't hang.
            script.exec.clear_blocking(tid);
            script.exec.tick_thread(tid, now, &mut ctx);
        }

        let _script_name = script.exec.main_thread.script.name.clone();
        for req in script.exec.sys_requests.drain(..) {
            match req {
                SysRequest::MakeExplosion {
                    name,
                    orientation,
                    at,
                } => {
                    commands.trigger(ScrOniSysEvent::MakeExplosion {
                        script_entity: entity,
                        name,
                        orientation,
                        at,
                    });
                }
                SysRequest::PlaySound(actor, name) => {
                    // Script-initiated sound — route through the FX-layer
                    // event so the audio dispatcher lives in one place.
                    commands.trigger(crate::fx_system::PlaySound {
                        script_entity: entity,
                        actor,
                        name,
                    });
                }
                SysRequest::TextureMovie {
                    target_name,
                    action,
                    arg,
                } => {
                    commands.trigger(ScrOniSysEvent::TextureMovie {
                        script_entity: entity,
                        target_name,
                        action,
                        arg,
                    });
                }
                SysRequest::Spawn {
                    script,
                    assign_to,
                    at,
                    name,
                } => {
                    info!(
                        "Spawn command: script={}, assign_to={:?}, at={:?}, name={:?}",
                        script, assign_to, at, name
                    );
                    commands.trigger(ScrOniSysEvent::Spawn {
                        script_entity: entity,
                        script,
                        assign_to,
                        at,
                        name,
                    });
                }
                SysRequest::Teleport { target, to, face } => {
                    commands.trigger(ScrOniSysEvent::Teleport {
                        script_entity: entity,
                        target,
                        to,
                        face,
                    });
                }
                SysRequest::SetFaction { actor, faction } => {
                    if let Ok(mut e_cmd) = commands.get_entity(actor) {
                        e_cmd.insert(crate::combat::faction::Faction(faction));
                    }
                }
                SysRequest::Retreat { actor, target } => {
                    if let Ok(mut e_cmd) = commands.get_entity(actor) {
                        e_cmd.insert(crate::ai::components::ActorRetreating {
                            avoid_target: target,
                        });
                    }
                }
                SysRequest::TakeCover {
                    actor,
                    duration: _,
                } => {
                    // Flip the actor's BehaviorRuntime into a TakeCover
                    // request — same handshake the goto/retreat scripts use
                    // (write pending_params, set request flag, FSM tick
                    // routes to TAKECOVER_STATE on the next FixedUpdate
                    // pass).  The script thread already parked on
                    // WaitingForBehavior { TakeCover } back in
                    // ops/combat.rs, so we don't touch blocking state
                    // here.
                    if let Ok(mut rt) = behavior_runtime_query.get_mut(actor) {
                        // No path/target_point — TakeCoverBehavior owns
                        // its own destination selection from the
                        // CoverPointManager.  Clear stale params just so
                        // a leftover Goto path can't leak in.
                        rt.pending_params = crate::behavior::BehaviorParams::default();
                        rt.ctx.requested_take_cover = true;
                    } else {
                        warn!(
                            "scroni takecover: entity {:?} has no BehaviorRuntime",
                            actor
                        );
                    }
                }
                SysRequest::CameraSetPackage(pkg_name) => {
                    commands.trigger(ScrOniSysEvent::CameraSetPackage(pkg_name));
                }
                SysRequest::CameraReset => {
                    commands.trigger(ScrOniSysEvent::CameraReset);
                }
                SysRequest::CameraMode(mode, time) => {
                    commands.trigger(ScrOniSysEvent::CameraMode(mode, time));
                }
                SysRequest::CameraSetFOV(fov, dur) => {
                    commands.trigger(ScrOniSysEvent::CameraSetFOV(fov, dur));
                }
                SysRequest::CameraShake => {
                    commands.trigger(ScrOniSysEvent::CameraShake);
                }
                SysRequest::CameraFollowActor(e) => {
                    commands.trigger(ScrOniSysEvent::CameraFollowActor(e));
                }
                SysRequest::CameraTrackActor(e) => {
                    commands.trigger(ScrOniSysEvent::CameraTrackActor(e));
                }
                SysRequest::CameraTrackPoint(p) => {
                    commands.trigger(ScrOniSysEvent::CameraTrackPoint(p));
                }
                SysRequest::CameraMoveToActor(e, dur) => {
                    commands.trigger(ScrOniSysEvent::CameraMoveToActor(e, dur));
                }
                SysRequest::CameraMoveToPoint(p, dur) => {
                    commands.trigger(ScrOniSysEvent::CameraMoveToPoint(p, dur));
                }
                SysRequest::CameraMoveAlongRail(r, dur) => {
                    commands.trigger(ScrOniSysEvent::CameraMoveAlongRail(r, dur));
                }
                SysRequest::RunGame { level, save_point } => {
                    commands.trigger(ScrOniSysEvent::RunGame { level, save_point });
                }
                SysRequest::At(x, y) => {
                    commands.trigger(ScrOniSysEvent::At(x, y));
                }
                SysRequest::ControlHead { actor, task } => {
                    commands.trigger(ScrOniSysEvent::ControlHead { actor, task });
                }
                SysRequest::DrawText(text) => {
                    commands.trigger(ScrOniSysEvent::DrawText(text));
                }
                SysRequest::MakeFx {
                    script_entity,
                    name,
                    at,
                } => {
                    commands.trigger(ScrOniSysEvent::MakeFx {
                        script_entity,
                        name,
                        at,
                    });
                }
                SysRequest::MakeProjectile {
                    script_entity,
                    name,
                    direction,
                    speed,
                    at,
                } => {
                    commands.trigger(ScrOniSysEvent::MakeProjectile {
                        script_entity,
                        name,
                        direction,
                        speed,
                        at,
                    });
                }
                SysRequest::SendAction {
                    action,
                    target,
                    component,
                } => {
                    commands.trigger(ScrOniSysEvent::SendAction {
                        action,
                        target,
                        component,
                    });
                }
                SysRequest::SetLightIntensity { light, intensity } => {
                    commands.trigger(ScrOniSysEvent::SetLightIntensity {
                        script_entity: entity,
                        light,
                        intensity,
                    });
                }
                SysRequest::SetShaderLocal { name, val } => {
                    commands.trigger(ScrOniSysEvent::SetShaderLocal {
                        script_entity: entity,
                        name,
                        val,
                    });
                }
                SysRequest::SetUpdateState { target, state } => {
                    commands.trigger(ScrOniSysEvent::SetUpdateState { target, state });
                }
                SysRequest::SetAiTarget { actor, target } => {
                    commands.trigger(ScrOniSysEvent::SetAiTarget { actor, target });
                }
                SysRequest::TriggerFight { actor, target } => {
                    commands.trigger(ScrOniSysEvent::TriggerFight { actor, target });
                }
                SysRequest::FollowActor { actor, target } => {
                    commands.trigger(ScrOniSysEvent::FollowActor { actor, target });
                }
                SysRequest::SetFullScreenColor { color, duration } => {
                    commands.trigger(ScrOniSysEvent::SetFullScreenColor { color, duration });
                }
                SysRequest::UsePad(ent) => {
                    commands.trigger(ScrOniSysEvent::UsePad { script_entity: ent });
                }
                SysRequest::PlayAmbientSound(
                    handle,
                    name,
                    volume,
                    pitch,
                    volume_ramp,
                    pitch_ramp,
                ) => {
                    commands.trigger(ScrOniSysEvent::PlayAmbientSound {
                        script_entity: entity,
                        handle,
                        name,
                        volume,
                        pitch,
                        volume_ramp,
                        pitch_ramp,
                    });
                }
                SysRequest::AmbientSoundVolumeRamp(handle, target_vol, duration) => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundVolumeRamp {
                        script_entity: entity,
                        handle,
                        target_vol,
                        duration,
                    });
                }
                SysRequest::AmbientSoundPitchRamp(handle, target_pitch, duration) => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundPitchRamp {
                        script_entity: entity,
                        handle,
                        target_pitch,
                        duration,
                    });
                }
                SysRequest::Hit {
                    target,
                    hit_type,
                    damage,
                } => {
                    injure_writer.write(crate::combat::events::InjureMessage {
                        target,
                        attacker: Some(entity),
                        damage,
                        hit_type,
                        from: None,
                        play_react: true,
                        disable_creature_detect: false,
                        attack_class: None,
                        attack_strength: None,
                        attack_target: None,
                        strike_react_enum: None,
                        react_distance: None,
                        face_with_react: false,
                        teleport_to: None,
                    });
                }
                SysRequest::AmbientSoundStop(handle) => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundStop {
                        script_entity: entity,
                        handle,
                    });
                }
                SysRequest::AmbientSoundStopAll => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundStopAll);
                }
                SysRequest::Destroy(ent) => {
                    commands.trigger(ScrOniSysEvent::Destroy(ent));
                }
                SysRequest::PlayerTaskBegin { timeout } => {
                    commands.trigger(ScrOniSysEvent::PlayerTaskBegin { timeout });
                }
                SysRequest::PlayerTaskSuccessful => {
                    commands.trigger(ScrOniSysEvent::PlayerTaskSuccessful);
                }
                SysRequest::PlayerTaskFailure => {
                    commands.trigger(ScrOniSysEvent::PlayerTaskFailure);
                }
            }
        }

        all_messages.append(&mut script.exec.outgoing_messages);

        if is_done {
            let is_placeholder = script.exec.owner == Entity::PLACEHOLDER;
            if is_placeholder {
                // Detached scripts (no actor owner) are despawned
                // entirely — there's nothing else to attach them to.
                commands.queue(move |world: &mut World| {
                    if let Ok(mut e) = world.get_entity_mut(entity) {
                        e.despawn();
                    }
                });
            } else {
                // Actor-bound scripts: keep the component, just mark
                // the executor inactive so `tick()` early-returns next
                // frame.  Removing it broke the legacy `Reset()`
                // semantic — `scrScrOniComponent` lives as long as the
                // actor (legacy `scrScrOniComponent`), and
                // a later SCRIPT handler from the frontend / a SCRIPT
                // op from another script needs to find this component
                // to swap in a new main thread.  Concrete trigger:
                // actor_Tunnel1's `MainScript=DoNothing` (an empty
                // script) finishes on its first tick, the component
                // got dropped, and Main_Menu's
                // `SCRIPT "actor_Tunnel1" "$newgame:CameraMotion"`
                // had nothing to swap.  Set `active=false`; the swap
                // path resets it back to true.
                script.exec.active = false;
            }
        }
    }
    drop(_scripts_span);

    let _deliver_span = bevy::log::info_span!("scroni::deliver_messages").entered();
    // Deliver messages
    for msg in all_messages {
        let is_action = msg.is_action;
        let msg_text = msg.msg.clone();
        let target_entity = msg.to;
        if let Ok((_, mut target_script, _, _, _)) = query.get_mut(target_entity) {
            target_script.exec.message_queue.push(msg);
        } else if is_action {
            // `sendaction activate` / `sendaction deactivate` commonly
            // target static props (gears, particle emitters, lights,
            // ambient-sound sources, etc.) that have no ScrOniScript
            // attached.  Drive those through the ActorAsleep pipeline:
            //   activate   → remove ActorAsleep (wake — animator,
            //                fx observers, physics gravity all resume)
            //   deactivate → insert ActorAsleep (quiescent)
            // The AsleepPlugin handles the physics-side bookkeeping
            // (gravity layer + velocity pin), so it's safe to toggle
            // asleep on physics-bearing props without the FPS blowout
            // the legacy note warned about.  Unknown action words are
            // silently ignored — they may target subsystems we haven't
            // ported (e.g. `sendaction destroy`) that aren't asleep-
            // related.
            match msg_text.to_ascii_lowercase().as_str() {
                "activate" => {
                    commands
                        .entity(target_entity)
                        .remove::<crate::oni2_loader::components::ActorAsleep>();
                }
                "deactivate" => {
                    commands
                        .entity(target_entity)
                        .insert(crate::oni2_loader::components::ActorAsleep);
                }
                _ => {}
            }
        } else {
            warn!(
                "VM: Failed to deliver message '{}' to {:?}: target not found or has no ScrOniScript",
                msg_text, target_entity
            );
        }
    }
}

/// Load and compile a .oni script file, returning ScriptDefs.
pub fn load_script_file(dir: &str, filename: &str) -> Result<ScriptFile, String> {
    let source = crate::vfs::read_to_string(dir, filename)
        .map_err(|e| format!("Failed to read {}/{}: {}", dir, filename, e))?;
    Compiler::compile(&source).map_err(|errors| {
        let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        format!(
            "Compile errors in {}/{}:\n{}",
            dir,
            filename,
            msgs.join("\n")
        )
    })
}

#[derive(Resource, Clone, Debug)]
pub struct ScreenFadeState {
    pub current_color: Vec3,
    pub start_color: Vec3,
    pub target_color: Vec3,
    pub timer: f32,
    pub duration: f32,
}
impl Default for ScreenFadeState {
    fn default() -> Self {
        Self {
            current_color: Vec3::ONE,
            start_color: Vec3::ONE,
            target_color: Vec3::ONE,
            timer: 0.0,
            duration: 0.0,
        }
    }
}

#[derive(Component)]
pub struct ScreenFadeUi;

pub fn update_screen_fade_system(
    mut commands: Commands,
    time: Res<Time>,
    state_res: Option<ResMut<ScreenFadeState>>,
    mut query: Query<&mut BackgroundColor, With<ScreenFadeUi>>,
) {
    let Some(mut state) = state_res else {
        return;
    };
    if state.timer < state.duration {
        state.timer += time.delta_secs();
        if state.timer > state.duration {
            state.timer = state.duration;
        }
        let t = if state.duration > 0.0 {
            state.timer / state.duration
        } else {
            1.0
        };
        state.current_color = state.start_color.lerp(state.target_color, t);
    } else if state.duration == 0.0 {
        state.current_color = state.target_color;
    }

    let r = state.current_color.x.clamp(0.0, 1.0);
    let g = state.current_color.y.clamp(0.0, 1.0);
    let b = state.current_color.z.clamp(0.0, 1.0);
    let mean = (r + g + b) / 3.0;
    let opacity = 1.0 - mean;

    let bevy_color = Color::srgba(0.0, 0.0, 0.0, opacity);

    if let Some(mut bg_color) = query.iter_mut().next() {
        if opacity <= 0.001 {
            bg_color.0 = Color::NONE;
        } else {
            bg_color.0 = bevy_color;
        }
    } else if opacity > 0.001 {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Vw(100.0),
                height: Val::Vh(100.0),
                ..default()
            },
            BackgroundColor(bevy_color),
            GlobalZIndex(9999),
            ScreenFadeUi,
            crate::menu::InGameEntity,
        ));
    }
}

#[derive(Resource, Default)]
pub struct ScroniTextState {
    pub current_x: f32,
    pub current_y: f32,
}

#[derive(Component)]
pub struct ScroniTextElement {
    pub expires_at: f64,
}

pub fn cleanup_scroni_text(
    mut commands: Commands,
    query: Query<(Entity, &ScroniTextElement, &Text)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for (entity, text_element, _text) in &query {
        if now > text_element.expires_at
            && let Ok(mut e_cmd) = commands.get_entity(entity)
        {
            e_cmd.try_despawn();
        }
    }
}

/// Even when tempted to use a Trigger here, keep this On-based.
/// Observer to handle ScrOni system requests (like TextureMovie)

pub fn audio_ramp_system(
    time: Res<Time>,
    mut commands: Commands,
    mut audio_query: Query<(
        Entity,
        &mut bevy::audio::AudioSink,
        Option<&mut AudioVolumeRamp>,
        Option<&mut AudioPitchRamp>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut sink, vol_ramp_opt, pitch_ramp_opt) in &mut audio_query {
        if let Some(mut vr) = vol_ramp_opt {
            if vr.start_vol < 0.0 {
                vr.start_vol = match sink.volume() {
                    bevy::audio::Volume::Linear(v) => v,
                    bevy::audio::Volume::Decibels(v) => 10.0_f32.powf(v / 20.0),
                };
            }
            vr.elapsed += dt;
            let t = if vr.duration > 0.0 {
                (vr.elapsed / vr.duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let current = vr.start_vol + (vr.end_vol - vr.start_vol) * t;
            sink.set_volume(bevy::audio::Volume::Linear(current));
            if t >= 1.0
                && let Ok(mut e_cmd) = commands.get_entity(entity)
            {
                e_cmd.remove::<AudioVolumeRamp>();
            }
        }

        if let Some(mut pr) = pitch_ramp_opt {
            if pr.start_pitch < 0.0 {
                pr.start_pitch = sink.speed(); // Initialize dynamically
            }
            pr.elapsed += dt;
            let t = if pr.duration > 0.0 {
                (pr.elapsed / pr.duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let current = pr.start_pitch + (pr.end_pitch - pr.start_pitch) * t;
            sink.set_speed(current);
            if t >= 1.0
                && let Ok(mut e_cmd) = commands.get_entity(entity)
            {
                e_cmd.remove::<AudioPitchRamp>();
            }
        }
    }
}

pub fn checkpoint_trigger_system(
    mut checkpoint_idx: ResMut<crate::oni2_loader::components::CurrentCheckpointIndex>,
    trigger_query: Query<(
        &crate::oni2_loader::components::CheckpointTrigger,
        &GlobalTransform,
    )>,
    player_query: Query<&GlobalTransform, With<crate::player::components::Player>>,
) {
    let Some(player_tf) = player_query.iter().next() else {
        return;
    };
    let player_pos = player_tf.translation();

    for (trigger, trigger_tf) in &trigger_query {
        let dist = player_pos.distance(trigger_tf.translation());
        if dist <= trigger.radius && checkpoint_idx.0 != trigger.index {
            checkpoint_idx.0 = trigger.index;
            info!(
                "Player entered CheckpointTrigger: updated checkpoint_index to {}",
                trigger.index
            );
        }
    }
}

pub fn apply_shader_locals_system(
    mut commands: Commands,
    query: Query<(Entity, &ShaderLocals), Changed<ShaderLocals>>,
    children_query: Query<&Children>,
    mut child_materials: Query<(
        Entity,
        &mut MeshMaterial3d<StandardMaterial>,
        Option<&ClonedShaderLocalMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, shader_locals) in query.iter() {
        let mut target_uv_offset = None;
        if let Some(val) = shader_locals.locals.get("occulation") {
            target_uv_offset = Some(*val);
        }

        if target_uv_offset.is_none() {
            continue;
        }

        let mut queue = vec![entity];
        while let Some(current) = queue.pop() {
            if let Ok(children) = children_query.get(current) {
                for child in children.iter() {
                    queue.push(child);
                }
            }

            if let Ok((child_entity, mut mesh_mat, cloned)) = child_materials.get_mut(current) {
                if cloned.is_none() {
                    if let Some(mat_asset) = materials.get(mesh_mat.id()) {
                        let mut cloned_mat_val = mat_asset.clone();

                        if let Some(offset) = target_uv_offset {
                            cloned_mat_val.uv_transform =
                                bevy::math::Affine2::from_translation(Vec2::new(offset, offset));
                        }

                        let new_handle = materials.add(cloned_mat_val);
                        mesh_mat.0 = new_handle;
                        if let Ok(mut e_cmd) = commands.get_entity(child_entity) {
                            e_cmd.insert(ClonedShaderLocalMaterial);
                        }
                    }
                } else if let Some(target_mat) = materials.get_mut(mesh_mat.id())
                    && let Some(offset) = target_uv_offset
                {
                    target_mat.uv_transform =
                        bevy::math::Affine2::from_translation(Vec2::new(offset, offset));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statemachine::drivers::behavior::BehaviorKind;

    fn make_entity(idx: u32) -> Entity {
        // Synthesize an Entity for unit-test keys without spinning up a
        // World — Entity::from_raw_u32 is `pub` exactly for this case.
        Entity::from_raw_u32(idx).unwrap()
    }

    #[test]
    fn resolve_behavior_wait_resolves_on_end_message() {
        let actor = make_entity(1);
        let mut ended = std::collections::HashMap::new();
        ended.insert((actor, BehaviorKind::TakeCover), false);
        // Behavior finished cleanly — should resolve with failed=false.
        let r = resolve_behavior_wait(actor, BehaviorKind::TakeCover, None, 100.0, &ended);
        assert_eq!(r, Some(false));
    }

    #[test]
    fn resolve_behavior_wait_propagates_failure() {
        let actor = make_entity(1);
        let mut ended = std::collections::HashMap::new();
        ended.insert((actor, BehaviorKind::TakeCover), true);
        // Behavior reported Failed — flag flows into blocking_failed.
        let r = resolve_behavior_wait(actor, BehaviorKind::TakeCover, None, 100.0, &ended);
        assert_eq!(r, Some(true));
    }

    #[test]
    fn resolve_behavior_wait_keeps_waiting_when_no_signal() {
        let actor = make_entity(1);
        let ended = std::collections::HashMap::new();
        // No EndBehaviorMessage, no deadline — must keep waiting.
        let r = resolve_behavior_wait(actor, BehaviorKind::TakeCover, None, 100.0, &ended);
        assert_eq!(r, None);
    }

    #[test]
    fn resolve_behavior_wait_fires_on_timeout() {
        let actor = make_entity(1);
        let ended = std::collections::HashMap::new();
        // Deadline elapsed without an EndBehaviorMessage — resolve as
        // a non-failing timeout (matches C++ DoBehaviorDoneOrTimeout).
        let r =
            resolve_behavior_wait(actor, BehaviorKind::TakeCover, Some(50.0), 100.0, &ended);
        assert_eq!(r, Some(false));
    }

    #[test]
    fn resolve_behavior_wait_holds_before_deadline() {
        let actor = make_entity(1);
        let ended = std::collections::HashMap::new();
        // Deadline still in the future — keep waiting.
        let r =
            resolve_behavior_wait(actor, BehaviorKind::TakeCover, Some(150.0), 100.0, &ended);
        assert_eq!(r, None);
    }

    #[test]
    fn resolve_behavior_wait_end_message_beats_pending_timeout() {
        let actor = make_entity(1);
        let mut ended = std::collections::HashMap::new();
        ended.insert((actor, BehaviorKind::TakeCover), true);
        // Behavior failed BEFORE the deadline elapses — the failure
        // path takes priority over the (not-yet-due) timeout.
        let r =
            resolve_behavior_wait(actor, BehaviorKind::TakeCover, Some(150.0), 100.0, &ended);
        assert_eq!(
            r,
            Some(true),
            "end-message failure must beat the pending timeout"
        );
    }

    #[test]
    fn resolve_behavior_wait_ignores_other_actor_other_kind() {
        let me = make_entity(1);
        let other = make_entity(2);
        let mut ended = std::collections::HashMap::new();
        ended.insert((other, BehaviorKind::TakeCover), false);
        ended.insert((me, BehaviorKind::Goto), false);
        // Right entity wrong kind, or right kind wrong entity — both miss.
        let r = resolve_behavior_wait(me, BehaviorKind::TakeCover, None, 100.0, &ended);
        assert_eq!(r, None);
    }
}
