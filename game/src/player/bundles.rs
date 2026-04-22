/*
 * player/bundles.rs — player-side spawn helpers.
 *
 * `PlayerIdentityBundle` groups every component that distinguishes the
 * player entity from AI creatures.  Used by both the layout-attach path
 * (player components merged onto an actor already loaded by the layout)
 * and the fallback capsule spawn.
 */
use bevy::prelude::*;
use uuid::Uuid;

use super::components::{InputState, Player};
use crate::combat::components::{Fighter, FighterId, Health};
use crate::combat::faction::Faction;

/// Player-specific identity + input + core fighter stats.  Applied
/// alongside `FighterBundle`, `CreaturePhysicsBundle`, and
/// `AnimatorBundle` to produce a fully functional player entity.
///
/// No `Default` impl on purpose — faction string and max HP are
/// gameplay-authored values that must be passed explicitly.  Use
/// [`PlayerIdentityBundle::new`].
#[derive(Bundle)]
pub struct PlayerIdentityBundle {
    pub player: Player,
    pub faction: Faction,
    pub input: InputState,
    pub fighter: Fighter,
    pub fighter_id: FighterId,
    pub health: Health,
}

impl PlayerIdentityBundle {
    /// Build a new player identity bundle with the given faction tag
    /// and starting/max HP.  Assigns a fresh UUID for `FighterId`.
    pub fn new(faction: impl Into<String>, max_hp: f32) -> Self {
        Self {
            player: Player,
            faction: Faction(faction.into()),
            input: InputState::default(),
            fighter: Fighter::default(),
            fighter_id: FighterId(Uuid::new_v4()),
            health: Health::new(max_hp),
        }
    }
}
