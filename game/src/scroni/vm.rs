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
    /// Current blocking action waiting to complete.
    pub blocking: Option<BlockingAction>,
    /// Incoming message queue.
    pub message_queue: Vec<ScriptMessage>,
    /// The entity this script is attached to.
    pub owner: Entity,
    /// Game time when script started (for timed operations).
    pub start_time: f64,
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
                VarType::ActorList => Value::None,
            };
            variables.insert(var.name.clone(), val);
        }

        Self {
            script,
            variables,
            state: ExecState::Running,
            seq_pc: 0,
            loop_stack: Vec::new(),
            blocking: None,
            message_queue: Vec::new(),
            owner,
            start_time,
        }
    }

    /// Execute one frame's worth of the script. Returns the execution state.
    /// The whenever block runs first, then the sequence block resumes.
    pub fn tick(&mut self, now: f64) -> ExecState {
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
                self.exec_stmt(stmt, now);
                if self.state == ExecState::Yielded {
                    self.state = ExecState::Running; // reset for sequence
                    break;
                }
            }
        }

        // Run sequence block from current PC
        self.run_sequence(now);

        self.state
    }

    fn run_sequence(&mut self, now: f64) {
        // If we're inside a loop, continue that loop
        while !self.loop_stack.is_empty() {
            if self.state != ExecState::Running {
                return;
            }
            // Take the loop off the stack to avoid borrow conflicts
            let mut ls = self.loop_stack.pop().unwrap();
            let (active, push_back) = self.step_loop(&mut ls, now);
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
            self.exec_stmt(stmt, now);
        }

        // Fell off end of sequence
        if self.seq_pc >= sequence.len() && self.loop_stack.is_empty() && self.state == ExecState::Running {
            self.state = ExecState::Done;
        }
    }

    /// Step a loop. Returns (still_active, should_push_back).
    fn step_loop(&mut self, ls: &mut LoopState, now: f64) -> (bool, bool) {
        match ls {
            LoopState::Forever { body, pc } => {
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now);
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
                let cond_val = self.eval_expr(&cond, now);
                if !cond_val.as_bool() {
                    return (false, false); // loop done
                }
                while *pc < body.len() && self.state == ExecState::Running {
                    let stmt = body[*pc].clone();
                    *pc += 1;
                    self.exec_stmt(&stmt, now);
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
                    self.exec_stmt(&stmt, now);
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
                    self.exec_stmt(&stmt, now);
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
                    self.exec_stmt(&stmt, now);
                }
                if *pc >= stmts.len() {
                    return (false, false); // block done
                }
                (true, true)
            }
        }
    }

    fn exec_stmt(&mut self, stmt: &Stmt, now: f64) {
        if self.state != ExecState::Running {
            return;
        }

        match stmt {
            Stmt::Set { var, value } => {
                let val = self.eval_expr(value, now);
                self.variables.insert(var.clone(), val);
            }
            Stmt::If { condition, then_branch, else_branch } => {
                let cond = self.eval_expr(condition, now);
                if cond.as_bool() {
                    self.exec_stmt(then_branch, now);
                } else if let Some(else_b) = else_branch {
                    self.exec_stmt(else_b, now);
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
                let n = self.eval_expr(count, now).as_int();
                let stmts = self.flatten_to_block(body);
                self.loop_stack.push(LoopState::NTimes { remaining: n, body: stmts, pc: 0 });
            }
            Stmt::DoForSeconds { seconds, body } => {
                let secs = self.eval_expr(seconds, now).as_float();
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
                self.state = ExecState::Done;
            }
            Stmt::Home => {
                // Reset to beginning of sequence
                self.seq_pc = 0;
                self.loop_stack.clear();
            }
            Stmt::Log(exprs) => {
                let parts: Vec<String> = exprs.iter().map(|e| {
                    let v = self.eval_expr(e, now);
                    v.as_string()
                }).collect();
                info!("[ScrOni] {}", parts.join(" "));
            }

            // Blocking commands — set blocking action and yield
            Stmt::Idle(expr) => {
                let secs = self.eval_expr(expr, now).as_float();
                self.blocking = Some(BlockingAction::Idle { end_time: now + secs as f64 });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurvePhase { phase, seconds } => {
                let p = self.eval_expr(phase, now).as_float();
                let s = self.eval_expr(seconds, now).as_float();
                self.blocking = Some(BlockingAction::GotoCurvePhase { target: p, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurveKnot { knot, seconds } => {
                let k = self.eval_expr(knot, now).as_int();
                let s = self.eval_expr(seconds, now).as_float();
                self.blocking = Some(BlockingAction::GotoCurveKnot { knot: k, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoCurveLerp { lerp, seconds } => {
                let l = self.eval_expr(lerp, now).as_float();
                let s = self.eval_expr(seconds, now).as_float();
                self.blocking = Some(BlockingAction::GotoCurveLerp { target: l, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::Face { target, seconds } => {
                let t = self.eval_expr(target, now);
                let s = seconds.as_ref().map(|e| self.eval_expr(e, now).as_float());
                self.blocking = Some(BlockingAction::Face { target: t, seconds: s });
                self.state = ExecState::Blocked;
            }
            Stmt::GotoPoint { target, within, speed } => {
                let t = self.eval_expr(target, now);
                let w = within.as_ref().map(|e| self.eval_expr(e, now).as_float());
                let s = speed.as_ref().map(|e| self.eval_expr(e, now).as_float());
                self.blocking = Some(BlockingAction::GotoPoint { target: t, within: w, speed: s });
                self.state = ExecState::Blocked;
            }
            Stmt::PlayAnimation { name, hold, rate } => {
                let n = self.eval_expr(name, now).as_string();
                let r = rate.as_ref().map(|e| self.eval_expr(e, now).as_float());
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
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_phase".into(), v);
            }
            Stmt::SetCurveSpeed(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_speed".into(), v);
            }
            Stmt::SetCurveKs(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_ks".into(), v);
            }
            Stmt::SetCurvePingPong(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_pingpong".into(), v);
            }
            Stmt::SetCurve { name, at_phase } => {
                let n = self.eval_expr(name, now);
                self.variables.insert("__curve_name".into(), n);
                if let Some(p) = at_phase {
                    let v = self.eval_expr(p, now);
                    self.variables.insert("__curve_phase".into(), v);
                }
            }
            Stmt::SetLerpCurve(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__lerp_curve".into(), v);
            }
            Stmt::SetLookUpCurve(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__lookup_curve".into(), v);
            }
            Stmt::SetCurveLookAtActor(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_lookat".into(), v);
            }
            Stmt::SetCurveLookAlongDistance(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_lookalong_dist".into(), v);
            }
            Stmt::SetCurveLookAlongDirection(expr) => {
                let v = self.eval_expr(expr, now);
                self.variables.insert("__curve_lookalong_dir".into(), v);
            }

            Stmt::InlineVarDecl(decl) => {
                let val = if let Some(init) = &decl.initializer {
                    self.eval_expr(init, now)
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

            // Stubs for commands we don't execute yet
            _ => {
                // Silently ignore unimplemented commands for now
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

    fn eval_expr(&self, expr: &Expr, now: f64) -> Value {
        match expr {
            Expr::IntLit(i) => Value::Int(*i),
            Expr::FloatLit(f) => Value::Float(*f),
            Expr::StringLit(s) => Value::String(s.clone()),
            Expr::Var(name) => {
                self.variables.get(name).cloned().unwrap_or(Value::None)
            }
            Expr::Me => Value::Actor(self.owner),
            Expr::Player => Value::None, // resolved externally
            Expr::Paren(inner) => self.eval_expr(inner, now),
            Expr::Not(inner) => {
                let v = self.eval_expr(inner, now);
                Value::Int(if v.as_bool() { 0 } else { 1 })
            }
            Expr::Negate(inner) => {
                let v = self.eval_expr(inner, now);
                match v {
                    Value::Int(i) => Value::Int(-i),
                    Value::Float(f) => Value::Float(-f),
                    _ => Value::Int(0),
                }
            }
            Expr::BinOp { op, left, right } => {
                let l = self.eval_expr(left, now);
                let r = self.eval_expr(right, now);
                eval_binop(*op, &l, &r)
            }
            Expr::Call { name, args: _ } => {
                let lower = name.to_lowercase();
                match lower.as_str() {
                    "clock" => Value::Float(now as f32),
                    "random" => Value::Int(rand::random::<i32>().abs() % 100),
                    "randomrange" => Value::Int(0), // stub
                    "randomrangefloat" => Value::Float(0.0), // stub
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
    mut query: Query<(Entity, &mut ScrOniScript)>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for (_entity, mut script) in &mut query {
        script.exec.tick(now);
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
