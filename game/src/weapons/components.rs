use bevy::prelude::*;

// === Weapon Classification ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponKind {
    Melee,
    Ranged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponId {
    Fists,
    Pipe,
    Sword,
    Pistol,
    Rifle,
}

// === Weapon Stats ===

#[derive(Debug, Clone)]
pub struct WeaponStats {
    pub id: WeaponId,
    pub kind: WeaponKind,
    pub name: &'static str,
    // Melee modifiers (applied to ActiveAttack parameters)
    pub damage_multiplier: f32,
    pub range_extension: f32,
    pub speed_multiplier: f32,
    pub hit_radius_bonus: f32,
    // Ranged parameters
    pub projectile_speed: f32,
    pub projectile_damage: f32,
    pub fire_rate: f32,
    pub max_ammo: u32,
    pub reload_time: f32,
    pub projectile_lifetime: f32,
}

impl WeaponStats {
    pub fn fists() -> Self {
        Self {
            id: WeaponId::Fists,
            kind: WeaponKind::Melee,
            name: "Fists",
            damage_multiplier: 1.0,
            range_extension: 0.0,
            speed_multiplier: 1.0,
            hit_radius_bonus: 0.0,
            projectile_speed: 0.0,
            projectile_damage: 0.0,
            fire_rate: 0.0,
            max_ammo: 0,
            reload_time: 0.0,
            projectile_lifetime: 0.0,
        }
    }

    pub fn pipe() -> Self {
        Self {
            id: WeaponId::Pipe,
            kind: WeaponKind::Melee,
            name: "Pipe",
            damage_multiplier: 1.4,
            range_extension: -0.3,
            speed_multiplier: 0.9,
            hit_radius_bonus: 0.15,
            projectile_speed: 0.0,
            projectile_damage: 0.0,
            fire_rate: 0.0,
            max_ammo: 0,
            reload_time: 0.0,
            projectile_lifetime: 0.0,
        }
    }

    pub fn sword() -> Self {
        Self {
            id: WeaponId::Sword,
            kind: WeaponKind::Melee,
            name: "Sword",
            damage_multiplier: 1.8,
            range_extension: -0.7,
            speed_multiplier: 1.2,
            hit_radius_bonus: 0.1,
            projectile_speed: 0.0,
            projectile_damage: 0.0,
            fire_rate: 0.0,
            max_ammo: 0,
            reload_time: 0.0,
            projectile_lifetime: 0.0,
        }
    }

    pub fn pistol() -> Self {
        Self {
            id: WeaponId::Pistol,
            kind: WeaponKind::Ranged,
            name: "Pistol",
            damage_multiplier: 1.0,
            range_extension: 0.0,
            speed_multiplier: 1.0,
            hit_radius_bonus: 0.0,
            projectile_speed: 40.0,
            projectile_damage: 15.0,
            fire_rate: 2.0,
            max_ammo: 12,
            reload_time: 1.5,
            projectile_lifetime: 3.0,
        }
    }

    pub fn rifle() -> Self {
        Self {
            id: WeaponId::Rifle,
            kind: WeaponKind::Ranged,
            name: "Rifle",
            damage_multiplier: 1.0,
            range_extension: 0.0,
            speed_multiplier: 1.0,
            hit_radius_bonus: 0.0,
            projectile_speed: 60.0,
            projectile_damage: 8.0,
            fire_rate: 5.0,
            max_ammo: 30,
            reload_time: 2.0,
            projectile_lifetime: 3.0,
        }
    }
}

// === Components on Fighter Entities ===

#[derive(Component)]
pub struct EquippedWeapon {
    pub stats: WeaponStats,
    pub ammo: u32,
    pub reload_until: f64,
    pub last_fire_time: f64,
}

impl Default for EquippedWeapon {
    fn default() -> Self {
        let stats = WeaponStats::fists();
        Self {
            ammo: stats.max_ammo,
            reload_until: 0.0,
            last_fire_time: 0.0,
            stats,
        }
    }
}

impl EquippedWeapon {
    pub fn from_stats(stats: WeaponStats) -> Self {
        Self {
            ammo: stats.max_ammo,
            reload_until: 0.0,
            last_fire_time: 0.0,
            stats,
        }
    }

    pub fn is_fists(&self) -> bool {
        self.stats.id == WeaponId::Fists
    }

    pub fn is_ranged(&self) -> bool {
        self.stats.kind == WeaponKind::Ranged
    }
}

// === World Pickup Entity ===

#[derive(Component)]
pub struct WeaponPickup {
    pub stats: WeaponStats,
    pub base_y: f32,
}

// === Projectile Entity ===

#[derive(Component)]
pub struct Projectile {
    pub owner: Entity,
    pub damage: f32,
    pub spawn_time: f64,
    pub lifetime: f32,
}

#[derive(Component)]
pub struct ProjectileVelocity(pub Vec3);

// === Visual Markers ===

#[derive(Component)]
pub struct WeaponVisual;

// === Materials Resource ===

#[derive(Resource)]
pub struct WeaponMaterials {
    pub pipe_mesh: Handle<Mesh>,
    pub pipe_mat: Handle<StandardMaterial>,
    pub sword_mesh: Handle<Mesh>,
    pub sword_mat: Handle<StandardMaterial>,
    pub pistol_mesh: Handle<Mesh>,
    pub pistol_mat: Handle<StandardMaterial>,
    pub rifle_mesh: Handle<Mesh>,
    pub rifle_mat: Handle<StandardMaterial>,
    pub projectile_mesh: Handle<Mesh>,
    pub projectile_mat: Handle<StandardMaterial>,
    pub pickup_glow_mat: Handle<StandardMaterial>,
}
