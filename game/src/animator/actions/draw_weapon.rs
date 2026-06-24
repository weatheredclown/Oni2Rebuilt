/*
 * animator/actions/draw_weapon.rs — DrawWeaponAction: draw and holster sequences.
 *
 * Implements the polymorphic Action trait for MainAction::DrawWeapon, handling
 * the drawing phase (on StartActionMessage) and the holstering phase (triggered
 * when the wielder requests to holster the weapon).
 *
 * Employs a 2-stage AnimSchedule to play the transition animations for both
 * Pistol and Rifle hold types:
 *   • Pistol Draw: ANIMWEAP_PISTOL_DRAW -> ANIMWEAP_PISTOL_DRAW_TO_FIRE
 *   • Pistol Holster: ANIMWEAP_PISTOL_FIRE_TO_HOLSTER -> ANIMWEAP_PISTOL_HOLSTER
 *   • Rifle Draw: ANIMWEAP_RIFLE_REACH -> ANIMWEAP_RIFLE_DRAW
 *   • Rifle Holster: ANIMWEAP_RIFLE_HOLST_STRT -> ANIMWEAP_RIFLE_HOLST_END
 *
 * Falls back to instant state changes for HoldType::Pipe, HoldType::Invalid,
 * or when the animation library lacks the required aliases.
 */
use super::{Action, ActionCtx, ActionUpdate};
use crate::oni2_loader::animation::{AnimId, Oni2AnimLibrary, Oni2AnimState};
use crate::animator::components::{
    action_flags, pending_flags, sub_state_1, MainAction, WeaponState,
};
use crate::animator::schedule::{AnimSchedule, AnimScheduleEntry};
use crate::weapons::components::HoldType;

pub struct DrawWeaponAction {
    substate: i32,
    is_holstering: bool,
}

impl Default for DrawWeaponAction {
    fn default() -> Self {
        Self {
            substate: 0,
            is_holstering: false,
        }
    }
}

impl DrawWeaponAction {
    pub fn new_holster() -> Self {
        Self {
            substate: 0,
            is_holstering: true,
        }
    }
}

impl Action for DrawWeaponAction {
    fn main_action(&self) -> MainAction {
        MainAction::DrawWeapon
    }

    fn can_enter(&self, ctx: &ActionCtx<'_>, _subaction: i32) -> bool {
        if self.is_holstering {
            ctx.ap.check_flags(action_flags::WEAPON)
        } else {
            if ctx.ap.find_at_least_one_flag(action_flags::REJECTLIST_DRAWWEAPON) {
                return false;
            }
            if ctx.ap.is_ending_crouch() {
                return false;
            }
            true
        }
    }

    fn on_enter(&mut self, ctx: &mut ActionCtx<'_>, subaction: i32) {
        self.substate = subaction;

        let fightstance = ctx.ap.check_flags(action_flags::FIGHTSTANCE);
        let hold_type = ctx.weapon_hold_type;

        if !self.is_holstering {
            // --- DRAWING ---
            ctx.ap.flags |= action_flags::WEAPON;
            ctx.ap.set_pending(pending_flags::DRAW | pending_flags::ARM_IK);

            // Weapon-ready idle the gait selector holds while drawn, so the
            // pistol/rifle stays up after DRAW_TO_FIRE instead of dropping to
            // ANIMNAV_STAND (the legacy ANIMSTACK_WEAPON idle overlay).
            ctx.ap.weapon_stand_anim = match hold_type {
                HoldType::Pistol => Some(AnimId::new("ANIMWEAP_PISTOL_STAND")),
                HoldType::Rifle => Some(AnimId::new("ANIMWEAP_RIFLE_STAND")),
                _ => None,
            };

            let schedule_opt = match hold_type {
                HoldType::Pistol => {
                    let first = if fightstance {
                        "ANIMWEAP_PISTOL_DRAW_STRT_FIGHT"
                    } else {
                        "ANIMWEAP_PISTOL_DRAW"
                    };
                    let second = "ANIMWEAP_PISTOL_DRAW_TO_FIRE";
                    Some((first, second))
                }
                HoldType::Rifle => {
                    let first = if fightstance {
                        "ANIMWEAP_RIFLE_REACH_FIGHT"
                    } else {
                        "ANIMWEAP_RIFLE_REACH"
                    };
                    let second = "ANIMWEAP_RIFLE_DRAW";
                    Some((first, second))
                }
                _ => None,
            };

            let mut scheduled = false;
            if let Some((first_alias, second_alias)) = schedule_opt {
                if ctx.lib.anims.contains_key(&AnimId::new(first_alias))
                    && ctx.lib.anims.contains_key(&AnimId::new(second_alias))
                {
                    let rec1 = AnimScheduleEntry::new(first_alias, sub_state_1::WEAPON_ACTION);
                    let rec2 = AnimScheduleEntry::new(second_alias, sub_state_1::WEAPON_GET_FROM_HOLSTER);
                    let mut schedule = AnimSchedule::new(vec![rec1, rec2]);

                    if let Some(first) = schedule.entries.first().cloned() {
                        if ctx.lib.play(&first.alias, ctx.anim_state) {
                            ctx.anim_state.speed_multiplier = first.rate;
                            ctx.anim_state.looping = first.hold_at_end;
                            ctx.ap.record_new_substate_1(first.substate_1);
                            schedule.mark_first_played();
                            *ctx.schedule = schedule;
                            scheduled = true;
                        }
                    }
                }
            }

            if !scheduled {
                // Fallback for Pipe, Invalid, or missing animations: instant draw
                ctx.ap.weapon_state = WeaponState::Drawn;
                ctx.ap.allow_targeting_ik = true;
                ctx.ap.clear_pending(pending_flags::DRAW | pending_flags::ARM_IK);
                ctx.schedule.clear();
            }
        } else {
            // --- HOLSTERING ---
            ctx.ap.allow_targeting_ik = false;
            // Weapon going away — drop the weapon-ready idle so the gait
            // selector returns to the unarmed stand.
            ctx.ap.weapon_stand_anim = None;

            let schedule_opt = match hold_type {
                HoldType::Pistol => {
                    let first = "ANIMWEAP_PISTOL_FIRE_TO_HOLSTER";
                    let second = if fightstance {
                        "ANIMWEAP_PISTOL_HOLST_END_FIGHT"
                    } else {
                        "ANIMWEAP_PISTOL_HOLSTER"
                    };
                    Some((first, second))
                }
                HoldType::Rifle => {
                    let first = "ANIMWEAP_RIFLE_HOLST_STRT";
                    let second = if fightstance {
                        "ANIMWEAP_RIFLE_HOLST_END_FIGHT"
                    } else {
                        "ANIMWEAP_RIFLE_HOLST_END"
                    };
                    Some((first, second))
                }
                _ => None,
            };

            let mut scheduled = false;
            if let Some((first_alias, second_alias)) = schedule_opt {
                if ctx.lib.anims.contains_key(&AnimId::new(first_alias))
                    && ctx.lib.anims.contains_key(&AnimId::new(second_alias))
                {
                    ctx.ap.set_pending(pending_flags::HOLSTER);

                    let rec1 = AnimScheduleEntry::new(first_alias, sub_state_1::WEAPON_ACTION);
                    let rec2 = AnimScheduleEntry::new(second_alias, sub_state_1::WEAPON_DROP_TO_HOLSTER);
                    let mut schedule = AnimSchedule::new(vec![rec1, rec2]);

                    if let Some(first) = schedule.entries.first().cloned() {
                        if ctx.lib.play(&first.alias, ctx.anim_state) {
                            ctx.anim_state.speed_multiplier = first.rate;
                            ctx.anim_state.looping = first.hold_at_end;
                            ctx.ap.record_new_substate_1(first.substate_1);
                            schedule.mark_first_played();
                            *ctx.schedule = schedule;
                            scheduled = true;
                        }
                    }
                }
            }

            if !scheduled {
                // Fallback for Pipe, Invalid, or missing animations: instant holster
                ctx.ap.flags &= !action_flags::WEAPON;
                ctx.ap.weapon_state = WeaponState::Holstered;
                ctx.ap.clear_pending(pending_flags::HOLSTER);
                ctx.schedule.clear();
            }
        }
    }

    fn update(&mut self, ctx: &mut ActionCtx<'_>, _dt: f32) -> ActionUpdate {
        if ctx.schedule.is_empty() {
            return ActionUpdate::Finished;
        }
        ActionUpdate::Continue
    }

    fn on_exit(&mut self, ctx: &mut ActionCtx<'_>) {
        if self.is_holstering {
            ctx.ap.flags &= !action_flags::WEAPON;
            ctx.ap.weapon_state = WeaponState::Holstered;
            ctx.ap.allow_targeting_ik = false;
            ctx.ap.clear_pending(pending_flags::HOLSTER);
        } else {
            ctx.ap.clear_pending(pending_flags::DRAW | pending_flags::ARM_IK);
        }
    }
}
