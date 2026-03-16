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
    /// Script completed (done instruction or fell off end of sequence).
    Done,
    /// Waiting for a blocking behavior to complete.
    Blocked,
}

/// A message sent between scripts.
#[derive(Debug, Clone)]
pub struct ScriptMessage {
    pub msg: String,
    pub from: Entity,
    pub to: Entity,
    pub args: Vec<Value>,
}

/// Represents a blocking command that the VM has issued and is waiting on.
#[derive(Debug, Clone)]
pub enum BlockingAction {
    Idle { end_time: f64 },
    GotoCurvePhase { target: f32, seconds: f32 },
    GotoCurveKnot { knot: i32, seconds: f32 },
    GotoCurveLerp { target: f32, seconds: f32 },
    Face { target: Value, seconds: Option<f32> },
    GotoPoint { target: Value, within: Option<f32>, speed: Option<f32> },
    PlayAnimation { name: String, hold: bool, rate: Option<f32> },
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
}

#[derive(Debug, Clone)]
pub struct CallFrame {
    pub script: ScriptDef,
    pub variables: HashMap<String, Value>,
    pub seq_pc: usize,
    pub loop_stack: Vec<LoopState>,
}

/// Execution context for a single script. Holds variables and program counter state.
pub struct ScriptExec {
    pub script: ScriptDef,
    pub variables: HashMap<String, Value>,
    pub state: ExecState,
    /// Program counter into the sequence block (index of next statement to execute).
    pub seq_pc: usize,
    /// Stack of loop state for nested control flow.
    pub loop_stack: Vec<LoopState>,
    /// Call stack for Stack / Return behavior.
    pub call_stack: Vec<CallFrame>,
    /// Other scripts in the same file that can be Switched / Stacked to.
    pub available_scripts: HashMap<String, ScriptDef>,
    /// Current blocking action waiting to complete.
    pub blocking: Option<BlockingAction>,
    /// Incoming message queue.
    pub message_queue: Vec<ScriptMessage>,
    /// Outgoing message queue.
    pub outgoing_messages: Vec<ScriptMessage>,
    /// Requests to the ECS system to perform engine-level actions.
    pub sys_requests: Vec<SysRequest>,
    /// The entity this script is attached to.
    pub owner: Entity,
    /// Game time when script started (for timed operations).
    pub start_time: f64,
}

#[derive(Debug, Clone)]
pub struct ScroniContext<'a, 'w_e, 's_e, 'w_t, 's_t> {
    pub all_entities: &'a Query<'w_e, 's_e, (Entity, &'static Transform, Option<&'static Name>)>,
    pub triggers: &'a Query<'w_t, 's_t, &'static BroadcastTrigger>,
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
        let mut variables = HashMap::new();
        for var in &script.variables {
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
            variables.insert(var.name.clone(), val);
        }

        Self {
            script,
            variables,
            state: ExecState::Running,
            seq_pc: 0,
            loop_stack: Vec::new(),
            call_stack: Vec::new(),
            available_scripts: HashMap::new(),
            blocking: None,
            message_queue: Vec::new(),
            outgoing_messages: Vec::new(),
            sys_requests: Vec::new(),
            owner,
            start_time,
        }
    }

    /// Execute one frame's worth of the script. Returns the execution state.
    /// The whenever block runs first, then the sequence block resumes.
    pub fn tick(&mut self, now: f64, ctx: &mut ScroniContext) -> ExecState {
        if self.state == ExecState::Done {
            return ExecState::Done;
        }

        // Check if blocking action completed
        if let Some(ref action) = self.blocking {
            match action {
                BlockingAction::Idle { end_time } => {
                    if now >= *end_time {
                        self.blocking = None;
                    } else {
                        return ExecState::Blocked;
                    }
                }
                // Other blocking actions are resolved externally by the game systems
                _ => return ExecState::Blocked,
            }
        }

        self.state = ExecState::Running;

        // Run whenever block (non-blocking, runs every frame)
        if let Some(ref whenever) = self.script.whenever.clone() {
            for stmt in whenever {
                self.exec_stmt(stmt, now, ctx);
                if self.state == ExecState::Yielded {
                    self.state = ExecState::Running; // reset for sequence
                    break;
                }
            }
        }

        // Run sequence block from current PC
        self.run_sequence(now, ctx);

        self.state
    }

    fn run_sequence(&mut self, now: f64, ctx: &mut ScroniContext) {
        // If we're inside a loop, continue that loop
        while !self.loop_stack.is_empty() {
            if self.state != ExecState::Running {
                return;
            }
            // Take the loop off the stack to avoid borrow conflicts
            let mut ls = self.loop_stack.pop().unwrap();
            let (active, push_back) = self.step_loop(&mut ls, now, ctx);
            if push_back {
                self.loop_stack.push(ls);
            }
            if active {
                return;
            }
            // Loop finished — continue to next outer loop or sequence
        }

        // Continue sequence from PC
        let sequence = self.script.sequence.clone();
        while self.seq_pc < sequence.len() && self.state == ExecState::Running {
            let stmt = &sequence[self.seq_pc];
            self.seq_pc += 1;
            self.exec_stmt(stmt, now, ctx);
        }

        // Fell off end of sequence
        if self.seq_pc >= sequence.len() && self.loop_stack.is_empty() && self.state == ExecState::Running {
            if let Some(frame) = self.call_stack.pop() {
                self.script = frame.script;
                self.variables = frame.variables;
                self.seq_pc = frame.seq_pc;
                self.loop_stack = frame.loop_stack;
            } else {
                self.state = ExecState::Done;
            }
        }
    }

    /// Step a loop. Returns (still_active, should_push_back).
    fn step_loop(&mut self, ls: &mut LoopState, now: f64, ctx: &mut ScroniContext) -> (bool, bool) {
        match ls {
            LoopState::Forever { body, pc } => {
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now, ctx);
                }
                if self.state == ExecState::Running {
                    *pc = 0; // restart loop
                    return (true, true);
                }
                if self.state == ExecState::Yielded {
                    self.state = ExecState::Running;
                    return (true, true);
                }
                (true, true) // blocked — keep loop
            }
            LoopState::While { condition, body, pc } => {
                let cond = condition.clone();
                let cond_val = self.eval_expr(&cond, now, ctx);
                if !cond_val.as_bool() {
                    return (false, false); // loop done
                }
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now, ctx);
                }
                if self.state == ExecState::Running {
                    *pc = 0;
                    return (true, true);
                }
                if self.state == ExecState::Yielded {
                    self.state = ExecState::Running;
                    return (true, true);
                }
                (true, true)
            }
            LoopState::NTimes { remaining, body, pc } => {
                if *remaining <= 0 {
                    return (false, false);
                }
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now, ctx);
                }
                if self.state == ExecState::Running {
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
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now, ctx);
                }
                if self.state == ExecState::Running {
                    *pc = 0;
                    return (true, true);
                }
                if self.state == ExecState::Yielded {
                    self.state = ExecState::Running;
                    return (true, true);
                }
                (true, true)
            }
            LoopState::Block { stmts, pc } => {
                while *pc < stmts.len() && self.state == ExecState::Running {
                    let stmt = stmts[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now, ctx);
                }
                if *pc >= stmts.len() {
                    return (false, false); // block done
                }
                (true, true)
            }
        }
    }

    fn exec_stmt(&mut self, stmt: &Stmt, now: f64, ctx: &mut ScroniContext) {
        if self.state != ExecState::Running {
            return;
        }

        match stmt {
            Stmt::Set { var, value } => {
                let val = self.eval_expr(value, now, ctx);
                self.variables.insert(var.clone(), val);
            }
            Stmt::AddToList { expr, list } => {
                let val = self.eval_expr(expr, now, ctx);
                let entry = self.variables.entry(list.clone()).or_insert_with(|| Value::ActorList(Vec::new(), 0));
                
                // Ensure it's treated as an ActorList even if it was mistakenly initialized as Int(0)
                if let Value::Int(0) = entry {
                    *entry = Value::ActorList(Vec::new(), 0);
                }

                if let Value::ActorList(vec, _) = entry {
                    if let Value::Actor(ent) = val {
                        vec.push(ent);
                    } else if let Value::Int(guid) = val {
                        // Sometimes guids are pushed to lists directly. For now, represent guid as a dummy entity
                        // or just ignore until we map guids to entities correctly.
                        // Assuming val is somehow resolved to entity or we store raw IDs.
                        // For Bevy Ents:
                        // Bevy entities are IDs, you can construct them with from_bits or from_raw depending on version.
                        vec.push(Entity::from_bits(guid as u64));
                    }
                }
            }
            Stmt::If { condition, then_branch, else_branch } => {
                let cond = self.eval_expr(condition, now, ctx);
                if cond.as_bool() {
                    self.exec_stmt(then_branch, now, ctx);
                } else if let Some(else_b) = else_branch {
                    self.exec_stmt(else_b, now, ctx);
                }
            }
            Stmt::Block(stmts) => {
                self.loop_stack.push(LoopState::Block { stmts: stmts.clone(), pc: 0 });
            }
            Stmt::DoForever(body) => {
                let stmts = self.flatten_to_block(body);
                self.loop_stack.push(LoopState::Forever { body: stmts, pc: 0 });
            }
            Stmt::DoWhile { condition, body } => {
                let stmts = self.flatten_to_block(body);
                self.loop_stack.push(LoopState::While {
                    condition: condition.clone(),
                    body: stmts,
                    pc: 0,
                });
            }
            Stmt::DoNTimes { count, body } => {
                let n = self.eval_expr(count, now, ctx).as_int();
                let stmts = self.flatten_to_block(body);
                self.loop_stack.push(LoopState::NTimes { remaining: n, body: stmts, pc: 0 });
            }
            Stmt::DoForSeconds { seconds, body } => {
                let secs = self.eval_expr(seconds, now, ctx).as_float();
                let stmts = self.flatten_to_block(body);
                self.loop_stack.push(LoopState::ForSeconds {
                    end_time: now + secs as f64,
                    body: stmts,
                    pc: 0,
                });
            }
            Stmt::Exit => {
                self.state = ExecState::Yielded;
            }
            Stmt::Done => {
                if let Some(frame) = self.call_stack.pop() {
                    self.script = frame.script;
                    self.variables = frame.variables;
                    self.seq_pc = frame.seq_pc;
                    self.loop_stack = frame.loop_stack;
                } else {
                    self.state = ExecState::Done;
                }
            }
            Stmt::Home => {
                // Reset to beginning of sequence
                self.seq_pc = 0;
                self.loop_stack.clear();
            }
            Stmt::Log(exprs) => {
                let parts: Vec<String> = exprs.iter().map(|e| {
                    let v = self.eval_expr(e, now, ctx);
                    v.as_string()
                }).collect();
                info!("[ScrOni] {}", parts.join(" "));
            }

            // Blocking commands — set blocking action and yield
            Stmt::Idle(expr) => {
                let secs = self.eval_expr(expr, now, ctx).as_float();
                self.blocking = Some(BlockingAction::Idle { end_time: now + secs as f64 });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurvePhase { phase, seconds } => {
                let p = self.eval_expr(phase, now, ctx).as_float();
                let s = self.eval_expr(seconds, now, ctx).as_float();
                self.blocking = Some(BlockingAction::GotoCurvePhase { target: p, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurveKnot { knot, seconds } => {
                let k = self.eval_expr(knot, now, ctx).as_int();
                let s = self.eval_expr(seconds, now, ctx).as_float();
                self.blocking = Some(BlockingAction::GotoCurveKnot { knot: k, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurveLerp { lerp, seconds } => {
                let l = self.eval_expr(lerp, now, ctx).as_float();
                let s = self.eval_expr(seconds, now, ctx).as_float();
                self.blocking = Some(BlockingAction::GotoCurveLerp { target: l, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::Face { target, seconds } => {
                let t = self.eval_expr(target, now, ctx);
                let s = seconds.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                self.blocking = Some(BlockingAction::Face { target: t, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoPoint { target, within, speed } => {
                let t = self.eval_expr(target, now, ctx);
                let w = within.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                let s = speed.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                self.blocking = Some(BlockingAction::GotoPoint { target: t, within: w, speed: s });
                self.state = ExecState::Blocked;
            }
            Stmt::PlayAnimation { name, hold, rate } => {
                let n = self.eval_expr(name, now, ctx).as_string();
                let r = rate.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                self.blocking = Some(BlockingAction::PlayAnimation { name: n, hold: *hold, rate: r });
                self.state = ExecState::Blocked;
            }
            Stmt::Fight => {
                self.blocking = Some(BlockingAction::Fight);
                self.state = ExecState::Blocked;
            }
            Stmt::Shoot => {
                self.blocking = Some(BlockingAction::Shoot);
                self.state = ExecState::Blocked;
            }

            // Non-blocking curve commands — set variables for external systems to read
            Stmt::SetCurvePhase(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_phase".into(), v);
            }
            Stmt::SetCurveSpeed(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_speed".into(), v);
            }
            Stmt::SetCurveKs(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_ks".into(), v);
            }
            Stmt::SetCurvePingPong(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_pingpong".into(), v);
            }
            Stmt::SetCurve { name, at_phase } => {
                let n = self.eval_expr(name, now, ctx);
                self.variables.insert("__curve_name".into(), n);
                if let Some(p) = at_phase {
                    let v = self.eval_expr(p, now, ctx);
                    self.variables.insert("__curve_phase".into(), v);
                }
            }
            Stmt::SetLerpCurve(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__lerp_curve".into(), v);
            }
            Stmt::SetLookUpCurve(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__lookup_curve".into(), v);
            }
            Stmt::SetCurveLookAtActor(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_lookat".into(), v);
            }
            Stmt::SetCurveLookAlongDistance(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_lookalong_dist".into(), v);
            }
            Stmt::SetCurveLookAlongDirection(expr) => {
                let v = self.eval_expr(expr, now, ctx);
                self.variables.insert("__curve_lookalong_dir".into(), v);
            }

            Stmt::InlineVarDecl(decl) => {
                let val = if let Some(init) = &decl.initializer {
                    self.eval_expr(init, now, ctx)
                } else {
                    match decl.var_type {
                        VarType::Integer => Value::Int(0),
                        VarType::Float => Value::Float(0.0),
                        VarType::Vector => Value::Vector(Vec3::ZERO),
                        VarType::String => Value::String(String::new()),
                        _ => Value::None,
                    }
                };
                self.variables.insert(decl.name.clone(), val);
            }

            Stmt::Find { list_var, conditions, range } => {
                let eval_conds = conditions.iter().map(|(k, e)| (k.clone(), self.eval_expr(e, now, ctx))).collect();
                let eval_range = range.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                self.blocking = Some(BlockingAction::Find {
                    list_var: list_var.clone(),
                    conditions: eval_conds,
                    range: eval_range,
                });
                self.state = ExecState::Blocked;
            }

            Stmt::TextureMovie { name, pass: _, action, arg } => {
                let target_name = self.eval_expr(name, now, ctx).as_string();
                let arg_val = self.eval_expr(arg, now, ctx);
                self.sys_requests.push(SysRequest::TextureMovie {
                    target_name,
                    action: *action,
                    arg: arg_val,
                });
            }

            Stmt::SendMessage { msg, to, with } => {
                let msg_str = self.eval_expr(msg, now, ctx).as_string();
                let target = self.eval_expr(to, now, ctx);
                if let Value::Actor(entity) = target {
                    let mut args = Vec::new();
                    for a in with {
                        args.push(self.eval_expr(a, now, ctx));
                    }
                    self.outgoing_messages.push(ScriptMessage {
                        msg: msg_str,
                        from: self.owner,
                        to: entity,
                        args,
                    });
                }
            }
            Stmt::Teleport { target, to, face } => {
                if let Value::Actor(ent) = self.eval_expr(target, now, ctx) {
                    let to_vec = to.as_ref().map(|e| {
                        match self.eval_expr(e, now, ctx) {
                            Value::Vector(v) => v,
                            _ => Vec3::ZERO,
                        }
                    });
                    let face_float = face.as_ref().map(|e| self.eval_expr(e, now, ctx).as_float());
                    
                    self.sys_requests.push(SysRequest::Teleport {
                        target: ent,
                        to: to_vec,
                        face: face_float,
                    });
                }
            }

            Stmt::Spawn { script, assign_to, at, name } => {
                let script_str = self.eval_expr(script, now, ctx).as_string();
                let assign = assign_to.clone();
                let at_pos = at.as_ref().map(|e| {
                    match self.eval_expr(e, now, ctx) {
                        Value::Vector(v) => v,
                        _ => Vec3::ZERO,
                    }
                });
                let target_name = name.as_ref().map(|e| self.eval_expr(e, now, ctx).as_string());
                
                self.sys_requests.push(SysRequest::Spawn {
                    script: script_str,
                    assign_to: assign,
                    at: at_pos,
                    name: target_name,
                });
            }

            Stmt::Stack(name_expr) => {
                let name = self.eval_expr(name_expr, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&name).cloned() {
                    self.call_stack.push(CallFrame {
                        script: self.script.clone(),
                        variables: self.variables.clone(),
                        seq_pc: self.seq_pc,
                        loop_stack: self.loop_stack.clone(),
                    });
                    self.script = new_script;
                    self.variables.clear();
                    for var in &self.script.variables {
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
                        self.variables.insert(var.name.clone(), val);
                    }
                    self.seq_pc = 0;
                    self.loop_stack.clear();
                } else {
                    warn!("Stack: Script '{}' not found in available scripts.", name);
                }
            }

            Stmt::Switch(name_expr) => {
                let name = self.eval_expr(name_expr, now, ctx).as_string();
                if let Some(new_script) = self.available_scripts.get(&name).cloned() {
                    self.script = new_script;
                    self.variables.clear();
                    for var in &self.script.variables {
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
                        self.variables.insert(var.name.clone(), val);
                    }
                    self.seq_pc = 0;
                    self.loop_stack.clear();
                } else {
                    warn!("Switch: Script '{}' not found in available scripts.", name);
                }
            }

            Stmt::CameraSetPackage(expr) => {
                let pkg_name = self.eval_expr(expr, now, ctx).as_string();
                self.sys_requests.push(SysRequest::CameraSetPackage(pkg_name));
            }
            Stmt::At(x_expr, y_expr) => {
                let x = self.eval_expr(x_expr, now, ctx).as_float();
                let y = self.eval_expr(y_expr, now, ctx).as_float();
                self.sys_requests.push(SysRequest::At(x, y));
            }
            Stmt::DrawText(text_expr) => {
                let text = self.eval_expr(text_expr, now, ctx).as_string();
                self.sys_requests.push(SysRequest::DrawText(text));
            }
            Stmt::CameraReset => {
                info!("VM: CameraReset (unimplemented)");
            }
            Stmt::CameraMode(expr) => {
                let mode = self.eval_expr(expr, now, ctx).as_string();
                info!("VM: CameraMode {} (unimplemented)", mode);
            }
            Stmt::CameraLetterbox(expr) => {
                let b = self.eval_expr(expr, now, ctx).as_int();
                info!("VM: CameraLetterbox {} (unimplemented)", b);
            }

            Stmt::SetFogType(expr) => {
                let fog_type = self.eval_expr(expr, now, ctx).as_string();
                info!("VM: SetFogType {} (unimplemented)", fog_type);
            }
            Stmt::SetFogClamp { args } => {
                info!("VM: SetFogClamp {:?} (unimplemented)", args);
            }
            Stmt::SetFogPalettePower { args } => {
                info!("VM: SetFogPalettePower {:?} (unimplemented)", args);
            }
            Stmt::SetFullScreenColor { args } => {
                info!("VM: SetFullScreenColor {:?} (unimplemented)", args);
            }
            Stmt::SetUpdateState(expr) => {
                let state = self.eval_expr(expr, now, ctx).as_string();
                info!("VM: SetUpdateState {} (unimplemented)", state);
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

    fn eval_expr(&mut self, expr: &Expr, now: f64, ctx: &mut ScroniContext) -> Value {
        match expr {
            Expr::IntLit(i) => Value::Int(*i),
            Expr::FloatLit(f) => Value::Float(*f),
            Expr::StringLit(s) => Value::String(s.clone()),
            Expr::Var(name) => {
                self.variables.get(name).cloned().unwrap_or(Value::None)
            }
            Expr::Me => Value::Actor(self.owner),
            Expr::Player => Value::None, // resolved externally
            Expr::Paren(inner) => self.eval_expr(inner, now, ctx),
            Expr::Not(inner) => {
                let v = self.eval_expr(inner, now, ctx);
                Value::Int(if v.as_bool() { 0 } else { 1 })
            }
            Expr::Negate(inner) => {
                let v = self.eval_expr(inner, now, ctx);
                match v {
                    Value::Int(i) => Value::Int(-i),
                    Value::Float(f) => Value::Float(-f),
                    _ => Value::Int(0),
                }
            }
            Expr::VectorLit(x_expr, y_expr, z_expr) => {
                let x = self.eval_expr(x_expr, now, ctx).as_float();
                let y = self.eval_expr(y_expr, now, ctx).as_float();
                let z = self.eval_expr(z_expr, now, ctx).as_float();
                Value::Vector(Vec3::new(x, y, z))
            }
            Expr::FieldAccess { base, field } => {
                let base_val = self.eval_expr(base, now, ctx);
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
                let l = self.eval_expr(left, now, ctx);
                let r = self.eval_expr(right, now, ctx);
                eval_binop(*op, &l, &r)
            }
            Expr::Call { name, args } => {
                let lower = name.to_lowercase();
                match lower.as_str() {
                    "clock" => Value::Float(now as f32),
                    "random" => Value::Int(rand::random::<i32>().abs() % 100),
                    "randomrange" => Value::Int(0), // stub
                    "randomrangefloat" => Value::Float(0.0), // stub
                    "triggerentered" => {
                        let trig_ent = args.get(0).map(|e| self.eval_expr(e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(e, now, ctx));
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
                        let trig_ent = args.get(0).map(|e| self.eval_expr(e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(e, now, ctx));
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
                        let trig_ent = args.get(0).map(|e| self.eval_expr(e, now, ctx));
                        let targ_ent = args.get(1).map(|e| self.eval_expr(e, now, ctx));
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
                            let target_msg = self.eval_expr(msg_expr, now, ctx).as_string();
                            if let Some(idx) = self.message_queue.iter().position(|m| m.msg == target_msg) {
                                self.message_queue.remove(idx);
                                return Value::Int(1);
                            }
                        }
                        Value::Int(0)
                    }
                    "first" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, _)) = self.variables.get(list_name) {
                                let updated = entities.clone();
                                if let Some(&first_ent) = updated.first() {
                                    self.variables.insert(list_name.clone(), Value::ActorList(updated, 1));
                                    return Value::Actor(first_ent);
                                } else {
                                    self.variables.insert(list_name.clone(), Value::ActorList(updated, 0));
                                    return Value::None;
                                }
                            }
                        }
                        Value::None
                    }
                    "next" => {
                        if let Some(Expr::Var(list_name)) = args.get(0) {
                            if let Some(Value::ActorList(entities, idx)) = self.variables.get(list_name) {
                                let updated = entities.clone();
                                let current_idx = *idx;
                                if current_idx < updated.len() {
                                    let ent = updated[current_idx];
                                    self.variables.insert(list_name.clone(), Value::ActorList(updated, current_idx + 1));
                                    return Value::Actor(ent);
                                } else {
                                    self.variables.insert(list_name.clone(), Value::ActorList(updated, current_idx));
                                    return Value::None;
                                }
                            }
                        }
                        Value::None
                    }
                    "guid" => {
                        let entity_name = args.get(0)
                            .map(|e| self.eval_expr(e, now, ctx).as_string())
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
                    _ => Value::None,
                }
            }
            Expr::Exists(_) => Value::Int(1), // stub: assume exists
        }
    }

    /// Clear the current blocking action (called by external systems when behavior completes).
    pub fn clear_blocking(&mut self) {
        self.blocking = None;
        if self.state == ExecState::Blocked {
            self.state = ExecState::Running;
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
) {
    let now = time.elapsed_secs_f64();
    let mut all_messages = Vec::new();

    for (entity, mut script, transform) in &mut query {
        let mut ctx = ScroniContext {
            all_entities: &all_entities,
            triggers: &triggers,
        };
        script.exec.tick(now, &mut ctx);

        // Handle Find request
        if let Some(BlockingAction::Find { list_var, conditions, range }) = script.exec.blocking.clone() {
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

            script.exec.variables.insert(list_var, Value::ActorList(found, 0));
            script.exec.clear_blocking();
            // Tick again to resume immediately
            script.exec.tick(now, &mut ctx);
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
    query: Query<(Entity, &ScroniTextElement)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for (entity, text_element) in &query {
        if now > text_element.expires_at {
            commands.entity(entity).despawn();
        }
    }
}

/// Observer to handle ScrOni system requests (like TextureMovie)
pub fn scroni_sys_event_observer(
    trigger: On<ScrOniSysEvent>,
    mut commands: Commands,
    mut transform_query: Query<&mut Transform>,
    children_query: Query<&Children>,
    mut materials_query: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut skinned_mesh_ibp: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut texture_collections: ResMut<crate::oni2_loader::TextureCollections>,
    layout_context: Option<Res<crate::oni2_loader::LayoutContext>>,
    layout_paths: Option<Res<crate::oni2_loader::LayoutPaths>>,
    mut active_camera_package: Option<ResMut<crate::oni2_loader::ActiveCameraPackage>>,
    mut scroni_text_state: ResMut<ScroniTextState>,
    time: Res<Time>,
) {
    let ev = (*trigger).clone();
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
                info!("Changing active camera package from {} to {}", active_pkg.name, pkg_name);
                active_pkg.name = pkg_name;
            } else {
                warn!("CameraSetPackage called but no ActiveCameraPackage resource found.");
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
            if let Ok(mut transform) = transform_query.get_mut(target) {
                if let Some(pos) = to {
                    transform.translation = pos;
                }
                if let Some(angles_y) = face {
                    let rad = angles_y.to_radians();
                    let current_rot = transform.rotation.to_euler(EulerRot::YXZ);
                    transform.rotation = Quat::from_euler(EulerRot::YXZ, rad, current_rot.1, current_rot.2);
                }
            }
        }
    }
}
