use std::collections::HashMap;

use bevy::prelude::*;

use super::ast::*;
use super::compiler::Compiler;

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
    Retreat,
    /// Internal: waiting for CurveFollower to reach its target phase.
    /// Set by the bridge system after configuring the CurveFollower from a GotoCurvePhase.
    WaitingForCurve,
    /// Internal: waiting for a non-looping animation to finish playing.
    WaitingForAnimation,
    /// Request to the ECS system to query entities and return an actor list.
    Find {
        list_var: String,
        conditions: Vec<(String, Value)>,
        range: Option<f32>,
    },
}

#[derive(Debug, Clone)]
pub enum SysRequest {
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
    CameraSetPackage(String),
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
    pub start_time: f64,
    pub in_whenever: bool,
}

impl ScrOniThread {
    pub fn new(
        thread_id: u32,
        parent_thread_id: Option<u32>,
        script: ScriptDef,
        start_time: f64,
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
            start_time,
            in_whenever: false,
        }
    }

    pub fn clear_blocking(&mut self) {
        self.blocking = None;
        if self.state == ExecState::Blocked {
            self.state = ExecState::Running;
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
    /// Warned tracking cache preventing log flooding.
    pub warned_unimplemented: std::collections::HashSet<String>,
    /// Number of frames this script has been alive. Used to delay first tick protecting hierarchy initialization natively.
    pub ticks_alive: u32,
}

#[derive(Debug, Clone)]
pub struct ScroniContext<'a, 'w_e, 's_e, 'w_t, 's_t> {
    pub all_entities: &'a Query<'w_e, 's_e, (Entity, &'static GlobalTransform, Option<&'static Name>)>,
    pub triggers: &'a Query<'w_t, 's_t, &'static BroadcastTrigger>,
    pub player: Option<Entity>,
    pub current_checkpoint: i32,
    pub layout_dir: String,
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
    pub fn new(script: ScriptDef, owner: Entity, start_time: f64) -> Self {
        Self {
            main_thread: ScrOniThread::new(0, None, script, start_time),
            child_threads: Vec::new(),
            next_thread_id: 1,
            available_scripts: HashMap::new(),
            message_queue: Vec::new(),
            outgoing_messages: Vec::new(),
            sys_requests: Vec::new(),
            owner,
            current_light: None,
            active: true,
            warned_unimplemented: std::collections::HashSet::new(),
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
                if current_state == ExecState::Yielded || current_state == ExecState::Blocked {
                    warn!(
                        "[ScrOni] Whenever block attempted to block or yield (state: {:?})",
                        current_state
                    );
                    self.get_thread_mut(tid).state = ExecState::Running;
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

                let mut ls = self.get_thread_mut(tid).loop_stack.pop().unwrap();
                let pre_len = self.get_thread(tid).loop_stack.len();

                let (active, push_back) = self.step_loop(
                    tid,
                    &mut ls,
                    &mut instruction_count,
                    max_instructions,
                    now,
                    ctx,
                );

                if push_back {
                    let cur_len = self.get_thread(tid).loop_stack.len();
                    if cur_len >= pre_len {
                        self.get_thread_mut(tid).loop_stack.insert(pre_len, ls);
                    }
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

    /// Step a loop. Returns (still_active, should_push_back).
    fn step_loop(
        &mut self,
        tid: u32,
        ls: &mut LoopState,
        ins_count: &mut usize,
        max_ins: usize,
        now: f64,
        ctx: &mut ScroniContext,
    ) -> (bool, bool) {
        match ls {
            LoopState::Forever { body, pc } => {
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame conditionally, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, true);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) {
                    return res;
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0; // restart loop
                    return (false, true); // active=false, push_back=true -> instantly loops without yielding a frame
                }
                (true, true) // blocked — keep loop
            }
            LoopState::While {
                condition,
                body,
                pc,
            } => {
                let cond = condition.clone();
                let cond_val = self.eval_expr(tid, &cond, now, ctx);
                if !cond_val.as_bool() {
                    return (false, false); // loop done
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, true);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) {
                    return res;
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0;
                    return (false, true); // loop instantly to condition evaluater without yielding
                }
                (true, true)
            }
            LoopState::NTimes {
                remaining,
                body,
                pc,
            } => {
                if *remaining <= 0 {
                    return (false, false);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, true);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) {
                    return res;
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *remaining -= 1;
                    *pc = 0;
                    let still_active = *remaining > 0;
                    return (false, still_active);
                }
                (true, true)
            }
            LoopState::ForSeconds { end_time, body, pc } => {
                if now >= *end_time {
                    return (false, false);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    if *ins_count >= max_ins {
                        warn!(
                            "[ScrOni][{}] Script exceeded {} instructions in a single frame natively, force-yielding to prevent engine lockup!",
                            self.get_thread(tid).script.name,
                            max_ins
                        );
                        self.get_thread_mut(tid).state = ExecState::Yielded;
                        return (true, true);
                    }
                    *ins_count += 1;

                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) {
                    return res;
                }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0;
                    return (true, true);
                }
                (true, true)
            }
            LoopState::Block { stmts, pc } => {
                while *pc < stmts.len() && self.get_thread(tid).state == ExecState::Running {
                    let stmt = stmts[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) {
                    return res;
                }
                if *pc >= stmts.len() {
                    return (false, false); // block done
                }
                (true, true)
            }
        }
    }

    fn exec_stmt(&mut self, tid: u32, stmt: &Stmt, now: f64, ctx: &mut ScroniContext) {
        if self.get_thread(tid).state != ExecState::Running {
            return;
        }

        match stmt {
            Stmt::Set { var, value } => {
                let val = self.eval_expr(tid, value, now, ctx);
                self.set_var(tid, var.clone(), val);
            }
            Stmt::AddToList { expr, list } => {
                let val = self.eval_expr(tid, expr, now, ctx);
                let mut current_list = self.get_var(tid, list);

                if matches!(current_list, Value::None | Value::Int(0)) {
                    current_list = Value::ActorList(Vec::new(), 0);
                }

                if let Value::ActorList(mut vec, idx) = current_list {
                    vec.extend(ctx.resolve_targets(&val));
                    self.set_var(tid, list.clone(), Value::ActorList(vec, idx));
                }
            }
            Stmt::RemoveFromList { expr, list } => {
                let val = self.eval_expr(tid, expr, now, ctx);

                // Directly extract entities without validating ECS liveness
                // so we can properly clean dead handles out of memory lists!
                let targets = match val {
                    Value::Actor(act) => vec![act],
                    Value::ActorList(acts, _) => acts,
                    Value::Int(_) => ctx.resolve_targets(&val),
                    _ => Vec::new(),
                };

                if targets.is_empty() {
                    return;
                }

                let current_list = self.get_var(tid, list);
                if let Value::ActorList(mut vec, mut idx) = current_list {
                    let original_len = vec.len();
                    vec.retain(|e| !targets.contains(e));
                    let removed_count = original_len - vec.len();
                    if removed_count > 0 && idx > 0 {
                        idx = idx.saturating_sub(removed_count);
                    }
                    self.set_var(tid, list.clone(), Value::ActorList(vec, idx));
                }
            }
            Stmt::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond = self.eval_expr(tid, condition, now, ctx);
                if cond.as_bool() {
                    self.exec_stmt(tid, then_branch, now, ctx);
                } else if let Some(else_b) = else_branch {
                    self.exec_stmt(tid, else_b, now, ctx);
                }
            }
            Stmt::Block(stmts) => {
                if self.get_thread(tid).in_whenever {
                    for stmt in stmts {
                        self.exec_stmt(tid, stmt, now, ctx);
                    }
                } else {
                    self.get_thread_mut(tid).loop_stack.push(LoopState::Block {
                        stmts: stmts.clone(),
                        pc: 0,
                    });
                    self.get_thread_mut(tid).state = ExecState::PushLoop;
                }
            }
            Stmt::DoForever(body) => {
                if self.get_thread(tid).in_whenever {
                    let mut count = 0;
                    loop {
                        self.exec_stmt(tid, body, now, ctx);
                        count += 1;
                        if count > 1000 {
                            warn!("Whenever block DoForever loop exceeded 1000 iterations!");
                            break;
                        }
                    }
                } else {
                    let stmts = self.flatten_to_block(body);
                    self.get_thread_mut(tid)
                        .loop_stack
                        .push(LoopState::Forever { body: stmts, pc: 0 });
                    self.get_thread_mut(tid).state = ExecState::PushLoop;
                }
            }
            Stmt::DoWhile { condition, body } => {
                if self.get_thread(tid).in_whenever {
                    let mut count = 0;
                    while self.eval_expr(tid, condition, now, ctx).as_bool() {
                        self.exec_stmt(tid, body, now, ctx);
                        count += 1;
                        if count > 1000 {
                            warn!("Whenever block DoWhile loop exceeded 1000 iterations!");
                            break;
                        }
                    }
                } else {
                    let stmts = self.flatten_to_block(body);
                    self.get_thread_mut(tid).loop_stack.push(LoopState::While {
                        condition: condition.clone(),
                        body: stmts,
                        pc: 0,
                    });
                    self.get_thread_mut(tid).state = ExecState::PushLoop;
                }
            }
            Stmt::DoNTimes { count, body } => {
                let n = self.eval_expr(tid, count, now, ctx).as_int();
                if self.get_thread(tid).in_whenever {
                    let mut loop_count = 0;
                    for _ in 0..n {
                        self.exec_stmt(tid, body, now, ctx);
                        loop_count += 1;
                        if loop_count > 1000 {
                            warn!("Whenever block DoNTimes loop exceeded 1000 iterations!");
                            break;
                        }
                    }
                } else {
                    let stmts = self.flatten_to_block(body);
                    self.get_thread_mut(tid).loop_stack.push(LoopState::NTimes {
                        remaining: n,
                        body: stmts,
                        pc: 0,
                    });
                    self.get_thread_mut(tid).state = ExecState::PushLoop;
                }
            }
            Stmt::DoForSeconds { seconds, body } => {
                let secs = self.eval_expr(tid, seconds, now, ctx).as_float();
                if self.get_thread(tid).in_whenever {
                    warn!("DoForSeconds is not supported synchronously inside a whenever block!");
                } else {
                    let stmts = self.flatten_to_block(body);
                    self.get_thread_mut(tid)
                        .loop_stack
                        .push(LoopState::ForSeconds {
                            end_time: now + secs as f64,
                            body: stmts,
                            pc: 0,
                        });
                    self.get_thread_mut(tid).state = ExecState::PushLoop;
                }
            }
            Stmt::Exit => {
                self.get_thread_mut(tid).state = ExecState::Yielded;
            }
            Stmt::Done => {
                let frame = self.get_thread_mut(tid).call_stack.pop();
                if let Some(frame) = frame {
                    let t = self.get_thread_mut(tid);
                    t.script = frame.script;
                    t.variables = frame.variables;
                    t.seq_pc = frame.seq_pc;
                    t.loop_stack = frame.loop_stack;
                    t.state = ExecState::AbortSequence;
                } else {
                    self.get_thread_mut(tid).state = ExecState::Done;
                }
            }
            Stmt::Home => {
                let t = self.get_thread_mut(tid);
                t.seq_pc = 0;
                t.loop_stack.clear();
                t.state = ExecState::AbortSequence;
            }
            Stmt::Log(exprs) => {
                let parts: Vec<String> = exprs
                    .iter()
                    .map(|e| {
                        let v = self.eval_expr(tid, e, now, ctx);
                        v.as_string()
                    })
                    .collect();
                info!(
                    "[ScrOni][{}] {}",
                    self.get_thread(tid).script.name,
                    parts.join(" ")
                );
            }

            // Blocking commands — set blocking action and yield
            Stmt::Idle(expr) => {
                let secs = self.eval_expr(tid, expr, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Idle {
                    end_time: now + secs as f64,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurvePhase { phase, seconds } => {
                let p = self.eval_expr(tid, phase, now, ctx).as_float();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurvePhase {
                    target: p,
                    seconds: s,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurveKnot { knot, seconds } => {
                let k = self.eval_expr(tid, knot, now, ctx).as_int();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurveKnot {
                    knot: k,
                    seconds: s,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurveLerp { lerp, seconds } => {
                let l = self.eval_expr(tid, lerp, now, ctx).as_float();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurveLerp {
                    target: l,
                    seconds: s,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::Face { target, seconds } => {
                let t = self.eval_expr(tid, target, now, ctx);
                let s = seconds
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Face {
                    target: t,
                    seconds: s,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoPoint {
                target,
                within,
                speed,
                duration,
            } => {
                let t = self.eval_expr(tid, target, now, ctx);
                let w = within
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let s = speed
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let d = duration
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoPoint {
                    target: t,
                    within: w,
                    speed: s,
                    duration: d,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::PlayAnimation {
                name,
                hold,
                loop_anim,
                rate,
                duration,
            } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let r = rate
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let d = duration
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::PlayAnimation {
                    name: n,
                    hold: *hold,
                    loop_anim: *loop_anim,
                    rate: r,
                    duration: d,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::PlayActionAnimation {
                name,
                hold,
                loop_anim,
                duration,
            } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let d = duration
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::PlayAnimation {
                    name: n,
                    hold: *hold,
                    loop_anim: *loop_anim,
                    rate: None,
                    duration: d,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::Fight => {
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Fight);
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::Shoot => {
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Shoot);
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }

            // Non-blocking curve commands — set variables for external systems to read
            Stmt::SetCurvePhase(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_phase".into(), v);
            }
            Stmt::SetCurveSpeed(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_speed".into(), v);
            }
            Stmt::SetCurveKs(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_ks".into(), v);
            }
            Stmt::SetCurvePingPong(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_pingpong".into(), v);
            }
            Stmt::SetCurve { name, at_phase } => {
                let n = self.eval_expr(tid, name, now, ctx);
                self.set_var(tid, "__curve_name".into(), n);
                if let Some(p) = at_phase {
                    let v = self.eval_expr(tid, p, now, ctx);
                    self.set_var(tid, "__curve_phase".into(), v);
                }
            }
            Stmt::SetLerpCurve(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__lerp_curve".into(), v);
            }
            Stmt::SetLookUpCurve(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__lookup_curve".into(), v);
            }
            Stmt::SetCurveLookAtActor(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_lookat".into(), v);
            }
            Stmt::SetCurveLookAlongDistance(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_lookalong_dist".into(), v);
            }
            Stmt::SetCurveLookAlongDirection(expr) => {
                let v = self.eval_expr(tid, expr, now, ctx);
                self.set_var(tid, "__curve_lookalong_dir".into(), v);
            }

            Stmt::Pickup(expr) => {
                let ent = self.eval_expr(tid, expr, now, ctx);
                info!("VM: Pickup {:?} (unimplemented)", ent);
            }
            Stmt::Dropoff { at } => {
                let at_val = at.as_ref().map(|e| self.eval_expr(tid, e, now, ctx));
                info!("VM: Dropoff at {:?} (unimplemented)", at_val);
            }

            Stmt::InlineVarDecl(decl) => {
                let val = if let Some(init) = &decl.initializer {
                    self.eval_expr(tid, init, now, ctx)
                } else {
                    match decl.var_type {
                        VarType::Integer => Value::Int(0),
                        VarType::Float => Value::Float(0.0),
                        VarType::Vector => Value::Vector(Vec3::ZERO),
                        VarType::String => Value::String(String::new()),
                        _ => Value::None,
                    }
                };
                self.set_var(tid, decl.name.clone(), val);
            }

            Stmt::Find {
                list_var,
                conditions,
                range,
            } => {
                let eval_conds = conditions
                    .iter()
                    .map(|(k, e)| (k.clone(), self.eval_expr(tid, e, now, ctx)))
                    .collect();
                let eval_range = range
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Find {
                    list_var: list_var.clone(),
                    conditions: eval_conds,
                    range: eval_range,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }

            Stmt::TextureMovie {
                name,
                pass: _,
                action,
                arg,
            } => {
                let target_name = self.eval_expr(tid, name, now, ctx).as_string();
                let arg_val = self.eval_expr(tid, arg, now, ctx);
                self.sys_requests.push(SysRequest::TextureMovie {
                    target_name,
                    action: *action,
                    arg: arg_val,
                });
            }

            Stmt::SendMessage { msg, to, with } => {
                let msg_str = self.eval_expr(tid, msg, now, ctx).as_string();
                let target = self.eval_expr(tid, to, now, ctx);

                let mut args = Vec::new();
                for a in with {
                    args.push(self.eval_expr(tid, a, now, ctx));
                }

                let targets = ctx.resolve_targets(&target);
                if targets.is_empty() {
                    warn!(
                        "[ScrOni][{}] VM: SendMessage '{}' failed, target {:?} unresolved",
                        self.get_thread(tid).script.name,
                        msg_str,
                        target
                    );
                }

                for entity in targets {
                    let mut target_name = format!("{:?}", entity);
                    if let Ok((_, _, Some(n))) = ctx.all_entities.get(entity) {
                        target_name = format!("{} ({:?})", n.as_str(), entity);
                    }

                    let mut sender_name = format!("{:?}", self.owner);
                    if let Ok((_, _, Some(n))) = ctx.all_entities.get(self.owner) {
                        sender_name = format!("{} ({:?})", n.as_str(), self.owner);
                    }

                    info!(
                        "[ScrOni][{}] VM: SendMessage '{}' from {} to {}",
                        self.get_thread(tid).script.name,
                        msg_str,
                        sender_name,
                        target_name
                    );
                    self.outgoing_messages.push(ScriptMessage {
                        msg: msg_str.clone(),
                        from: self.owner,
                        to: entity,
                        args: args.clone(),
                        is_action: false,
                    });
                }
            }
            Stmt::SendAction {
                action,
                target,
                component,
            } => {
                let act_str = match action {
                    Expr::Var(n) => n.clone(),
                    Expr::StringLit(s) => s.clone(),
                    _ => self.eval_expr(tid, action, now, ctx).as_string(),
                };

                let mut targets = if let Some(target_expr) = target {
                    let tgt = self.eval_expr(tid, target_expr, now, ctx);
                    let res = ctx.resolve_targets(&tgt);
                    if res.is_empty() {
                        warn!(
                            "VM: SendAction '{}' failed, target {:?} unresolved",
                            act_str, tgt
                        );
                    }
                    res
                } else {
                    vec![self.owner]
                };

                for entity in targets {
                    if let Some(comp_expr) = component {
                        let comp_str = match comp_expr {
                            Expr::Var(n) => n.clone(),
                            Expr::StringLit(s) => s.clone(),
                            _ => self.eval_expr(tid, comp_expr, now, ctx).as_string(),
                        };
                        self.sys_requests.push(SysRequest::SendAction {
                            action: act_str.clone(),
                            target: entity,
                            component: comp_str,
                        });
                    } else {
                        self.outgoing_messages.push(ScriptMessage {
                            msg: act_str.clone(),
                            from: self.owner,
                            to: entity,
                            args: Vec::new(),
                            is_action: true,
                        });
                    }
                }
            }
            Stmt::Teleport { target, to, face } => {
                if let Value::Actor(ent) = self.eval_expr(tid, target, now, ctx) {
                    let to_vec = to.as_ref().map(|e| match self.eval_expr(tid, e, now, ctx) {
                        Value::Vector(v) => v,
                        _ => Vec3::ZERO,
                    });
                    let face_float = face
                        .as_ref()
                        .map(|e| self.eval_expr(tid, e, now, ctx).as_float());

                    self.sys_requests.push(SysRequest::Teleport {
                        target: ent,
                        to: to_vec,
                        face: face_float,
                    });
                }
            }

            Stmt::Spawn {
                script,
                assign_to,
                at,
                name,
            } => {
                info!(
                    "Spawn command: script={:?}, assign_to={:?}, at={:?}, name={:?}",
                    script, assign_to, at, name
                );
                let script_str = self.eval_expr(tid, script, now, ctx).as_string();
                let assign = assign_to.clone();
                let at_pos = at.as_ref().map(|e| match self.eval_expr(tid, e, now, ctx) {
                    Value::Vector(v) => v,
                    _ => Vec3::ZERO,
                });
                let target_name = name
                    .as_ref()
                    .map(|e| self.eval_expr(tid, e, now, ctx).as_string());

                self.sys_requests.push(SysRequest::Spawn {
                    script: script_str,
                    assign_to: assign,
                    at: at_pos,
                    name: target_name,
                });
            }

            Stmt::MakeFx { name, at } => {
                let fx_name = self.eval_expr(tid, name, now, ctx).as_string();
                let fx_pos = at.as_ref().map(|e| match self.eval_expr(tid, e, now, ctx) {
                    Value::Vector(v) => v,
                    Value::Actor(ent) => {
                        if let Ok((_, tf, _)) = ctx.all_entities.get(ent) {
                            tf.translation()
                        } else {
                            Vec3::ZERO
                        }
                    }
                    _ => Vec3::ZERO,
                });

                self.sys_requests.push(SysRequest::MakeFx {
                    script_entity: self.owner,
                    name: fx_name,
                    at: fx_pos,
                });
            }

            Stmt::Stack(name_expr) => {
                let name = self.eval_expr(tid, name_expr, now, ctx).as_string();
                if let Some(new_script) = self.resolve_script(&name, ctx) {
                    let t = self.get_thread_mut(tid);
                    let frame = CallFrame {
                        script: t.script.clone(),
                        variables: t.variables.clone(),
                        seq_pc: t.seq_pc,
                        loop_stack: t.loop_stack.clone(),
                    };
                    t.call_stack.push(frame);
                    t.script = new_script.clone();
                    init_variables(&mut t.variables, &t.script.variables);
                    t.seq_pc = 0;
                    t.loop_stack.clear();
                    t.state = ExecState::AbortSequence; // Yield to prevent executing rest of old block
                } else {
                    warn!(
                        "[ScrOni][{}] Fork: Target Script '{}' not found",
                        self.get_thread(tid).script.name,
                        name
                    );
                }
            }

            Stmt::Switch(name_expr) => {
                let name = self.eval_expr(tid, name_expr, now, ctx).as_string();
                if let Some(new_script) = self.resolve_script(&name, ctx) {
                    let t = self.get_thread_mut(tid);
                    t.script = new_script;
                    init_variables(&mut t.variables, &t.script.variables);
                    t.seq_pc = 0;
                    t.loop_stack.clear();

                    // Clear the message queue when the main script switches,
                    // matching the original engine's scrGroupContext::Reset behavior.
                    if tid == 0 {
                        self.message_queue.clear();
                    }

                    self.get_thread_mut(tid).state = ExecState::AbortSequence; // Yield to prevent executing rest of old block
                } else {
                    warn!(
                        "[ScrOni][{}] Switch: Target Script '{}' not found",
                        self.get_thread(tid).script.name,
                        name
                    );
                }
            }

            Stmt::ChildStack { var, script } => {
                let script_name = self.eval_expr(tid, script, now, ctx).as_string();
                if let Some(new_script) = self.resolve_script(&script_name, ctx) {
                    let new_tid = self.next_thread_id;
                    self.next_thread_id += 1;
                    let mut new_thread = ScrOniThread::new(new_tid, Some(tid), new_script, now);
                    for var_decl in &new_thread.script.variables {
                        if var_decl.is_parent {
                            continue;
                        }
                        new_thread.variables.insert(
                            var_decl.name.clone(),
                            Value::default_for_type(&var_decl.var_type),
                        );
                    }
                    self.child_threads.push(new_thread);
                    self.set_var(tid, var.clone(), Value::Int(new_tid as i32));
                } else {
                    warn!(
                        "[ScrOni][{}] ChildStack: Target Script '{}' not found",
                        self.get_thread(tid).script.name,
                        script_name
                    );
                }
            }

            Stmt::ChildSwitch { var, script } => {
                let script_name = self.eval_expr(tid, script, now, ctx).as_string();
                if let Some(new_script) = self.resolve_script(&script_name, ctx) {
                    let new_tid = self.next_thread_id;
                    self.next_thread_id += 1;
                    let mut new_thread = ScrOniThread::new(new_tid, Some(tid), new_script, now);
                    self.child_threads.push(new_thread);
                    self.set_var(tid, var.clone(), Value::Int(new_tid as i32));
                } else {
                    warn!(
                        "[ScrOni][{}] ChildSwitch: Target Script '{}' not found",
                        self.get_thread(tid).script.name,
                        script_name
                    );
                }
            }

            Stmt::UsePad => {
                if ctx.player != Some(self.owner) {
                    self.sys_requests.push(SysRequest::UsePad(self.owner));
                }
                self.get_thread_mut(tid).state = ExecState::Yielded;
            }

            Stmt::CameraSetPackage(expr) => {
                let pkg_name = self.eval_expr(tid, expr, now, ctx).as_string();
                self.sys_requests
                    .push(SysRequest::CameraSetPackage(pkg_name));
            }
            Stmt::At(x_expr, y_expr) => {
                let x = self.eval_expr(tid, x_expr, now, ctx).as_float();
                let y = self.eval_expr(tid, y_expr, now, ctx).as_float();
                self.sys_requests.push(SysRequest::At(x, y));
            }
            Stmt::DrawText(text_expr) => {
                let text = self.eval_expr(tid, text_expr, now, ctx).as_string();
                self.sys_requests.push(SysRequest::DrawText(text));
            }
            Stmt::Sound { args } => {
                if args.len() >= 3 {
                    // sound [actor] play [name]
                    let actor_val = self.eval_expr(tid, &args[0], now, ctx);
                    let action = self.eval_expr(tid, &args[1], now, ctx).as_string();
                    let name = self.eval_expr(tid, &args[2], now, ctx).as_string();
                    if action.eq_ignore_ascii_case("play") {
                        let actor_str = if let Value::Int(0) = actor_val {
                            None
                        } else {
                            Some(actor_val.as_string())
                        };
                        self.sys_requests.push(SysRequest::PlaySound(actor_str, name));
                    } else {
                        info!("VM: Sound unsupported action {}", action);
                    }
                } else if args.len() >= 2 {
                     // sound play [name]
                     let action = self.eval_expr(tid, &args[0], now, ctx).as_string();
                     let name = self.eval_expr(tid, &args[1], now, ctx).as_string();
                     if action.eq_ignore_ascii_case("play") {
                         self.sys_requests.push(SysRequest::PlaySound(None, name));
                     }
                } else {
                    info!("VM: Sound {:?} (invalid args)", args);
                }
            }
            Stmt::AmbientSound { args } => {
                if args.len() == 2 {
                    let handle = self.eval_expr(tid, &args[0], now, ctx).as_int();
                    let action = self.eval_expr(tid, &args[1], now, ctx).as_string();
                    if action.eq_ignore_ascii_case("stop") {
                        self.sys_requests.push(SysRequest::AmbientSoundStop(handle));
                        info!("VM: AmbientSound Stop {}", handle);
                    } else {
                        info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                    }
                } else if args.len() == 4 {
                    let handle = self.eval_expr(tid, &args[0], now, ctx).as_int();
                    let action = self.eval_expr(tid, &args[1], now, ctx).as_string();
                    if action.eq_ignore_ascii_case("volumeramp") {
                        let target_vol = self.eval_expr(tid, &args[2], now, ctx).as_float();
                        let duration = self.eval_expr(tid, &args[3], now, ctx).as_float();
                        self.sys_requests.push(SysRequest::AmbientSoundVolumeRamp(handle, target_vol, duration));
                        info!("VM: AmbientSound VolumeRamp {} -> {} in {}", handle, target_vol, duration);
                    } else if action.eq_ignore_ascii_case("pitchramp") {
                        let target_pitch = self.eval_expr(tid, &args[2], now, ctx).as_float();
                        let duration = self.eval_expr(tid, &args[3], now, ctx).as_float();
                        self.sys_requests.push(SysRequest::AmbientSoundPitchRamp(handle, target_pitch, duration));
                        info!("VM: AmbientSound PitchRamp {} -> {} in {}", handle, target_pitch, duration);
                    } else {
                        info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                    }
                } else if args.len() == 3 {
                    let handle = self.eval_expr(tid, &args[0], now, ctx).as_int();
                    let action = self.eval_expr(tid, &args[1], now, ctx).as_string();
                    if action.eq_ignore_ascii_case("pitch") {
                        let target_pitch = self.eval_expr(tid, &args[2], now, ctx).as_float();
                        self.sys_requests.push(SysRequest::AmbientSoundPitchRamp(handle, target_pitch, 0.0));
                        info!("VM: AmbientSound Pitch {} -> {}", handle, target_pitch);
                    } else if action.eq_ignore_ascii_case("volume") {
                        let target_vol = self.eval_expr(tid, &args[2], now, ctx).as_float();
                        self.sys_requests.push(SysRequest::AmbientSoundVolumeRamp(handle, target_vol, 0.0));
                        info!("VM: AmbientSound Volume {} -> {}", handle, target_vol);
                    } else {
                        info!("VM: AmbientSound {:?} (unsupported action: {})", args, action);
                    }
                } else {
                    info!("VM: AmbientSound {:?} (unimplemented format)", args);
                }
            }
            Stmt::PlayAmbientSound { name, volume, pitch, volume_ramp, pitch_ramp } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let v = volume.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let p = pitch.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let vr = volume_ramp.as_ref().map(|(s, e, d)| {
                    (
                        self.eval_expr(tid, s, now, ctx).as_float(),
                        self.eval_expr(tid, e, now, ctx).as_float(),
                        self.eval_expr(tid, d, now, ctx).as_float(),
                    )
                });
                let pr = pitch_ramp.as_ref().map(|(s, e, d)| {
                    (
                        self.eval_expr(tid, s, now, ctx).as_float(),
                        self.eval_expr(tid, e, now, ctx).as_float(),
                        self.eval_expr(tid, d, now, ctx).as_float(),
                    )
                });
                info!("VM: PlayAmbientSound {} v:{:?} p:{:?} vr:{:?} pr:{:?}", n, v, p, vr, pr);
                self.sys_requests.push(SysRequest::PlayAmbientSound(0, n, v, p, vr, pr));
            }
            Stmt::MusicPlay(expr) => {
                let m = self.eval_expr(tid, expr, now, ctx).as_string();
                info!("VM: MusicPlay {} (unimplemented)", m);
            }
            Stmt::MusicStop => {
                info!("VM: MusicStop (unimplemented)");
            }
            Stmt::CameraReset => {
                info!("VM: CameraReset (unimplemented)");
            }
            Stmt::CameraMode(expr) => {
                let mode = self.eval_expr(tid, expr, now, ctx).as_string();
                info!("VM: CameraMode {} (unimplemented)", mode);
            }
            Stmt::CameraLetterbox(expr) => {
                let b = self.eval_expr(tid, expr, now, ctx).as_int();
                info!("VM: CameraLetterbox {} (unimplemented)", b);
            }
            Stmt::CameraFollowActor { args } => {
                info!("VM: CameraFollowActor {:?} (unimplemented)", args);
            }
            Stmt::CameraTrackActor { args } => {
                info!("VM: CameraTrackActor {:?} (unimplemented)", args);
            }
            Stmt::CameraTrackPoint { args } => {
                info!("VM: CameraTrackPoint {:?} (unimplemented)", args);
            }
            Stmt::CameraMoveToActor { args } => {
                info!("VM: CameraMoveToActor {:?} (unimplemented)", args);
            }
            Stmt::CameraMoveToPoint { args } => {
                info!("VM: CameraMoveToPoint {:?} (unimplemented)", args);
            }
            Stmt::CameraCutToActor { args } => {
                info!("VM: CameraCutToActor {:?} (unimplemented)", args);
            }
            Stmt::CameraCutToPoint { args } => {
                info!("VM: CameraCutToPoint {:?} (unimplemented)", args);
            }
            Stmt::CameraSetFOV { args } => {
                info!("VM: CameraSetFOV {:?} (unimplemented)", args);
            }
            Stmt::CameraShake => {
                info!("VM: CameraShake (unimplemented)");
            }

            Stmt::SetFogType(expr) => {
                let fog_type = self.eval_expr(tid, expr, now, ctx).as_string();
                info!("VM: SetFogType {} (unimplemented)", fog_type);
            }
            Stmt::SetFogRange { min, max } => {
                info!("VM: SetFogRange {:?} {:?} (unimplemented)", min, max);
            }
            Stmt::SetFogColor { args } => {
                info!("VM: SetFogColor {:?} (unimplemented)", args);
            }
            Stmt::SetFogClamp { args } => {
                info!("VM: SetFogClamp {:?} (unimplemented)", args);
            }
            Stmt::SetFogPalettePower { args } => {
                info!("VM: SetFogPalettePower {:?} (unimplemented)", args);
            }
            Stmt::SetShaderLocal { args } => {
                let name = self.eval_expr(tid, &args[0], now, ctx).as_string();
                let val = self.eval_expr(tid, &args[1], now, ctx).as_float();
                self.sys_requests
                    .push(SysRequest::SetShaderLocal { name, val });
            }
            Stmt::SetLightParameter { args } => {
                let light = self.eval_expr(tid, &args[0], now, ctx).as_string();
                self.current_light = Some(light);
            }
            Stmt::Intensity { args } => {
                let val = self.eval_expr(tid, &args[0], now, ctx).as_float();
                if let Some(light) = &self.current_light {
                    self.sys_requests.push(SysRequest::SetLightIntensity {
                        light: light.clone(),
                        intensity: val,
                    });
                }
            }
            Stmt::SetFullScreenColor { args } => {
                info!("VM: SetFullScreenColor {:?} (unimplemented)", args);
            }
            Stmt::SetUpdateState { target, state } => {
                let target_val = self.eval_expr(tid, target, now, ctx).as_string();
                let state_val = self.eval_expr(tid, state, now, ctx).as_string();
                self.sys_requests.push(SysRequest::SetUpdateState {
                    target: target_val,
                    state: state_val,
                });
            }

            // TODO: store a map of warned about commands and warn about each once!
            // Stubs for commands we don't execute yet
            Stmt::ControlHead { args } => {
                let caller_actor = self.owner;
                let Some(first) = args.first() else { return; };
                let keyword = self.eval_expr(tid, first, now, ctx).as_string().to_lowercase();
                
                use crate::oni2_loader::components::ControlHeadTask;
                let task = match keyword.as_str() {
                    "disable" => Some(ControlHeadTask::Disable),
                    "trackclosest" => Some(ControlHeadTask::TrackClosest),
                    "trackactor" => {
                        if args.len() > 1 {
                             if let Value::Actor(ent) = self.eval_expr(tid, &args[1], now, ctx) {
                                 Some(ControlHeadTask::TrackActor(ent))
                             } else { None }
                        } else { None }
                    },
                    "trackpos" => {
                        if args.len() > 1 {
                             if let Value::Vector(v) = self.eval_expr(tid, &args[1], now, ctx) {
                                Some(ControlHeadTask::TrackPos(v))
                             } else { None }
                        } else { None }
                    },
                    "set" => {
                         if args.len() > 1 {
                             let val = self.eval_expr(tid, &args[1], now, ctx);
                             Some(ControlHeadTask::Set { azimuth: val.as_float(), incline: 0.0 })
                         } else { None }
                    },
                    "scan" => {
                        let mut range = 80.0;
                        let mut period = 2.0;
                        let mut i = 1;
                        while i < args.len() {
                            let arg_str = self.eval_expr(tid, &args[i], now, ctx).as_string().to_lowercase();
                            if arg_str == "range" && i + 1 < args.len() {
                                range = self.eval_expr(tid, &args[i+1], now, ctx).as_float();
                                i += 2;
                            } else if arg_str == "in" && i + 1 < args.len() {
                                period = self.eval_expr(tid, &args[i+1], now, ctx).as_float();
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        Some(ControlHeadTask::Scan { range, period })
                    },
                    _ => None,
                };
                
                if let Some(task) = task {
                    self.sys_requests.push(SysRequest::ControlHead {
                        actor: caller_actor,
                        task,
                    });
                } else {
                    debug!("controlhead could not parse correctly or invalid args: {:?}", args);
                }
            }

            Stmt::Hit { hit_type, victim, damage } => {
                let hit_t = match &hit_type {
                    Expr::Var(name) => name.clone(),
                    _ => self.eval_expr(tid, hit_type, now, ctx).as_string(),
                };
                let target = self.eval_expr(tid, victim, now, ctx);
                let dmg = self.eval_expr(tid, damage, now, ctx).as_float();
                if let Value::Actor(ent) = target {
                    self.sys_requests.push(SysRequest::Hit {
                        target: ent,
                        hit_type: hit_t,
                        damage: dmg,
                    });
                }
            }

            Stmt::Unimplemented { command, args } => {
                let lower = command.to_lowercase();
                if lower == "retreat" {
                    // Retreat command stub
                    let target_str = if !args.is_empty() {
                        self.eval_expr(tid, &args[0], now, ctx).as_string()
                    } else {
                        "me".to_string()
                    };
                    if self
                        .warned_unimplemented
                        .insert(format!("retreat_{}", target_str))
                    {
                        info!("VM: Retreat {} (unimplemented)", target_str);
                    }
                } else {
                    if self.warned_unimplemented.insert(lower.clone()) {
                        info!("Unimplemented command: {}", command);
                    }
                }
            }
            // AI Commands that inherently block execution natively waiting for physical locomotion responses
            Stmt::Retreat | Stmt::Patrol(_) | Stmt::Follow(_) | Stmt::Attack(_) => {
                if self.warned_unimplemented.insert(format!("{:?}", stmt)) {
                    info!(
                        "VM: Unimplemented AI action {:?} (Yielding continuously)",
                        stmt
                    );
                }
                self.get_thread_mut(tid).state = ExecState::Yielded;
            }

            _ => {
                // Non-silently ignore unimplemented commands for now
                if self.warned_unimplemented.insert(format!("{:?}", stmt)) {
                    info!("Unimplemented command: {:?}", stmt);
                }
            }
        }
    }

    fn flatten_to_block(&self, stmt: &Stmt) -> Vec<Stmt> {
        match stmt {
            Stmt::Block(stmts) => stmts.clone(),
            other => vec![other.clone()],
        }
    }

    // ---- Expression evaluation ----

    fn eval_expr(&mut self, tid: u32, expr: &Expr, now: f64, ctx: &mut ScroniContext) -> Value {
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
            Expr::Var(name) => self.get_var(tid, name),
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
                    "makestring" => {
                        let mut s = String::new();
                        for arg in args {
                            s.push_str(&self.eval_expr(tid, arg, now, ctx).as_string());
                        }
                        Value::String(s)
                    }
                    "random" => Value::Int(rand::random::<i32>().abs() % 100),
                    "randomrange" => Value::Int(0),          // stub
                    "randomrangefloat" => Value::Float(0.0), // stub
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
                            if !ents.is_empty() {
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "location" => {
                        let target = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Actor(act)) = target {
                            if let Ok((_, tf, _)) = ctx.all_entities.get(act) {
                                let p = tf.translation();
                                return Value::Vector(Vec3::new(-p.x, p.y, -p.z));
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
                                        Some(Vec3::new(-p.x, p.y, -p.z))
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
                                p1 = Some(Vec3::new(-my_p.x, my_p.y, -my_p.z));
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
                            if let Some(Value::ActorList(entities, _)) =
                                self.get_thread(tid).variables.get(list_name)
                            {
                                let updated = entities.clone();
                                if let Some(&first_ent) = updated.first() {
                                    self.get_thread_mut(tid)
                                        .variables
                                        .insert(list_name.clone(), Value::ActorList(updated, 1));
                                    return Value::Actor(first_ent);
                                } else {
                                    self.get_thread_mut(tid)
                                        .variables
                                        .insert(list_name.clone(), Value::ActorList(updated, 0));
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
                        info!("VM: Expr PlayAmbientSound {} v:{:?} p:{:?} vr:{:?} pr:{:?} (Handle: {})", n, v, p, vr, pr, handle);
                        self.sys_requests.push(SysRequest::PlayAmbientSound(handle, n, v, p, vr, pr));
                        Value::Int(handle)
                    }
                    "size" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, _)) =
                                self.get_thread(tid).variables.get(list_name)
                            {
                                return Value::Int(entities.len() as i32);
                            }
                        }
                        Value::Int(0)
                    }
                    "next" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, idx)) =
                                self.get_thread(tid).variables.get(list_name)
                            {
                                let updated = entities.clone();
                                let current_idx = *idx;
                                if current_idx < updated.len() {
                                    let ent = updated[current_idx];
                                    self.get_thread_mut(tid).variables.insert(
                                        list_name.clone(),
                                        Value::ActorList(updated, current_idx + 1),
                                    );
                                    return Value::Actor(ent);
                                } else {
                                    self.get_thread_mut(tid).variables.insert(
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
                    _ => Value::None,
                }
            }
            Expr::Exists(_) => Value::Int(1), // stub: assume exists
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
        BinOp::Dot | BinOp::Cross => Value::Float(0.0), // stub
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
    mut query: Query<(Entity, &mut ScrOniScript, &GlobalTransform)>,
    all_entities: Query<(Entity, &'static GlobalTransform, Option<&'static Name>)>,
    triggers: Query<&'static BroadcastTrigger>,
    time: Res<Time>,
    player_query: Query<Entity, With<crate::player::components::Player>>,
    current_checkpoint: Res<crate::oni2_loader::components::CurrentCheckpointIndex>,
    layout_context: Option<Res<crate::oni2_loader::environment::LayoutContext>>,
    mut health_query: Query<&mut crate::combat::components::Health>,
) {
    let now = time.elapsed_secs_f64();
    let delta_time = time.delta_secs();
    let mut all_messages = Vec::new();
    let player_ent = player_query.iter().next();

    for (entity, mut script, transform) in &mut query {
        if script.exec.ticks_alive == 0 {
            script.exec.ticks_alive += 1;
            continue;
        }
        script.exec.ticks_alive += 1;

        let mut ctx = ScroniContext {
            all_entities: &all_entities,
            triggers: &triggers,
            player: player_ent,
            current_checkpoint: current_checkpoint.0,
            layout_dir: layout_context
                .as_ref()
                .map(|c| c.layout_dir.clone())
                .unwrap_or_default(),
        };
        script.exec.tick(now, delta_time, &mut ctx);

        if script.exec.main_thread.state == ExecState::Done {
            if script.exec.owner == Entity::PLACEHOLDER {
                commands.entity(entity).despawn();
            } else {
                commands.entity(entity).remove::<ScrOniScript>();
            }
            continue;
        }

        let mut finds_to_resolve = Vec::new();
        for t in script.exec.all_threads_mut() {
            if let Some(BlockingAction::Find {
                list_var,
                conditions,
                range,
            }) = t.blocking.clone()
            {
                finds_to_resolve.push((t.thread_id, list_var, conditions, range));
            }
        }

        // Handle Find request
        for (tid, list_var, conditions, range) in finds_to_resolve {
            let mut found = Vec::new();
            let mut my_pos = transform.translation(); // Fallback default to script entity center
            let max_dist = range.unwrap_or(9999.0);

            for (k, v) in &conditions {
                if k.to_lowercase() == "at" {
                    if let Value::Vector(vec) = v {
                        // The vector evaluated by the AST (e.g., location(me)) was generated in internal Oni coordinate space
                        // We must convert it back to Bevy layout coordinates (-X, Y, -Z) for physics evaluation mathematically
                        my_pos = Vec3::new(-vec[0] as f32, vec[1] as f32, -vec[2] as f32);
                    }
                }
            }

            for (other_ent, other_tf, name_opt) in &all_entities {
                if entity == other_ent {
                    continue;
                }

                let dist = my_pos.distance(other_tf.translation());
                if dist <= max_dist {
                    let mut matches_all = true;
                    for (k, v) in &conditions {
                        let k_lower = k.to_lowercase();
                        if k_lower == "name" || k_lower == "group" {
                            let expected_name = v.as_string();
                            let actual_name = name_opt.map(|n| n.as_str()).unwrap_or("");
                            if actual_name != expected_name {
                                matches_all = false;
                                break;
                            }
                        }
                    }
                    if matches_all {
                        found.push(other_ent);
                    }
                }
            }

            script
                .exec
                .set_var(tid, list_var, Value::ActorList(found, 0));
            script.exec.clear_blocking(tid);
            // Tick again to resume immediately
            script.exec.tick_thread(tid, now, &mut ctx);
        }
        let script_name = script.exec.main_thread.script.name.clone();
        for req in script.exec.sys_requests.drain(..) {
            match req {
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
                SysRequest::CameraSetPackage(pkg_name) => {
                    commands.trigger(ScrOniSysEvent::CameraSetPackage(pkg_name));
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
                    if let Ok(mut health) = health_query.get_mut(target) {
                        let final_damage = if hit_type.eq_ignore_ascii_case("environmentalhazard") {
                            damage * time.delta_secs()
                        } else {
                            damage
                        };
                        health.current = (health.current - final_damage).max(0.0);
                    }
                }
                SysRequest::AmbientSoundStop(handle) => {
                    commands.trigger(ScrOniSysEvent::AmbientSoundStop {
                        script_entity: entity,
                        handle,
                    });
                }
            }
        }

        all_messages.append(&mut script.exec.outgoing_messages);
    }

    // Deliver messages
    for msg in all_messages {
        if let Ok((_, mut target_script, _)) = query.get_mut(msg.to) {
            target_script.exec.message_queue.push(msg);
        } else {
            warn!(
                "VM: Failed to deliver message '{}' to {:?}: target not found or has no ScrOniScript",
                msg.msg, msg.to
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
            commands.entity(entity).despawn();
        }
    }
}

/// Even when tempted to use a Trigger here, keep this On-based.
/// Observer to handle ScrOni system requests (like TextureMovie)
pub fn scroni_sys_event_observer(
    trigger: On<ScrOniSysEvent>,
    mut commands: Commands,
    assets: (
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
        Res<AssetServer>,
        ResMut<Assets<bevy::audio::AudioSource>>,
        ResMut<crate::oni2_loader::TextureCollections>,
        ResMut<crate::oni2_loader::registries::EntityLibrary>,
        ResMut<crate::oni2_loader::registries::AnimRegistry>,
        Local<Option<crate::oni2_loader::parsers::td::SoundBankDirectory>>,
        Local<Option<crate::oni2_loader::parsers::audiopackages::AudioPackagesDirectory>>,
    ),
    layout_data: (
        Option<Res<crate::oni2_loader::LayoutContext>>,
        Option<Res<crate::oni2_loader::LayoutPaths>>,
    ),
    active_camera_package: Option<ResMut<crate::oni2_loader::ActiveCameraPackage>>,
    mut scroni_text_state: ResMut<ScroniTextState>,
    time: Res<Time>,
    misc_queries: (
        Query<(
            &mut Transform,
            Option<&mut avian3d::prelude::LinearVelocity>,
        )>,
        Query<&Children>,
        Query<&mut MeshMaterial3d<StandardMaterial>>,
        Query<&mut crate::camera::components::CameraRig>,
        Query<(&Name, Option<&mut PointLight>, Option<&mut SpotLight>)>,
        Query<(Entity, &mut ScrOniScript, Option<&Name>)>,
        Query<(), With<crate::player::components::Player>>,
        Query<(Entity, &ActiveAmbientSound)>,
    ),
) {
    let ev = (*trigger).clone();
    let (mut transform_query, children_query, mut materials_query, mut camera_query, mut lights_query, mut script_query, player_query, ambient_sound_query) = misc_queries;
    let (
        mut materials,
        mut meshes,
        mut images,
        mut skinned_mesh_ibp,
        asset_server,
        mut audio_sources,
        mut texture_collections,
        mut entity_lib,
        mut anim_registry,
        mut td_directory,
        mut audio_packages,
    ) = assets;
    
    if td_directory.is_none() {
        let assets_path = crate::get_assets_path();
        let path = std::path::Path::new(assets_path).join("Audio").join("banks");
        *td_directory = Some(crate::oni2_loader::parsers::td::load_all_tds(&path));
    }

    if audio_packages.is_none() {
        let assets_path = crate::get_assets_path();
        let pkgs_path = std::path::Path::new(assets_path).join("Audio").join("rb.audiopackages");
        if let Ok(content) = std::fs::read_to_string(&pkgs_path) {
            *audio_packages = Some(crate::oni2_loader::parsers::audiopackages::parse_audiopackages(&content));
        } else {
            *audio_packages = Some(std::collections::HashMap::new());
        }
    }

    match ev {
        ScrOniSysEvent::PlaySound { script_entity, actor, name } => {
            let mut resolved_name = name.clone();
            let mut final_volume = 1.0;
            let mut final_pitch = 1.0;

            if let Some(pkgs) = audio_packages.as_ref() {
                if let Some(pkg) = pkgs.get(&name) {
                    if !pkg.nuggets.is_empty() {
                        use rand::Rng;
                        let mut rng = rand::thread_rng();
                        let idx = rng.gen_range(0..pkg.nuggets.len());
                        let nugget = &pkg.nuggets[idx];
                        
                        resolved_name = nugget.sound.clone();
                        final_volume = nugget.volume * rng.gen_range(nugget.random_min_volume..=nugget.random_max_volume);
                        final_pitch = nugget.pitch * rng.gen_range(nugget.random_min_pitch..=nugget.random_max_pitch);
                    }
                }
            }

            if let Some(dir) = td_directory.as_ref() {
                if let Some((bank_name, vag_index)) = dir.sounds.get(&resolved_name) {
                    let hd_name = format!("{}.hd", bank_name);
                    let bd_name = format!("{}.bd", bank_name);
                    
                    let hd_paths = [
                        hd_name.clone(),
                    ];
                    
                    let mut hd_bytes_opt = None;
                    for p in &hd_paths {
                        if let Ok(b) = crate::vfs::read("", p) {
                            hd_bytes_opt = Some(b);
                            break;
                        }
                    }
                    
                    if let Some(hd_bytes) = hd_bytes_opt {
                        if let Ok(header) = crate::oni2_loader::parsers::hd_bd::parse_hd(&hd_bytes) {
                            // Find the target subsong (1-indexed but vag_index is 0-indexed)
                            // Wait, the user said NUMVAGS 13, and the split has vag_index 12.
                            // The HD subsongs are usually accessed 1..=total_subsongs.
                            // So we need vag_index + 1
                            let target_index = vag_index + 1;
                            if let Some(subsong) = header.subsongs.iter().find(|s| s.index == target_index) {
                                let bd_paths = [
                                    bd_name.clone(),
                                    format!("Audio/banks/{}", bd_name),
                                ];
                                
                                let mut bd_bytes_opt = None;
                                for p in &bd_paths {
                                    if let Ok(b) = crate::vfs::read("", p) {
                                        bd_bytes_opt = Some(b);
                                        break;
                                    }
                                }
                                
                                if let Some(bd_bytes) = bd_bytes_opt {
                                    let start = subsong.stream_offset as usize;
                                    let end = start + subsong.stream_size as usize;
                                    if end <= bd_bytes.len() {
                                        let payload = &bd_bytes[start..end];
                                        if let Ok(pcm) = crate::oni2_loader::parsers::hd_bd::decode_psx_adpcm(payload, subsong.num_samples) {
                                            if let Ok(wav) = crate::oni2_loader::parsers::hd_bd::create_wav_bytes(&pcm, subsong.sample_rate, subsong.channels) {
                                                let source_handle = audio_sources.add(bevy::audio::AudioSource {
                                                    bytes: std::sync::Arc::from(wav),
                                                });
                                                
                                                // 2D Ambient Playback Requested
                                                commands.spawn((
                                                    bevy::audio::AudioPlayer(source_handle),
                                                    bevy::audio::PlaybackSettings {
                                                        mode: if subsong.loop_flag { bevy::audio::PlaybackMode::Loop } else { bevy::audio::PlaybackMode::Despawn },
                                                        volume: bevy::audio::Volume::Linear(final_volume),
                                                        speed: final_pitch,
                                                        ..Default::default()
                                                    },
                                                ));
                                                info!("Playing sound `{}` (bank: {}, subsong: {}, loop: {}) vol: {} pitch: {}", resolved_name, bank_name, target_index, subsong.loop_flag, final_volume, final_pitch);
                                            }
                                        }
                                    } else {
                                        warn!("Subsong payload overflows BD file.");
                                    }
                                } else {
                                    warn!("BD file not found in VFS: {}", bd_name);
                                }
                            } else {
                                warn!("Subsong {} not found in HD header.", target_index);
                            }
                        } else {
                            warn!("Failed to parse HD header for {}", hd_name);
                        }
                    } else {
                        warn!("HD file not found in VFS: {}", hd_name);
                    }
                } else {
                    warn!("Sound `{}` not found in .td manifest directory.", name);
                }
            }
        }
        ScrOniSysEvent::At(x, y) => {
            scroni_text_state.current_x = x;
            scroni_text_state.current_y = y;
        }
        ScrOniSysEvent::DrawText(text) => {
            // Coordinate system is top-left based, so (0.5, 0.5) is center.
            let px = scroni_text_state.current_x * 100.0;
            let py = scroni_text_state.current_y * 100.0;

            commands.spawn((
                Text::new(text),
                TextFont {
                    font_size: 24.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(px),
                    top: Val::Percent(py),
                    ..default()
                },
                crate::menu::InGameEntity,
                ScroniTextElement {
                    // Ephemeral: lasts slightly longer than 1 frame at 60fps (16ms)
                    expires_at: time.elapsed_secs_f64() + 0.05,
                },
            ));
        }
        ScrOniSysEvent::CameraSetPackage(pkg_name) => {
            if let Some(mut active_pkg) = active_camera_package {
                if active_pkg.name != pkg_name {
                    info!(
                        "Changing active camera package from {} to {}",
                        active_pkg.name, pkg_name
                    );
                    active_pkg.name = pkg_name;
                }
            } else {
                warn!("CameraSetPackage called but no ActiveCameraPackage resource found.");
            }
            // Transition camera script to SmartFollow mode
            for mut rig in &mut camera_query {
                rig.mode = crate::camera::components::CameraMode::SmartFollow;
            }
        }
        ScrOniSysEvent::TextureMovie {
            script_entity,
            target_name,
            action,
            arg,
        } => {
            match action {
                super::ast::TextureMovieAction::SetFrame => {
                    let frame = arg.as_int() as usize;

                    // Get preloaded texture handle directly from the collections resource
                    if let Some(frames) = texture_collections.collections.get(&target_name) {
                        if frame < frames.len() {
                            let tex_handle = frames[frame].clone();
                            let mut stack = vec![script_entity];
                            while let Some(ent) = stack.pop() {
                                if let Ok(mut mat_handle) = materials_query.get_mut(ent) {
                                    if let Some(old_mat) = materials.get(&mat_handle.0) {
                                        let mut new_mat = old_mat.clone();
                                        new_mat.base_color_texture = Some(tex_handle.clone());
                                        new_mat.base_color = Color::WHITE;
                                        let new_handle = materials.add(new_mat);
                                        mat_handle.0 = new_handle;
                                    }
                                }
                                if let Ok(children) = children_query.get(ent) {
                                    stack.extend(children.iter());
                                }
                            }
                        } else {
                            warn!(
                                "TextureMovie SetFrame {} out of bounds for {}",
                                frame, target_name
                            );
                        }
                    } else {
                        warn!(
                            "TextureMovie: No preloaded textures found for {}",
                            target_name
                        );
                    }
                }
                _ => {}
            }
        }
        ScrOniSysEvent::Spawn {
            script_entity: _,
            script,
            assign_to,
            at,
            name,
        } => {
            info!(
                "Received spawn request: script={}, at={:?}, name={:?}",
                script, at, name
            );

            // Scripts operate purely in ONI coordinates recursively natively. Translate down into Bevy spatial coordinates physically here.
            let pos_opt = at.map(|p| Vec3::new(-p.x, p.y, -p.z));
            let actor_name = name.clone().unwrap_or(script.clone());

            if let (Some(ctx), Some(paths)) = (layout_data.0.as_ref(), layout_data.1.as_ref()) {
                let mut spawn_assets = crate::oni2_loader::SpawnAssets {
                    commands: &mut commands,
                    meshes: &mut meshes,
                    materials: &mut materials,
                    images: &mut images,
                    skinned_mesh_ibp: &mut skinned_mesh_ibp,
                    entity_lib: &mut entity_lib,
                    anim_registry: &mut anim_registry,
                    texture_collections: &mut texture_collections,
                };

                // Call the shared spawn function
                if let Some((_new_entity, _actor)) = crate::oni2_loader::spawn_layout_actor(
                    &mut spawn_assets,
                    &script,
                    ctx,
                    paths,
                    pos_opt,
                    true,
                    Some(&actor_name),
                ) {
                    info!(
                        "Spawned {} with position override {:?}",
                        actor_name, pos_opt
                    );
                    if let Some(var_name) = assign_to {
                        warn!(
                            "Assigning spawn result to {} is not yet supported synchronously.",
                            var_name
                        );
                    }
                    return; // Successfully spawned!
                } else {
                    warn!(
                        "Failed to spawn actor {} using spawn_layout_actor",
                        actor_name
                    );
                }
            } else {
                warn!(
                    "Spawn command needs a LayoutContext and LayoutPaths resource to fully spawn {}.",
                    script
                );
            }

            // Fallback Stub: spawn a basic entity placeholder instead if proper spawning fails
            let pos_fallback = pos_opt.unwrap_or(Vec3::ZERO);
            let _new_entity = commands
                .spawn((
                    Transform::from_translation(pos_fallback),
                    Visibility::Visible,
                    crate::oni2_loader::Oni2Entity {
                        name: actor_name.clone(),
                    },
                    Name::new(actor_name.clone()),
                    crate::menu::InGameEntity,
                ))
                .id();

            if let Some(var_name) = assign_to {
                warn!(
                    "Assigning spawn result to {} is not yet supported synchronously.",
                    var_name
                );
            }
        }
        ScrOniSysEvent::Teleport {
            script_entity,
            target,
            to,
            face,
        } => {
            if player_query.get(target).is_ok() {
                let caller_name = if let Ok((_, script, _)) = script_query.get(script_entity) {
                    script.exec.main_thread.script.name.clone()
                } else {
                    "UnknownScript".to_string()
                };
            }
            if let Ok((mut transform, mut opt_vel)) = transform_query.get_mut(target) {
                if let Some(pos) = to {
                    let bevy_pos = Vec3::new(-pos.x, pos.y, -pos.z);
                    transform.translation = bevy_pos;
                    commands
                        .entity(target)
                        .insert(crate::oni2_loader::spawn::NeedsGroundSnap {
                            origin: bevy_pos,
                            wait_frames: 4,
                        });
                }
                if let Some(angles_y) = face {
                    let rad = angles_y.to_radians();
                    let current_rot = transform.rotation.to_euler(EulerRot::YXZ);
                    transform.rotation =
                        Quat::from_euler(EulerRot::YXZ, rad, current_rot.1, current_rot.2);
                }

                if let Some(vel) = opt_vel.as_deref_mut() {
                    vel.0 = Vec3::ZERO;
                }
            }
        }
        ScrOniSysEvent::MakeFx {
            script_entity,
            name,
            at,
        } => {
            commands.trigger(crate::fx_system::SpawnFx {
                name: name,
                at: at,
                parent: Some(script_entity),
                start_active: true,
            });
        }
        ScrOniSysEvent::SendAction {
            action,
            target,
            component,
        } => {
            if component.eq_ignore_ascii_case("fx") {
                commands.trigger(crate::fx_system::FxAction {
                    action: action.clone(),
                    target: target,
                });
            } else {
                warn!("SendAction: Unrecognized component '{}'", component);
            }
        }
        ScrOniSysEvent::ControlHead { actor, task } => {
            use crate::oni2_loader::components::ActiveHeadIK;
            if let Ok(mut entity_cmds) = commands.get_entity(actor.clone()) {
                entity_cmds.insert(ActiveHeadIK { task: task.clone() });
                debug!("VM: Observed ControlHead {:?} onto actor {:?}", task, actor);
            }
        }
        ScrOniSysEvent::SetLightIntensity {
            script_entity: _,
            light,
            intensity,
        } => {
            for (name, mut point, mut spot) in &mut lights_query {
                if name.as_str().eq_ignore_ascii_case(&light) {
                    // Multiply scaling heuristic to adapt Oni floats to PBR luminous intensity
                    if let Some(p) = point.as_deref_mut() {
                        p.intensity = intensity * 100.0;
                    }
                    if let Some(s) = spot.as_deref_mut() {
                        s.intensity = intensity * 100.0;
                    }
                }
            }
        }
        ScrOniSysEvent::SetShaderLocal {
            script_entity: _,
            name,
            val,
        } => {
            debug!(
                "VM: Observed SetShaderLocal {} = {} (Unimplemented Material Target)",
                name, val
            );
        }
        ScrOniSysEvent::SetUpdateState { target, state } => {
            let active = state.eq_ignore_ascii_case("Active");
            let target_hash = target.parse::<i32>().ok();

            for (entity, mut script, name_opt) in &mut script_query {
                if let Some(n) = name_opt {
                    let mut is_match = n.as_str() == target;

                    if !is_match {
                        if let Some(h) = target_hash {
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            std::hash::Hash::hash(n.as_str(), &mut hasher);
                            let hashed = (std::hash::Hasher::finish(&hasher) % 100000) as i32;
                            if hashed == h {
                                is_match = true;
                            }
                        }
                    }

                    if is_match {
                        if active {
                            commands
                                .entity(entity)
                                .remove::<crate::oni2_loader::components::ActorAsleep>();
                        } else {
                            commands
                                .entity(entity)
                                .insert(crate::oni2_loader::components::ActorAsleep);
                        }
                        script.exec.active = active;
                        info!(
                            "SetUpdateState: toggled '{}' script active={}",
                            n.as_str(),
                            active
                        );
                    }
                }
            }
        }
        ScrOniSysEvent::UsePad { script_entity } => {
            commands
                .entity(script_entity)
                .insert(crate::player::components::Player);
            commands
                .entity(script_entity)
                .remove::<crate::ai::components::AiFighter>();
            for mut rig in &mut camera_query {
                rig.target = script_entity;
            }
            info!("VM: Actor {:?} took player pad controls", script_entity);
        }
        ScrOniSysEvent::PlayAmbientSound {
            script_entity: _,
            handle,
            name,
            volume,
            pitch,
            volume_ramp,
            pitch_ramp,
        } => {
            let mut source_handle = None;
            let mut file_name = name.clone();
            if file_name.starts_with("Stream:") {
                let mut stream_name = file_name.replace("Stream:", "");
                stream_name.push_str(".stm");
                
                if let Ok(bytes) = crate::vfs::read("", &stream_name) {
                    if let Ok(decoded) = crate::oni2_loader::parsers::stm::decode_stm(&bytes) {
                        if let Ok(wav) = crate::oni2_loader::parsers::stm::create_wav_bytes(&decoded) {
                            source_handle = Some(audio_sources.add(bevy::audio::AudioSource {
                                bytes: std::sync::Arc::from(wav),
                            }));
                        } else {
                            warn!("Failed to create wav for stream: {}", stream_name);
                        }
                    } else {
                        warn!("Failed to decode stream: {}", stream_name);
                    }
                } else {
                    warn!("Stream file not found in VFS: {}", stream_name);
                }
            } else if file_name.starts_with("SFX_Blast_Chamber:") || file_name.starts_with("SFX_") {
                info!("Skipping non-stream SFX audio implementation for: {}", file_name);
                return;
            } else {
                info!("Unknown audio prefix for playambientsound: {}", file_name);
                return;
            }

            let Some(source_handle) = source_handle else {
                return;
            };
            let mut settings = bevy::audio::PlaybackSettings::DESPAWN;
            if let Some(vol) = volume {
                settings = settings.with_volume(bevy::audio::Volume::Linear(vol));
            } else if let Some((start_vol, _, _)) = volume_ramp {
                settings = settings.with_volume(bevy::audio::Volume::Linear(start_vol));
            }
            if let Some(p) = pitch {
                settings = settings.with_speed(p);
            }
            else if let Some((start_pitch, _, _)) = pitch_ramp {
                settings = settings.with_speed(start_pitch);
            }

            let mut ent = commands.spawn((
                bevy::audio::AudioPlayer(source_handle),
                settings,
                ActiveAmbientSound { handle },
            ));

            if let Some((start_vol, end_vol, duration)) = volume_ramp {
                ent.insert(AudioVolumeRamp {
                    start_vol,
                    end_vol,
                    duration,
                    elapsed: 0.0,
                });
            }

            if let Some((start_pitch, end_pitch, duration)) = pitch_ramp {
                ent.insert(AudioPitchRamp {
                    start_pitch,
                    end_pitch,
                    duration,
                    elapsed: 0.0,
                });
            }
        }
        ScrOniSysEvent::AmbientSoundVolumeRamp {
            script_entity: _,
            handle,
            target_vol,
            duration,
        } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Applying AmbientSoundVolumeRamp to handle {}", handle);
                    commands.entity(entity).insert(AudioVolumeRamp {
                        start_vol: -1.0, // Marker to initialize from current sink volume
                        end_vol: target_vol,
                        duration,
                        elapsed: 0.0,
                    });
                }
            }
        }
        ScrOniSysEvent::AmbientSoundPitchRamp {
            script_entity: _,
            handle,
            target_pitch,
            duration,
        } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Applying AmbientSoundPitchRamp to handle {}", handle);
                    commands.entity(entity).insert(AudioPitchRamp {
                        start_pitch: -1.0,
                        end_pitch: target_pitch,
                        duration,
                        elapsed: 0.0,
                    });
                }
            }
        }
        ScrOniSysEvent::AmbientSoundStop { script_entity: _, handle } => {
            for (entity, active_sound) in &ambient_sound_query {
                if active_sound.handle == handle {
                    info!("VM: Despawning AmbientSound handle {}", handle);
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}

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
