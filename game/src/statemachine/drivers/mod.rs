/*
 * statemachine/drivers/mod.rs — concrete SmDriver implementations.
 *
 * Each submodule implements `SmDriver` for one of ONI2's state-machine
 * dialects.  They share the generic core (states/rules/eval loop) from
 * `super::core` and pick the matching `.fsm` parser dialect.
 *
 *   input              — pad-driven player FSM (player.fsm/enemy.fsm; uses
 *                        `parse_input_fsm`)
 *   fight              — AI cooperative-fighting coordinator (fight.fsm;
 *                        uses `parse_aifight_sm`)
 *   squad              — squad-level coordinator (squad.fsm; uses
 *                        `parse_aifight_sm`)
 *   attack             — per-attack stateful action scheduler (.atk files;
 *                        own format-2 parser in `atk_parser`)
 *   animator           — top-level character mode orchestrator (uses
 *                        `parse_input_fsm` against the embedded
 *                        ANIMATOR_FSM string)
 *   behavior           — behavior-layer state machine (uses
 *                        `parse_input_fsm` against the embedded
 *                        BEHAVIOR_FSM string)
 *   parse              — shared lexer + driver-callback type aliases
 *   parse_input_fsm    — `#STATENAME` + `if Event { ... }` grammar
 *   parse_aifight_sm   — outer-`{}` + `S_NAME { ... }` grammar
 */

pub mod animator;
pub mod atk_parser;
pub mod attack;
pub mod behavior;
pub mod fight;
pub mod input;
pub mod parse;
pub mod parse_aifight_sm;
pub mod parse_input_fsm;
pub mod squad;
