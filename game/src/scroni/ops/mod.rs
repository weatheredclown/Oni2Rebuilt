/*
 * scroni/ops/mod.rs — scripting opcode dispatch and shared OpsCtx.
 *
 * OpsCtx bundles the mutable ScriptExec, thread id, timestamp, and
 * ScroniContext needed by all opcode handlers.  Each sub-module (animation,
 * audio, camera, combat, core, movement) implements an exec(ctx, stmt) → bool
 * function dispatching the statement variants it owns.
 */
use crate::scroni::ast::Expr;
use crate::scroni::vm::{ScriptExec, ScroniContext, Value, SysRequest, ScrOniThread, BlockingAction, ExecState};

pub mod animation;
pub mod audio;
pub mod camera;
pub mod combat;
pub mod core;
pub mod movement;

pub struct OpsCtx<'a, 'b, 'w_e, 's_e, 'w_t, 's_t> {
    pub exec: &'a mut ScriptExec,
    pub tid: u32,
    pub now: f64,
    pub ctx: &'a mut ScroniContext<'b, 'w_e, 's_e, 'w_t, 's_t>,
}

impl<'a, 'b, 'w_e, 's_e, 'w_t, 's_t> OpsCtx<'a, 'b, 'w_e, 's_e, 'w_t, 's_t> {
    pub fn eval(&mut self, expr: &Expr) -> Value {
        self.exec.eval_expr(self.tid, expr, self.now, self.ctx)
    }

    pub fn eval_string(&mut self, expr: &Expr) -> String {
        self.eval(expr).as_string()
    }

    pub fn eval_float(&mut self, expr: &Expr) -> f32 {
        self.eval(expr).as_float()
    }

    pub fn eval_int(&mut self, expr: &Expr) -> i32 {
        self.eval(expr).as_int()
    }

    pub fn eval_bool(&mut self, expr: &Expr) -> bool {
        self.eval(expr).as_bool()
    }

    pub fn sys_request(&mut self, req: SysRequest) {
        self.exec.sys_requests.push(req);
    }

    pub fn thread(&self) -> &ScrOniThread {
        self.exec.get_thread(self.tid)
    }

    pub fn thread_mut(&mut self) -> &mut ScrOniThread {
        self.exec.get_thread_mut(self.tid)
    }

    pub fn block(&mut self, action: BlockingAction) {
        let t = self.thread_mut();
        t.blocking = Some(action);
        t.state = ExecState::Blocked;
    }

    pub fn yield_thread(&mut self) {
        self.thread_mut().state = ExecState::Yielded;
    }

    pub fn set_var(&mut self, name: String, val: Value) {
        self.exec.set_var(self.tid, name, val);
    }

    pub fn get_var(&self, name: &str) -> Value {
        self.exec.get_var(self.tid, name)
    }
}
