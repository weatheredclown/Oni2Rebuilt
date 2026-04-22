/*
 * common/layered.rs — LayeredValue: stack-of-overrides primitive.
 *
 * Replaces the "everyone writes to a single field, someone else must
 * remember to reset it" anti-pattern.  Each contributor `push`es a
 * layer tagged with a string key; `remove` pops that specific layer
 * regardless of stack position; `current()` returns the top layer
 * (or `base` when the stack is empty).
 *
 * Use cases:
 *   • gravity     — jump pushes a scaled factor, cutscene might push
 *                   a freeze factor; either can be removed independently
 *                   without clobbering the other
 *   • speed       — slow-time fx pushes 0.5; stun overlay pushes 0.2;
 *                   whoever ends last wins naturally
 *   • invulnerability, control-lockouts, etc.
 *
 * Semantic: **last push wins**.  When multiple layers are active at
 * the same time, `current()` returns the most recently pushed one.
 * This matches game-feel expectations — the newest modifier is "in
 * effect right now."  Remove preserves this: older layers remain in
 * their original relative order.
 *
 * For cases needing priority order (e.g. "death override ALWAYS wins
 * over cutscene"), compose by checking a sentinel layer before
 * consulting `current()`.  Most gameplay cases don't need priority.
 */

/// Stack-of-overrides wrapper for a base value.  Contributors tag their
/// pushes with a string key so they can remove their own layer without
/// knowing who else has contributed.
///
/// Deliberately keeps `layers` as a `Vec<(String, T)>` rather than a
/// `HashMap` — preservation of push order is load-bearing (last-wins
/// semantic), and the per-entity layer count is almost always small
/// (1-3 active layers max).  Linear scan beats HashMap for both.
#[derive(Clone, Debug)]
pub struct LayeredValue<T: Clone> {
    base: T,
    layers: Vec<(String, T)>,
}

impl<T: Clone> LayeredValue<T> {
    /// Create a new layered value with just the base (no overrides).
    pub fn new(base: T) -> Self {
        Self {
            base,
            layers: Vec::new(),
        }
    }

    /// Read the currently-effective value.  Returns the most recently
    /// pushed layer's value, or `base` when no layers are active.
    pub fn current(&self) -> &T {
        self.layers.last().map(|(_, v)| v).unwrap_or(&self.base)
    }

    /// The base value without any active overrides.  Exposed for
    /// diagnostics and for systems that want to distinguish "no one has
    /// overridden" from "someone pushed the base value as an override."
    pub fn base(&self) -> &T {
        &self.base
    }

    /// Change the base value.  Doesn't affect active layers — they
    /// continue to override until removed.
    pub fn set_base(&mut self, base: T) {
        self.base = base;
    }

    /// Push a layer tagged with `key`.  If a layer with the same key
    /// already exists, it is REPLACED (and promoted to the top of the
    /// stack) — idempotent by design, so systems that re-push every
    /// frame don't leak layers.
    pub fn push(&mut self, key: impl Into<String>, value: T) {
        let key = key.into();
        let replaced = self.layers.iter().any(|(k, _)| k == &key);
        self.layers.retain(|(k, _)| k != &key);
        self.layers.push((key.clone(), value));
        bevy::log::info!(
            "LayeredValue::push('{}') {} — layer_count now {}",
            key,
            if replaced {
                "replaced existing"
            } else {
                "new layer"
            },
            self.layers.len(),
        );
    }

    /// Remove the layer with the given key if present.  No-op if
    /// nobody has pushed a layer with that key.  Returns the removed
    /// value for diagnostics.
    pub fn remove(&mut self, key: &str) -> Option<T> {
        let idx = self.layers.iter().position(|(k, _)| k == key);
        match idx {
            Some(i) => {
                let (_, v) = self.layers.remove(i);
                bevy::log::info!(
                    "LayeredValue::remove('{}') — layer_count now {}",
                    key,
                    self.layers.len(),
                );
                Some(v)
            }
            None => {
                bevy::log::info!(
                    "LayeredValue::remove('{}') — NOT FOUND (layer_count stays {})",
                    key,
                    self.layers.len(),
                );
                None
            }
        }
    }

    /// True iff any layer is currently active.
    pub fn is_overridden(&self) -> bool {
        !self.layers.is_empty()
    }

    /// True iff a specific keyed layer is currently active.
    pub fn has_layer(&self, key: &str) -> bool {
        self.layers.iter().any(|(k, _)| k == key)
    }

    /// Number of active layers — mostly for diagnostics.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Iterator over active layers, bottom of stack to top.  Exposed
    /// for diagnostics / change-logging — prefer `current()` for the
    /// effective value in normal use.
    pub fn layers_iter(&self) -> impl Iterator<Item = &(String, T)> {
        self.layers.iter()
    }

    /// Clear all layers, leaving only the base.  Use sparingly — the
    /// point of this type is to NOT have "reset everything" semantics;
    /// `remove(key)` is almost always what you want.  Exposed for
    /// despawn-cleanup / scene-reset paths.
    pub fn clear_all_layers(&mut self) {
        self.layers.clear();
    }
}

impl<T: Clone + Default> Default for LayeredValue<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// ---------------------------------------------------------------------------
// PartialEq only when T: PartialEq, so LayeredValue<f32> stays diff-able
// for change-detection in sync systems without requiring Eq.
// ---------------------------------------------------------------------------

impl<T: Clone + PartialEq> PartialEq for LayeredValue<T> {
    fn eq(&self, other: &Self) -> bool {
        self.base == other.base
            && self.layers.len() == other.layers.len()
            && self
                .layers
                .iter()
                .zip(other.layers.iter())
                .all(|(a, b)| a.0 == b.0 && a.1 == b.1)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_when_empty() {
        let v: LayeredValue<f32> = LayeredValue::new(1.0);
        assert_eq!(*v.current(), 1.0);
        assert!(!v.is_overridden());
    }

    #[test]
    fn push_overrides_base() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        assert_eq!(*v.current(), 3.2);
        assert!(v.is_overridden());
        assert!(v.has_layer("jump"));
    }

    #[test]
    fn remove_restores_base() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        let removed = v.remove("jump");
        assert_eq!(removed, Some(3.2));
        assert_eq!(*v.current(), 1.0);
        assert!(!v.is_overridden());
    }

    #[test]
    fn last_push_wins() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        v.push("cutscene", 0.0);
        assert_eq!(*v.current(), 0.0, "most recent layer takes effect");
        // Remove the cutscene; jump's layer is still there.
        v.remove("cutscene");
        assert_eq!(*v.current(), 3.2);
    }

    #[test]
    fn remove_middle_layer_preserves_order() {
        let mut v = LayeredValue::new(1.0);
        v.push("a", 10.0);
        v.push("b", 20.0);
        v.push("c", 30.0);
        v.remove("b");
        // `c` was most recently pushed, should still be top.
        assert_eq!(*v.current(), 30.0);
        v.remove("c");
        // `a` is the only remaining layer.
        assert_eq!(*v.current(), 10.0);
    }

    #[test]
    fn repush_same_key_is_idempotent_and_promotes() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        v.push("cutscene", 0.0);
        assert_eq!(*v.current(), 0.0);
        // Re-push "jump" — should replace the old entry and move to top.
        v.push("jump", 5.0);
        assert_eq!(v.layer_count(), 2, "no leak: still 2 layers");
        assert_eq!(*v.current(), 5.0, "re-pushed layer is now top");
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        assert_eq!(v.remove("ghost"), None);
        assert_eq!(*v.current(), 3.2, "unrelated layer untouched");
    }

    #[test]
    fn set_base_doesnt_affect_layers() {
        let mut v = LayeredValue::new(1.0);
        v.push("jump", 3.2);
        v.set_base(2.0);
        assert_eq!(*v.current(), 3.2, "layer still wins");
        v.remove("jump");
        assert_eq!(*v.current(), 2.0, "new base now visible");
    }

    #[test]
    fn clear_all_layers_resets_to_base() {
        let mut v = LayeredValue::new(1.0);
        v.push("a", 10.0);
        v.push("b", 20.0);
        v.clear_all_layers();
        assert_eq!(*v.current(), 1.0);
        assert!(!v.is_overridden());
    }
}
