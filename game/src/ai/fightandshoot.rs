/*
 * ai/fightandshoot.rs — shoot-vs-melee decision spine.
 *
 * Core port of `aiFightAndShoot` (rb/src/aifight/fightandshoot.cpp): the
 * behavior that decides, for an armed AI fighter, whether to SHOOT the target
 * at range or close in and MELEE.  The legacy class is a big state machine
 * (INIT/GETTING_CLOSE/FIGHTING/SHOOTING/DRAWING/HOLSTERING/COVER) wrapping a
 * Shooter + cover sub-behavior + disarm/grapple/targeting bookkeeping; this is
 * the live decision spine that drives the existing `ai_shooter_system` (fire)
 * vs. the melee fight FSM (position + swing):
 *
 *   • ShootDistance with hysteresis — `GetDistFightToShoot`/`GetDistShootToFight`
 *     (ShootDistance ± 0.5) so the mode doesn't chatter in the boundary band.
 *   • Unarmed / no target → always Fighting.
 *
 * The shooter only fires in `Shooting` mode; in `Fighting` mode it stands down
 * so the melee fight engages.
 *
 * TODO(fightandshoot): the deeper sub-features are unported — cover/roll
 * (aiShootCover), disarm (UpdateDisarming), grapple (UpdateGrappling), the
 * aggressiveness→attack-table swap (UpdateAttackTable: aggressive/medium/
 * cautious), weapon draw/holster animation states, and the distraction layer
 * (aiFightAndShootWithDistraction: main vs. temporary target).  ShootDistance
 * IS now sourced from the equipped weapon's `AiParameters::desired_range`
 * (modulated by attack_preference, GetShootDistance); the default constant is
 * only the fallback until a weapon resolves.
 */
use bevy::prelude::*;

use crate::ai::components::AiFighter;
use crate::inventory::components::Inventory;

/// Default ShootDistance (matches `AiParameters::desired_range` default).
/// TODO(fightandshoot): replace with the equipped weapon's `desired_range`.
const DEFAULT_SHOOT_DISTANCE: f32 = 8.0;

/// Hysteresis band around ShootDistance (legacy MAGIC 0.5).
const HYSTERESIS: f32 = 0.5;

/// Whether an armed AI fighter should shoot at range or close to melee.
/// Reduction of `aiFightAndShoot`'s state enum to the live shoot/fight choice.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum FightShootMode {
    /// Melee — target is within ShootDistance, or the fighter is unarmed.
    #[default]
    Fighting,
    /// Ranged — armed and the target is beyond ShootDistance.
    Shooting,
}

#[derive(Component, Debug)]
pub struct FightAndShoot {
    pub mode: FightShootMode,
    /// Boundary between shooting (beyond) and melee (within).
    pub shoot_distance: f32,
    /// 0..1; higher = more aggressive (shorter shoot distance, hotter table).
    pub aggressiveness: f32,
    /// Higher = prefers shooting (longer shoot distance) — `GetShootDistance`.
    pub attack_preference: f32,
}

impl Default for FightAndShoot {
    fn default() -> Self {
        FightAndShoot {
            mode: FightShootMode::Fighting,
            shoot_distance: DEFAULT_SHOOT_DISTANCE,
            aggressiveness: 0.5,
            attack_preference: 0.5,
        }
    }
}

/// Decide shoot vs. melee per armed AI fighter, with hysteresis.  Runs before
/// `ai_shooter_system`, which only fires when the mode is `Shooting`.
pub fn ai_fight_and_shoot_system(
    mut query: Query<(&AiFighter, &mut FightAndShoot, &GlobalTransform, Option<&Inventory>)>,
    transforms: Query<&GlobalTransform>,
    weapons: Query<&crate::weapons::components::Weapon>,
) {
    for (fighter, mut fas, self_tf, inv_opt) in &mut query {
        // Unarmed or no target → melee.
        let weapon_ent = inv_opt.and_then(|inv| inv.current_weapon_entity());
        let Some(target) = fighter.target.filter(|_| weapon_ent.is_some()) else {
            fas.mode = FightShootMode::Fighting;
            continue;
        };

        // Source ShootDistance from the equipped weapon's AI tuning
        // (`AiParameters::desired_range` on the active operational mode),
        // modulated by attack_preference (GetShootDistance: a fighter that
        // prefers shooting holds at a longer range).  Falls back to the
        // component's existing value if the weapon/mode can't be resolved.
        if let Some(we) = weapon_ent
            && let Ok(weapon) = weapons.get(we)
            && let Some(mode) = weapon.ty.op_modes.get(weapon.op_mode)
        {
            fas.shoot_distance = mode.ai_params.desired_range * (0.75 + 0.5 * fas.attack_preference);
        }
        let Ok(tgt_tf) = transforms.get(target) else {
            fas.mode = FightShootMode::Fighting;
            continue;
        };

        let dist = self_tf.translation().distance(tgt_tf.translation());
        let sd = fas.shoot_distance;
        // Hysteresis: switch to fight only when clearly inside, to shoot only
        // when clearly outside; hold the current mode in the ±0.5 band.
        fas.mode = match fas.mode {
            FightShootMode::Shooting if dist < sd - HYSTERESIS => FightShootMode::Fighting,
            FightShootMode::Fighting if dist > sd + HYSTERESIS => FightShootMode::Shooting,
            other => other,
        };
    }
}
