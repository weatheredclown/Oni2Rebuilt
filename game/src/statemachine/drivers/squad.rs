/*
 * statemachine/drivers/squad.rs — SquadDriver stub for squad.fsm.
 *
 * squad.fsm in the legacy codebase is loaded as
 * a single global aiStateMachineData instance owned by aiFightManager.  It uses
 * the same FORMAT_ORIGINAL nested-brace syntax as fight.fsm (not the player/enemy
 * input-FSM syntax).  The parity port keeps the resource slot so the global
 * singleton is present even before the Format-2 parser and squad coordinator land.
 * Vocabulary is empty for now — events/actions are accepted as no-ops so the
 * cache can still eagerly hold the parsed (or empty) data.
 */
use std::collections::HashMap;

use super::super::core::{SmAdvance, SmDriver, SmRuntime};
use super::parse::{ActionParser, EventParser};

#[derive(Clone, Debug)]
pub enum SquadEvent {
    Always,
}

#[derive(Clone, Debug)]
pub enum SquadAction {}

#[derive(Default)]
pub struct SquadCtx;

#[derive(Default)]
pub struct SquadOutput;

pub struct SquadDriver;

impl SmDriver for SquadDriver {
    type Event = SquadEvent;
    type Action = SquadAction;
    type Context = SquadCtx;
    type Output = SquadOutput;

    fn eval_event(_ctx: &Self::Context, event: &Self::Event, _runtime: &SmRuntime<Self>) -> bool {
        match event {
            SquadEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        _action: &Self::Action,
        _output: &mut Self::Output,
        _adv: &mut SmAdvance<Self>,
    ) {
    }
}

pub fn parse_squad_event(
    _text: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<SquadEvent, String> {
    Ok(SquadEvent::Always)
}

pub fn parse_squad_action(
    _line: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<Option<SquadAction>, String> {
    Ok(None)
}

pub const SQUAD_EVENT_PARSER: EventParser<SquadDriver> = parse_squad_event;
pub const SQUAD_ACTION_PARSER: ActionParser<SquadDriver> = parse_squad_action;
