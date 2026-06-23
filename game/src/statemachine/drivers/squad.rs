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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SquadOrder {
    #[default]
    Idle = 0,
    Fight = 1,
    Retreat = 2,
    Formation = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SquadEvent {
    OrderFight,
    OrderRetreat,
    OrderFormation,
    OrderIdle,
    TargetLeftCircle,
    MembersChanged,
    FighterLostPos,
    FighterReacting,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SquadAction {
    GlobalRetreat,
    GlobalFormation,
    Regroup,
    GlobalIdle,
    GlobalFight,
}

#[derive(Default, Clone, Debug)]
pub struct SquadCtx {
    pub order: SquadOrder,
    pub target_left_circle: bool,
    pub members_changed: bool,
    pub fighter_lost_pos: bool,
    pub fighter_reacting: bool,
}

#[derive(Default, Clone, Debug)]
pub struct SquadOutput {
    pub requested_actions: Vec<SquadAction>,
}

pub struct SquadDriver;

impl SmDriver for SquadDriver {
    type Event = SquadEvent;
    type Action = SquadAction;
    type Context = SquadCtx;
    type Output = SquadOutput;

    fn eval_event(ctx: &Self::Context, event: &Self::Event, _runtime: &SmRuntime<Self>) -> bool {
        match event {
            SquadEvent::OrderFight => ctx.order == SquadOrder::Fight,
            SquadEvent::OrderRetreat => ctx.order == SquadOrder::Retreat,
            SquadEvent::OrderFormation => ctx.order == SquadOrder::Formation,
            SquadEvent::OrderIdle => ctx.order == SquadOrder::Idle,
            SquadEvent::TargetLeftCircle => ctx.target_left_circle,
            SquadEvent::MembersChanged => ctx.members_changed,
            SquadEvent::FighterLostPos => ctx.fighter_lost_pos,
            SquadEvent::FighterReacting => ctx.fighter_reacting,
            SquadEvent::Always => true,
        }
    }

    fn apply_action(
        _ctx: &mut Self::Context,
        action: &Self::Action,
        output: &mut Self::Output,
        _adv: &mut SmAdvance<Self>,
    ) {
        output.requested_actions.push(action.clone());
    }
}

pub fn parse_squad_event(
    text: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<SquadEvent, String> {
    let t = text.trim();
    Ok(match t {
        "E_ORDER_FIGHT" => SquadEvent::OrderFight,
        "E_ORDER_RETREAT" => SquadEvent::OrderRetreat,
        "E_ORDER_FORMATION" => SquadEvent::OrderFormation,
        "E_ORDER_IDLE" => SquadEvent::OrderIdle,
        "E_TARGET_LEFT_CIRCLE" => SquadEvent::TargetLeftCircle,
        "E_MEMBERS_CHANGED" => SquadEvent::MembersChanged,
        "E_FIGHTER_LOST_POS" => SquadEvent::FighterLostPos,
        "E_FIGHTER_REACTING" => SquadEvent::FighterReacting,
        "E_ALWAYS" | "Always" | "True" | "" => SquadEvent::Always,
        other => return Err(format!("squad: unknown event '{}'", other)),
    })
}

pub fn parse_squad_action(
    line: &str,
    _state_index: &HashMap<String, usize>,
) -> Result<Option<SquadAction>, String> {
    let verb = line.trim();
    if verb.is_empty() {
        return Ok(None);
    }
    Ok(match verb {
        "A_GLOBAL_RETREAT" => Some(SquadAction::GlobalRetreat),
        "A_GLOBAL_FORMATION" => Some(SquadAction::GlobalFormation),
        "A_REGROUP" => Some(SquadAction::Regroup),
        "A_GLOBAL_IDLE" => Some(SquadAction::GlobalIdle),
        "A_GLOBAL_FIGHT" => Some(SquadAction::GlobalFight),
        _ => None,
    })
}

pub const SQUAD_EVENT_PARSER: EventParser<SquadDriver> = parse_squad_event;
pub const SQUAD_ACTION_PARSER: ActionParser<SquadDriver> = parse_squad_action;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statemachine::drivers::parse_aifight_sm::parse_aifight_sm;

    #[test]
    fn test_squad_fsm_parsing() {
        let content = r#"
{
	S_IDLING
	{
		E_ORDER_FIGHT		{						/ S_STARTING_FIGHT	}
		E_ORDER_RETREAT		{	A_GLOBAL_RETREAT	/ S_RETREATING		}
		E_ORDER_FORMATION	{	A_GLOBAL_FORMATION	/ S_IN_FORMATION	}
	}
	S_STARTING_FIGHT
	{
							{	A_REGROUP			/ S_FIGHTING		}
	}
	S_FIGHTING
	{
		E_ORDER_IDLE		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_RETREAT		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_FORMATION	{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_TARGET_LEFT_CIRCLE {	A_REGROUP			/					}
		E_MEMBERS_CHANGED	{	A_REGROUP			/					}
		E_FIGHTER_LOST_POS	{	A_REGROUP			/					}
		E_FIGHTER_REACTING	{	A_REGROUP			/					}
	}
	S_RETREATING
	{
		E_ORDER_IDLE		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_FIGHT		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_FORMATION	{	A_GLOBAL_IDLE		/ S_IDLING			}
	}
	S_IN_FORMATION
	{
		E_ORDER_IDLE		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_FIGHT		{	A_GLOBAL_IDLE		/ S_IDLING			}
		E_ORDER_RETREAT		{	A_GLOBAL_IDLE		/ S_IDLING			}
	}
}
"#;
        let sm = parse_aifight_sm::<SquadDriver>(content, SQUAD_EVENT_PARSER, SQUAD_ACTION_PARSER)
            .unwrap();
        assert_eq!(sm.states.len(), 5);
        assert!(sm.state_index.contains_key("S_IDLING"));
        assert!(sm.state_index.contains_key("S_STARTING_FIGHT"));
        assert!(sm.state_index.contains_key("S_FIGHTING"));
        assert!(sm.state_index.contains_key("S_RETREATING"));
        assert!(sm.state_index.contains_key("S_IN_FORMATION"));
    }
}
