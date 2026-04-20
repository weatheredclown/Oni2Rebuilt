/*
 * statemachine/drivers/mod.rs — concrete SmDriver implementations.
 *
 * Each submodule implements `SmDriver` for one of ONI2's state-machine
 * dialects.  They share the generic core (states/rules/eval loop) from
 * `super::core` and the parser skeleton from `parse`.
 *
 *   input    — pad-driven player FSM (.fsm files; mirrors aiInputStateMachine)
 *   fight    — AI cooperative-fighting coordinator (.sm files; mirrors
 *              aiFightStateMachine; mostly stubbed pending aifight port)
 *   attack   — per-attack stateful action scheduler (.atk files; mirrors
 *              aiAttackStateMachine; stateful via the `AtkAction` trait)
 *   animator — top-level character mode orchestrator (jump/fall/ledge/etc;
 *              FSM text embedded in binary since assets ship via .dat)
 *   parse    — generic state/rule skeleton parser shared by all four
 */

pub mod animator;
pub mod attack;
pub mod fight;
pub mod input;
pub mod parse;
