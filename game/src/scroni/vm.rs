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
            Value::Actor(act) => format!("Actor({:?})", act),
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
    /// Internal: waiting for CurveFollower to reach its target phase.
    /// Set by the bridge system after configuring the CurveFollower from a GotoCurvePhase.
    WaitingForCurve,
    /// Internal: waiting for a non-looping animation to finish playing.
    WaitingForAnimation,
    WaitingForPath,
}

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
    CameraSetPackage(String),
    CameraReset,
    CameraMode(String),
    CameraSetFOV(f32, f32), // Target FOV, Duration
    CameraShake,
    CameraFollowActor(Entity),
    CameraTrackActor(Entity),
    CameraTrackPoint(Vec3),
    CameraMoveToActor(Entity, f32), // Target, Duration
    CameraMoveToPoint(Vec3, f32),
    CameraMoveAlongRail(String, f32),
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
    PlaySound {
        script_entity: Entity,
        actor: Option<String>,
        name: String,
    },
    Teleport {
        script_entity: Entity,
        target: Entity,
        to: Option<Vec3>,
        face: Option<f32>,
    },
    CameraSetPackage(String),
    CameraReset,
    CameraMode(String),
    CameraSetFOV(f32, f32), // Target FOV, Duration
    CameraShake,
    CameraFollowActor(Entity),
    CameraTrackActor(Entity),
    CameraTrackPoint(Vec3),
    CameraMoveToActor(Entity, f32),
    CameraMoveToPoint(Vec3, f32),
    CameraMoveAlongRail(String, f32),
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
}

impl ScrOniThread {
    pub fn new(
        thread_id: u32,
        parent_thread_id: Option<u32>,
        script: ScriptDef,
    ) -> Self {
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
        if let Some(ref expr) = var.initializer {
            if let Some(c) = eval_constant(expr) {
                val = c;
            }
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
    pub all_entities: &'a Query<'w_e, 's_e, (Entity, &'static GlobalTransform, Option<&'static Name>)>,
    pub triggers: &'a Query<'w_t, 's_t, &'static BroadcastTrigger>,
    pub player: Option<Entity>,
    pub current_checkpoint: i32,
    pub layout_dir: String,
    /// Per-entity status string: "alive", "dead", "fighting". Built each frame.
    pub actor_statuses: &'a std::collections::HashMap<Entity, String>,
    /// Optional line-of-sight checker: (from, to, exclude_a, exclude_b) -> bool.
    pub line_of_sight: Option<&'a dyn Fn(Vec3, Vec3, Entity, Entity) -> bool>,
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
                    if let Some(n) = name_opt {
                        if hash_name(n.as_str()) == *guid {
                            targets.push(e);
                        }
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

        // Degrade timer variables (only local ones to avoid double-decrement on inherited variables)
        for thread in std::iter::once(&mut self.main_thread).chain(self.child_threads.iter_mut()) {
            if thread.state == ExecState::Done {
                continue;
            }
            for var_decl in &thread.script.variables {
                if var_decl.var_type == VarType::Timer && !var_decl.is_parent {
                    if let Some(val) = thread.variables.get(&var_decl.name) {
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
                        self.get_thread(tid).script.name, current_state
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
        let mut instruction_count = 0;
        let max_instructions = 10000;

        loop {
            // If we're inside a loop, continue that loop
            while !self.get_thread(tid).loop_stack.is_empty() {
                if self.get_thread(tid).state != ExecState::Running {
                    return;
                }

                let (active, should_pop) = self.step_top_loop(
                    tid,
                    &mut instruction_count,
                    max_instructions,
                    now,
                    ctx,
                );

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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Forever { body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Forever { body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::While { condition: condition.clone(), body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::While { condition: condition.clone(), body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::NTimes { remaining: *remaining, body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::NTimes { remaining: *remaining, body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::ForSeconds { end_time: *end_time, body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::ForSeconds { end_time: *end_time, body: body.clone(), pc: *pc };
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
                    self.get_thread_mut(tid).loop_stack[top_idx] = LoopState::Block { stmts: stmts.clone(), pc: *pc };
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
                let lower = name.to_lowercase();
                match lower.as_str() {
                    "clock" => Value::Float(now as f32),
                    "sin" => {
                        let val = args.get(0).map_or(0.0, |e| self.eval_expr(tid, e, now, ctx).as_float());
                        Value::Float(val.to_radians().sin())
                    }
                    "cos" => {
                        let val = args.get(0).map_or(0.0, |e| self.eval_expr(tid, e, now, ctx).as_float());
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
                        let min = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx).as_int()).unwrap_or(0);
                        let max = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx).as_int()).unwrap_or(100);
                        if max > min {
                            Value::Int(min + (rand::random::<i32>().unsigned_abs() as i32 % (max - min + 1)))
                        } else {
                            Value::Int(min)
                        }
                    }
                    "randomrangefloat" => {
                        let min = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx).as_float()).unwrap_or(0.0);
                        let max = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx).as_float()).unwrap_or(1.0);
                        let r: f32 = rand::random();
                        Value::Float(min + r * (max - min))
                    }
                    "guid" => {
                        if let Some(e) = args.get(0) {
                            let val = self.eval_expr(tid, e, now, ctx);
                            return Value::Int(hash_name(&val.as_string()));
                        }
                        Value::None
                    }
                    "exists" => {
                        let target = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(t) = target {
                            let ents = ctx.resolve_targets(&t);
                            for ent in ents {
                                if ctx.actor_statuses.get(&ent).map(|s| s.as_str()) != Some("dead") {
                                    return Value::Int(1);
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "status" => {
                        let target = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(t) = target {
                            let ents = ctx.resolve_targets(&t);
                            if let Some(&ent) = ents.first() {
                                let s = ctx.actor_statuses.get(&ent).map(|s| s.as_str()).unwrap_or("dead");
                                return Value::String(s.to_string());
                            }
                        }
                        Value::String("dead".to_string())
                    }
                    "alive" => Value::String("alive".to_string()),
                    "dead" => Value::String("dead".to_string()),
                    "fighting" => Value::String("fighting".to_string()),
                    "notdying" => Value::String("notdying".to_string()),
                    "knockeddown" => Value::String("knockeddown".to_string()),
                    "attacking" => Value::String("attacking".to_string()),
                    "location" => {
                        let target = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Actor(act)) = target {
                            if let Ok((_, tf, _)) = ctx.all_entities.get(act) {
                                let p = tf.translation();
                                return Value::Vector(space::to_oni2_space_pos(p));
                            }
                        }
                        Value::None
                    }
                    "distance" => {
                        let arg1 = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
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

                        if p1.is_some() && p2.is_none() {
                            if let Ok((_, my_tf, _)) = ctx.all_entities.get(self.owner) {
                                p2 = p1;
                                let my_p = my_tf.translation();
                                p1 = Some(space::to_oni2_space_pos(my_p));
                            }
                        }

                        if let (Some(a), Some(b)) = (p1, p2) {
                            return Value::Float(a.distance(b));
                        }
                        Value::Float(99999.0)
                    }
                    "trigger" => {
                        if let Some(event_expr) = args.get(0) {
                            let event_name = match event_expr {
                                Expr::Var(n) => n.clone(),
                                Expr::StringLit(s) => s.clone(),
                                _ => self.eval_expr(tid, event_expr, now, ctx).as_string(),
                            }
                            .to_lowercase();

                            if let Ok(trigger) = ctx.triggers.get(self.owner) {
                                match event_name.as_str() {
                                    "playerenter" => {
                                        if let Some(p) = ctx.player {
                                            if trigger.just_entered.contains(&p) {
                                                return Value::Int(1);
                                            }
                                        }
                                    }
                                    "playerexit" => {
                                        if let Some(p) = ctx.player {
                                            if trigger.just_exited.contains(&p) {
                                                return Value::Int(1);
                                            }
                                        }
                                    }
                                    "playerinside" => {
                                        if let Some(p) = ctx.player {
                                            if trigger.inside.contains(&p) {
                                                return Value::Int(1);
                                            }
                                        }
                                    }
                                    "playeroutside" => {
                                        if let Some(p) = ctx.player {
                                            if !trigger.inside.contains(&p) {
                                                return Value::Int(1);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        return Value::Int(0);
                    }
                    "triggerentered" => {
                        let trig_ent = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                        {
                            if let Ok(trigger) = ctx.triggers.get(t) {
                                if trigger.just_entered.contains(&e) {
                                    return Value::Int(1);
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "triggerexited" => {
                        let trig_ent = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                        {
                            if let Ok(trigger) = ctx.triggers.get(t) {
                                if trigger.just_exited.contains(&e) {
                                    return Value::Int(1);
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "triggerinside" => {
                        let trig_ent = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent)
                        {
                            if let Ok(trigger) = ctx.triggers.get(t) {
                                if trigger.inside.contains(&e) {
                                    return Value::Int(1);
                                }
                            }
                        }
                        Value::Int(0)
                    }
                    "receivemessage" => {
                        if let Some(msg_expr) = args.get(0) {
                            let target_msg = self.eval_expr(tid, msg_expr, now, ctx).as_string();
                            if let Some(idx) = self
                                .message_queue
                                .iter()
                                .position(|m| !m.is_action && m.msg == target_msg)
                            {
                                info!(
                                    "[ScrOni][{}] Received message '{}' (from {:?})",
                                    self.get_thread(tid).script.name, target_msg, self.message_queue[idx].from
                                );
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "receiveaction" => {
                        if let Some(msg_expr) = args.get(0) {
                            let target_msg = self.eval_expr(tid, msg_expr, now, ctx).as_string();
                            if let Some(idx) = self
                                .message_queue
                                .iter()
                                .position(|m| m.is_action && m.msg == target_msg)
                            {
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        } else {
                            if let Some(idx) = self.message_queue.iter().position(|m| m.is_action) {
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "getcheckpointindex" => Value::Int(ctx.current_checkpoint),
                    "first" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Value::ActorList(entities, _) = self.get_var(tid, list_name)
                            {
                                let updated = entities.clone();
                                if let Some(&first_ent) = updated.first() {
                                    self.set_var(
                                        tid,
                                        list_name.clone(),
                                        Value::ActorList(updated, 1),
                                    );
                                    return Value::Actor(first_ent);
                                } else {
                                    self.set_var(
                                        tid,
                                        list_name.clone(),
                                        Value::ActorList(updated, 0),
                                    );
                                    return Value::None;
                                }
                            }
                        }
                        Value::None
                    }
                    "playambientsound" => {
                        let n = args.get(0).map_or(String::new(), |e| self.eval_expr(tid, e, now, ctx).as_string());
                        let mut v = None;
                        let mut p = None;
                        let mut vr = None;
                        let mut pr = None;
                        let mut i = 1;
                        while i < args.len() {
                            if let Expr::StringLit(m) = &args[i] {
                                if m == "volumeramp" && i + 3 < args.len() {
                                    vr = Some((
                                        self.eval_expr(tid, &args[i+1], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i+2], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i+3], now, ctx).as_float(),
                                    ));
                                    i += 4;
                                    continue;
                                } else if m == "pitchramp" && i + 3 < args.len() {
                                    pr = Some((
                                        self.eval_expr(tid, &args[i+1], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i+2], now, ctx).as_float(),
                                        self.eval_expr(tid, &args[i+3], now, ctx).as_float(),
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
                        self.sys_requests.push(SysRequest::PlayAmbientSound(handle, n, v, p, vr, pr));
                        Value::Int(handle)
                    }
                    "size" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Value::ActorList(entities, _) = self.get_var(tid, list_name)
                            {
                                return Value::Int(entities.len() as i32);
                            }
                        }
                        Value::Int(0)
                    }
                    "next" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Value::ActorList(entities, idx) = self.get_var(tid, list_name)
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
                        }
                        Value::None
                    }
                    "isdone" => {
                        let target_var = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
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
                    "navpoint" | "path" => {
                        // navpoint("Name") or path("Name") -> return the name as a string so that
                        // the goto resolver can look it up in nav.names or path configs.
                        if let Some(e) = args.get(0) {
                            Value::String(self.eval_expr(tid, e, now, ctx).as_string())
                        } else {
                            Value::None
                        }
                    }
                    "lineofsight" => {
                        // lineofsight(actor_a, actor_b) -> bool (1 if unobstructed)
                        let a = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        let b = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        let resolve_pos = |val: &Value| -> Option<(Vec3, Entity)> {
                            match val {
                                Value::Actor(ent) => {
                                    ctx.all_entities.get(*ent).ok().map(|(e, tf, _)| (tf.translation(), e))
                                }
                                _ => None,
                            }
                        };
                        if let (Some(av), Some(bv)) = (a.as_ref(), b.as_ref()) {
                            if let (Some((pos_a, ent_a)), Some((pos_b, ent_b))) =
                                (resolve_pos(av), resolve_pos(bv))
                            {
                                if let Some(los_fn) = ctx.line_of_sight {
                                    return Value::Int(if los_fn(pos_a, pos_b, ent_a, ent_b) { 1 } else { 0 });
                                }
                            }
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
                let alive = ents.iter().any(|ent| {
                    ctx.actor_statuses.get(ent).map(|s| s.as_str()) != Some("dead")
                });
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
            .or_else(|| {
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
            if let (Value::Int(guid), Value::Actor(ent)) | (Value::Actor(ent), Value::Int(guid)) = (l, r) {
                if *guid == 0 {
                    return Value::Int(0);
                }
                let mut matched = false;
                if let Ok((_, _, Some(name))) = ctx.all_entities.get(*ent) {
                    if hash_name(name.as_str()) == *guid {
                        matched = true;
                    }
                }
                return Value::Int(if matched { 1 } else { 0 });
            }
            if let (Value::Actor(_), Value::None) | (Value::None, Value::Actor(_)) = (l, r) {
                return Value::Int(0);
            }
            if let (Value::String(s1), Value::String(s2)) = (l, r) {
                return Value::Int(if s1.to_lowercase() == s2.to_lowercase() { 1 } else { 0 });
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
    mut query: Query<(Entity, &mut ScrOniScript, &GlobalTransform, Option<&crate::ai::navigation::ActorPathfollower>, Option<&crate::ai::components::ActorRetreating>)>,
    all_entities: Query<(Entity, &'static GlobalTransform, Option<&'static Name>)>,
    triggers: Query<&'static BroadcastTrigger>,
    time: Res<Time>,
    player_query: Query<Entity, With<crate::player::components::Player>>,
    current_checkpoint: Res<crate::oni2_loader::components::CurrentCheckpointIndex>,
    layout_context: Option<Res<crate::oni2_loader::environment::LayoutContext>>,
    mut health_query: Query<(Entity, &mut crate::combat::components::Health, Option<&crate::ai::components::AiFighter>)>,
    nav_graph_opt: Option<Res<crate::ai::navigation::NavGraph>>,
    layout_paths: Option<Res<crate::oni2_loader::environment::LayoutPaths>>,
    spatial_query: avian3d::prelude::SpatialQuery,
    mut injure_writer: MessageWriter<crate::combat::events::InjureMessage>,
) {
    let now = time.elapsed_secs_f64();
    let delta_time = time.delta_secs();
    let mut all_messages = Vec::new();
    let player_ent = player_query.iter().next();

    let mut actor_statuses = std::collections::HashMap::new();
    for (ent, health, ai_opt) in health_query.iter() {
        let status = if health.current <= 0.0 {
            "dead"
        } else if let Some(ai) = ai_opt {
            // [AUDIT]: Prototype leakage. `AiState` variants here match the prototype logic to emit generic "fighting".
            // A real combat AI should decouple its specific behavior trees from the VM's high-level `status` inquiry.
            match ai.state {
                crate::ai::components::AiState::Pursuing
                | crate::ai::components::AiState::Circling
                | crate::ai::components::AiState::Attacking
                | crate::ai::components::AiState::Recovering => "fighting",
                _ => "alive",
            }
        } else {
            "alive"
        };
        actor_statuses.insert(ent, status.to_string());
    }

    for (entity, mut script, transform, pathfollower_opt, retreating_opt) in &mut query {
        if script.exec.ticks_alive == 0 {
            script.exec.ticks_alive += 1;
            continue;
        }
        script.exec.ticks_alive += 1;

        let los_checker = |from: Vec3, to: Vec3, exc_a: Entity, exc_b: Entity| -> bool {
            let delta = to - from;
            let dist = delta.length();
            if dist < 0.01 {
                return true;
            }
            let Ok(dir) = Dir3::new(delta / dist) else { return true; };
            let filter = avian3d::prelude::SpatialQueryFilter::from_excluded_entities([exc_a, exc_b]);
            spatial_query.cast_ray(from, dir, dist * 0.99, true, &filter).is_none()
        };
        let los_ref: &dyn Fn(Vec3, Vec3, Entity, Entity) -> bool = &los_checker;
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
        };
        script.exec.tick(now, delta_time, &mut ctx);

        let is_done = script.exec.main_thread.state == ExecState::Done;

        let mut gotos_to_resolve = Vec::new();
        let mut patrols_to_resolve = Vec::new();
        let mut waiting_for_path = Vec::new();
        for t in script.exec.all_threads_mut() {
            if let Some(BlockingAction::GotoPoint { target, within, speed, duration }) = t.blocking.clone() {
                gotos_to_resolve.push((t.thread_id, target, within, speed, duration));
            } else if let Some(BlockingAction::Patrol(path_val)) = t.blocking.clone() {
                patrols_to_resolve.push((t.thread_id, path_val));
            } else if let Some(BlockingAction::WaitingForPath) = t.blocking.clone() {
                waiting_for_path.push(t.thread_id);
            }
        }
        
        for tid in waiting_for_path {
            if pathfollower_opt.is_none() {
                script.exec.clear_blocking(tid);
                script.exec.tick_thread(tid, now, &mut ctx);
            }
        }

        for (tid, path_val) in patrols_to_resolve {
            let path_name = path_val.as_string();
            let waypoints = layout_paths
                .as_ref()
                .and_then(|lp| lp.curves.iter().find(|(n, _)| n.eq_ignore_ascii_case(&path_name)))
                .map(|(_, pts)| pts.clone());
            if let Some(pts) = waypoints {
                commands.entity(entity).insert(crate::ai::navigation::ActorPathfollower {
                    path: pts,
                    current_wp: 0,
                    speed_throttle: 1.0,
                    within: None,
                });
                script.exec.get_thread_mut(tid).blocking = Some(BlockingAction::WaitingForPath);
            } else {
                warn!("patrol: path '{}' not found in LayoutPaths", path_name);
                script.exec.clear_blocking(tid);
                script.exec.tick_thread(tid, now, &mut ctx);
            }
        }

        for (tid, target, within, speed, duration) in gotos_to_resolve {
            let mut resolved_pos = None;
            if let Value::Vector(v) = target {
                resolved_pos = Some(space::to_bevy_space_pos(v)); // Convert to Bevy coords
            } else if let Value::String(s) = target {
                if let Some(nav) = &nav_graph_opt {
                    if let Some(idx) = nav.names.get(&s) {
                        resolved_pos = Some(nav.points[*idx]);
                    }
                }
            } else if let Value::Actor(act) = target {
                if let Ok((_, tf, _)) = all_entities.get(act) {
                    resolved_pos = Some(tf.translation());
                }
            }
            
            if let Some(pos) = resolved_pos {
                if let Some(nav) = &nav_graph_opt {
                    if let Some(path) = nav.find_path_to_point(transform.translation(), pos) {
                        let throttle = speed.unwrap_or(1.0);
                        commands.entity(entity).insert(crate::ai::navigation::ActorPathfollower {
                            path,
                            current_wp: 0,
                            speed_throttle: throttle,
                            within,
                        });
                        script.exec.get_thread_mut(tid).blocking = Some(BlockingAction::WaitingForPath);
                        continue;
                    }
                }
            }
            
            // If we get here, pathfinding failed or target resolved to None. Just skip.
            script.exec.clear_blocking(tid);
            script.exec.tick_thread(tid, now, &mut ctx);
        }

        let script_name = script.exec.main_thread.script.name.clone();
        for req in script.exec.sys_requests.drain(..) {
            match req {
                SysRequest::MakeExplosion { name, orientation, at } => {
                    commands.trigger(ScrOniSysEvent::MakeExplosion {
                        script_entity: entity,
                        name,
                        orientation,
                        at,
                    });
                }
                SysRequest::PlaySound(actor, name) => {
                    commands.trigger(ScrOniSysEvent::PlaySound {
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
                    commands.entity(actor).insert(crate::combat::faction::Faction(faction));
                }
                SysRequest::Retreat { actor, target } => {
                    commands.entity(actor).insert(crate::ai::components::ActorRetreating { avoid_target: target });
                }
                SysRequest::CameraSetPackage(pkg_name) => {
                    commands.trigger(ScrOniSysEvent::CameraSetPackage(pkg_name));
                }
                SysRequest::CameraReset => {
                    commands.trigger(ScrOniSysEvent::CameraReset);
                }
                SysRequest::CameraMode(mode) => {
                    commands.trigger(ScrOniSysEvent::CameraMode(mode));
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
                SysRequest::PlayAmbientSound(handle, name, volume, pitch, volume_ramp, pitch_ramp) => {
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
                SysRequest::Hit { target, hit_type, damage } => {
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
                        strike_react_enum: None,
                    });
                }
                SysRequest::AmbientSoundStop(handle) => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundStop {
                        script_entity: entity,
                        handle,
                    });
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
            commands.queue(move |world: &mut World| {
                if let Ok(mut e) = world.get_entity_mut(entity) {
                    if is_placeholder {
                        e.despawn();
                    } else {
                        e.remove::<ScrOniScript>();
                    }
                }
            });
        }
    }

    // Deliver messages
    for msg in all_messages {
        let is_action = msg.is_action;
        let msg_text = msg.msg.clone();
        let target_entity = msg.to;
        if let Ok((_, mut target_script, _, _, _)) = query.get_mut(target_entity) {
            target_script.exec.message_queue.push(msg);
        } else {
            // "sendaction activate" and "sendaction deactivate" target raw props without script components
            // regularly in Oni level scripts. So silence the warning if the message was sent as an action.
            if is_action {
                // "sendaction activate" targets static props, we specifically want to silence the target missing warning here.
                // We should NOT actually remove ActorAsleep natively via script actions for raw props because it forces continuous physics simulation on things like gears and drops the FPS massively.
                // TODO: ensure we actually send activate and deactivate messages to static props in the engine *somewhere*
                // this will control particles/sounds/scroni/animations/etc on that actor
            } else {
                warn!(
                    "VM: Failed to deliver message '{}' to {:?}: target not found or has no ScrOniScript",
                    msg_text, target_entity
                );
            }
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
    mut state_res: Option<ResMut<ScreenFadeState>>,
    mut query: Query<&mut BackgroundColor, With<ScreenFadeUi>>,
) {
    let Some(mut state) = state_res else { return; };
    if state.timer < state.duration {
        state.timer += time.delta_secs();
        if state.timer > state.duration {
            state.timer = state.duration;
        }
        let t = if state.duration > 0.0 { state.timer / state.duration } else { 1.0 };
        state.current_color = state.start_color.lerp(state.target_color, t);
    } else {
        if state.duration == 0.0 {
            state.current_color = state.target_color;
        }
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
    } else {
        if opacity > 0.001 {
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
        if now > text_element.expires_at {
            commands.entity(entity).try_despawn();
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
    for (entity, mut sink, mut vol_ramp_opt, mut pitch_ramp_opt) in &mut audio_query {
        if let Some(mut vr) = vol_ramp_opt {
            if vr.start_vol < 0.0 {
                vr.start_vol = match sink.volume() {
                    bevy::audio::Volume::Linear(v) => v,
                    bevy::audio::Volume::Decibels(v) => 10.0_f32.powf(v / 20.0),
                };
            }
            vr.elapsed += dt;
            let t = if vr.duration > 0.0 { (vr.elapsed / vr.duration).clamp(0.0, 1.0) } else { 1.0 };
            let current = vr.start_vol + (vr.end_vol - vr.start_vol) * t;
            sink.set_volume(bevy::audio::Volume::Linear(current));
            if t >= 1.0 {
                commands.entity(entity).remove::<AudioVolumeRamp>();
            }
        }
        
        if let Some(mut pr) = pitch_ramp_opt {
            if pr.start_pitch < 0.0 {
                pr.start_pitch = sink.speed(); // Initialize dynamically
            }
            pr.elapsed += dt;
            let t = if pr.duration > 0.0 { (pr.elapsed / pr.duration).clamp(0.0, 1.0) } else { 1.0 };
            let current = pr.start_pitch + (pr.end_pitch - pr.start_pitch) * t;
            sink.set_speed(current);
            if t >= 1.0 {
                commands.entity(entity).remove::<AudioPitchRamp>();
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
        if dist <= trigger.radius {
            if checkpoint_idx.0 != trigger.index {
                checkpoint_idx.0 = trigger.index;
                info!(
                    "Player entered CheckpointTrigger: updated checkpoint_index to {}",
                    trigger.index
                );
            }
        }
    }
}

pub fn apply_shader_locals_system(
    mut commands: Commands,
    query: Query<(Entity, &ShaderLocals), Changed<ShaderLocals>>,
    children_query: Query<&Children>,
    mut child_materials: Query<(Entity, &mut MeshMaterial3d<StandardMaterial>, Option<&ClonedShaderLocalMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, shader_locals) in query.iter() {
        // Evaluate supported parameters mapped functionally
        let mut target_uv_offset = None;
        if let Some(val) = shader_locals.locals.get("occulation") {
            // "occulation" mathematically translates UVs in our shader definition logic
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
                // Lazily isolate instance material Handle exactly once per mesh utilizing COW
                if cloned.is_none() {
                    if let Some(mat_asset) = materials.get(mesh_mat.id()) {
                        let mut cloned_mat_val = mat_asset.clone();
                        
                        if let Some(offset) = target_uv_offset {
                            // maybe invert?
                            cloned_mat_val.uv_transform = bevy::math::Affine2::from_translation(
                                Vec2::new(offset, offset));
                        }
                        
                        let new_handle = materials.add(cloned_mat_val);
                        mesh_mat.0 = new_handle;
                        commands.entity(child_entity).insert(ClonedShaderLocalMaterial);
                    }
                } else {
                    // Already cloned, mutating in-place is instance-safe
                    if let Some(target_mat) = materials.get_mut(mesh_mat.id()) {
                        if let Some(offset) = target_uv_offset {
                            // maybe invert?
                            target_mat.uv_transform = bevy::math::Affine2::from_translation(
                                Vec2::new(offset, offset));
                        }
                    }
                }
            }
        }
    }
}