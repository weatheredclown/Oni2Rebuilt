/*
 * fight/fx_table.rs — impact FX lookup keyed on (target, strength, class).
 *
 * Mirrors `crAttackFXSetTable` + `crAttackFXSetTableManager`
 * (rb/src/fight/fxtable.h:18, .cpp).  The C++ stores a fixed-size 3D table
 * indexed on attack target × strength × class; each cell holds a pointer to
 * an `fxEffectSet` describing the particle / sound / decal bundle to play
 * when an attack of that combination lands.
 *
 * The Rust port is a thin selector over the **already-wired** FX and audio
 * pipelines, both living in `fx_system.rs`:
 *   • `SpawnFx { name, ... }` resolves against `FxLibrary` /
 *     `ParticleLibrary` (oni2_loader/registries.rs).
 *   • `PlaySound { name, ... }` resolves against the loaded audio packages
 *     and TD manifest (oni2_loader/parsers/audiopackages.rs + td.rs).
 *
 * `FxSet` holds the string names that those downstream systems expect — so
 * populating a table is a matter of supplying real names that already exist
 * in the loaded libraries.  The table never owns assets; it just routes.
 *
 * Per-fighter-type tables are registered by name (e.g. "konoko", "striker")
 * so different characters can have different impact FX for the same hit.
 * A named "default" table is auto-registered at plugin build so lookups are
 * always structurally valid; cells are empty until the data/loader pipeline
 * populates them with real FX + sound names.
 */
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use crate::combat::components::{AttackClass, AttackStrength, AttackTarget};
use crate::fx_system::{PlaySound, SpawnFx};

// ---------------------------------------------------------------------------
// Table dimensions — keep in sync with the enum variants.
// ---------------------------------------------------------------------------

const NUM_TARGET: usize = 3; // Head, Body, Legs
const NUM_STRENGTH: usize = 3; // Low, High, Super
const NUM_CLASS: usize = 4; // Punch, Kick, Grab, RangedShot

fn target_idx(t: AttackTarget) -> usize {
    match t {
        AttackTarget::Head => 0,
        AttackTarget::Body => 1,
        AttackTarget::Legs => 2,
    }
}

fn strength_idx(s: AttackStrength) -> usize {
    match s {
        AttackStrength::Low => 0,
        AttackStrength::High => 1,
        AttackStrength::Super => 2,
    }
}

fn class_idx(c: AttackClass) -> usize {
    match c {
        AttackClass::Punch => 0,
        AttackClass::Kick => 1,
        AttackClass::Grab => 2,
        AttackClass::RangedShot => 3,
    }
}

// ---------------------------------------------------------------------------
// FxSet — one impact bundle
// ---------------------------------------------------------------------------

/// One named impact bundle.  Mirrors `fxEffectSet` (C++ fx subsystem).  The
/// fields hold lookup names that must already exist in the loaded libraries:
///   • `impact_fx`    → a name in `FxLibrary::effects` (spawns via `SpawnFx`)
///   • `impact_sound` → an audio-package clip name (routed via `PlaySound`)
///   • `decal`        → a decal name (reserved; no dispatch yet)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FxSet {
    pub name: String,
    pub impact_fx: Option<String>,
    pub impact_sound: Option<String>,
    pub decal: Option<String>,
}

impl FxSet {
    pub fn new(name: impl Into<String>) -> Self {
        FxSet {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn with_fx(mut self, name: impl Into<String>) -> Self {
        self.impact_fx = Some(name.into());
        self
    }

    pub fn with_sound(mut self, name: impl Into<String>) -> Self {
        self.impact_sound = Some(name.into());
        self
    }

    pub fn with_decal(mut self, name: impl Into<String>) -> Self {
        self.decal = Some(name.into());
        self
    }

    /// Fire this bundle's events into the live FX/audio pipelines.
    ///
    /// * `at`       — world-space position for the particle effect; passed
    ///                straight through to `SpawnFx.at` so the FX system can
    ///                attach it to the hit point.  Pass `None` to have the
    ///                FX follow whichever parent it's attached to.
    /// * `parent`   — entity to attach the spawned FX to (typically the
    ///                struck actor).  Propagated to `SpawnFx.parent`.
    /// * `sound_origin` — entity the sound is emitted from.  Used as
    ///                `PlaySound.script_entity` so 3D audio can position
    ///                the clip at the hit actor.
    pub fn dispatch(
        &self,
        commands: &mut Commands,
        at: Option<Vec3>,
        parent: Option<Entity>,
        sound_origin: Entity,
    ) {
        if let Some(fx_name) = &self.impact_fx {
            commands.trigger(SpawnFx {
                name: fx_name.clone(),
                at,
                parent,
                start_active: true,
            });
        }
        if let Some(sound_name) = &self.impact_sound {
            commands.trigger(PlaySound {
                script_entity: sound_origin,
                actor: None,
                name: sound_name.clone(),
            });
        }
        // decals: routed through a separate pipeline once one exists.
    }
}

// ---------------------------------------------------------------------------
// AttackFxTable — the 3D lookup
// ---------------------------------------------------------------------------

/// One named lookup table.  `name` is typically a fighter-type identifier
/// (e.g. "konoko", "striker") so different characters can have different
/// impact FX for the same hit class.
pub struct AttackFxTable {
    pub name: String,
    table: [[[Option<Arc<FxSet>>; NUM_CLASS]; NUM_STRENGTH]; NUM_TARGET],
    /// Fallback used when a specific cell is empty.  `None` means "no
    /// fallback — empty cells return None and the caller no-ops."
    pub fallback: Option<Arc<FxSet>>,
}

impl AttackFxTable {
    pub fn new(name: impl Into<String>) -> Self {
        AttackFxTable {
            name: name.into(),
            table: Default::default(),
            fallback: None,
        }
    }

    /// Insert an FX set at the given cell.  Overwrites any prior entry.
    pub fn set(
        &mut self,
        target: AttackTarget,
        strength: AttackStrength,
        class: AttackClass,
        fx: Arc<FxSet>,
    ) {
        self.table[target_idx(target)][strength_idx(strength)][class_idx(class)] = Some(fx);
    }

    /// Look up the FX set for a combo.  Returns the explicit cell entry if
    /// present, otherwise `fallback`, otherwise `None`.  Mirrors
    /// `crAttackFXSetTable::GetFXSet`.
    pub fn get(
        &self,
        target: AttackTarget,
        strength: AttackStrength,
        class: AttackClass,
    ) -> Option<&Arc<FxSet>> {
        self.table[target_idx(target)][strength_idx(strength)][class_idx(class)]
            .as_ref()
            .or(self.fallback.as_ref())
    }
}

// ---------------------------------------------------------------------------
// AttackFxRegistry — Bevy resource holding all named tables + sets
// ---------------------------------------------------------------------------

/// Bevy resource holding every named FxSet and AttackFxTable.  Mirrors
/// `crAttackFXSetTableManager` (fxtable.h:39).  Hit-detection systems look
/// up by table name (per fighter type) + the hit's (target, strength, class)
/// tuple, then call `fx_set.dispatch(...)` to fire the real events.
#[derive(Resource, Default)]
pub struct AttackFxRegistry {
    pub sets: HashMap<String, Arc<FxSet>>,
    pub tables: HashMap<String, Arc<AttackFxTable>>,
}

impl AttackFxRegistry {
    /// Register an `FxSet` and return the shareable handle.  Tables
    /// `set(...)` their cells using these handles.
    pub fn register_set(&mut self, fx: FxSet) -> Arc<FxSet> {
        let arc = Arc::new(fx);
        self.sets.insert(arc.name.clone(), arc.clone());
        arc
    }

    /// Register a fully-populated table.  Subsequent `lookup(...)` calls
    /// using `name` will hit it.
    pub fn register_table(&mut self, table: AttackFxTable) -> Arc<AttackFxTable> {
        let arc = Arc::new(table);
        self.tables.insert(arc.name.clone(), arc.clone());
        arc
    }

    /// Look up an FxSet for (table_name, target, strength, class).  Returns
    /// `None` if either the table or the cell is unpopulated AND no
    /// fallback is defined.
    pub fn lookup(
        &self,
        table_name: &str,
        target: AttackTarget,
        strength: AttackStrength,
        class: AttackClass,
    ) -> Option<&Arc<FxSet>> {
        self.tables
            .get(table_name)
            .and_then(|t| t.get(target, strength, class))
    }

    /// Look up by FxSet name directly.  Useful for one-off effects not tied
    /// to a particular attack combination.
    pub fn get_set(&self, name: &str) -> Option<&Arc<FxSet>> {
        self.sets.get(name)
    }
}

// ---------------------------------------------------------------------------
// Default seed — one empty table so lookups always have structural validity
// ---------------------------------------------------------------------------

/// Register an empty "default" table on `registry`.  Cells are unpopulated
/// — the data/loader pipeline is expected to fill them with real FxLibrary
/// / audio-package names per fighter type.  Having the table present means
/// `lookup("default", ...)` returns `None` cleanly rather than missing-key;
/// callers treat `None` as "no impact FX this combo" and no-op.
pub fn register_default(registry: &mut AttackFxRegistry) {
    registry.register_table(AttackFxTable::new("default"));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_is_structurally_present_but_empty() {
        let mut reg = AttackFxRegistry::default();
        register_default(&mut reg);
        // Table exists; every cell is None (no fallback configured).
        assert!(
            reg.lookup(
                "default",
                AttackTarget::Body,
                AttackStrength::Low,
                AttackClass::Punch
            )
            .is_none()
        );
    }

    #[test]
    fn unknown_table_returns_none() {
        let reg = AttackFxRegistry::default();
        assert!(
            reg.lookup(
                "no_such_fighter",
                AttackTarget::Body,
                AttackStrength::Low,
                AttackClass::Punch,
            )
            .is_none()
        );
    }

    #[test]
    fn explicit_cell_overrides_fallback() {
        let mut reg = AttackFxRegistry::default();
        let fallback = reg.register_set(FxSet::new("generic_impact"));
        let punch_low = reg.register_set(FxSet::new("konoko_punch_low"));

        let mut table = AttackFxTable::new("konoko");
        table.fallback = Some(fallback.clone());
        table.set(
            AttackTarget::Body,
            AttackStrength::Low,
            AttackClass::Punch,
            punch_low.clone(),
        );
        reg.register_table(table);

        // Explicit cell wins.
        assert_eq!(
            reg.lookup(
                "konoko",
                AttackTarget::Body,
                AttackStrength::Low,
                AttackClass::Punch
            )
            .map(|fx| fx.name.as_str()),
            Some("konoko_punch_low"),
        );
        // Other cells fall back.
        assert_eq!(
            reg.lookup(
                "konoko",
                AttackTarget::Body,
                AttackStrength::Low,
                AttackClass::Kick
            )
            .map(|fx| fx.name.as_str()),
            Some("generic_impact"),
        );
    }

    #[test]
    fn fx_set_has_no_names_by_default() {
        let fx = FxSet::new("empty");
        assert!(fx.impact_fx.is_none());
        assert!(fx.impact_sound.is_none());
        assert!(fx.decal.is_none());

        let populated = FxSet::new("kick")
            .with_fx("hit_kick_fx")
            .with_sound("sfx_kick_impact");
        assert_eq!(populated.impact_fx.as_deref(), Some("hit_kick_fx"));
        assert_eq!(populated.impact_sound.as_deref(), Some("sfx_kick_impact"));
    }
}
