use super::ast::*;
use super::token::{Token, TokenCode};
use super::tokenizer::Tokenizer;

/// Compile errors with source location.
#[derive(Debug)]
pub struct CompileError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}:{}: {}", self.line, self.col, self.message)
    }
}

/// Parser state: walks the token stream and builds an AST.
pub struct Compiler {
    tokens: Vec<Token>,
    pos: usize,
    pub errors: Vec<CompileError>,
}

impl Compiler {
    /// Parse a ScrOni source file and return the AST.
    pub fn compile(source: &str) -> Result<ScriptFile, Vec<CompileError>> {
        let mut tokenizer = Tokenizer::new(source);
        let tokens = tokenizer.tokenize();
        let mut compiler = Compiler { tokens, pos: 0, errors: Vec::new() };
        let file = compiler.parse_file();
        if compiler.errors.is_empty() {
            Ok(file)
        } else {
            Err(compiler.errors)
        }
    }

    // ---- Helpers ----

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&self.tokens[self.tokens.len() - 1])
    }

    fn code(&self) -> TokenCode {
        self.peek().code
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: TokenCode) -> bool {
        if self.code() == expected {
            self.advance();
            true
        } else {
            self.error(format!("expected {:?}, found {:?} '{}'", expected, self.code(), self.peek().text));
            false
        }
    }

    fn skip_if(&mut self, code: TokenCode) -> bool {
        if self.code() == code {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&mut self, msg: String) {
        let tok = self.peek();
        self.errors.push(CompileError { line: tok.line, col: tok.col, message: msg });
    }

    fn at_end(&self) -> bool {
        self.code() == TokenCode::Eof
    }

    // ---- File-level parsing ----

    fn parse_file(&mut self) -> ScriptFile {
        let mut uses = Vec::new();
        let mut scripts = Vec::new();

        while !self.at_end() {
            match self.code() {
                TokenCode::Uses => {
                    self.advance();
                    if self.code() == TokenCode::StringConstant {
                        uses.push(self.peek().text.clone());
                        self.advance();
                    } else {
                        self.error("expected string after 'uses'".into());
                    }
                }
                TokenCode::Script => {
                    if let Some(s) = self.parse_script_def() {
                        scripts.push(s);
                    }
                }
                _ => {
                    self.error(format!("unexpected token {:?} at file level", self.code()));
                    self.advance();
                }
            }
        }

        ScriptFile { uses, scripts }
    }

    fn parse_script_def(&mut self) -> Option<ScriptDef> {
        self.advance(); // skip 'Script'
        let name = if self.code() == TokenCode::Identifier {
            let n = self.peek().text.clone();
            self.advance();
            n
        } else {
            self.error("expected script name".into());
            return None;
        };

        if !self.expect(TokenCode::Begin) {
            return None;
        }

        // Optional variable section
        let mut variables = Vec::new();
        if self.code() == TokenCode::Variable {
            self.advance();
            while self.code() != TokenCode::Whenever
                && self.code() != TokenCode::Sequence
                && self.code() != TokenCode::End
                && !self.at_end()
                && is_var_type(self.code())
            {
                if let Some(v) = self.parse_var_decl() {
                    variables.push(v);
                }
            }
        }

        // Optional whenever block
        let whenever = if self.code() == TokenCode::Whenever {
            self.advance();
            Some(self.parse_section_until(&[TokenCode::Sequence, TokenCode::End]))
        } else {
            None
        };

        // Sequence block (may or may not have explicit keyword)
        let sequence = if self.code() == TokenCode::Sequence {
            self.advance();
            self.parse_section_until(&[TokenCode::End])
        } else {
            // Everything until 'end' is the sequence
            self.parse_section_until(&[TokenCode::End])
        };

        self.skip_if(TokenCode::End); // skip 'end'

        Some(ScriptDef { name, variables, whenever, sequence })
    }

    fn parse_var_decl(&mut self) -> Option<VarDecl> {
        let var_type = match self.code() {
            TokenCode::Integer => VarType::Integer,
            TokenCode::Float => VarType::Float,
            TokenCode::Vector => VarType::Vector,
            TokenCode::StringKw => VarType::String,
            TokenCode::Timer => VarType::Timer,
            TokenCode::Label => VarType::Label,
            TokenCode::ActorList => VarType::ActorList,
            _ => {
                self.error(format!("expected type keyword, found {:?}", self.code()));
                return None;
            }
        };
        self.advance();

        // Variable names can be identifiers or keywords used as names (e.g. "speed")
        let name = if self.code() == TokenCode::Identifier || self.code() == TokenCode::Eof {
            let n = self.peek().text.clone();
            self.advance();
            n
        } else if !matches!(self.code(),
            TokenCode::IntegerConstant | TokenCode::FloatConstant
            | TokenCode::StringConstant | TokenCode::Eof
            | TokenCode::Begin | TokenCode::End | TokenCode::Whenever | TokenCode::Sequence
        ) {
            // Accept keyword tokens as variable names (ScrOni allows this)
            let n = self.peek().text.clone();
            self.advance();
            n
        } else {
            self.error("expected variable name".into());
            return None;
        };

        // Optional initializer: `= <expr>`
        let initializer = if self.code() == TokenCode::Equal {
            self.advance();
            Some(self.parse_expr())
        } else {
            None
        };

        Some(VarDecl { var_type, name, initializer })
    }

    // ---- Section / block parsing ----

    fn parse_section_until(&mut self, end_tokens: &[TokenCode]) -> Block {
        let mut stmts = Vec::new();
        while !end_tokens.contains(&self.code()) && !self.at_end() {
            stmts.push(self.parse_stmt());
        }
        stmts
    }

    fn parse_block(&mut self) -> Block {
        if !self.expect(TokenCode::Begin) {
            return Vec::new();
        }
        let mut stmts = Vec::new();
        while self.code() != TokenCode::End && !self.at_end() {
            stmts.push(self.parse_stmt());
        }
        self.skip_if(TokenCode::End);
        stmts
    }

    // ---- Statement parsing ----

    fn parse_stmt(&mut self) -> Stmt {
        match self.code() {
            TokenCode::Set => self.parse_set(),
            TokenCode::If => self.parse_if(),
            TokenCode::Begin => {
                let block = self.parse_block();
                Stmt::Block(block)
            }
            TokenCode::Do => self.parse_do(),
            TokenCode::Exit => { self.advance(); Stmt::Exit }
            TokenCode::Done => { self.advance(); Stmt::Done }
            TokenCode::Home => { self.advance(); Stmt::Home }
            TokenCode::Log => self.parse_log(),

            // Inline variable declarations
            TokenCode::Integer | TokenCode::Float | TokenCode::Vector
            | TokenCode::StringKw | TokenCode::Timer | TokenCode::Label
            | TokenCode::ActorList => {
                if let Some(v) = self.parse_var_decl() {
                    Stmt::InlineVarDecl(v)
                } else {
                    Stmt::Unimplemented { command: "var_decl".into(), args: vec![] }
                }
            }

            // Curve commands
            TokenCode::GotoCurvePhase => self.parse_goto_curve_phase(),
            TokenCode::GotoCurveKnot => self.parse_goto_curve_knot(),
            TokenCode::GotoCurveLerp => self.parse_goto_curve_lerp(),
            TokenCode::SetCurvePhase => { self.advance(); Stmt::SetCurvePhase(self.parse_expr()) }
            TokenCode::SetCurveSpeed => { self.advance(); Stmt::SetCurveSpeed(self.parse_expr()) }
            TokenCode::SetCurveKs => { self.advance(); Stmt::SetCurveKs(self.parse_expr()) }
            TokenCode::SetCurvePingPong => { self.advance(); Stmt::SetCurvePingPong(self.parse_expr()) }
            TokenCode::SetCurve => self.parse_set_curve(),
            TokenCode::SetLerpCurve => { self.advance(); Stmt::SetLerpCurve(self.parse_expr()) }
            TokenCode::SetLookUpCurve => { self.advance(); Stmt::SetLookUpCurve(self.parse_expr()) }
            TokenCode::SetCurveLookAtActor => { self.advance(); Stmt::SetCurveLookAtActor(self.parse_expr()) }
            TokenCode::SetCurveLookAlongDistance => { self.advance(); Stmt::SetCurveLookAlongDistance(self.parse_expr()) }
            TokenCode::SetCurveLookAlongDirection => { self.advance(); Stmt::SetCurveLookAlongDirection(self.parse_expr()) }

            // Animation
            TokenCode::PlayAnimation => self.parse_play_animation(),
            TokenCode::PlayActionAnimation => self.parse_play_action_animation(),
            TokenCode::ControlAnimation => { self.advance(); Stmt::ControlAnimation { name: self.parse_expr() } }

            // Movement / combat (blocking)
            TokenCode::Idle => { self.advance(); Stmt::Idle(self.parse_expr()) }
            TokenCode::Face => self.parse_face(),
            TokenCode::Goto => self.parse_goto(),
            TokenCode::Fight => { self.advance(); Stmt::Fight }
            TokenCode::Shoot => { self.advance(); Stmt::Shoot }
            TokenCode::Patrol => { self.advance(); Stmt::Patrol(self.parse_expr()) }
            TokenCode::Follow => { self.advance(); Stmt::Follow(self.parse_expr()) }
            TokenCode::Attack => { self.advance(); Stmt::Attack(self.parse_expr()) }
            TokenCode::Retreat => { self.advance(); Stmt::Retreat }

            // Script flow
            TokenCode::Stack => { self.advance(); Stmt::Stack(self.parse_expr()) }
            TokenCode::Switch => { self.advance(); Stmt::Switch(self.parse_expr()) }
            TokenCode::ChildStack => { self.advance(); Stmt::ChildStack(self.parse_expr()) }
            TokenCode::ChildSwitch => { self.advance(); Stmt::ChildSwitch(self.parse_expr()) }
            TokenCode::ChildDone => { self.advance(); Stmt::ChildDone }
            TokenCode::ChildHome => { self.advance(); Stmt::ChildHome }
            TokenCode::ChildStop => { self.advance(); Stmt::ChildStop }

            // Actor management
            TokenCode::Spawn => self.parse_spawn(),
            TokenCode::Destroy => { self.advance(); Stmt::Destroy }
            TokenCode::Teleport => self.parse_teleport(),

            // Messaging
            TokenCode::SendMessage => self.parse_send_message(),
            TokenCode::SendGroupMessage => self.parse_send_group_message(),
            TokenCode::SendGroupMembersMessage => self.parse_send_group_members_message(),

            // Properties
            TokenCode::SetHealth => { self.advance(); Stmt::SetHealth(self.parse_expr()) }
            TokenCode::ResetHealth => { self.advance(); Stmt::ResetHealth }
            TokenCode::SetActorEnabled => { self.advance(); Stmt::SetActorEnabled(self.parse_expr()) }
            TokenCode::SetFaction => { self.advance(); Stmt::SetFaction(self.parse_expr()) }
            TokenCode::SetUnbreakable => { self.advance(); Stmt::SetUnbreakable(self.parse_expr()) }
            TokenCode::SetCrouch => { self.advance(); Stmt::SetCrouch(self.parse_expr()) }
            TokenCode::SetAttackTable => { self.advance(); Stmt::SetAttackTable(self.parse_expr()) }
            TokenCode::DrawWeapon => { self.advance(); Stmt::DrawWeapon }
            TokenCode::HolsterWeapon => { self.advance(); Stmt::HolsterWeapon }

            // Camera
            TokenCode::CameraReset => { self.advance(); Stmt::CameraReset }
            TokenCode::CameraMode => { self.advance(); Stmt::CameraMode(self.parse_expr()) }
            TokenCode::CameraLetterbox => { self.advance(); Stmt::CameraLetterbox(self.parse_expr()) }
            TokenCode::CameraFollowActor => { self.advance(); Stmt::CameraFollowActor(self.parse_expr()) }
            TokenCode::CameraTrackActor => { self.advance(); Stmt::CameraTrackActor(self.parse_expr()) }
            TokenCode::CameraTrackPoint => { self.advance(); Stmt::CameraTrackPoint(self.parse_expr()) }
            TokenCode::CameraSetFOV => { self.advance(); Stmt::CameraSetFOV(self.parse_expr()) }
            TokenCode::CameraSetPackage => { self.advance(); Stmt::CameraSetPackage(self.parse_expr()) }
            TokenCode::CameraShake => { self.advance(); Stmt::CameraShake }

            // Sound
            TokenCode::Sound => { self.advance(); Stmt::Sound(self.parse_expr()) }
            TokenCode::PlayAmbientSound => self.parse_play_ambient_sound(),
            TokenCode::MusicPlay => { self.advance(); Stmt::MusicPlay(self.parse_expr()) }
            TokenCode::MusicStop => { self.advance(); Stmt::MusicStop }

            // Fog
            TokenCode::SetFogType => { self.advance(); Stmt::SetFogType(self.parse_expr()) }

            TokenCode::Find => self.parse_find(),
            TokenCode::TextureMovie => self.parse_texture_movie(),

            _ => {
                // Unknown command — skip token and collect trailing exprs until next command
                let cmd = self.peek().text.clone();
                self.advance();
                let mut args = Vec::new();
                while is_expr_start(self.code()) {
                    args.push(self.parse_expr());
                    self.skip_if(TokenCode::Comma);
                }
                Stmt::Unimplemented { command: cmd, args }
            }
        }
    }

    // ---- Specific statement parsers ----

    fn parse_set(&mut self) -> Stmt {
        self.advance(); // skip 'set'
        let var = self.peek().text.clone();
        self.advance(); // skip identifier
        self.skip_if(TokenCode::To); // skip 'to'
        let value = self.parse_expr();
        Stmt::Set { var, value }
    }

    fn parse_if(&mut self) -> Stmt {
        self.advance(); // skip 'if'
        let condition = self.parse_expr();
        self.skip_if(TokenCode::Then);
        let then_branch = Box::new(self.parse_stmt());
        let else_branch = if self.skip_if(TokenCode::Else) {
            Some(Box::new(self.parse_stmt()))
        } else {
            None
        };
        Stmt::If { condition, then_branch, else_branch }
    }

    fn parse_do(&mut self) -> Stmt {
        self.advance(); // skip 'do'
        match self.code() {
            TokenCode::Forever => {
                self.advance();
                let body = Box::new(self.parse_stmt());
                Stmt::DoForever(body)
            }
            TokenCode::While => {
                self.advance();
                let condition = self.parse_expr();
                let body = Box::new(self.parse_stmt());
                Stmt::DoWhile { condition, body }
            }
            TokenCode::For => {
                self.advance();
                let seconds = self.parse_expr();
                self.skip_if(TokenCode::Seconds);
                let body = Box::new(self.parse_stmt());
                Stmt::DoForSeconds { seconds, body }
            }
            _ => {
                let count = self.parse_expr();
                self.skip_if(TokenCode::Times);
                let body = Box::new(self.parse_stmt());
                Stmt::DoNTimes { count, body }
            }
        }
    }

    fn parse_log(&mut self) -> Stmt {
        self.advance(); // skip 'log'
        let mut exprs = vec![self.parse_expr()];
        while self.code() == TokenCode::Comma {
            self.advance();
            exprs.push(self.parse_expr());
        }
        Stmt::Log(exprs)
    }

    fn parse_goto_curve_phase(&mut self) -> Stmt {
        self.advance(); // skip 'GotoCurvePhase'
        let phase = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) {
            self.parse_expr()
        } else {
            Expr::FloatLit(1.0)
        };
        Stmt::GotoCurvePhase { phase, seconds }
    }

    fn parse_goto_curve_knot(&mut self) -> Stmt {
        self.advance();
        let knot = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) { self.parse_expr() } else { Expr::FloatLit(1.0) };
        Stmt::GotoCurveKnot { knot, seconds }
    }

    fn parse_goto_curve_lerp(&mut self) -> Stmt {
        self.advance();
        let lerp = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) { self.parse_expr() } else { Expr::FloatLit(1.0) };
        Stmt::GotoCurveLerp { lerp, seconds }
    }

    fn parse_set_curve(&mut self) -> Stmt {
        self.advance(); // skip 'SetCurve'
        let name = self.parse_expr();
        let at_phase = if self.skip_if(TokenCode::At) {
            Some(self.parse_expr())
        } else {
            None
        };
        Stmt::SetCurve { name, at_phase }
    }

    fn parse_play_animation(&mut self) -> Stmt {
        self.advance();
        let name = self.parse_expr();
        let hold = self.skip_if(TokenCode::Hold);
        let rate = if self.skip_if(TokenCode::Rate) { Some(self.parse_expr()) } else { None };
        Stmt::PlayAnimation { name, hold, rate }
    }

    fn parse_play_action_animation(&mut self) -> Stmt {
        self.advance();
        let name = self.parse_expr();
        let hold = self.skip_if(TokenCode::Hold);
        Stmt::PlayActionAnimation { name, hold }
    }

    fn parse_face(&mut self) -> Stmt {
        self.advance();
        let target = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) { Some(self.parse_expr()) } else { None };
        Stmt::Face { target, seconds }
    }

    fn parse_goto(&mut self) -> Stmt {
        self.advance();
        let target = self.parse_expr();
        let within = if self.skip_if(TokenCode::Within) { Some(self.parse_expr()) } else { None };
        let speed = if self.skip_if(TokenCode::Speed) { Some(self.parse_expr()) } else { None };
        Stmt::GotoPoint { target, within, speed }
    }

    fn parse_spawn(&mut self) -> Stmt {
        self.advance();
        let script = self.parse_expr();
        let assign_to = if self.skip_if(TokenCode::Assign) {
            self.skip_if(TokenCode::To);
            let name = self.peek().text.clone();
            self.advance();
            Some(name)
        } else {
            None
        };
        let at = if self.skip_if(TokenCode::At) { Some(self.parse_expr()) } else { None };
        let name = if self.skip_if(TokenCode::Name) { Some(self.parse_expr()) } else { None };
        Stmt::Spawn { script, assign_to, at, name }
    }

    fn parse_teleport(&mut self) -> Stmt {
        self.advance(); // consume `teleport`
        let target = self.parse_expr();
        let mut to = None;
        let mut face = None;

        loop {
            if self.code() == TokenCode::To {
                self.advance();
                to = Some(self.parse_expr());
            } else if self.code() == TokenCode::Face {
                self.advance();
                face = Some(self.parse_expr());
            } else {
                break;
            }
        }

        Stmt::Teleport { target, to, face }
    }

    fn parse_send_message(&mut self) -> Stmt {
        self.advance();
        let msg = self.parse_expr();
        self.skip_if(TokenCode::To);
        let to = self.parse_expr();
        let mut with = Vec::new();
        if self.skip_if(TokenCode::With) {
            with.push(self.parse_expr());
            while self.skip_if(TokenCode::Comma) {
                with.push(self.parse_expr());
            }
        }
        Stmt::SendMessage { msg, to, with }
    }

    fn parse_send_group_message(&mut self) -> Stmt {
        self.advance();
        let msg = self.parse_expr();
        self.skip_if(TokenCode::To);
        let to = self.parse_expr();
        Stmt::SendGroupMessage { msg, to }
    }

    fn parse_send_group_members_message(&mut self) -> Stmt {
        self.advance();
        let msg = self.parse_expr();
        self.skip_if(TokenCode::To);
        let to = self.parse_expr();
        Stmt::SendGroupMembersMessage { msg, to }
    }

    fn parse_play_ambient_sound(&mut self) -> Stmt {
        self.advance();
        // playambientsound("...")  or  playambientsound(<expr>) volume(<expr>)
        let name = self.parse_expr();
        let volume = if self.skip_if(TokenCode::Volume) {
            // might have parens
            if self.skip_if(TokenCode::LeftParen) {
                let v = self.parse_expr();
                self.skip_if(TokenCode::RightParen);
                Some(v)
            } else {
                Some(self.parse_expr())
            }
        } else {
            None
        };
        Stmt::PlayAmbientSound { name, volume }
    }

    fn parse_find(&mut self) -> Stmt {
        self.advance(); // skip 'find'
        let list_var = self.peek().text.clone();
        self.advance();

        let mut conditions = Vec::new();
        let mut range = None;

        while self.code() != TokenCode::Range && !self.at_end() && !is_command_start(self.code()) {
            let key = self.peek().text.clone();
            self.advance(); // e.g. 'status' or 'name' or 'group'
            
            // Sometimes there's an 'is' or '=' between key and value
            if self.code() == TokenCode::Is || self.code() == TokenCode::Equal {
                self.advance();
            }

            let val = self.parse_expr();
            conditions.push((key, val));
            
            // Optional comma between conditions
            self.skip_if(TokenCode::Comma);
        }

        if self.skip_if(TokenCode::Range) {
            range = Some(self.parse_expr());
        }

        Stmt::Find { list_var, conditions, range }
    }

    fn parse_texture_movie(&mut self) -> Stmt {
        self.advance(); // skip 'TextureMovie'
        let name = self.parse_expr();

        let pass = if self.skip_if(TokenCode::Pass) {
            Some(self.parse_expr())
        } else {
            None
        };

        let action = if self.skip_if(TokenCode::SetFrame) {
            TextureMovieAction::SetFrame
        } else if self.skip_if(TokenCode::SetRate) {
            TextureMovieAction::SetRate
        } else {
            // Default to SetFrame if omitted but an arg follows, or error
            self.error("Expected SetFrame or SetRate after TextureMovie".into());
            TextureMovieAction::SetFrame
        };

        let arg = self.parse_expr();

        Stmt::TextureMovie { name, pass, action, arg }
    }

    // ---- Expression parsing (Pratt-style precedence climbing) ----

    fn parse_expr(&mut self) -> Expr {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Expr {
        let mut left = self.parse_and();
        while self.code() == TokenCode::Or {
            self.advance();
            let right = self.parse_and();
            left = Expr::BinOp { op: BinOp::Or, left: Box::new(left), right: Box::new(right) };
        }
        left
    }

    fn parse_and(&mut self) -> Expr {
        let mut left = self.parse_comparison();
        while self.code() == TokenCode::And {
            self.advance();
            let right = self.parse_comparison();
            left = Expr::BinOp { op: BinOp::And, left: Box::new(left), right: Box::new(right) };
        }
        left
    }

    fn parse_comparison(&mut self) -> Expr {
        let mut left = self.parse_additive();
        loop {
            let op = match self.code() {
                TokenCode::Equal => BinOp::Equal,
                TokenCode::NotEqual => BinOp::NotEqual,
                TokenCode::Less => BinOp::Less,
                TokenCode::LessOrEqual => BinOp::LessOrEqual,
                TokenCode::Greater => BinOp::Greater,
                TokenCode::GreaterOrEqual => BinOp::GreaterOrEqual,
                TokenCode::Is => {
                    // `status X is alive` pattern — treat "is" as equality
                    self.advance();
                    let right = self.parse_additive();
                    left = Expr::BinOp { op: BinOp::Equal, left: Box::new(left), right: Box::new(right) };
                    continue;
                }
                _ => break,
            };
            self.advance();
            let right = self.parse_additive();
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        left
    }

    fn parse_additive(&mut self) -> Expr {
        let mut left = self.parse_multiplicative();
        loop {
            let op = match self.code() {
                TokenCode::Plus | TokenCode::Add => BinOp::Add,
                TokenCode::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative();
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        left
    }

    fn parse_multiplicative(&mut self) -> Expr {
        let mut left = self.parse_unary();
        loop {
            let op = match self.code() {
                TokenCode::Star => BinOp::Mul,
                TokenCode::Slash => BinOp::Div,
                TokenCode::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary();
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right) };
        }
        left
    }

    fn parse_unary(&mut self) -> Expr {
        match self.code() {
            TokenCode::Not => {
                self.advance();
                Expr::Not(Box::new(self.parse_unary()))
            }
            TokenCode::Minus => {
                self.advance();
                Expr::Negate(Box::new(self.parse_unary()))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut base = self.parse_primary();
        
        // Handle field access like `loc.x`
        while self.code() == TokenCode::Period {
            self.advance();
            if self.code() == TokenCode::Identifier || (!matches!(self.code(), TokenCode::IntegerConstant | TokenCode::FloatConstant | TokenCode::StringConstant | TokenCode::Eof | TokenCode::Begin | TokenCode::End | TokenCode::Whenever | TokenCode::Sequence | TokenCode::Comma | TokenCode::LeftParen | TokenCode::RightParen | TokenCode::LeftCurlyBracket | TokenCode::RightCurlyBracket | TokenCode::Colon | TokenCode::Period | TokenCode::Plus | TokenCode::Minus | TokenCode::Star | TokenCode::Slash | TokenCode::Percent | TokenCode::Equal | TokenCode::NotEqual | TokenCode::Greater | TokenCode::GreaterOrEqual | TokenCode::Less | TokenCode::LessOrEqual | TokenCode::Cross)) {
                let field = self.peek().text.clone();
                self.advance();
                base = Expr::FieldAccess {
                    base: Box::new(base),
                    field,
                };
            } else {
                self.error("expected field name after '.'".into());
            }
        }
        
        base
    }

    fn parse_primary(&mut self) -> Expr {
        match self.code() {
            TokenCode::IntegerConstant => {
                let val = self.peek().int_value;
                self.advance();
                Expr::IntLit(val)
            }
            TokenCode::FloatConstant => {
                let val = self.peek().float_value;
                self.advance();
                Expr::FloatLit(val)
            }
            TokenCode::StringConstant => {
                let s = self.peek().text.clone();
                self.advance();
                Expr::StringLit(s)
            }
            TokenCode::Me => {
                self.advance();
                Expr::Me
            }
            TokenCode::Player => {
                self.advance();
                Expr::Player
            }
            TokenCode::LeftParen => {
                self.advance();
                // Check for (player) pattern
                if self.code() == TokenCode::Player {
                    self.advance();
                    self.skip_if(TokenCode::RightParen);
                    return Expr::Player;
                }
                let expr = self.parse_expr();
                self.skip_if(TokenCode::RightParen);
                Expr::Paren(Box::new(expr))
            }
            TokenCode::LeftCurlyBracket => {
                self.advance(); // skip '{'
                let x = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let y = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let z = self.parse_expr();
                self.skip_if(TokenCode::RightCurlyBracket);
                Expr::VectorLit(Box::new(x), Box::new(y), Box::new(z))
            }
            // Query functions that look like function calls
            TokenCode::Location | TokenCode::Direction | TokenCode::Distance
            | TokenCode::Health | TokenCode::Guid | TokenCode::Status
            | TokenCode::Random | TokenCode::RandomRange | TokenCode::RandomRangeFloat
            | TokenCode::LineOfSight | TokenCode::Heard | TokenCode::OnCamera
            | TokenCode::GetCurvePhase | TokenCode::GetCurveReachedEnd
            | TokenCode::GetCurveReachedStart | TokenCode::GetCurveReachedGoto
            | TokenCode::GetFaction | TokenCode::Exists
            | TokenCode::Magnitude | TokenCode::Normalize
            | TokenCode::Sin | TokenCode::Cos | TokenCode::Sqrt
            | TokenCode::DeltaHeight | TokenCode::NavPoint
            | TokenCode::IncomingAttack | TokenCode::IncomingAttackTime
            | TokenCode::SuccessiveAttacks
            | TokenCode::GetActiveWeapon | TokenCode::GetWeaponAmmo | TokenCode::GetWeaponType
            | TokenCode::GetScript | TokenCode::IsDone | TokenCode::IsHome
            | TokenCode::ActorEnabled | TokenCode::GetNumKnockdowns
            | TokenCode::ReceiveMessage | TokenCode::ReceiveAction
            | TokenCode::GetCheckPointIndex | TokenCode::GetDetectedActor
            | TokenCode::GetUIItemValue | TokenCode::GetLastHitType
            | TokenCode::BlockingCommandFailed | TokenCode::IsRestricted
            | TokenCode::First | TokenCode::Next | TokenCode::Size
            | TokenCode::MakeString
            | TokenCode::Damage
            | TokenCode::Min | TokenCode::Max
            | TokenCode::Alive | TokenCode::Fighting | TokenCode::Attacking
            | TokenCode::Blocking | TokenCode::KnockedDown
            | TokenCode::Armed | TokenCode::Asleep | TokenCode::Dormant
            | TokenCode::Crouched | TokenCode::WeaponDrawn
            | TokenCode::Grappled | TokenCode::Grappling | TokenCode::GrapplingMe
            | TokenCode::TriggerInside | TokenCode::TriggerOutside
            | TokenCode::TriggerEntered | TokenCode::TriggerExited
            | TokenCode::TriggerGetActorsInside
            | TokenCode::PlayAmbientSound
            | TokenCode::AmbientSoundStatus
            => {
                self.parse_call_expr()
            }
            _ => {
                if !matches!(self.code(), TokenCode::IntegerConstant | TokenCode::FloatConstant | TokenCode::StringConstant | TokenCode::Eof | TokenCode::Begin | TokenCode::End | TokenCode::Whenever | TokenCode::Sequence | TokenCode::Comma | TokenCode::LeftParen | TokenCode::RightParen | TokenCode::LeftCurlyBracket | TokenCode::RightCurlyBracket | TokenCode::Colon | TokenCode::Period | TokenCode::Plus | TokenCode::Minus | TokenCode::Star | TokenCode::Slash | TokenCode::Percent | TokenCode::Equal | TokenCode::NotEqual | TokenCode::Greater | TokenCode::GreaterOrEqual | TokenCode::Less | TokenCode::LessOrEqual | TokenCode::Cross) {
                    let name = self.peek().text.clone();
                    self.advance();
                    // Check for function call: name(...)
                    if self.code() == TokenCode::LeftParen {
                        self.advance();
                        let mut args = Vec::new();
                        if self.code() != TokenCode::RightParen {
                            args.push(self.parse_expr());
                            while self.skip_if(TokenCode::Comma) {
                                args.push(self.parse_expr());
                            }
                        }
                        self.skip_if(TokenCode::RightParen);
                        Expr::Call { name, args }
                    } else {
                        Expr::Var(name)
                    }
                } else {
                    self.error(format!("expected expression, found {:?} '{}'", self.code(), self.peek().text));
                    self.advance();
                    Expr::IntLit(0)
                }
            }
        }
    }

    fn parse_call_expr(&mut self) -> Expr {
        let name = self.peek().text.clone();
        self.advance();
        let mut args = Vec::new();
        if self.code() == TokenCode::LeftParen {
            self.advance();
            if self.code() != TokenCode::RightParen {
                args.push(self.parse_expr());
                while self.skip_if(TokenCode::Comma) {
                    args.push(self.parse_expr());
                }
            }
            self.skip_if(TokenCode::RightParen);
        }
        // Some queries take a trailing argument without parens
        // e.g. `status tgtActor is alive` — "status" already consumed
        // but "tgtActor" is not in parens. Handle this for single-arg queries.
        if args.is_empty() && is_expr_start(self.code()) {
            // Only consume if it looks like a value, not a command keyword
            if !is_command_start(self.code()) {
                args.push(self.parse_expr());
            }
        }

        // Parse trailing 'with' kwargs for message query expressions
        if name.eq_ignore_ascii_case("receivemessage") || name.eq_ignore_ascii_case("receiveaction") {
            if self.skip_if(TokenCode::With) {
                args.push(self.parse_expr());
                while self.skip_if(TokenCode::Comma) {
                    args.push(self.parse_expr());
                }
            }
        }

        Expr::Call { name, args }
    }
}

fn is_var_type(code: TokenCode) -> bool {
    matches!(code,
        TokenCode::Integer | TokenCode::Float | TokenCode::Vector
        | TokenCode::StringKw | TokenCode::Timer | TokenCode::Label
        | TokenCode::ActorList
    )
}

fn is_expr_start(code: TokenCode) -> bool {
    matches!(code,
        TokenCode::IntegerConstant | TokenCode::FloatConstant
        | TokenCode::StringConstant | TokenCode::Identifier
        | TokenCode::LeftParen | TokenCode::Me | TokenCode::Player
        | TokenCode::Not | TokenCode::Minus
        | TokenCode::Location | TokenCode::Direction | TokenCode::Distance
        | TokenCode::Health | TokenCode::Guid | TokenCode::Status
        | TokenCode::Random | TokenCode::RandomRange | TokenCode::RandomRangeFloat
        | TokenCode::Exists | TokenCode::Magnitude | TokenCode::Normalize
        | TokenCode::GetCurvePhase | TokenCode::GetCurveReachedEnd
        | TokenCode::GetCurveReachedStart | TokenCode::GetCurveReachedGoto
        | TokenCode::LineOfSight | TokenCode::Heard | TokenCode::OnCamera
        | TokenCode::Sin | TokenCode::Cos | TokenCode::Sqrt
        | TokenCode::IsDone | TokenCode::IsHome
        | TokenCode::First | TokenCode::Next | TokenCode::Size
        | TokenCode::Damage | TokenCode::Min | TokenCode::Max
        | TokenCode::Alive | TokenCode::Fighting | TokenCode::Attacking
        | TokenCode::Blocking | TokenCode::KnockedDown
        | TokenCode::ReceiveMessage | TokenCode::ReceiveAction
        | TokenCode::MakeString
    )
}

fn is_command_start(code: TokenCode) -> bool {
    matches!(code,
        TokenCode::Set | TokenCode::If | TokenCode::Begin | TokenCode::Do
        | TokenCode::Exit | TokenCode::Done | TokenCode::Home | TokenCode::Log
        | TokenCode::GotoCurvePhase | TokenCode::SetCurvePhase | TokenCode::SetCurveSpeed
        | TokenCode::SetCurvePingPong | TokenCode::SetCurve
        | TokenCode::PlayAnimation | TokenCode::PlayActionAnimation
        | TokenCode::Idle | TokenCode::Face | TokenCode::Goto
        | TokenCode::Fight | TokenCode::Shoot | TokenCode::Patrol | TokenCode::Follow
        | TokenCode::Stack | TokenCode::Switch | TokenCode::ChildStack | TokenCode::ChildSwitch
        | TokenCode::SendMessage | TokenCode::Spawn | TokenCode::Destroy
        | TokenCode::End | TokenCode::Eof
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_road_script() {
        let src = r#"
Script Road
begin
    do forever
    begin
        GotoCurvePhase 1.0 in 5
        SetCurvePhase 0
    end
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        assert_eq!(file.scripts.len(), 1);
        assert_eq!(file.scripts[0].name, "Road");
        assert_eq!(file.scripts[0].sequence.len(), 1); // do forever
    }

    #[test]
    fn compile_cube_script() {
        let src = r#"
Script Cube
begin
sequence
    SetCurve "curve1" at 0.5
    SetLookUpCurve "curve2"
    SetCurvePingPong 1
    SetCurveSpeed 0.3
    GotoCurvePhase 0.8 in 5
    idle 2.0
    GotoCurvePhase 0.2 in 5
    idle 2.0
    SetCurveSpeed 1.0
    do forever
        exit
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        assert_eq!(file.scripts[0].name, "Cube");
        assert!(file.scripts[0].sequence.len() >= 8);
    }

    #[test]
    fn compile_with_variables() {
        let src = r#"
Script Test
begin
variable
    integer counter
    float speed
sequence
    set counter to 0
    set speed to 1.5
    do forever
    begin
        set counter to counter add 1
        exit
    end
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        assert_eq!(file.scripts[0].variables.len(), 2);
    }

    #[test]
    fn compile_if_then_else() {
        let src = r#"
Script Logic
begin
sequence
    if 1 > 0 then
        log "yes"
    else
        log "no"
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        match &file.scripts[0].sequence[0] {
            Stmt::If { else_branch, .. } => assert!(else_branch.is_some()),
            _ => panic!("expected if statement"),
        }
    }
}
