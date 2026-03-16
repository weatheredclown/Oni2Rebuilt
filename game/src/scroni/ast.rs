/// A complete .oni file can contain `uses` declarations and multiple scripts.
#[derive(Debug, Clone)]
pub struct ScriptFile {
    pub uses: Vec<String>,
    pub scripts: Vec<ScriptDef>,
}

/// A single `Script <name> begin ... end` definition.
#[derive(Debug, Clone)]
pub struct ScriptDef {
    pub name: String,
    pub variables: Vec<VarDecl>,
    pub whenever: Option<Block>,
    pub sequence: Block,
}

/// Variable declaration: `integer x`, `float speed`, `vector pos`, etc.
#[derive(Debug, Clone)]
pub struct VarDecl {
    pub var_type: VarType,
    pub name: String,
    pub initializer: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Integer,
    Float,
    Vector,
    String,
    Timer,
    Label,
    ActorList,
    Child,
}

/// A block is a list of statements (commands).
pub type Block = Vec<Stmt>;

/// A statement (command) in ScrOni.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// `set <var> to <expr>`
    Set { var: String, value: Expr },
    /// `if <expr> then <stmt> [else <stmt>]`
    If { condition: Expr, then_branch: Box<Stmt>, else_branch: Option<Box<Stmt>> },
    /// `begin ... end` block
    Block(Block),
    /// `do forever <stmt>`
    DoForever(Box<Stmt>),
    /// `do while <expr> <stmt>`
    DoWhile { condition: Expr, body: Box<Stmt> },
    /// `do <expr> times <stmt>`
    DoNTimes { count: Expr, body: Box<Stmt> },
    /// `do for <expr> seconds <stmt>`
    DoForSeconds { seconds: Expr, body: Box<Stmt> },
    /// `exit`
    Exit,
    /// `done`
    Done,
    /// `home`
    Home,
    /// `log <expr>, ...`
    Log(Vec<Expr>),

    // --- Blocking commands (sequence only) ---
    /// `idle <expr>`
    Idle(Expr),
    /// `GotoCurvePhase <expr> in <expr>`
    GotoCurvePhase { phase: Expr, seconds: Expr },
    /// `GotoCurveKnot <expr> in <expr>`
    GotoCurveKnot { knot: Expr, seconds: Expr },
    /// `GotoCurveLerp <expr> in <expr>`
    GotoCurveLerp { lerp: Expr, seconds: Expr },
    /// `SetCurvePhase <expr>`
    SetCurvePhase(Expr),
    /// `SetCurveSpeed <expr>`
    SetCurveSpeed(Expr),
    /// `SetCurveKs <expr>`
    SetCurveKs(Expr),
    /// `SetCurvePingPong <expr>`
    SetCurvePingPong(Expr),
    /// `SetCurve <string> at <expr>`
    SetCurve { name: Expr, at_phase: Option<Expr> },
    /// `SetLerpCurve <string>`
    SetLerpCurve(Expr),
    /// `SetLookUpCurve <string>`
    SetLookUpCurve(Expr),
    /// `SetCurveLookAtActor (<expr>)`
    SetCurveLookAtActor(Expr),
    /// `SetCurveLookAlongDistance <expr>`
    SetCurveLookAlongDistance(Expr),
    /// `SetCurveLookAlongDirection <expr>`
    SetCurveLookAlongDirection(Expr),

    /// `PlayAnimation <string> [hold] [rate <expr>] [for <expr>]`
    PlayAnimation { name: Expr, hold: bool, rate: Option<Expr>, duration: Option<Expr> },
    /// `PlayActionAnimation <string> [hold] [for <expr>]`
    PlayActionAnimation { name: Expr, hold: bool, duration: Option<Expr> },
    /// `ControlAnimation <string> ...`
    ControlAnimation { name: Expr },

    /// `face <expr> [in <expr>]`
    Face { target: Expr, seconds: Option<Expr> },
    /// `goto <expr> [within <expr>] [speed <expr>] [for <expr>]`
    GotoPoint { target: Expr, within: Option<Expr>, speed: Option<Expr>, duration: Option<Expr> },
    /// `fight`
    Fight,
    /// `shoot`
    Shoot,
    /// `patrol <expr>`
    Patrol(Expr),
    /// `follow <expr>`
    Follow(Expr),
    /// `attack <expr>`
    Attack(Expr),
    /// `retreat`
    Retreat,

    /// `stack <string>`
    Stack(Expr),
    /// `switch <string>`
    Switch(Expr),
    /// `childstack <string>`
    ChildStack(Expr),
    /// `childswitch <string>`
    ChildSwitch(Expr),
    /// `childdone`
    ChildDone,
    /// `childhome`
    ChildHome,
    /// `childstop`
    ChildStop,

    /// `spawn <string> [assign to <var>] [at <expr>] [name <string>]`
    Spawn { script: Expr, assign_to: Option<String>, at: Option<Expr>, name: Option<Expr> },
    /// `destroy`
    Destroy,
    /// `teleport <expr> [to <vector>] [face <expr>]`
    Teleport { target: Expr, to: Option<Expr>, face: Option<Expr> },

    /// `sendmessage <string> to <expr> [with <expr>, ...]`
    SendMessage { msg: Expr, to: Expr, with: Vec<Expr> },
    /// `sendgroupmessage <string> to <expr>`
    SendGroupMessage { msg: Expr, to: Expr },
    /// `sendgroupmembersmessage <string> to <expr>`
    SendGroupMembersMessage { msg: Expr, to: Expr },

    /// `find <var> [conditions...] range <expr>`
    Find { list_var: String, conditions: Vec<(String, Expr)>, range: Option<Expr> },

    /// `TextureMovie <string> [pass <expr>] <action> <expr>`
    TextureMovie { name: Expr, pass: Option<Expr>, action: TextureMovieAction, arg: Expr },

    /// `SetHealth <expr>`
    SetHealth(Expr),
    /// `ResetHealth`
    ResetHealth,
    /// `SetActorEnabled <expr>`
    SetActorEnabled(Expr),
    /// `SetFaction <expr>`
    SetFaction(Expr),
    /// `SetUnbreakable <expr>`
    SetUnbreakable(Expr),
    /// `SetCrouch <expr>`
    SetCrouch(Expr),
    /// `SetAttackTable <expr>`
    SetAttackTable(Expr),

    /// `DrawWeapon`
    DrawWeapon,
    /// `HolsterWeapon`
    HolsterWeapon,

    /// Camera commands
    CameraReset,
    CameraMode(Expr),
    CameraLetterbox(Expr),
    CameraFollowActor(Expr),
    CameraTrackActor(Expr),
    CameraTrackPoint(Expr),
    CameraMoveToActor { actor: Expr, seconds: Option<Expr> },
    CameraMoveToPoint { point: Expr, seconds: Option<Expr> },
    CameraCutToActor(Expr),
    CameraCutToPoint(Expr),
    CameraSetFOV(Expr),
    CameraSetPackage(Expr),
    CameraShake,

    /// Sound commands
    Sound(Expr),
    PlayAmbientSound { name: Expr, volume: Option<Expr> },
    AmbientSound { args: Vec<Expr> },
    MusicPlay(Expr),
    MusicStop,

    /// Fog commands
    SetFogType(Expr),
    SetFogRange { min: Expr, max: Expr },
    SetFogColor { r: Expr, g: Expr, b: Expr, a: Expr },
    SetFogClamp { args: Vec<Expr> },
    SetFogPalettePower { args: Vec<Expr> },
    
    /// Global Screen commands
    SetFullScreenColor { args: Vec<Expr> },
    SetUpdateState(Expr),

    /// HUD
    SetHud { args: Vec<Expr> },

    /// `ControlHead <keyword> [<expr>]`
    ControlHead { args: Vec<Expr> },

    /// `add <expr> to <var>`
    AddToList { expr: Expr, list: String },

    /// Inline variable declaration: `integer x = <expr>`
    InlineVarDecl(VarDecl),

    /// `at <expr> <expr>`
    At(Expr, Expr),
    /// `drawtext <expr>`
    DrawText(Expr),

    /// Catch-all for commands we parse but don't fully implement yet.
    /// Stores the command name and any trailing arguments.
    Unimplemented { command: String, args: Vec<Expr> },
}

/// Expression in ScrOni.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer literal
    IntLit(i32),
    /// Float literal
    FloatLit(f32),
    /// String literal
    StringLit(String),
    /// Variable reference
    Var(String),
    /// `me` (self-reference)
    Me,
    /// `(player)` - player actor reference
    Player,
    /// Binary op: `<expr> <op> <expr>`
    BinOp { op: BinOp, left: Box<Expr>, right: Box<Expr> },
    /// Unary not: `not <expr>`
    Not(Box<Expr>),
    /// Unary negate: `-<expr>`
    Negate(Box<Expr>),
    /// Function call / query: `distance(me, player)`, `location(me)`, etc.
    Call { name: String, args: Vec<Expr> },
    /// `exists()` or `exists(<expr>)`
    Exists(Option<Box<Expr>>),
    /// Parenthesized expression
    Paren(Box<Expr>),
    /// Vector literal: `{ x, y, z }`
    VectorLit(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Field access: `<expr>.<field>` (e.g., `loc.x`)
    FieldAccess { base: Box<Expr>, field: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Mod,
    Div,
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    And,
    Or,
    Dot,   // dot product
    Cross, // cross product
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureMovieAction {
    SetFrame,
    SetRate,
}
