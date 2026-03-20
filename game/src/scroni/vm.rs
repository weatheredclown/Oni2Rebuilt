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
            _ => String::new(),
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
    Idle { end_time: f64 },
    GotoCurvePhase { target: f32, seconds: f32 },
    GotoCurveKnot { knot: i32, seconds: f32 },
    GotoCurveLerp { target: f32, seconds: f32 },
    Face { target: Value, seconds: Option<f32> },
    GotoPoint { target: Value, within: Option<f32>, speed: Option<f32>, duration: Option<f32> },
    PlayAnimation { name: String, hold: bool, rate: Option<f32>, duration: Option<f32> },
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
    Find { list_var: String, conditions: Vec<(String, Value)>, range: Option<f32> },
}

#[derive(Debug, Clone)]
pub enum SysRequest {
    TextureMovie { target_name: String, action: super::ast::TextureMovieAction, arg: Value },
    Spawn { script: String, assign_to: Option<String>, at: Option<Vec3>, name: Option<String> },
    Teleport { target: Entity, to: Option<Vec3>, face: Option<f32> },
    CameraSetPackage(String),
    DrawText(String),
    At(f32, f32),
    MakeFx { script_entity: Entity, name: String, at: Option<Vec3> },
    SendAction { action: String, target: Entity, component: String },
    SetLightIntensity { light: String, intensity: f32 },
    SetShaderLocal { name: String, val: f32 },
}

#[derive(Event, Debug, Clone)]
pub enum ScrOniSysEvent {
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
        script_entity: Entity,
        light: String,
        intensity: f32,
    },
    SetShaderLocal {
        script_entity: Entity,
        name: String,
        val: f32,
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
}

impl ScrOniThread {
    pub fn new(thread_id: u32, parent_thread_id: Option<u32>, script: ScriptDef, start_time: f64) -> Self {
        let mut variables = HashMap::new();
        for var in &script.variables {
            if var.is_parent { continue; } // Do not allocate locally if inherited
            variables.insert(var.name.clone(), Value::default_for_type(&var.var_type));
        }

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
        }
    }

    pub fn clear_blocking(&mut self) {
        self.blocking = None;
        if self.state == ExecState::Blocked {
            self.state = ExecState::Running;
        }
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
}

#[derive(Debug, Clone)]
pub struct ScroniContext<'a, 'w_e, 's_e, 'w_t, 's_t> {
    pub all_entities: &'a Query<'w_e, 's_e, (Entity, &'static Transform, Option<&'static Name>)>,
    pub triggers: &'a Query<'w_t, 's_t, &'static BroadcastTrigger>,
    pub player: Option<Entity>,
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
            if target_ent == trigger_ent { continue; }
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
    Forever { body: Vec<Stmt>, pc: usize },
    While { condition: Expr, body: Vec<Stmt>, pc: usize },
    NTimes { remaining: i32, body: Vec<Stmt>, pc: usize },
    ForSeconds { end_time: f64, body: Vec<Stmt>, pc: usize },
    Block { stmts: Vec<Stmt>, pc: usize },
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
        }
    }

    pub fn all_threads_mut(&mut self) -> impl Iterator<Item = &mut ScrOniThread> {
        std::iter::once(&mut self.main_thread).chain(self.child_threads.iter_mut())
    }

    pub fn get_thread(&self, tid: u32) -> &ScrOniThread {
        if tid == 0 {
            &self.main_thread
        } else {
            self.child_threads.iter().find(|t| t.thread_id == tid).unwrap()
        }
    }

    pub fn get_thread_mut(&mut self, tid: u32) -> &mut ScrOniThread {
        if tid == 0 {
            &mut self.main_thread
        } else {
            self.child_threads.iter_mut().find(|t| t.thread_id == tid).unwrap()
        }
    }

    pub fn get_var(&self, tid: u32, name: &str) -> Value {
        let thread = self.get_thread(tid);
        if let Some(v) = thread.variables.get(name) {
            return v.clone();
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
    /// The whenever block runs first, then the sequence block resumes.
    pub fn tick(&mut self, now: f64, ctx: &mut ScroniContext) -> ExecState {
        // Execute main thread
        self.tick_thread(0, now, ctx);

        let mut i = 0;
        while i < self.child_threads.len() {
            let tid = self.child_threads[i].thread_id;
            self.tick_thread(tid, now, ctx);

            if self.child_threads[i].state == ExecState::Done {
                self.child_threads.remove(i);
            } else {
                i += 1;
            }
        }

        self.main_thread.state
    }

    fn tick_thread(&mut self, tid: u32, now: f64, ctx: &mut ScroniContext) {
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

        // Run whenever block (non-blocking, runs every frame)
        let whenever = self.get_thread(tid).script.whenever.clone();
        if let Some(ref whenever_stmts) = whenever {
            for stmt in whenever_stmts {
                self.exec_stmt(tid, stmt, now, ctx);
                if self.get_thread(tid).state == ExecState::Yielded {
                    self.get_thread_mut(tid).state = ExecState::Running; // reset for sequence
                    break;
                }
            }
        }

        // Run sequence block from current PC
        self.run_sequence(tid, now, ctx);
    }

    fn run_sequence(&mut self, tid: u32, now: f64, ctx: &mut ScroniContext) {
        loop {
            // If we're inside a loop, continue that loop
            while !self.get_thread(tid).loop_stack.is_empty() {
                if self.get_thread(tid).state != ExecState::Running {
                    return;
                }
                
                let mut ls = self.get_thread_mut(tid).loop_stack.pop().unwrap();
                let pre_len = self.get_thread(tid).loop_stack.len();
                
                let (active, push_back) = self.step_loop(tid, &mut ls, now, ctx);
                
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

            if self.get_thread(tid).state != ExecState::Running { return; }

            let mut broke_for_loop = false;
            
            // Continue sequence from PC
            while self.get_thread(tid).state == ExecState::Running {
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
                self.get_thread_mut(tid).state = ExecState::Running;
                Some((true, true))
            }
            _ => None,
        }
    }

    /// Step a loop. Returns (still_active, should_push_back).
    fn step_loop(&mut self, tid: u32, ls: &mut LoopState, now: f64, ctx: &mut ScroniContext) -> (bool, bool) {
        match ls {
            LoopState::Forever { body, pc } => {
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) { return res; }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0; // restart loop
                    return (true, true);
                }
                (true, true) // blocked — keep loop
            }
            LoopState::While { condition, body, pc } => {
                let cond = condition.clone();
                let cond_val = self.eval_expr(tid, &cond, now, ctx);
                if !cond_val.as_bool() {
                    return (false, false); // loop done
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) { return res; }
                if self.get_thread(tid).state == ExecState::Running {
                    *pc = 0;
                    return (true, true);
                }
                (true, true)
            }
            LoopState::NTimes { remaining, body, pc } => {
                if *remaining <= 0 {
                    return (false, false);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) { return res; }
                if self.get_thread(tid).state == ExecState::Running {
                    *remaining -= 1;
                    *pc = 0;
                    let still_active = *remaining > 0;
                    return (still_active, still_active);
                }
                (true, true)
            }
            LoopState::ForSeconds { end_time, body, pc } => {
                if now >= *end_time {
                    return (false, false);
                }
                while *pc < body.len() && self.get_thread(tid).state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(tid, &stmt, now, ctx);
                }
                if let Some(res) = self.check_loop_state(tid) { return res; }
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
                if let Some(res) = self.check_loop_state(tid) { return res; }
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
                    if let Value::Actor(ent) = val {
                        vec.push(ent);
                    } else if let Value::Int(guid) = val {
                        vec.push(Entity::from_bits(guid as u64));
                    }
                    self.set_var(tid, list.clone(), Value::ActorList(vec, idx));
                }
            }
            Stmt::If { condition, then_branch, else_branch } => {
                let cond = self.eval_expr(tid, condition, now, ctx);
                if cond.as_bool() {
                    self.exec_stmt(tid, then_branch, now, ctx);
                } else if let Some(else_b) = else_branch {
                    self.exec_stmt(tid, else_b, now, ctx);
                }
            }
            Stmt::Block(stmts) => {
                self.get_thread_mut(tid).loop_stack.push(LoopState::Block { stmts: stmts.clone(), pc: 0 });
                self.get_thread_mut(tid).state = ExecState::PushLoop;
            }
            Stmt::DoForever(body) => {
                let stmts = self.flatten_to_block(body);
                self.get_thread_mut(tid).loop_stack.push(LoopState::Forever { body: stmts, pc: 0 });
                self.get_thread_mut(tid).state = ExecState::PushLoop;
            }
            Stmt::DoWhile { condition, body } => {
                let stmts = self.flatten_to_block(body);
                self.get_thread_mut(tid).loop_stack.push(LoopState::While {
                    condition: condition.clone(),
                    body: stmts,
                    pc: 0,
                });
                self.get_thread_mut(tid).state = ExecState::PushLoop;
            }
            Stmt::DoNTimes { count, body } => {
                let n = self.eval_expr(tid, count, now, ctx).as_int();
                let stmts = self.flatten_to_block(body);
                self.get_thread_mut(tid).loop_stack.push(LoopState::NTimes { remaining: n, body: stmts, pc: 0 });
                self.get_thread_mut(tid).state = ExecState::PushLoop;
            }
            Stmt::DoForSeconds { seconds, body } => {
                let secs = self.eval_expr(tid, seconds, now, ctx).as_float();
                let stmts = self.flatten_to_block(body);
                self.get_thread_mut(tid).loop_stack.push(LoopState::ForSeconds {
                    end_time: now + secs as f64,
                    body: stmts,
                    pc: 0,
                });
                self.get_thread_mut(tid).state = ExecState::PushLoop;
            }
            Stmt::Exit => {
                self.get_thread_mut(tid).state = ExecState::Yielded;
            }
            Stmt::Done => {
                let frame = self.get_thread_mut(tid).call_stack.pop();
                if let Some(frame) = frame {
                    let mut t = self.get_thread_mut(tid);
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
                let mut t = self.get_thread_mut(tid);
                t.seq_pc = 0;
                t.loop_stack.clear();
                t.state = ExecState::AbortSequence;
            }
            Stmt::Log(exprs) => {
                let parts: Vec<String> = exprs.iter().map(|e| {
                    let v = self.eval_expr(tid, e, now, ctx);
                    v.as_string()
                }).collect();
                info!("[ScrOni] {}", parts.join(" "));
            }

            // Blocking commands — set blocking action and yield
            Stmt::Idle(expr) => {
                let secs = self.eval_expr(tid, expr, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Idle { end_time: now + secs as f64 });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurvePhase { phase, seconds } => {
                let p = self.eval_expr(tid, phase, now, ctx).as_float();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurvePhase { target: p, seconds: s });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurveKnot { knot, seconds } => {
                let k = self.eval_expr(tid, knot, now, ctx).as_int();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurveKnot { knot: k, seconds: s });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoCurveLerp { lerp, seconds } => {
                let l = self.eval_expr(tid, lerp, now, ctx).as_float();
                let s = self.eval_expr(tid, seconds, now, ctx).as_float();
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoCurveLerp { target: l, seconds: s });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::Face { target, seconds } => {
                let t = self.eval_expr(tid, target, now, ctx);
                let s = seconds.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Face { target: t, seconds: s });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::GotoPoint { target, within, speed, duration } => {
                let t = self.eval_expr(tid, target, now, ctx);
                let w = within.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let s = speed.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let d = duration.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::GotoPoint { target: t, within: w, speed: s, duration: d });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::PlayAnimation { name, hold, rate, duration } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let r = rate.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                let d = duration.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::PlayAnimation { name: n, hold: *hold, rate: r, duration: d });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }
            Stmt::PlayActionAnimation { name, hold, duration } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let d = duration.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::PlayAnimation { name: n, hold: *hold, rate: None, duration: d });
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

            Stmt::Find { list_var, conditions, range } => {
                let eval_conds = conditions.iter().map(|(k, e)| (k.clone(), self.eval_expr(tid, e, now, ctx))).collect();
                let eval_range = range.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                self.get_thread_mut(tid).blocking = Some(BlockingAction::Find {
                    list_var: list_var.clone(),
                    conditions: eval_conds,
                    range: eval_range,
                });
                self.get_thread_mut(tid).state = ExecState::Blocked;
            }

            Stmt::TextureMovie { name, pass: _, action, arg } => {
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
                if let Value::Actor(entity) = target {
                    let mut args = Vec::new();
                    for a in with {
                        args.push(self.eval_expr(tid, a, now, ctx));
                    }
                    self.outgoing_messages.push(ScriptMessage {
                        msg: msg_str,
                        from: self.owner,
                        to: entity,
                        args,
                        is_action: false,
                    });
                }
            }
            Stmt::SendAction { action, target, component } => {
                let act_str = self.eval_expr(tid, action, now, ctx).as_string();
                let tgt = self.eval_expr(tid, target, now, ctx);
                if let Value::Actor(entity) = tgt {
                    if let Some(comp_expr) = component {
                        let comp_str = self.eval_expr(tid, comp_expr, now, ctx).as_string();
                        self.sys_requests.push(SysRequest::SendAction {
                            action: act_str,
                            target: entity,
                            component: comp_str,
                        });
                    } else {
                        self.outgoing_messages.push(ScriptMessage {
                            msg: act_str,
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
                    let to_vec = to.as_ref().map(|e| {
                        match self.eval_expr(tid, e, now, ctx) {
                            Value::Vector(v) => v,
                            _ => Vec3::ZERO,
                        }
                    });
                    let face_float = face.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                    
                    self.sys_requests.push(SysRequest::Teleport {
                        target: ent,
                        to: to_vec,
                        face: face_float,
                    });
                }
            }

            Stmt::Spawn { script, assign_to, at, name } => {
                let script_str = self.eval_expr(tid, script, now, ctx).as_string();
                let assign = assign_to.clone();
                let at_pos = at.as_ref().map(|e| {
                    match self.eval_expr(tid, e, now, ctx) {
                        Value::Vector(v) => v,
                        _ => Vec3::ZERO,
                    }
                });
                let target_name = name.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_string());
                
                self.sys_requests.push(SysRequest::Spawn {
                    script: script_str,
                    assign_to: assign,
                    at: at_pos,
                    name: target_name,
                });
            }

            Stmt::MakeFx { name, at } => {
                let fx_name = self.eval_expr(tid, name, now, ctx).as_string();
                let fx_pos = at.as_ref().map(|e| {
                    match self.eval_expr(tid, e, now, ctx) {
                        Value::Vector(v) => v,
                        Value::Actor(ent) => {
                            if let Ok((_, tf, _)) = ctx.all_entities.get(ent) {
                                tf.translation
                            } else {
                                Vec3::ZERO
                            }
                        }
                        _ => Vec3::ZERO,
                    }
                });

                self.sys_requests.push(SysRequest::MakeFx {
                    script_entity: self.owner,
                    name: fx_name,
                    at: fx_pos,
                });
            }

            Stmt::Stack(name_expr) => {
                let name = self.eval_expr(tid, name_expr, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&name).cloned() {
                    let mut t = self.get_thread_mut(tid);
                    let frame = CallFrame {
                        script: t.script.clone(),
                        variables: t.variables.clone(),
                        seq_pc: t.seq_pc,
                        loop_stack: t.loop_stack.clone(),
                    };
                    t.call_stack.push(frame);
                    t.script = new_script.clone();
                    t.variables.clear();
                    for var in &t.script.variables {
                        if var.is_parent { continue; }
                        let val = match var.var_type {
                            VarType::Integer => Value::Int(0),
                            VarType::Float => Value::Float(0.0),
                            VarType::Vector => Value::Vector(Vec3::ZERO),
                            VarType::String => Value::String(String::new()),
                            VarType::Timer => Value::Float(0.0),
                            VarType::Label => Value::String(String::new()),
                            VarType::ActorList => Value::ActorList(Vec::new(), 0),
                            VarType::Child => Value::Int(0),
                        };
                        t.variables.insert(var.name.clone(), val);
                    }
                    t.seq_pc = 0;
                    t.loop_stack.clear();
                    t.state = ExecState::AbortSequence; // Yield to prevent executing rest of old block
                } else {
                    warn!("Stack: Script '{}' not found in available scripts.", name);
                }
            }

            Stmt::Switch(name_expr) => {
                let name = self.eval_expr(tid, name_expr, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&name).cloned() {
                    let mut t = self.get_thread_mut(tid);
                    t.script = new_script;
                    t.variables.clear();
                    for var in &t.script.variables {
                        if var.is_parent { continue; }
                        let val = match var.var_type {
                            VarType::Integer => Value::Int(0),
                            VarType::Float => Value::Float(0.0),
                            VarType::Vector => Value::Vector(Vec3::ZERO),
                            VarType::String => Value::String(String::new()),
                            VarType::Timer => Value::Float(0.0),
                            VarType::Label => Value::String(String::new()),
                            VarType::ActorList => Value::ActorList(Vec::new(), 0),
                            VarType::Child => Value::Int(0),
                        };
                        t.variables.insert(var.name.clone(), val);
                    }
                    t.seq_pc = 0;
                    t.loop_stack.clear();
                    self.get_thread_mut(tid).state = ExecState::AbortSequence; // Yield to prevent executing rest of old block
                } else {
                    warn!("Switch: Script '{}' not found in available scripts.", name);
                }
            }

            Stmt::ChildStack { var, script } => {
                let script_name = self.eval_expr(tid, script, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&script_name).cloned() {
                    let new_tid = self.next_thread_id;
                    self.next_thread_id += 1;
                    let mut new_thread = ScrOniThread::new(new_tid, Some(tid), new_script, now);
                    for var_decl in &new_thread.script.variables {
                        if var_decl.is_parent { continue; }
                        new_thread.variables.insert(var_decl.name.clone(), Value::default_for_type(&var_decl.var_type));
                    }
                    self.child_threads.push(new_thread);
                    self.set_var(tid, var.clone(), Value::Int(new_tid as i32));
                } else {
                    warn!("ChildStack: Script '{}' not found", script_name);
                }
            }

            Stmt::ChildSwitch { var, script } => {
                let script_name = self.eval_expr(tid, script, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&script_name).cloned() {
                    let new_tid = self.next_thread_id;
                    self.next_thread_id += 1;
                    let mut new_thread = ScrOniThread::new(new_tid, Some(tid), new_script, now);
                    for var_decl in &new_thread.script.variables {
                        if var_decl.is_parent { continue; }
                        new_thread.variables.insert(var_decl.name.clone(), Value::default_for_type(&var_decl.var_type));
                    }
                    self.child_threads.push(new_thread);
                    self.set_var(tid, var.clone(), Value::Int(new_tid as i32));
                } else {
                    warn!("ChildSwitch: Script '{}' not found", script_name);
                }
            }

            Stmt::CameraSetPackage(expr) => {
                let pkg_name = self.eval_expr(tid, expr, now, ctx).as_string();
                self.sys_requests.push(SysRequest::CameraSetPackage(pkg_name));
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
                info!("VM: Sound {:?} (unimplemented)", args);
            }
            Stmt::AmbientSound { args } => {
                info!("VM: AmbientSound {:?} (unimplemented)", args);
            }
            Stmt::PlayAmbientSound { name, volume } => {
                let n = self.eval_expr(tid, name, now, ctx).as_string();
                let v = volume.as_ref().map(|e| self.eval_expr(tid, e, now, ctx).as_float());
                info!("VM: PlayAmbientSound {} {:?} (unimplemented)", n, v);
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
            Stmt::CameraFollowActor { args } => { info!("VM: CameraFollowActor {:?} (unimplemented)", args); }
            Stmt::CameraTrackActor { args } => { info!("VM: CameraTrackActor {:?} (unimplemented)", args); }
            Stmt::CameraTrackPoint { args } => { info!("VM: CameraTrackPoint {:?} (unimplemented)", args); }
            Stmt::CameraMoveToActor { args } => { info!("VM: CameraMoveToActor {:?} (unimplemented)", args); }
            Stmt::CameraMoveToPoint { args } => { info!("VM: CameraMoveToPoint {:?} (unimplemented)", args); }
            Stmt::CameraCutToActor { args } => { info!("VM: CameraCutToActor {:?} (unimplemented)", args); }
            Stmt::CameraCutToPoint { args } => { info!("VM: CameraCutToPoint {:?} (unimplemented)", args); }
            Stmt::CameraSetFOV { args } => { info!("VM: CameraSetFOV {:?} (unimplemented)", args); }
            Stmt::CameraShake => { info!("VM: CameraShake (unimplemented)"); }

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
                self.sys_requests.push(SysRequest::SetShaderLocal { name, val });
            }
            Stmt::SetLightParameter { args } => {
                let light = self.eval_expr(tid, &args[0], now, ctx).as_string();
                self.current_light = Some(light);
            }
            Stmt::Intensity { args } => {
                let val = self.eval_expr(tid, &args[0], now, ctx).as_float();
                if let Some(light) = &self.current_light {
                    self.sys_requests.push(SysRequest::SetLightIntensity { light: light.clone(), intensity: val });
                }
            }
            Stmt::SetFullScreenColor { args } => {
                info!("VM: SetFullScreenColor {:?} (unimplemented)", args);
            }
            Stmt::SetUpdateState { target, state } => {
                let target_val = self.eval_expr(tid, target, now, ctx).as_string();
                let state_val = self.eval_expr(tid, state, now, ctx).as_string();
                info!("VM: SetUpdateState {} {} (unimplemented)", target_val, state_val);
            }

            // Stubs for commands we don't execute yet
            _ => {
                // Non-silently ignore unimplemented commands for now
                info!("Unimplemented command: {:?}", stmt);
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
                    if let Value::Actor(ent) = v {
                        ents.push(ent);
                    } else if let Value::Int(guid) = v {
                        ents.push(Entity::from_bits(guid as u64));
                    }
                }
                Value::ActorList(ents, 0)
            }
            Expr::Var(name) => {
                self.get_var(tid, name)
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
                eval_binop(*op, &l, &r)
            }
            Expr::Call { name, args } => {
                let lower = name.to_lowercase();
                match lower.as_str() {
                    "clock" => Value::Float(now as f32),
                    "random" => Value::Int(rand::random::<i32>().abs() % 100),
                    "randomrange" => Value::Int(0), // stub
                    "randomrangefloat" => Value::Float(0.0), // stub
                    "location" => {
                        let target = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let Some(Value::Actor(act)) = target {
                            if let Ok((_, tf, _)) = ctx.all_entities.get(act) {
                                return Value::Vector(tf.translation);
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
                                        Some(tf.translation)
                                    } else {
                                        None
                                    }
                                }
                                _ => None
                            }
                        };
                        
                        let mut p1 = arg1.and_then(resolve_pos);
                        let mut p2 = arg2.and_then(resolve_pos);
                        
                        if p1.is_some() && p2.is_none() {
                            if let Ok((_, my_tf, _)) = ctx.all_entities.get(self.owner) {
                                p2 = p1;
                                p1 = Some(my_tf.translation);
                            }
                        }
                        
                        if let (Some(a), Some(b)) = (p1, p2) {
                            return Value::Float(a.distance(b));
                        }
                        Value::Float(99999.0)
                    }
                    "triggerentered" => {
                        let trig_ent = args.get(0).map(|e| self.eval_expr(tid, e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(tid, e, now, ctx));
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent) {
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
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent) {
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
                        if let (Some(Value::Actor(t)), Some(Value::Actor(e))) = (trig_ent, targ_ent) {
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
                            if let Some(idx) = self.message_queue.iter().position(|m| !m.is_action && m.msg == target_msg) {
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "receiveaction" => {
                        if let Some(msg_expr) = args.get(0) {
                            let target_msg = self.eval_expr(tid, msg_expr, now, ctx).as_string();
                            if let Some(idx) = self.message_queue.iter().position(|m| m.is_action && m.msg == target_msg) {
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
                    "first" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, _)) = self.get_thread(tid).variables.get(list_name) {
                                let updated = entities.clone();
                                if let Some(&first_ent) = updated.first() {
                                    self.get_thread_mut(tid).variables.insert(list_name.clone(), Value::ActorList(updated, 1));
                                    return Value::Actor(first_ent);
                                } else {
                                    self.get_thread_mut(tid).variables.insert(list_name.clone(), Value::ActorList(updated, 0));
                                    return Value::None;
                                }
                            }
                        }
                        Value::None
                    }
                    "next" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, idx)) = self.get_thread(tid).variables.get(list_name) {
                                let updated = entities.clone();
                                let current_idx = *idx;
                                if current_idx < updated.len() {
                                    let ent = updated[current_idx];
                                    self.get_thread_mut(tid).variables.insert(list_name.clone(), Value::ActorList(updated, current_idx + 1));
                                    return Value::Actor(ent);
                                } else {
                                    self.get_thread_mut(tid).variables.insert(list_name.clone(), Value::ActorList(updated, current_idx));
                                    return Value::None;
                                }
                            }
                        }
                        Value::None
                    }
                    "guid" => {
                        let entity_name = args.get(0)
                            .map(|e| self.eval_expr(tid, e, now, ctx).as_string())
                            .unwrap_or_default();
                        for (other_ent, _, name_opt) in ctx.all_entities {
                            if let Some(n) = name_opt {
                                if n.as_str() == entity_name {
                                    return Value::Actor(other_ent);
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
        if let Some(t) = self.child_threads.iter_mut().find(|t| t.thread_id == tid).or_else(|| {
            if tid == 0 { Some(&mut self.main_thread) } else { None }
        }) {
            t.blocking = None;
            if t.state == ExecState::Blocked {
                t.state = ExecState::Running;
            }
        }
    }
}

fn eval_binop(op: BinOp, l: &Value, r: &Value) -> Value {
    match op {
        BinOp::Mod => {
            match (l, r) {
                (Value::Int(a), Value::Int(b)) => {
                    if *b == 0 { Value::Int(0) } else { Value::Int(a % b) }
                }
                _ => {
                    let rv = r.as_float();
                    if rv == 0.0 { Value::Float(0.0) } else { Value::Float(l.as_float() % rv) }
                }
            }
        }
        BinOp::Add => {
            match (l, r) {
                (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
                (Value::Vector(a), Value::Vector(b)) => Value::Vector(*a + *b),
                _ => Value::Float(l.as_float() + r.as_float()),
            }
        }
        BinOp::Sub => {
            match (l, r) {
                (Value::Int(a), Value::Int(b)) => Value::Int(a - b),
                (Value::Vector(a), Value::Vector(b)) => Value::Vector(*a - *b),
                _ => Value::Float(l.as_float() - r.as_float()),
            }
        }
        BinOp::Mul => {
            match (l, r) {
                (Value::Int(a), Value::Int(b)) => Value::Int(a * b),
                _ => Value::Float(l.as_float() * r.as_float()),
            }
        }
        BinOp::Div => {
            let rv = r.as_float();
            if rv == 0.0 { Value::Float(0.0) } else { Value::Float(l.as_float() / rv) }
        }
        BinOp::Equal => Value::Int(if (l.as_float() - r.as_float()).abs() < f32::EPSILON { 1 } else { 0 }),
        BinOp::NotEqual => Value::Int(if (l.as_float() - r.as_float()).abs() >= f32::EPSILON { 1 } else { 0 }),
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
    mut query: Query<(Entity, &mut ScrOniScript, &Transform)>,
    all_entities: Query<(Entity, &'static Transform, Option<&'static Name>)>,
    triggers: Query<&'static BroadcastTrigger>,
    time: Res<Time>,
    player_query: Query<Entity, With<crate::player::components::Player>>,
) {
    let now = time.elapsed_secs_f64();
    let mut all_messages = Vec::new();
    let player_ent = player_query.iter().next();

    for (entity, mut script, transform) in &mut query {
        let mut ctx = ScroniContext {
            all_entities: &all_entities,
            triggers: &triggers,
            player: player_ent,
        };
        script.exec.tick(now, &mut ctx);

        let mut finds_to_resolve = Vec::new();
        for t in script.exec.all_threads_mut() {
            if let Some(BlockingAction::Find { list_var, conditions, range }) = t.blocking.clone() {
                finds_to_resolve.push((t.thread_id, list_var, conditions, range));
            }
        }

        // Handle Find request
        for (tid, list_var, conditions, range) in finds_to_resolve {
            let mut found = Vec::new();
            let my_pos = transform.translation;
            let max_dist = range.unwrap_or(9999.0);

            for (other_ent, other_tf, name_opt) in &all_entities {
                if entity == other_ent { continue; }

                let dist = my_pos.distance(other_tf.translation);
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

            script.exec.set_var(tid, list_var, Value::ActorList(found, 0));
            script.exec.clear_blocking(tid);
            // Tick again to resume immediately
            script.exec.tick_thread(tid, now, &mut ctx);
        }

        for req in script.exec.sys_requests.drain(..) {
            match req {
                SysRequest::TextureMovie { target_name, action, arg } => {
                    commands.trigger(ScrOniSysEvent::TextureMovie {
                        script_entity: entity,
                        target_name,
                        action,
                        arg,
                    });
                }
                SysRequest::Spawn { script, assign_to, at, name } => {
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
                SysRequest::DrawText(text) => {
                    commands.trigger(ScrOniSysEvent::DrawText(text));
                }
                SysRequest::MakeFx { script_entity, name, at } => {
                    commands.trigger(ScrOniSysEvent::MakeFx {
                        script_entity,
                        name,
                        at,
                    });
                }
                SysRequest::SendAction { action, target, component } => {
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
            }
        }

        all_messages.append(&mut script.exec.outgoing_messages);
    }

    // Deliver messages
    for msg in all_messages {
        if let Ok((_, mut target_script, _)) = query.get_mut(msg.to) {
            target_script.exec.message_queue.push(msg);
        }
    }
}

/// Load and compile a .oni script file, returning ScriptDefs.
pub fn load_script_file(dir: &str, filename: &str) -> Result<ScriptFile, String> {
    let source = crate::vfs::read_to_string(dir, filename)
        .map_err(|e| format!("Failed to read {}/{}: {}", dir, filename, e))?;
    Compiler::compile(&source)
        .map_err(|errors| {
            let msgs: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
            format!("Compile errors in {}/{}:\n{}", dir, filename, msgs.join("\n"))
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

/// Observer to handle ScrOni system requests (like TextureMovie)
pub fn scroni_sys_event_observer(
    trigger: On<ScrOniSysEvent>,
    mut commands: Commands,
    mut transform_query: Query<(&mut Transform, Option<&mut avian3d::prelude::LinearVelocity>)>,
    children_query: Query<&Children>,
    mut materials_query: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut assets: (
        ResMut<Assets<StandardMaterial>>,
        ResMut<Assets<Mesh>>,
        ResMut<Assets<Image>>,
        ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    ),
    mut texture_collections: ResMut<crate::oni2_loader::TextureCollections>,
    layout_context: Option<Res<crate::oni2_loader::LayoutContext>>,
    layout_paths: Option<Res<crate::oni2_loader::LayoutPaths>>,
    mut active_camera_package: Option<ResMut<crate::oni2_loader::ActiveCameraPackage>>,
    mut scroni_text_state: ResMut<ScroniTextState>,
    time: Res<Time>,
    mut entity_lib: ResMut<crate::oni2_loader::registries::EntityLibrary>,
    mut anim_registry: ResMut<crate::oni2_loader::registries::AnimRegistry>,
    mut camera_query: Query<&mut crate::camera::components::CameraRig>,
    mut lights_query: Query<(&Name, Option<&mut PointLight>, Option<&mut SpotLight>)>,
) {
    let ev = (*trigger).clone();
    let (mut materials, mut meshes, mut images, mut skinned_mesh_ibp) = assets;
    match ev {
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
                    info!("Changing active camera package from {} to {}", active_pkg.name, pkg_name);
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
        ScrOniSysEvent::TextureMovie { script_entity, target_name, action, arg } => {
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
                            warn!("TextureMovie SetFrame {} out of bounds for {}", frame, target_name);
                        }
                    } else {
                        warn!("TextureMovie: No preloaded textures found for {}", target_name);
                    }
                }
                _ => {}
            }
        }
        ScrOniSysEvent::Spawn { script_entity: _, script, assign_to, at, name } => {
            info!("Received spawn request: script={}, at={:?}, name={:?}", script, at, name);
            
            let pos = at.unwrap_or(Vec3::ZERO);
            let actor_name = name.clone().unwrap_or(script.clone());
            
            if let (Some(layout_ctx), Some(paths)) = (&layout_context, &layout_paths) {
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
                    &actor_name,
                    layout_ctx,
                    paths,
                    Some(pos),
                ) {
                    info!("Spawned {} at {:?}", actor_name, pos);
                    if let Some(var_name) = assign_to {
                        warn!("Assigning spawn result to {} is not yet supported synchronously.", var_name);
                    }
                    return; // Successfully spawned!
                } else {
                    warn!("Failed to spawn actor {} using spawn_layout_actor", actor_name);
                }
            } else {
                warn!("Spawn command needs a LayoutContext and LayoutPaths resource to fully spawn {}.", script);
            }

            // Fallback Stub: spawn a basic entity placeholder instead if proper spawning fails
            let _new_entity = commands.spawn((
                Transform::from_translation(pos),
                Visibility::Visible,
                crate::oni2_loader::Oni2Entity { name: actor_name.clone() },
                Name::new(actor_name.clone()),
                crate::menu::InGameEntity,
            )).id();

            if let Some(var_name) = assign_to {
                warn!("Assigning spawn result to {} is not yet supported synchronously.", var_name);
            }
        }
        ScrOniSysEvent::Teleport { target, to, face } => {
            if let Ok((mut transform, mut opt_vel)) = transform_query.get_mut(target) {
                if let Some(pos) = to {
                    let bevy_pos = Vec3::new(-pos.x, pos.y, -pos.z);
                    transform.translation = bevy_pos;
                    commands.entity(target).insert(crate::oni2_loader::spawn::NeedsGroundSnap {
                        origin: bevy_pos,
                        wait_frames: 4,
                    });
                }
                if let Some(angles_y) = face {
                    let rad = angles_y.to_radians();
                    let current_rot = transform.rotation.to_euler(EulerRot::YXZ);
                    transform.rotation = Quat::from_euler(EulerRot::YXZ, rad, current_rot.1, current_rot.2);
                }
                
                if let Some(vel) = opt_vel.as_deref_mut() {
                    vel.0 = Vec3::ZERO;
                }
            }
        }
        ScrOniSysEvent::MakeFx { script_entity, name, at } => {
            commands.trigger(crate::fx_system::SpawnFx {
                name: name,
                at: at,
                parent: Some(script_entity),
            });
        }
        ScrOniSysEvent::SendAction { action, target, component } => {
            if component.eq_ignore_ascii_case("fx") {
                commands.trigger(crate::fx_system::FxAction {
                    action: action.clone(),
                    target: target,
                });
            } else {
                warn!("SendAction: Unrecognized component '{}'", component);
            }
        }
        ScrOniSysEvent::SetLightIntensity { script_entity: _, light, intensity } => {
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
        ScrOniSysEvent::SetShaderLocal { script_entity: _, name, val } => {
            debug!("VM: Observed SetShaderLocal {} = {} (Unimplemented Material Target)", name, val);
        }
    }
}
