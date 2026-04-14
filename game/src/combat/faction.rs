use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Faction(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactionStatus {
    Friendly,
    Neutral,
    Enemy,
}

#[derive(Resource, Default)]
pub struct FactionManager {
    relations: HashMap<(String, String), FactionStatus>,
}

impl FactionManager {
    /// Gets the relationship status between two factions.
    /// By default, TCTF and Syndicate are recognized as Enemies.
    /// Otherwise returns Neutral if unrecognized.
    pub fn get_status(&self, f1: &str, f2: &str) -> FactionStatus {
        if f1 == f2 {
            return FactionStatus::Friendly;
        }

        let key1 = (f1.to_string(), f2.to_string());
        if let Some(&status) = self.relations.get(&key1) {
            return status;
        }
        
        let key2 = (f2.to_string(), f1.to_string());
        if let Some(&status) = self.relations.get(&key2) {
            return status;
        }

        // Hardcoded defaults commonly found in Oni levels
        if (f1.eq_ignore_ascii_case("tctf") && f2.eq_ignore_ascii_case("syndicate")) ||
           (f1.eq_ignore_ascii_case("syndicate") && f2.eq_ignore_ascii_case("tctf")) {
            return FactionStatus::Enemy;
        }

        FactionStatus::Neutral
    }

    pub fn set_relationship(&mut self, f1: &str, f2: &str, status: FactionStatus) {
        let key = (f1.to_string(), f2.to_string());
        self.relations.insert(key, status);
    }
}

pub struct FactionPlugin;

impl Plugin for FactionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionManager>();
    }
}
