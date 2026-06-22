/*
 * scroni/compiler.rs — ScrOni token stream → AST compiler.
 *
 * Recursive-descent parser that produces a ScriptFile from a token stream.
 * CompileError carries line/col for diagnostics.  Handles the full ONI2 ScrOni
 * grammar: variable declarations, whenever / sequence blocks, all statement
 * kinds, and expression evaluation with operator precedence.
 */
use super::ast::*;
use super::token::{Token, TokenCode};
use super::tokenizer::Tokenizer;

/// Compile errors with source location.
#[derive(Debug)]
pub struct CompileError {
    pub line: usize,
    pub col: usize,
    pub script_name: Option<String>,
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref s) = self.script_name {
            write!(
                f,
                "[script '{}'] line {}:{}: {}",
                s, self.line, self.col, self.message
            )
        } else {
            write!(f, "line {}:{}: {}", self.line, self.col, self.message)
        }
    }
}

/// Parser state: walks the token stream and builds an AST.
pub struct Compiler {
    tokens: Vec<Token>,
    pos: usize,
    pub errors: Vec<CompileError>,
    pub current_inline_vars: Vec<VarDecl>,
    pub current_script_name: Option<String>,
}

impl Compiler {
    /// Parse a ScrOni source file and return the AST.
    pub fn compile(source: &str) -> Result<ScriptFile, Vec<CompileError>> {
        let mut tokenizer = Tokenizer::new(source);
        let tokens = tokenizer.tokenize();
        let mut compiler = Compiler {
            tokens,
            pos: 0,
            errors: Vec::new(),
            current_inline_vars: Vec::new(),
            current_script_name: None,
        };
        let file = compiler.parse_file();
        if compiler.errors.is_empty() {
            Ok(file)
        } else {
            Err(compiler.errors)
        }
    }

    // ---- Helpers ----

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or(&self.tokens[self.tokens.len() - 1])
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
            self.error(format!(
                "expected {:?}, found {:?} '{}'",
                expected,
                self.code(),
                self.peek().text
            ));
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
        self.errors.push(CompileError {
            line: tok.line,
            col: tok.col,
            script_name: self.current_script_name.clone(),
            message: msg,
        });
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
        self.current_inline_vars.clear();
        self.advance(); // skip 'Script'
        let name = if self.code() == TokenCode::Identifier {
            let n = self.peek().text.clone();
            self.advance();
            n
        } else {
            self.error("expected script name".into());
            return None;
        };

        self.current_script_name = Some(name.clone());

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
                && (is_var_type(self.code()) || self.code() == TokenCode::Parent)
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

        let mut all_vars = variables;
        all_vars.append(&mut self.current_inline_vars);

        Some(ScriptDef {
            name,
            variables: all_vars,
            whenever,
            sequence,
        })
    }

    fn parse_var_decl(&mut self) -> Option<VarDecl> {
        let is_parent = self.skip_if(TokenCode::Parent);

        let var_type = match self.code() {
            TokenCode::Integer => VarType::Integer,
            TokenCode::Float => VarType::Float,
            TokenCode::Vector => VarType::Vector,
            TokenCode::StringKw => VarType::String,
            TokenCode::Timer => VarType::Timer,
            TokenCode::Label => VarType::Label,
            TokenCode::ActorList => VarType::ActorList,
            TokenCode::Child => VarType::Child,
            _ => {
                self.error(format!("expected type keyword, found {:?}", self.code()));
                return None;
            }
        };
        self.advance();

        // Variable names can be identifiers or keywords used as names (e.g. "speed")
        let name = if self.code() == TokenCode::Identifier || self.code() == TokenCode::Eof {
            let n = self.peek().text.clone().to_lowercase();
            self.advance();
            n
        } else if !matches!(
            self.code(),
            TokenCode::IntegerConstant
                | TokenCode::FloatConstant
                | TokenCode::StringConstant
                | TokenCode::Eof
                | TokenCode::Begin
                | TokenCode::End
                | TokenCode::Whenever
                | TokenCode::Sequence
        ) {
            // Accept keyword tokens as variable names (ScrOni allows this)
            let n = self.peek().text.clone().to_lowercase();
            self.advance();
            n
        } else {
            self.error("expected variable name".into());
            return None;
        };

        // Optional initializer: `= <expr> [, <expr>...]`
        let initializer = if self.code() == TokenCode::Equal {
            self.advance();
            let first = self.parse_expr();
            if self.code() == TokenCode::Comma {
                let mut list = vec![first];
                while self.skip_if(TokenCode::Comma) {
                    list.push(self.parse_expr());
                }
                Some(Expr::List(list))
            } else {
                Some(first)
            }
        } else {
            None
        };

        Some(VarDecl {
            is_parent,
            var_type,
            name,
            initializer,
        })
    }

    // ---- Section / block parsing ----

    fn parse_section_until(&mut self, end_tokens: &[TokenCode]) -> Block {
        let mut stmts = Vec::new();
        while !end_tokens.contains(&self.code()) && !self.at_end() {
            let stmt = self.parse_stmt();
            if let Stmt::Block(mut inner) = stmt {
                stmts.append(&mut inner);
            } else {
                stmts.push(stmt);
            }
        }
        stmts
    }

    fn parse_block(&mut self) -> Block {
        if !self.expect(TokenCode::Begin) {
            return Vec::new();
        }
        let mut stmts = Vec::new();
        while self.code() != TokenCode::End && !self.at_end() {
            let stmt = self.parse_stmt();
            if let Stmt::Block(mut inner) = stmt {
                stmts.append(&mut inner);
            } else {
                stmts.push(stmt);
            }
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
            TokenCode::Exit => {
                self.advance();
                Stmt::Exit
            }
            TokenCode::Done => {
                self.advance();
                Stmt::Done
            }
            TokenCode::Home => {
                self.advance();
                Stmt::Home
            }
            TokenCode::Log => self.parse_log(),

            // Inline variable declarations
            TokenCode::Integer
            | TokenCode::Float
            | TokenCode::Vector
            | TokenCode::StringKw
            | TokenCode::Timer
            | TokenCode::Label
            | TokenCode::ActorList => {
                if let Some(v) = self.parse_var_decl() {
                    self.current_inline_vars.push(v.clone());
                    Stmt::InlineVarDecl(v)
                } else {
                    Stmt::Unimplemented {
                        command: "var_decl".into(),
                        args: vec![],
                    }
                }
            }

            // Curve commands
            TokenCode::GotoCurvePhase => self.parse_goto_curve_phase(),
            TokenCode::GotoCurveKnot => self.parse_goto_curve_knot(),
            TokenCode::GotoCurveLerp => self.parse_goto_curve_lerp(),
            TokenCode::SetCurvePhase => {
                self.advance();
                Stmt::SetCurvePhase(self.parse_expr())
            }
            TokenCode::SetCurveSpeed => {
                self.advance();
                Stmt::SetCurveSpeed(self.parse_expr())
            }
            TokenCode::SetCurveKs => {
                self.advance();
                Stmt::SetCurveKs(self.parse_expr())
            }
            TokenCode::SetCurvePingPong => {
                self.advance();
                Stmt::SetCurvePingPong(self.parse_expr())
            }
            TokenCode::SetCurve => self.parse_set_curve(),
            TokenCode::SetLerpCurve => {
                self.advance();
                Stmt::SetLerpCurve(self.parse_expr())
            }
            TokenCode::SetLookUpCurve => {
                self.advance();
                Stmt::SetLookUpCurve(self.parse_expr())
            }
            TokenCode::SetCurveLookAtActor => {
                self.advance();
                Stmt::SetCurveLookAtActor(self.parse_expr())
            }
            TokenCode::SetCurveLookAlongDistance => {
                self.advance();
                Stmt::SetCurveLookAlongDistance(self.parse_expr())
            }
            TokenCode::SetCurveLookAlongDirection => {
                self.advance();
                Stmt::SetCurveLookAlongDirection(self.parse_expr())
            }

            // Animation
            TokenCode::PlayAnimation => self.parse_play_animation(),
            TokenCode::PlayActionAnimation => self.parse_play_action_animation(),
            TokenCode::ControlAnimation => {
                self.advance();
                Stmt::ControlAnimation {
                    name: self.parse_expr(),
                }
            }

            // Movement / combat (blocking)
            TokenCode::Idle => {
                self.advance();
                Stmt::Idle(self.parse_expr())
            }
            TokenCode::For => {
                self.advance();
                Stmt::Idle(self.parse_expr())
            }
            TokenCode::Face => self.parse_face(),
            TokenCode::Goto => self.parse_goto(),
            TokenCode::Fight => {
                // `fight [<guid>] [leader (support|attack)] [for <duration>]`.
                // The leader-mode
                // and duration modifiers aren't surfaced to the runtime
                // yet — just consume them so they don't dangle as hanging
                // values (seen in gruntsC.oni `fight tgtActor leader
                // support` and `fight tgtActor leader attack for 10`).
                self.advance();
                let target = if is_expr_start(self.code()) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                if self.skip_if(TokenCode::Leader) {
                    // Expect one of support / attack.
                    match self.code() {
                        TokenCode::Support | TokenCode::Attack => {
                            self.advance();
                        }
                        _ => {}
                    }
                }
                if self.skip_if(TokenCode::For) {
                    let _ = self.parse_expr();
                }
                Stmt::Fight(target)
            }
            TokenCode::Shoot => self.parse_shoot(),
            TokenCode::Look => self.parse_look(),
            TokenCode::Hit => self.parse_hit(),
            TokenCode::MakeExplosion => self.parse_make_explosion(),
            TokenCode::Patrol => {
                self.advance();
                self.skip_if(TokenCode::Path);
                let path_expr = self.parse_expr();
                // Consume optional patrol modifiers (speed <expr>, listen, start <expr>, etc.)
                loop {
                    match self.code() {
                        TokenCode::Speed
                        | TokenCode::Start
                        | TokenCode::Stop
                        | TokenCode::Pause
                        | TokenCode::Look
                        | TokenCode::Scan
                        | TokenCode::Face
                        | TokenCode::Direction => {
                            self.advance();
                            if is_expr_start(self.code()) {
                                self.parse_expr();
                            }
                        }
                        TokenCode::Listen => {
                            self.advance();
                        }
                        _ => break,
                    }
                }
                Stmt::Patrol(path_expr)
            }
            TokenCode::Follow => {
                self.advance();
                Stmt::Follow(self.parse_expr())
            }
            TokenCode::Attack => {
                self.advance();
                Stmt::Attack(self.parse_expr())
            }
            TokenCode::Retreat => {
                self.advance();
                let target = if self.skip_if(TokenCode::To) || is_expr_start(self.code()) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                Stmt::Retreat(target)
            }
            TokenCode::TakeCover => {
                // `takecover [for <expr>]` — port of the legacy
                // blocking `takecover` command.  The optional duration consumes
                // the same `for <value>` that the C++ parser accepts; we
                // capture it but the runtime currently treats takecover as
                // run-until-done (cover reached) or run-until-failed (no
                // cover point reachable).
                self.advance();
                let duration = if self.skip_if(TokenCode::For) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                Stmt::TakeCover { duration }
            }
            TokenCode::Destroy => {
                self.advance();
                // Check if it's called like a function `destroy(expr)` or as a command `destroy expr`
                if self.code() == TokenCode::LeftParen {
                    self.advance();
                    let target = self.parse_expr();
                    self.skip_if(TokenCode::RightParen);
                    Stmt::Destroy(target)
                } else {
                    Stmt::Destroy(self.parse_expr())
                }
            }

            // Script flow
            TokenCode::Stack => {
                self.advance();
                Stmt::Stack(self.parse_expr())
            }
            TokenCode::Switch => {
                self.advance();
                Stmt::Switch(self.parse_expr())
            }
            TokenCode::ChildStack => {
                self.advance();
                let var = self.peek().text.clone().to_lowercase();
                self.advance();
                let script = self.parse_expr();
                Stmt::ChildStack { var, script }
            }
            TokenCode::ChildSwitch => {
                self.advance();
                let var = self.peek().text.clone().to_lowercase();
                self.advance();
                let script = self.parse_expr();
                Stmt::ChildSwitch { var, script }
            }
            TokenCode::ChildDone => {
                self.advance();
                let var = self.parse_optional_child_var();
                Stmt::ChildDone { var }
            }
            TokenCode::ChildHome => {
                self.advance();
                let var = self.parse_optional_child_var();
                Stmt::ChildHome { var }
            }
            TokenCode::ChildStop => {
                self.advance();
                let var = self.parse_optional_child_var();
                Stmt::ChildStop { var }
            }

            // Lists
            TokenCode::Add => self.parse_add(),
            TokenCode::Remove => self.parse_remove(),

            // Actor management
            TokenCode::Spawn => self.parse_spawn(),

            TokenCode::Teleport => self.parse_teleport(),
            TokenCode::MakeFX => self.parse_make_fx(),
            TokenCode::MakeProjectile => self.parse_make_projectile(),
            TokenCode::UsePad => {
                self.advance();
                Stmt::UsePad
            }

            // Messaging
            TokenCode::SendMessage => self.parse_send_message(),
            TokenCode::SendGroupMessage => self.parse_send_group_message(),
            TokenCode::SendGroupMembersMessage => self.parse_send_group_members_message(),
            TokenCode::SendAction => self.parse_send_action(),

            // Item interactions
            TokenCode::Pickup => {
                self.advance();
                Stmt::Pickup(self.parse_expr())
            }
            TokenCode::Dropoff => self.parse_dropoff(),

            // Properties
            TokenCode::SetHealth => {
                self.advance();
                Stmt::SetHealth(self.parse_expr())
            }
            TokenCode::ResetHealth => {
                self.advance();
                Stmt::ResetHealth
            }
            TokenCode::SetActorEnabled => {
                self.advance();
                Stmt::SetActorEnabled(self.parse_expr())
            }
            TokenCode::SetFaction => {
                self.advance();
                Stmt::SetFaction(self.parse_expr())
            }
            TokenCode::SetUnbreakable => {
                self.advance();
                Stmt::SetUnbreakable(self.parse_expr())
            }
            TokenCode::SetCrouch => {
                self.advance();
                Stmt::SetCrouch(self.parse_expr())
            }
            TokenCode::SetAttackTable => {
                self.advance();
                Stmt::SetAttackTable(self.parse_expr())
            }
            TokenCode::DrawWeapon => {
                self.advance();
                Stmt::DrawWeapon
            }
            TokenCode::HolsterWeapon => {
                self.advance();
                Stmt::HolsterWeapon
            }

            // Camera
            TokenCode::CameraReset => {
                self.advance();
                Stmt::CameraReset
            }
            TokenCode::CameraMode => {
                // `cameramode (script|game) [time <value>]`.
                self.advance();
                let mode = self.parse_expr();
                let time = if self.skip_if(TokenCode::Time) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                Stmt::CameraMode { mode, time }
            }
            TokenCode::CameraLetterbox => {
                self.advance();
                Stmt::CameraLetterbox(self.parse_expr())
            }
            TokenCode::CameraFollowActor => {
                let args = self.parse_camera_args();
                Stmt::CameraFollowActor { args }
            }
            TokenCode::CameraTrackActor => {
                let args = self.parse_camera_args();
                Stmt::CameraTrackActor { args }
            }
            TokenCode::CameraTrackPoint => {
                let args = self.parse_camera_args();
                Stmt::CameraTrackPoint { args }
            }
            TokenCode::CameraMoveToActor => {
                let args = self.parse_camera_args();
                Stmt::CameraMoveToActor { args }
            }
            TokenCode::CameraMoveToPoint => {
                let args = self.parse_camera_args();
                Stmt::CameraMoveToPoint { args }
            }
            TokenCode::CameraCutToActor => {
                let args = self.parse_camera_args();
                Stmt::CameraCutToActor { args }
            }
            TokenCode::CameraCutToPoint => {
                let args = self.parse_camera_args();
                Stmt::CameraCutToPoint { args }
            }
            TokenCode::CameraSetFOV => {
                self.parse_generic_args(TokenCode::CameraSetFOV, |args| Stmt::CameraSetFOV { args })
            }
            TokenCode::CameraSetPackage => {
                self.advance();
                Stmt::CameraSetPackage(self.parse_expr())
            }
            TokenCode::CameraShake => {
                self.advance();
                Stmt::CameraShake
            }

            // Sound
            TokenCode::Sound => self.parse_sound(),
            TokenCode::PlayAmbientSound => self.parse_play_ambient_sound(),
            TokenCode::AmbientSound => self.parse_ambient_sound(),
            TokenCode::MusicPlay => {
                self.advance();
                Stmt::MusicPlay(self.parse_expr())
            }
            TokenCode::MusicStop => {
                self.advance();
                Stmt::MusicStop
            }
            TokenCode::StartCutSceneAudio => {
                self.advance();
                let mut args = Vec::new();
                while !self.at_end() {
                    if !is_expr_start(self.code()) {
                        break;
                    }
                    args.push(self.parse_expr());
                    self.skip_if(TokenCode::Comma);
                }
                Stmt::StartCutSceneAudio { args }
            }
            TokenCode::EndCutSceneAudio => {
                self.advance();
                Stmt::EndCutSceneAudio
            }

            // Fog
            TokenCode::SetFogType => {
                self.advance();
                Stmt::SetFogType(self.parse_expr())
            }
            TokenCode::SetFogColor => {
                self.parse_generic_args(TokenCode::SetFogColor, |args| Stmt::SetFogColor { args })
            }
            TokenCode::SetFogClamp => {
                self.parse_generic_args(TokenCode::SetFogClamp, |args| Stmt::SetFogClamp { args })
            }
            TokenCode::SetFogPalettePower => self
                .parse_generic_args(TokenCode::SetFogPalettePower, |args| {
                    Stmt::SetFogPalettePower { args }
                }),

            // Shaders / Lights
            TokenCode::SetShaderLocal => self
                .parse_generic_args(TokenCode::SetShaderLocal, |args| Stmt::SetShaderLocal {
                    args,
                }),
            TokenCode::SetLightParameter => self
                .parse_generic_args(TokenCode::SetLightParameter, |args| {
                    Stmt::SetLightParameter { args }
                }),
            TokenCode::Intensity => {
                self.parse_generic_args(TokenCode::Intensity, |args| Stmt::Intensity { args })
            }

            // Global
            TokenCode::SetFullScreenColor => self
                .parse_generic_args(TokenCode::SetFullScreenColor, |args| {
                    Stmt::SetFullScreenColor { args }
                }),
            TokenCode::SetUpdateState => {
                self.advance();
                let target = self.parse_expr();
                let state = self.parse_expr();
                Stmt::SetUpdateState { target, state }
            }
            TokenCode::ControlHead => self.parse_control_head(),

            TokenCode::Find => self.parse_find(),
            TokenCode::TextureMovie => self.parse_texture_movie(),
            TokenCode::At => {
                self.advance();
                let x = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let y = self.parse_expr();
                Stmt::At(x, y)
            }
            TokenCode::DrawText => {
                self.advance();
                Stmt::DrawText(self.parse_expr())
            }
            TokenCode::Color => {
                // `color R,G,B,A` — sets the current debug-draw color.
                // Legacy scripts write it on the same line as drawtext.
                self.advance();
                let r = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let g = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let b = self.parse_expr();
                self.skip_if(TokenCode::Comma);
                let a = self.parse_expr();
                Stmt::Color { r, g, b, a }
            }
            TokenCode::Clear => {
                // `clear <listvar>` — empty an actor-list variable.
                self.advance();
                let list = self.peek().text.clone().to_lowercase();
                self.advance();
                Stmt::ClearList { list }
            }
            TokenCode::Normalize => {
                // `normalize <vectorvar>` — normalize in-place.
                self.advance();
                let var = self.peek().text.clone().to_lowercase();
                self.advance();
                Stmt::Normalize { var }
            }
            TokenCode::SetHud => {
                // `sethud <value>`.  HUD
                // rendering isn't wired yet; consume the arg.
                self.advance();
                let arg = self.parse_expr();
                Stmt::Unimplemented {
                    command: "sethud".to_string(),
                    args: vec![arg],
                }
            }
            TokenCode::SetFogRange => {
                // `setFogRange <a> <b> <c>` three numeric args.
                // Fog isn't wired.
                self.advance();
                let mut args = Vec::new();
                for _ in 0..3 {
                    if is_expr_start(self.code()) {
                        args.push(self.parse_expr());
                    }
                }
                Stmt::Unimplemented {
                    command: "setfogrange".to_string(),
                    args,
                }
            }
            TokenCode::LevelComplete => {
                self.advance();
                Stmt::Unimplemented {
                    command: "levelcomplete".to_string(),
                    args: Vec::new(),
                }
            }
            TokenCode::EndGrapple => {
                self.advance();
                Stmt::Unimplemented {
                    command: "endgrapple".to_string(),
                    args: Vec::new(),
                }
            }
            TokenCode::Form => {
                // `form <string>`.
                self.advance();
                let arg = self.parse_expr();
                Stmt::Unimplemented {
                    command: "form".to_string(),
                    args: vec![arg],
                }
            }
            TokenCode::PadRumbleLargeMotor => {
                // `padRumbleLargeMotor [totaltime N] [rampuptime N]
                //    [dampentime N] [frequency N] [min N] [max N]`.
                self.advance();
                let mut args = Vec::new();
                loop {
                    let tag = match self.code() {
                        TokenCode::TotalTime => "totaltime",
                        TokenCode::RampUpTime => "rampuptime",
                        TokenCode::DampenTime => "dampentime",
                        TokenCode::Frequency => "frequency",
                        TokenCode::Min => "min",
                        TokenCode::Max => "max",
                        _ => break,
                    };
                    self.advance();
                    args.push(Expr::StringLit(tag.to_string()));
                    if is_expr_start(self.code()) {
                        args.push(self.parse_expr());
                    }
                }
                Stmt::Unimplemented {
                    command: "padrumblelargemotor".to_string(),
                    args,
                }
            }
            TokenCode::PadRumbleSmallMotor => {
                // `padRumbleSmallMotor time <value>`
                self.advance();
                let mut args = Vec::new();
                if self.skip_if(TokenCode::Time) && is_expr_start(self.code()) {
                    args.push(self.parse_expr());
                }
                Stmt::Unimplemented {
                    command: "padrumblesmallmotor".to_string(),
                    args,
                }
            }
            TokenCode::PadRumbleLargeMotorStop
            | TokenCode::PadRumblePause
            | TokenCode::PadRumbleResume
            | TokenCode::PadRumbleSmallMotorStop
            | TokenCode::PadRumbleStopMotors => {
                let name = self.peek().text.clone().to_lowercase();
                self.advance();
                Stmt::Unimplemented {
                    command: name,
                    args: Vec::new(),
                }
            }
            TokenCode::Jump => {
                // `jump [index <val> | onlyrunning | onlystanding]
                //    [from <vec>] to <vec>`
                // or `jump [index <val>] [from <vec>] [to <vec>]`
                // Consume all
                // modifiers.  Runtime effect is stubbed — the AI jump
                // pathing isn't wired yet.
                self.advance();
                let mut args = Vec::new();
                match self.code() {
                    TokenCode::Index => {
                        self.advance();
                        args.push(Expr::StringLit("index".to_string()));
                        if is_expr_start(self.code()) {
                            args.push(self.parse_expr());
                        }
                    }
                    TokenCode::OnlyRunning => {
                        self.advance();
                        args.push(Expr::StringLit("onlyrunning".to_string()));
                    }
                    TokenCode::OnlyStanding => {
                        self.advance();
                        args.push(Expr::StringLit("onlystanding".to_string()));
                    }
                    _ => {}
                }
                if self.skip_if(TokenCode::From) {
                    args.push(Expr::StringLit("from".to_string()));
                    args.push(self.parse_expr());
                }
                if self.skip_if(TokenCode::To) {
                    args.push(Expr::StringLit("to".to_string()));
                    args.push(self.parse_expr());
                }
                Stmt::Unimplemented {
                    command: "jump".to_string(),
                    args,
                }
            }

            TokenCode::PlayerTaskBegin => {
                self.advance();
                let timeout = if is_expr_start(self.code()) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                Stmt::PlayerTaskBegin(timeout)
            }
            TokenCode::PlayerTaskSuccessful => {
                self.advance();
                Stmt::PlayerTaskSuccessful
            }
            TokenCode::PlayerTaskFailure => {
                self.advance();
                Stmt::PlayerTaskFailure
            }

            TokenCode::RunGame => {
                // `RunGame [Level <expr> [SavePoint <expr>]]` — mirrors
                // `scrCompiler::ParseRunGame`.
                // Bare `RunGame` re-runs the current level; `Level <n>`
                // sets the index; `SavePoint <n>` is only valid after
                // `Level`.
                self.advance(); // skip 'RunGame'
                let mut level = None;
                let mut save_point = None;
                if self.code() == TokenCode::Level {
                    self.advance(); // skip 'Level'
                    level = Some(self.parse_expr());
                    if self.code() == TokenCode::SavePoint {
                        self.advance(); // skip 'SavePoint'
                        save_point = Some(self.parse_expr());
                    }
                }
                Stmt::RunGame { level, save_point }
            }

            _ => {
                // Unknown command — skip token and collect trailing exprs until next command
                let cmd = self.peek().text.clone();

                // CRASH ON UNIMPLEMENTED/HANGING VALUES
                self.error(format!("Hanging value or unrecognized command: '{}'. Fatal error enabled for debugging.", cmd));

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

    fn parse_generic_args<F>(&mut self, _code: TokenCode, constructor: F) -> Stmt
    where
        F: FnOnce(Vec<Expr>) -> Stmt,
    {
        self.advance();
        let mut args = Vec::new();
        while !self.at_end() && is_expr_start(self.code()) {
            args.push(self.parse_expr());
            self.skip_if(TokenCode::Comma);
        }
        constructor(args)
    }

    /// Parses camera position/actor commands that use the `offset {v}` and `time X` optional suffixes.
    /// Returns args as [primary_expr, duration] where duration defaults to 0.0 if `time` is absent.
    fn parse_camera_args(&mut self) -> Vec<Expr> {
        self.advance(); // skip command keyword
        let mut args = Vec::new();

        if is_expr_start(self.code()) {
            args.push(self.parse_expr());
        }

        // `offset {x,y,z}` — consumed but not forwarded (look-at offsets not yet implemented)
        if self.code() == TokenCode::Offset {
            self.advance();
            if is_expr_start(self.code()) {
                self.parse_expr();
            }
        }

        // `time X` — consumed and forwarded as the duration argument
        if self.code() == TokenCode::Time {
            self.advance();
            if is_expr_start(self.code()) {
                args.push(self.parse_expr());
            }
        }

        args
    }

    fn parse_set(&mut self) -> Stmt {
        self.advance(); // skip 'set'
        let var = self.peek().text.clone().to_lowercase();
        self.advance(); // skip identifier
        self.skip_if(TokenCode::To); // skip 'to'

        let first = self.parse_expr();
        let value = if self.code() == TokenCode::Comma {
            let mut list = vec![first];
            while self.skip_if(TokenCode::Comma) {
                list.push(self.parse_expr());
            }
            Expr::List(list)
        } else {
            first
        };

        Stmt::Set { var, value }
    }

    fn parse_add(&mut self) -> Stmt {
        self.advance(); // skip 'add'
        let mut exprs = vec![self.parse_expr()];
        while self.skip_if(TokenCode::Comma) {
            exprs.push(self.parse_expr());
        }
        self.skip_if(TokenCode::To); // skip 'to'

        // Target list can be an identifier or a keyword
        let list = self.peek().text.clone().to_lowercase();
        self.advance();

        Stmt::AddToList { exprs, list }
    }

    fn parse_remove(&mut self) -> Stmt {
        self.advance(); // skip 'remove'
        let mut exprs = vec![self.parse_expr()];
        while self.skip_if(TokenCode::Comma) {
            exprs.push(self.parse_expr());
        }
        self.skip_if(TokenCode::From); // skip 'from'

        // Target list can be an identifier or a keyword
        let list = self.peek().text.clone().to_lowercase();
        self.advance();

        Stmt::RemoveFromList { exprs, list }
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
        Stmt::If {
            condition,
            then_branch,
            else_branch,
        }
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
        let seconds = if self.skip_if(TokenCode::In) {
            self.parse_expr()
        } else {
            Expr::FloatLit(1.0)
        };
        Stmt::GotoCurveKnot { knot, seconds }
    }

    fn parse_goto_curve_lerp(&mut self) -> Stmt {
        self.advance();
        let lerp = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) {
            self.parse_expr()
        } else {
            Expr::FloatLit(1.0)
        };
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
        let mut hold = false;
        let mut loop_anim = false;
        let mut rate = None;
        let mut duration = None;

        loop {
            if self.skip_if(TokenCode::Hold) {
                hold = true;
            } else if self.skip_if(TokenCode::Loop) {
                loop_anim = true;
            } else if self.skip_if(TokenCode::Rate) {
                rate = Some(self.parse_expr());
            } else if self.skip_if(TokenCode::For) {
                duration = Some(self.parse_expr());
            } else {
                break;
            }
        }

        Stmt::PlayAnimation {
            name,
            hold,
            loop_anim,
            rate,
            duration,
        }
    }

    fn parse_play_action_animation(&mut self) -> Stmt {
        self.advance();
        let name = self.parse_expr();
        let mut hold = false;
        let mut loop_anim = false;
        let mut duration = None;

        loop {
            if self.skip_if(TokenCode::Hold) {
                hold = true;
            } else if self.skip_if(TokenCode::Loop) {
                loop_anim = true;
            } else if self.skip_if(TokenCode::For) {
                duration = Some(self.parse_expr());
            } else {
                break;
            }
        }

        Stmt::PlayActionAnimation {
            name,
            hold,
            loop_anim,
            duration,
        }
    }

    fn parse_face(&mut self) -> Stmt {
        self.advance();
        let target = self.parse_expr();
        let seconds = if self.skip_if(TokenCode::In) || self.skip_if(TokenCode::For) {
            Some(self.parse_expr())
        } else {
            None
        };
        Stmt::Face { target, seconds }
    }

    fn parse_goto(&mut self) -> Stmt {
        self.advance();
        let target = self.parse_expr();
        let within = if self.skip_if(TokenCode::Within) {
            Some(self.parse_expr())
        } else {
            None
        };
        let speed = if self.skip_if(TokenCode::Speed) {
            Some(self.parse_expr())
        } else {
            None
        };
        let duration = if self.skip_if(TokenCode::For) {
            Some(self.parse_expr())
        } else {
            None
        };
        Stmt::GotoPoint {
            target,
            within,
            speed,
            duration,
        }
    }

    fn parse_spawn(&mut self) -> Stmt {
        self.advance();
        let script = self.parse_expr();
        let assign_to = if self.skip_if(TokenCode::Assign) {
            self.skip_if(TokenCode::To);
            let name = self.peek().text.clone().to_lowercase();
            self.advance();
            Some(name)
        } else {
            None
        };
        let at = if self.skip_if(TokenCode::At) {
            Some(self.parse_expr())
        } else {
            None
        };
        let name = if self.skip_if(TokenCode::Name) {
            Some(self.parse_expr())
        } else {
            None
        };
        Stmt::Spawn {
            script,
            assign_to,
            at,
            name,
        }
    }

    fn parse_make_fx(&mut self) -> Stmt {
        self.advance(); // consume `makefx`
        let name = self.parse_expr();
        let at = if self.skip_if(TokenCode::At) {
            Some(self.parse_expr())
        } else {
            None
        };
        Stmt::MakeFx { name, at }
    }

    /// `makeprojectile <name> direction <vec> speed <num> [at <expr>]`
    /// — `direction`, `speed`, and `at` keyword clauses can appear in
    /// any order after the name expression.
    fn parse_make_projectile(&mut self) -> Stmt {
        self.advance(); // consume `makeprojectile`
        let name = self.parse_expr();
        let mut direction: Option<Expr> = None;
        let mut speed: Option<Expr> = None;
        let mut at: Option<Expr> = None;
        loop {
            match self.code() {
                TokenCode::Direction => {
                    self.advance();
                    direction = Some(self.parse_expr());
                }
                TokenCode::Speed => {
                    self.advance();
                    speed = Some(self.parse_expr());
                }
                TokenCode::At => {
                    self.advance();
                    at = Some(self.parse_expr());
                }
                _ => break,
            }
        }
        Stmt::MakeProjectile {
            name,
            direction: direction.unwrap_or(Expr::VectorLit(
                Box::new(Expr::FloatLit(0.0)),
                Box::new(Expr::FloatLit(0.0)),
                Box::new(Expr::FloatLit(1.0)),
            )),
            speed: speed.unwrap_or(Expr::FloatLit(0.0)),
            at,
        }
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

    fn parse_dropoff(&mut self) -> Stmt {
        self.advance();
        let mut at = None;
        if self.skip_if(TokenCode::At) {
            at = Some(self.parse_expr());
        }
        Stmt::Dropoff { at }
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

    fn parse_send_action(&mut self) -> Stmt {
        self.advance(); // consume `sendaction`
        let action = self.parse_expr();

        let target = if self.skip_if(TokenCode::To) {
            Some(self.parse_expr())
        } else if self.peek().code != TokenCode::Component {
            Some(self.parse_expr())
        } else {
            None
        };

        let component = if self.skip_if(TokenCode::Component) {
            Some(self.parse_expr())
        } else {
            None
        };

        Stmt::SendAction {
            action,
            target,
            component,
        }
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

        let mut volume_ramp = None;
        if self.skip_if(TokenCode::VolumeRamp) {
            let has_paren = self.skip_if(TokenCode::LeftParen);
            let start = self.parse_expr();
            let end = self.parse_expr();
            let time = self.parse_expr();
            if has_paren {
                self.skip_if(TokenCode::RightParen);
            }
            volume_ramp = Some((start, end, time));
        }

        let mut pitch = None;
        if self.skip_if(TokenCode::Pitch) {
            if self.skip_if(TokenCode::LeftParen) {
                let v = self.parse_expr();
                self.skip_if(TokenCode::RightParen);
                pitch = Some(v);
            } else {
                pitch = Some(self.parse_expr());
            }
        }

        let mut pitch_ramp = None;
        if self.skip_if(TokenCode::PitchRamp) {
            let has_paren = self.skip_if(TokenCode::LeftParen);
            let start = self.parse_expr();
            let end = self.parse_expr();
            let time = self.parse_expr();
            if has_paren {
                self.skip_if(TokenCode::RightParen);
            }
            pitch_ramp = Some((start, end, time));
        }

        Stmt::PlayAmbientSound {
            name,
            volume,
            pitch,
            volume_ramp,
            pitch_ramp,
        }
    }

    fn parse_sound(&mut self) -> Stmt {
        // Mirror C++ scrCompiler::ParseSound (SoundCommand.cpp):
        //   sound <channel-expr> <action> [<arg-expr>]
        // <action> is one of: play, pause, stop, pitch, volume,
        // fadeIn, fadeOut.  `play` optionally takes a package-name
        // string; pitch/volume/fadeIn/fadeOut each take one numeric
        // expression.  We collect the channel + an action string
        // literal + any trailing arg into `Stmt::Sound { args }`;
        // the op handler unpacks them into a real verb.
        self.advance(); // skip 'sound'
        let mut args = Vec::new();
        while !self.at_end()
            && (is_expr_start(self.code())
                || matches!(
                    self.code(),
                    TokenCode::Play
                        | TokenCode::Stop
                        | TokenCode::Pause
                        | TokenCode::Pitch
                        | TokenCode::Volume
                        | TokenCode::FadeIn
                        | TokenCode::FadeOut
                ))
        {
            match self.code() {
                TokenCode::Play => {
                    self.advance();
                    args.push(Expr::StringLit("play".to_string()));
                }
                TokenCode::Stop => {
                    self.advance();
                    args.push(Expr::StringLit("stop".to_string()));
                }
                TokenCode::Pause => {
                    self.advance();
                    args.push(Expr::StringLit("pause".to_string()));
                }
                TokenCode::Pitch => {
                    self.advance();
                    args.push(Expr::StringLit("pitch".to_string()));
                }
                TokenCode::Volume => {
                    self.advance();
                    args.push(Expr::StringLit("volume".to_string()));
                }
                TokenCode::FadeIn => {
                    self.advance();
                    args.push(Expr::StringLit("fadein".to_string()));
                }
                TokenCode::FadeOut => {
                    self.advance();
                    args.push(Expr::StringLit("fadeout".to_string()));
                }
                _ => args.push(self.parse_expr()),
            }
            self.skip_if(TokenCode::Comma);
        }
        Stmt::Sound { args }
    }

    fn parse_make_explosion(&mut self) -> Stmt {
        // `makeExplosion <name> [orientation <vec>] at <vec>`
        // `at` and `orientation` are dedicated keyword tokens
        // (TokenCode::At / TokenCode::Orientation), NOT Identifier — the
        // prior `skip_if(TokenCode::Identifier)` calls never consumed them
        // so downstream `parse_expr` choked on the trailing var refs like
        // `blastPos` / `blastOrientation` (seen in DoMovieBlast / DoBlast).
        self.advance(); // skip 'makeexplosion'
        let name = self.parse_expr();
        let orientation = if self.skip_if(TokenCode::Orientation) {
            Some(self.parse_expr())
        } else {
            None
        };
        self.skip_if(TokenCode::At);
        let at = self.parse_expr();
        Stmt::MakeExplosion {
            name,
            orientation,
            at,
        }
    }

    fn parse_hit(&mut self) -> Stmt {
        self.advance(); // skip 'hit'
        let expr1 = self.parse_expr();

        let (hit_type, victim) = if self.code() == TokenCode::For || !is_expr_start(self.code()) {
            (Expr::StringLit("instant".to_string()), expr1)
        } else {
            (expr1, self.parse_expr())
        };

        let damage = if self.skip_if(TokenCode::For) {
            self.parse_expr()
        } else {
            Expr::FloatLit(1.0)
        };
        Stmt::Hit {
            hit_type,
            victim,
            damage,
        }
    }

    /// Consume an optional child-variable identifier that follows a
    /// `childdone`/`childhome`/`childstop` keyword.  Returns `Some(name)`
    /// when an identifier is next; `None` when the next token is a
    /// statement keyword / comment / EOF.  Ensures grunt2.oni's `childdone
    /// awareChild` / `childhome actionChild` style parses cleanly rather
    /// than leaving `awareChild` as a hanging token.
    fn parse_optional_child_var(&mut self) -> Option<String> {
        if self.code() == TokenCode::Identifier {
            let name = self.peek().text.clone().to_lowercase();
            self.advance();
            Some(name)
        } else {
            None
        }
    }

    /// Parse `shoot <target> [miss] [bullets|rate <expr>] [for <expr>]`.
    ///
    /// Same structural shape as `parse_ambient_sound`: slurp a variable-
    /// length arg list where dedicated keyword tokens (`Miss`, `Bullets`,
    /// `Rate`, `For`) are captured as string literals and other tokens
    /// flow through `parse_expr`.  Keyword tokens MUST be matched
    /// explicitly — `is_expr_start` returns false for them, so without
    /// this the arg loop breaks after the first expression and the
    /// remaining tokens fall through the stmt dispatcher as "Hanging
    /// value" errors (see feedback memory `feedback_scroni_keyword_tokens`).
    /// Parse `look <listvar> status <state>, <state>, ...` (AI awareness
    /// scan in grunt2.oni `Awareness_LookAndListen` script).  Treats
    /// `Status`, `Alive`, `Enemy`, `Friendly`, `Dead` keyword tokens as
    /// string-literal action words so they don't trip the catch-all
    /// "Hanging value" error.  See the feedback memory on ScrOni keyword-
    /// token parsing — this is the same structural pattern as
    /// `parse_ambient_sound` and `parse_shoot`.
    fn parse_look(&mut self) -> Stmt {
        self.advance(); // skip 'look'
        let mut args = Vec::new();
        while !self.at_end() {
            let mut is_kw = true;
            // ScrOni `look <list> status <state>, <state>, ...` accepts
            // a fixed vocabulary of state keywords.  Each maps to the
            // legacy `scrStatus::k*` enum and is matched against the
            // candidate target in the look op's filter loop (see
            // `ops/combat.rs`).  Keywords arrive as their own
            // `TokenCode` variant (not generic identifiers), so each
            // one we accept here must be explicitly listed — otherwise
            // is_expr_start() returns false for them and the parser
            // trips the "Hanging value" error.
            match self.code() {
                TokenCode::Status => args.push(Expr::StringLit("status".to_string())),
                TokenCode::Alive => args.push(Expr::StringLit("alive".to_string())),
                TokenCode::Enemy => args.push(Expr::StringLit("enemy".to_string())),
                TokenCode::Fighting => args.push(Expr::StringLit("fighting".to_string())),
                TokenCode::Player => args.push(Expr::StringLit("player".to_string())),
                TokenCode::Hanging => args.push(Expr::StringLit("hanging".to_string())),
                TokenCode::Creature => args.push(Expr::StringLit("creature".to_string())),
                TokenCode::HasHitch => args.push(Expr::StringLit("hashitch".to_string())),
                _ => is_kw = false,
            }
            if is_kw {
                self.advance();
            } else {
                if !is_expr_start(self.code()) {
                    break;
                }
                args.push(self.parse_expr());
            }
            self.skip_if(TokenCode::Comma);
        }
        Stmt::Look { args }
    }

    fn parse_shoot(&mut self) -> Stmt {
        self.advance(); // skip 'shoot'
        let mut args = Vec::new();
        while !self.at_end() {
            let mut is_kw = true;
            match self.code() {
                TokenCode::Miss => args.push(Expr::StringLit("miss".to_string())),
                TokenCode::Bullets => args.push(Expr::StringLit("bullets".to_string())),
                TokenCode::Rate => args.push(Expr::StringLit("rate".to_string())),
                TokenCode::For => args.push(Expr::StringLit("for".to_string())),
                _ => is_kw = false,
            }
            if is_kw {
                self.advance();
            } else {
                if !is_expr_start(self.code()) {
                    break;
                }
                args.push(self.parse_expr());
            }
            self.skip_if(TokenCode::Comma);
        }
        Stmt::Shoot { args }
    }

    fn parse_ambient_sound(&mut self) -> Stmt {
        self.advance(); // skip 'ambientsound'
        let mut args = Vec::new();
        while !self.at_end() {
            let mut is_kw = true;
            match self.code() {
                TokenCode::All => args.push(Expr::StringLit("all".to_string())),
                TokenCode::Volume => args.push(Expr::StringLit("volume".to_string())),
                TokenCode::Pitch => args.push(Expr::StringLit("pitch".to_string())),
                TokenCode::PitchRamp => args.push(Expr::StringLit("pitchramp".to_string())),
                TokenCode::VolumeRamp => args.push(Expr::StringLit("volumeramp".to_string())),
                // `Stop` / `Play` are dedicated keyword tokens but inside
                // an `AmbientSound X Stop` statement they act as action
                // literals — mirror how `Stmt::Sound` handles them.
                TokenCode::Stop => args.push(Expr::StringLit("stop".to_string())),
                TokenCode::Play => args.push(Expr::StringLit("play".to_string())),
                _ => is_kw = false,
            }

            if is_kw {
                self.advance();
            } else {
                if !is_expr_start(self.code()) {
                    break;
                }
                args.push(self.parse_expr());
            }
            self.skip_if(TokenCode::Comma);
        }
        Stmt::AmbientSound { args }
    }

    fn parse_control_head(&mut self) -> Stmt {
        // Grammar:
        //   controlhead trackactor <guid>
        //   controlhead trackpos <vector>
        //   controlhead trackclosest
        //   controlhead scan [range <value>] [in <value>]
        //   controlhead set <value>
        //   controlhead disable
        // Exactly one mode keyword follows the verb.  Earlier versions of
        // this parser looped over keyword tokens indefinitely and swallowed
        // unrelated `set`/`in`/`range` from later statements (e.g. the
        // `set rand to RandomRange(0,1)` on the next line of
        // scavenger.oni:Scv_Recognition).
        self.advance(); // skip 'controlhead'
        let mut args = Vec::new();
        let mode = self.code();
        let mode_str = match mode {
            TokenCode::TrackActor => "trackactor",
            TokenCode::TrackPos => "trackpos",
            TokenCode::TrackClosest => "trackclosest",
            TokenCode::Scan => "scan",
            TokenCode::Set => "set",
            TokenCode::Disable => "disable",
            _ => {
                // Unknown mode — leave args empty, don't advance.  The
                // fallthrough dispatcher will report it if needed.
                return Stmt::ControlHead { args };
            }
        };
        args.push(Expr::StringLit(mode_str.to_string()));
        self.advance();

        match mode {
            TokenCode::TrackActor | TokenCode::TrackPos | TokenCode::Set => {
                if is_expr_start(self.code()) {
                    args.push(self.parse_expr());
                }
            }
            TokenCode::Scan => {
                if self.skip_if(TokenCode::Range) {
                    args.push(Expr::StringLit("range".to_string()));
                    args.push(self.parse_expr());
                }
                if self.skip_if(TokenCode::In) {
                    args.push(Expr::StringLit("in".to_string()));
                    args.push(self.parse_expr());
                }
            }
            // trackclosest / disable — no args
            _ => {}
        }
        Stmt::ControlHead { args }
    }

    fn parse_find(&mut self) -> Stmt {
        self.advance(); // skip 'find'
        let list_var = self.peek().text.clone().to_lowercase();
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

            let val = if is_expr_start(self.code()) {
                self.parse_expr()
            } else {
                Expr::IntLit(1) // Flag keywords like 'enemy' take no args
            };

            conditions.push((key, val));

            // Optional comma between conditions
            self.skip_if(TokenCode::Comma);
        }

        if self.skip_if(TokenCode::Range) {
            range = Some(self.parse_expr());
        }

        Stmt::Find {
            list_var,
            conditions,
            range,
        }
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

        Stmt::TextureMovie {
            name,
            pass,
            action,
            arg,
        }
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
            left = Expr::BinOp {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_and(&mut self) -> Expr {
        let mut left = self.parse_comparison();
        while self.code() == TokenCode::And {
            self.advance();
            let right = self.parse_comparison();
            left = Expr::BinOp {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
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
                    self.advance();
                    let first = self.parse_additive();

                    // Special-case `status <actor> is <state>[, <state>...]`.
                    // Legacy C++ `ParseStatusList` parses the RHS as a comma-separated
                    // list where each entry is an independent boolean
                    // PREDICATE against the actor (alive/enemy/knockeddown/
                    // etc.) — AND-reduced.  Equality-against-single-string
                    // can't express this because `status(X)` returns one
                    // state; rewrite the LHS expression tree so each state
                    // becomes `status_is(actor, "<state>")` and AND them.
                    //
                    // Detection: the LHS we just built is `Expr::Call { name:
                    // "status", args: [actor_expr] }` (see parse_call_expr
                    // for the `Status` token branch).  Pull the actor out
                    // and build per-state predicates.  Other `is` uses fall
                    // through to equality.
                    if let Expr::Call { name, args } = &left
                        && name.eq_ignore_ascii_case("status")
                        && args.len() == 1
                    {
                        let actor = args[0].clone();
                        let state_to_name = |e: Expr| match e {
                            Expr::StringLit(s) => s,
                            Expr::Var(s) => s,
                            Expr::Call { name, .. } => name,
                            other => format!("{:?}", other),
                        };
                        let first_state = state_to_name(first);
                        let mut acc = Expr::Call {
                            name: "status_is".to_string(),
                            args: vec![actor.clone(), Expr::StringLit(first_state)],
                        };
                        while self.skip_if(TokenCode::Comma) {
                            let more = self.parse_additive();
                            let more_state = state_to_name(more);
                            let pred = Expr::Call {
                                name: "status_is".to_string(),
                                args: vec![actor.clone(), Expr::StringLit(more_state)],
                            };
                            acc = Expr::BinOp {
                                op: BinOp::And,
                                left: Box::new(acc),
                                right: Box::new(pred),
                            };
                        }
                        left = acc;
                    } else {
                        // Non-status `is`: plain equality.  No comma tail
                        // expected here — if one appears, let the outer
                        // parser handle it.
                        left = Expr::BinOp {
                            op: BinOp::Equal,
                            left: Box::new(left),
                            right: Box::new(first),
                        };
                    }
                    continue;
                }
                _ => break,
            };
            self.advance();
            let right = self.parse_additive();
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        left
    }

    fn parse_additive(&mut self) -> Expr {
        let mut left = self.parse_multiplicative();
        loop {
            let op = match self.code() {
                TokenCode::Plus => BinOp::Add,
                TokenCode::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative();
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
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
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
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
            if self.code() == TokenCode::Identifier
                || (!matches!(
                    self.code(),
                    TokenCode::IntegerConstant
                        | TokenCode::FloatConstant
                        | TokenCode::StringConstant
                        | TokenCode::Eof
                        | TokenCode::Begin
                        | TokenCode::End
                        | TokenCode::Whenever
                        | TokenCode::Sequence
                        | TokenCode::Comma
                        | TokenCode::LeftParen
                        | TokenCode::RightParen
                        | TokenCode::LeftCurlyBracket
                        | TokenCode::RightCurlyBracket
                        | TokenCode::Colon
                        | TokenCode::Period
                        | TokenCode::Plus
                        | TokenCode::Minus
                        | TokenCode::Star
                        | TokenCode::Slash
                        | TokenCode::Percent
                        | TokenCode::Equal
                        | TokenCode::NotEqual
                        | TokenCode::Greater
                        | TokenCode::GreaterOrEqual
                        | TokenCode::Less
                        | TokenCode::LessOrEqual
                        | TokenCode::Cross
                ))
            {
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
            TokenCode::Location
            | TokenCode::Direction
            | TokenCode::Distance
            | TokenCode::Health
            | TokenCode::Guid
            | TokenCode::Status
            | TokenCode::Random
            | TokenCode::RandomRange
            | TokenCode::RandomRangeFloat
            | TokenCode::LineOfSight
            | TokenCode::Heard
            | TokenCode::OnCamera
            | TokenCode::GetCurvePhase
            | TokenCode::GetCurveReachedEnd
            | TokenCode::GetCurveReachedStart
            | TokenCode::GetCurveReachedGoto
            | TokenCode::GetFaction
            | TokenCode::Exists
            | TokenCode::Magnitude
            | TokenCode::Normalize
            | TokenCode::Sin
            | TokenCode::Cos
            | TokenCode::Sqrt
            | TokenCode::DeltaHeight
            | TokenCode::NavPoint
            | TokenCode::Path
            | TokenCode::IncomingAttack
            | TokenCode::IncomingAttackTime
            | TokenCode::SuccessiveAttacks
            | TokenCode::GetActiveWeapon
            | TokenCode::GetWeaponAmmo
            | TokenCode::GetWeaponType
            | TokenCode::GetScript
            | TokenCode::IsDone
            | TokenCode::IsHome
            | TokenCode::ActorEnabled
            | TokenCode::GetNumKnockdowns
            | TokenCode::ReceiveMessage
            | TokenCode::ReceiveAction
            | TokenCode::GetCheckPointIndex
            | TokenCode::GetDetectedActor
            | TokenCode::GetUIItemValue
            | TokenCode::GetLastHitType
            | TokenCode::BlockingCommandFailed
            | TokenCode::IsRestricted
            | TokenCode::First
            | TokenCode::Next
            | TokenCode::Size
            | TokenCode::MakeString
            | TokenCode::Damage
            | TokenCode::Min
            | TokenCode::Max
            | TokenCode::Alive
            | TokenCode::Fighting
            | TokenCode::Attacking
            | TokenCode::Blocking
            | TokenCode::KnockedDown
            | TokenCode::Armed
            | TokenCode::Asleep
            | TokenCode::Dormant
            | TokenCode::Crouched
            | TokenCode::WeaponDrawn
            | TokenCode::Grappled
            | TokenCode::Grappling
            | TokenCode::GrapplingMe
            | TokenCode::TriggerInside
            | TokenCode::TriggerOutside
            | TokenCode::TriggerEntered
            | TokenCode::TriggerExited
            | TokenCode::TriggerGetActorsInside
            | TokenCode::PlayAmbientSound
            | TokenCode::AmbientSoundStatus => self.parse_call_expr(),
            _ => {
                if !matches!(
                    self.code(),
                    TokenCode::IntegerConstant
                        | TokenCode::FloatConstant
                        | TokenCode::StringConstant
                        | TokenCode::Eof
                        | TokenCode::Begin
                        | TokenCode::End
                        | TokenCode::Whenever
                        | TokenCode::Sequence
                        | TokenCode::Comma
                        | TokenCode::LeftParen
                        | TokenCode::RightParen
                        | TokenCode::LeftCurlyBracket
                        | TokenCode::RightCurlyBracket
                        | TokenCode::Colon
                        | TokenCode::Period
                        | TokenCode::Plus
                        | TokenCode::Minus
                        | TokenCode::Star
                        | TokenCode::Slash
                        | TokenCode::Percent
                        | TokenCode::Equal
                        | TokenCode::NotEqual
                        | TokenCode::Greater
                        | TokenCode::GreaterOrEqual
                        | TokenCode::Less
                        | TokenCode::LessOrEqual
                        | TokenCode::Cross
                ) {
                    let name = self.peek().text.clone().to_lowercase();
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
                    } else if name == "true" {
                        Expr::IntLit(1)
                    } else if name == "false" {
                        Expr::IntLit(0)
                    } else {
                        Expr::Var(name)
                    }
                } else {
                    self.error(format!(
                        "expected expression, found {:?} '{}'",
                        self.code(),
                        self.peek().text
                    ));
                    self.advance();
                    Expr::IntLit(0)
                }
            }
        }
    }

    fn parse_call_expr(&mut self) -> Expr {
        let name = self.peek().text.clone().to_lowercase();
        self.advance();
        let mut args = Vec::new();
        if self.code() == TokenCode::LeftParen {
            self.advance();
            // Legacy ScrOni accepts both comma- and whitespace-
            // separated argument lists inside `( ... )`.  E.g.
            // `GetUIItemValue( "Choose_Level" "LevelList" )` from
            // newGame.oni:68 has no commas.  We consume optional
            // commas between args and stop at the closing paren or
            // EOF.  Each iteration must make progress (parse_expr
            // advances the token stream) — bail if it doesn't, to
            // avoid infinite loops on malformed input.
            while self.code() != TokenCode::RightParen && !self.at_end() {
                let pos_before = self.pos;
                args.push(self.parse_expr());
                self.skip_if(TokenCode::Comma);
                if self.pos == pos_before {
                    break;
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
                args.push(self.parse_unary());
            }
        }

        // Parse trailing 'with' kwargs for message query expressions
        if (name.eq_ignore_ascii_case("receivemessage")
            || name.eq_ignore_ascii_case("receiveaction"))
            && self.skip_if(TokenCode::With)
        {
            args.push(self.parse_expr());
            while self.skip_if(TokenCode::Comma) {
                args.push(self.parse_expr());
            }
        }

        if name.eq_ignore_ascii_case("playambientsound") {
            if self.skip_if(TokenCode::Volume) {
                if self.skip_if(TokenCode::LeftParen) {
                    args.push(self.parse_expr());
                    self.skip_if(TokenCode::RightParen);
                } else {
                    args.push(self.parse_expr());
                }
            }
            if self.skip_if(TokenCode::VolumeRamp) {
                args.push(Expr::StringLit("volumeramp".to_string()));
                let has_paren = self.skip_if(TokenCode::LeftParen);
                args.push(self.parse_expr());
                args.push(self.parse_expr());
                args.push(self.parse_expr());
                if has_paren {
                    self.skip_if(TokenCode::RightParen);
                }
            }
            if self.skip_if(TokenCode::PitchRamp) {
                args.push(Expr::StringLit("pitchramp".to_string()));
                let has_paren = self.skip_if(TokenCode::LeftParen);
                args.push(self.parse_expr());
                args.push(self.parse_expr());
                args.push(self.parse_expr());
                if has_paren {
                    self.skip_if(TokenCode::RightParen);
                }
            }
        }

        Expr::Call { name, args }
    }
}

fn is_var_type(code: TokenCode) -> bool {
    matches!(
        code,
        TokenCode::Integer
            | TokenCode::Float
            | TokenCode::Vector
            | TokenCode::StringKw
            | TokenCode::Timer
            | TokenCode::Label
            | TokenCode::ActorList
            | TokenCode::Child
    )
}

fn is_expr_start(code: TokenCode) -> bool {
    matches!(
        code,
        TokenCode::IntegerConstant
            | TokenCode::FloatConstant
            | TokenCode::StringConstant
            | TokenCode::Identifier
            | TokenCode::LeftParen
            | TokenCode::LeftCurlyBracket
            | TokenCode::Me
            | TokenCode::Player
            | TokenCode::Not
            | TokenCode::Minus
            | TokenCode::Location
            | TokenCode::Direction
            | TokenCode::Distance
            | TokenCode::Health
            | TokenCode::Guid
            | TokenCode::Status
            | TokenCode::Random
            | TokenCode::RandomRange
            | TokenCode::RandomRangeFloat
            | TokenCode::Exists
            | TokenCode::Magnitude
            | TokenCode::Normalize
            | TokenCode::GetCurvePhase
            | TokenCode::GetCurveReachedEnd
            | TokenCode::GetCurveReachedStart
            | TokenCode::GetCurveReachedGoto
            | TokenCode::LineOfSight
            | TokenCode::Heard
            | TokenCode::OnCamera
            | TokenCode::Sin
            | TokenCode::Cos
            | TokenCode::Sqrt
            | TokenCode::IsDone
            | TokenCode::IsHome
            | TokenCode::First
            | TokenCode::Next
            | TokenCode::Size
            | TokenCode::Damage
            | TokenCode::Min
            | TokenCode::Max
            | TokenCode::Alive
            | TokenCode::Fighting
            | TokenCode::Attacking
            | TokenCode::Blocking
            | TokenCode::KnockedDown
            | TokenCode::ReceiveMessage
            | TokenCode::ReceiveAction
            | TokenCode::MakeString
    )
}

fn is_command_start(code: TokenCode) -> bool {
    matches!(
        code,
        TokenCode::Set
            | TokenCode::If
            | TokenCode::Begin
            | TokenCode::Do
            | TokenCode::Exit
            | TokenCode::Done
            | TokenCode::Home
            | TokenCode::Log
            | TokenCode::GotoCurvePhase
            | TokenCode::SetCurvePhase
            | TokenCode::SetCurveSpeed
            | TokenCode::SetCurvePingPong
            | TokenCode::SetCurve
            | TokenCode::PlayAnimation
            | TokenCode::PlayActionAnimation
            | TokenCode::Idle
            | TokenCode::For
            | TokenCode::Face
            | TokenCode::Goto
            | TokenCode::Fight
            | TokenCode::Shoot
            | TokenCode::Patrol
            | TokenCode::Follow
            | TokenCode::Stack
            | TokenCode::Switch
            | TokenCode::ChildStack
            | TokenCode::ChildSwitch
            | TokenCode::SendMessage
            | TokenCode::Spawn
            | TokenCode::Destroy
            | TokenCode::End
            | TokenCode::Eof
            | TokenCode::DrawText
            | TokenCode::CameraFollowActor
            | TokenCode::CameraTrackActor
            | TokenCode::CameraTrackPoint
            | TokenCode::CameraMoveToActor
            | TokenCode::CameraMoveToPoint
            | TokenCode::CameraCutToActor
            | TokenCode::CameraCutToPoint
            | TokenCode::CameraSetFOV
            | TokenCode::CameraShake
            | TokenCode::Sound
            | TokenCode::AmbientSound
            | TokenCode::SetFogColor
            | TokenCode::SetFogClamp
            | TokenCode::SetFogPalettePower
            | TokenCode::SetFogType
            | TokenCode::SetShaderLocal
            | TokenCode::SetLightParameter
            | TokenCode::Intensity
            | TokenCode::RunGame
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_literal_with_plus() {
        let src = r#"
Script Q
begin
sequence
    vector p = location(player)
    set p to {p.x+1, p.y+2, p.z+3}
end
"#;
        Compiler::compile(src).expect("should compile");
    }

    #[test]
    fn vector_literal_with_minus() {
        let src = r#"
Script Q
begin
sequence
    vector p = location(player)
    set p to {p.x-1, p.y-2, p.z-3}
end
"#;
        Compiler::compile(src).expect("should compile");
    }

    #[test]
    fn vector_literal_simple_field_access() {
        // Narrower: just `{p.x, p.y, p.z}` — no arithmetic.
        let src = r#"
Script Q
begin
sequence
    vector p = location(player)
    set p to {p.x, p.y, p.z}
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        assert_eq!(file.scripts.len(), 1);
    }

    #[test]
    fn vector_literal_with_field_access_and_arith() {
        // Reproduction for movies.oni BossDefeated line ~93:
        //   CameraMoveToPoint {player_loc.x-5, player_loc.y+5, player_loc.z+5} time 5
        // Each element is `var.field ± literal`.  This used to fail at the
        // second comma / closing brace because something in the expression
        // chain swallowed a `,` it shouldn't.
        let src = r#"
Script BossDefeated
begin
sequence
    vector player_loc = location(player)
    CameraMoveToPoint {player_loc.x-5, player_loc.y+5, player_loc.z+5} time 5
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        assert_eq!(file.scripts.len(), 1);
        assert!(file.scripts[0].sequence.len() >= 2);
    }

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

    #[test]
    fn compile_takecover_bare() {
        // Bare `takecover` — the form scavenger_cover.oni:52 / :55 use.
        // Before the port, parser fell through to "Hanging value or
        // unrecognized command: 'takecover'" and the whole script
        // (Scv_Cover_Awareness) refused to compile.
        let src = r#"
Script Scv_Cover_Awareness
begin
sequence
    takecover
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        let body = &file.scripts[0].sequence;
        assert!(
            matches!(body.first(), Some(Stmt::TakeCover { duration: None })),
            "first stmt should be TakeCover with no duration, got {:?}",
            body.first()
        );
    }

    #[test]
    fn compile_takecover_with_duration() {
        // The C++ parser also accepts `takecover for <expr>`;
        // even though no
        // shipping script in the current corpus uses it, the parser
        // shouldn't reject it.
        let src = r#"
Script S
begin
sequence
    takecover for 5
end
"#;
        let file = Compiler::compile(src).expect("should compile");
        let body = &file.scripts[0].sequence;
        assert!(
            matches!(body.first(), Some(Stmt::TakeCover { duration: Some(_) })),
            "first stmt should be TakeCover with a duration, got {:?}",
            body.first()
        );
    }

    #[test]
    fn compile_takecover_retry_loop() {
        // The actual scavenger_cover.oni:48–56 control flow.  This is
        // the integration test for the full port: the script must
        // compile when takecover is used inside a `do while
        // blockingcommandfailed` retry loop with retreat fallbacks,
        // because before the port the unrecognized `takecover` would
        // poison the entire script (including the surrounding
        // do-while + retreat statements).
        let src = r#"
Script Scv_Cover_Awareness
begin
sequence
    controlhead disable
    takecover
    do while blockingcommandfailed begin
        retreat for 0.5
        takecover
    end
end
"#;
        Compiler::compile(src).expect("should compile");
    }

    #[test]
    fn compile_rungame_bare() {
        let src = r#"
Script S
begin
sequence
    do 1 times
    begin
        RunGame
        exit
    end
end
"#;
        Compiler::compile(src).expect("should compile");
    }

    #[test]
    fn compile_rungame_level_savepoint_with_getuiitemvalue() {
        // Reproduction for newGame.oni:68 — the form was failing
        // because (a) RunGame wasn't a recognized statement, and
        // (b) parse_call_expr required commas between arguments,
        // but the legacy author used whitespace-only separation.
        let src = r#"
Script RunLoadedLevel
begin
sequence
    do 1 times
    begin
        RunGame Level GetUIItemValue( "Choose_Level" "LevelList" ) SavePoint GetUIItemValue( "Choose_Level_Save_Point" "LevelSavePointList" )
        exit
    end
end
"#;
        Compiler::compile(src).expect("should compile");
    }

    #[test]
    fn compile_newgame_oni_real() {
        // The shipped newGame.oni for uitest layout — whole file used
        // to fail because of unrecognized RunGame/GetUIItemValue, and
        // because the file contains comma-less function calls and a
        // mix of CameraMotion / DoNothing / RunGame variants.
        crate::set_assets_path("../../oni2/zips/assets");
        let path = format!(
            "{}/layout/uitest/scripts/NewGame.oni",
            crate::get_assets_path()
        );
        let content = std::fs::read_to_string(path).unwrap();
        if let Err(errs) = Compiler::compile(&content) {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("Failed to compile:\n{joined}");
        }
    }

    #[test]
    fn test_compile_konoko_oni() {
        crate::set_assets_path("../../oni2/zips/assets");
        let path = format!(
            "{}/layout/M03_A01_Blast_Chambers/Scripts/konoko.oni",
            crate::get_assets_path()
        );
        let content = std::fs::read_to_string(path).unwrap();
        if let Err(errs) = Compiler::compile(&content) {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("Failed to compile:\n{joined}");
        }
    }
    #[test]
    fn test_compiler_multiline_comment_user_snippet() {
        let src = r#"
Script test_comment
begin
log "[a]"
##
log "[b]"
		if exists(blastInnerCog) then sendmessage "STPP" to blastInnerCog
		if exists(blastOuterCog) then sendmessage "STPP" to blastOuterCog
		if exists(BlastDoor) then sendmessage "CLSE" to BlastDoor
		if exists(blastGuard1) then sendmessage "OUCH" to blastGuard1
		if exists(blastGuard2) then sendmessage "OUCH" to blastGuard2
		if exists(blastGuard3) then sendmessage "OUCH" to blastGuard3
		set closedDoor to true
##		
log "[b2]"
end
"#;
        match Compiler::compile(src) {
            Ok(file) => {
                let script = &file.scripts[0];
                let has_log_b2 = script.sequence.iter().any(|stmt| {
                    if let super::Stmt::Log(exprs) = stmt {
                        if let Some(super::Expr::StringLit(s)) = exprs.get(0) {
                            return s == "[b2]";
                        }
                    }
                    false
                });
                assert!(
                    has_log_b2,
                    "Log [b2] not found in AST. Sequence length: {}",
                    script.sequence.len()
                );
            }
            Err(e) => {
                panic!("Compile errors: {:?}", e);
            }
        }
    }
}
