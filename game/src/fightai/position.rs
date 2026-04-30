/*
 * fightai/position.rs — port of `aifight::resources` (rb/src/aifight/
 * resources.{cpp,h}) plus the position/cookie helpers on `aiFighter`
 * (rb/src/aifight/fighter.cpp:966-1136).
 *
 * Two ECS components:
 *
 *   FightResources   — DEFENDER side.  Lives on every fighter.
 *                      Holds 8 directional position slots around the
 *                      fighter and a single cookie.  Each slot is a
 *                      priority queue: the highest-priority requester
 *                      is "offered" the slot every frame (peek, not
 *                      pop) — they stay in the queue until they Grab
 *                      it.  Diverges from legacy `aiFightPosition::Update`
 *                      which popped on offer; the legacy model required
 *                      multi-step-per-tick FSM evaluation to stay in
 *                      sync, which we don't have, so making the offer
 *                      a derived/persistent property of the queue head
 *                      avoids losing it across an FSM transit gap (e.g.
 *                      S_REQUESTING_POSITION → S_WAITING_FOR_POSITION
 *                      crossing a frame boundary).
 *                      Mirrors `aiFightResources` + `aiFightPosition`.
 *
 *   FightSlotState   — ATTACKER side.  Lives on every fighter.
 *                      Tracks which slot of which target this fighter
 *                      currently holds (`current_position` /
 *                      `resource_target`), pending offers from peers,
 *                      whether we hold a cookie, plus a deferred-op
 *                      queue the FSM writes into during its tick.
 *                      Mirrors the position/cookie fields on
 *                      `aiFighter` (rb/src/aifight/fighter.h:780).
 *
 * Two systems:
 *
 *   fight_resources_offer_system — runs FIRST.  Clears stale offers
 *      from FightSlotState, then for each FightResources pops the
 *      next-priority requester from each empty slot / cookie queue
 *      and writes the offer onto both ends (slot.offered_to AND the
 *      requester's offered_positions).
 *
 *   fight_resource_apply_system — runs LAST.  Drains the
 *      `pending_ops` queue every FightSlotState built up during
 *      `fight_runtime_update_system` and applies the cross-entity
 *      mutation (push to target's queue, claim a slot, release, …).
 *
 * The FSM's `FightCtx::has_position` / `can_attack` /
 * `position_offered` / `cookie_offered` fields are populated from
 * FightSlotState/FightResources by `fight_runtime_update_system`,
 * which is the single point where the FSM tick reads world state.
 *
 * Default behavior tweak vs. legacy:
 *   `aiFighter::RequestPosition` only enqueued when AssignedPosition
 *   was set externally (`>= 0`), but no legacy code ever calls
 *   `SetAssignedPosition`, so in vanilla play the queue is never
 *   populated and `E_POSITION_OFFERED`/`E_HAS_POSITION` never fire.
 *   Our `request_position` falls back to the slot CLOSEST to the
 *   attacker's current world position (port of
 *   `aiPrimaryDirections::ClosestDirection`).  This lets the FSM
 *   actually progress past S_STARTING_FIGHT.
 */
use bevy::prelude::*;

pub const NUM_POSITIONS: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One slot around a fighter.  Mirrors `aiFightPosition`
/// (rb/src/aifight/resources.h:40-124).
#[derive(Default, Clone, Debug)]
pub struct PositionSlot {
    /// Requesters waiting for this slot.  We sort by priority desc on each
    /// `Update` (legacy uses `aiPriorityQueue`, an explicit priqueue).
    pub queue: Vec<(i32, Entity)>,
    /// Fighter currently being offered the slot.  Cleared/overwritten each
    /// frame by `fight_resources_offer_system` (mirrors the one-frame
    /// validity rule in `aiFightPosition::Update`).
    pub offered_to: Option<Entity>,
    /// Fighter that has claimed (grabbed) this slot.  None = available.
    pub holder: Option<Entity>,
    /// `aiFightPosition::FLAG_GRABBED_BY_OWNER`: this slot is reserved
    /// by the slot's owner for use by their own current target — kept
    /// so the *target* can `DefendAgainst()` later without a separate
    /// queue lookup.
    pub grabbed_by_owner: bool,
}

/// Defender-side resources component.  Mirrors `aiFightResources`
/// (rb/src/aifight/resources.h:222-356).  Lives on every fighter,
/// because in legacy any actor can be attacked at any time.
#[derive(Component, Debug)]
pub struct FightResources {
    pub positions: [PositionSlot; NUM_POSITIONS],
    pub cookie_queue: Vec<(i32, Entity)>,
    pub cookie_offered_to: Option<Entity>,
    pub cookie_holder: Option<Entity>,
    /// Absolute time (seconds) when the cookie was last released.
    pub cookie_released_at: f64,
    /// Mandatory minimum delay between cookie releases — set at grab
    /// time from `crFighter::GetCookieDelay` (legacy).  Defaults to 0.
    pub cookie_delay: f32,
}

impl Default for FightResources {
    fn default() -> Self {
        Self {
            positions: Default::default(),
            cookie_queue: Vec::new(),
            cookie_offered_to: None,
            cookie_holder: None,
            cookie_released_at: 0.0,
            cookie_delay: 0.0,
        }
    }
}

/// Attacker-side state component.  Lives on every fighter (the player
/// side too — defending against a fighter that grabs a position needs
/// the same fields).  Mirrors the position/cookie fields on
/// `aiFighter` (rb/src/aifight/fighter.h:771-789).
#[derive(Component, Debug)]
pub struct FightSlotState {
    /// Who we're currently engaged with (the holder of `current_position`
    /// is us, around `resource_target`).
    pub resource_target: Option<Entity>,
    /// 0..7 of `resource_target`'s ring we hold; -1 = none.
    pub current_position: i32,
    /// Preferred slot, or -1 = pick the closest at request time.
    /// Legacy never assigns this; we mostly leave it -1 too.
    pub assigned_position: i32,
    /// Active offers for this frame: (offerer, slot_idx).  Cleared at
    /// the start of each `fight_resources_offer_system` tick.  Read by
    /// the FSM as `E_POSITION_OFFERED`.
    pub offered_positions: Vec<(Entity, i32)>,
    /// True iff we've grabbed `resource_target`'s cookie.
    pub cookie_held: bool,
    /// Frame-valid offer of cookie from `resource_target` (or anyone
    /// in defend mode).  None means no cookie offered to us.
    pub cookie_offered_by: Option<Entity>,
    /// Pending cross-entity ops written by the FSM during its tick;
    /// drained by `fight_resource_apply_system`.
    pub pending_ops: Vec<ResourceOp>,
}

impl Default for FightSlotState {
    fn default() -> Self {
        Self {
            resource_target: None,
            current_position: -1,
            assigned_position: -1,
            offered_positions: Vec::new(),
            cookie_held: false,
            cookie_offered_by: None,
            pending_ops: Vec::new(),
        }
    }
}

/// A cross-entity mutation enqueued by the FSM's apply_action branch.
/// Drained next frame by `fight_resource_apply_system`.  This indirection
/// avoids needing simultaneous mutable access to both the FightRuntime
/// query (the actor) and other fighters' FightResources during the FSM
/// tick.
#[derive(Debug, Clone)]
pub enum ResourceOp {
    /// Push (priority, self) onto target's slot queue.  If `slot < 0`
    /// the apply system picks the closest direction at apply time.
    /// Mirrors `aiFighter::RequestPosition`.
    RequestPosition {
        target: Entity,
        slot: i32,
        priority: i32,
    },
    /// Grab the first item in `offered_positions`.  Mirrors
    /// `aiFightStateMachine::AGrabPosition`.
    GrabPosition,
    /// Release current_position; mirrors `aiFighter::ReleasePosition`.
    ReleasePosition,
    /// Push self onto target's cookie queue.  Mirrors
    /// `aiFighter::RequestCookie`.
    RequestCookie {
        target: Entity,
        priority: i32,
    },
    /// Grab the cookie offered to us (validates against `resource_target`).
    GrabCookie,
    /// Release the cookie back to the owner.  Mirrors
    /// `aiFightResources::ReleaseCookie`.
    ReleaseCookie,
}

// ---------------------------------------------------------------------------
// Direction helpers
// ---------------------------------------------------------------------------

/// XZ unit vectors for the 8 cardinal+ordinal directions, in legacy
/// order.  Index 0 is "+X" (legacy choice), then CCW by 45°.  Used to
/// place the slot circles for debug rendering and to find the closest
/// direction when assigning a slot at request time.  Mirrors the static
/// `xz[8]` table in `aiPrimaryDirections::GetLocation`
/// (rb/src/aifight/resources.cpp:320-329).
const SLOT_DIRECTIONS: [(f32, f32); NUM_POSITIONS] = [
    (1.0, 0.0),
    (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
    (0.0, 1.0),
    (-std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2),
    (-1.0, 0.0),
    (
        -std::f32::consts::FRAC_1_SQRT_2,
        -std::f32::consts::FRAC_1_SQRT_2,
    ),
    (0.0, -1.0),
    (std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2),
];

/// World-space offset from `target_pos` for slot `slot_idx` at the
/// given range.  Port of `aiPrimaryDirections::GetLocation`.
pub fn slot_world_pos(target_pos: Vec3, slot_idx: usize, range: f32) -> Vec3 {
    let (dx, dz) = SLOT_DIRECTIONS[slot_idx % NUM_POSITIONS];
    Vec3::new(target_pos.x + dx * range, target_pos.y, target_pos.z + dz * range)
}

/// Find the slot/direction closest to `attacker_pos` relative to
/// `target_pos`.  Bit-twiddling port of
/// `aiPrimaryDirections::ClosestDirection` (rb/src/aifight/
/// resources.cpp:340-371).  Returns 0..7.
pub fn closest_direction(attacker_pos: Vec3, target_pos: Vec3) -> i32 {
    let dx0 = attacker_pos.x - target_pos.x;
    let dz0 = attacker_pos.z - target_pos.z;
    if dx0 == 0.0 && dz0 == 0.0 {
        return 0;
    }
    // Rotate the (dx, dz) by π/8 so octants align with the cardinal table.
    let rx = 0.92387953_f32; // cosf(PI/8)
    let rz = 0.38268343_f32; // sinf(PI/8)
    let mut x = rx * dz0 + rz * dx0;
    let mut z = -rz * dz0 + rx * dx0;
    let mut oct = 0i32;
    if x < 0.0 {
        oct += 4;
        x = -x;
        z = -z;
    }
    if z < 0.0 {
        oct += 2;
        let t = z;
        z = x;
        x = -t;
    }
    if x > z {
        oct += 1;
    }
    oct & 7
}

// ---------------------------------------------------------------------------
// Update systems
// ---------------------------------------------------------------------------

/// Drains queues into one-frame offers.  Runs BEFORE the FSM tick so
/// `E_POSITION_OFFERED` / `E_COOKIE_OFFERED` see fresh values.
pub fn fight_resources_offer_system(
    time: Res<Time>,
    mut state_q: Query<&mut FightSlotState>,
    mut resources_q: Query<(Entity, &mut FightResources)>,
) {
    // 1. Clear stale per-frame offers on every state.
    for mut st in &mut state_q {
        st.offered_positions.clear();
        st.cookie_offered_by = None;
    }

    let now = time.elapsed_secs_f64();

    // 2. For each defender, offer any free slot to the next queued requester
    //    and offer the cookie if past the cooldown.  We collect notifications
    //    in temporaries so we can apply them with `state_q.get_mut(...)`
    //    after the resources-iter borrow ends.
    let mut position_offers: Vec<(Entity, Entity, i32)> = Vec::new();
    let mut cookie_offers: Vec<(Entity, Entity)> = Vec::new();

    for (owner, mut res) in &mut resources_q {
        for slot_idx in 0..NUM_POSITIONS {
            let slot = &mut res.positions[slot_idx];
            slot.offered_to = None;
            if slot.holder.is_some() || slot.grabbed_by_owner || slot.queue.is_empty() {
                continue;
            }
            // Peek the highest-priority requester (sort asc, last is highest).
            // Non-destructive: the requester stays in the queue until they
            // either Grab the slot (which pops them in `fight_resource_apply_system`)
            // or some other condition removes them.  Re-deriving the offer
            // from the queue every frame means a one-frame FSM transit gap
            // (e.g. S_REQUESTING_POSITION -> S_WAITING_FOR_POSITION) doesn't
            // lose the offer, and a higher-priority requester arriving later
            // naturally takes the offer next frame.
            slot.queue.sort_by(|a, b| a.0.cmp(&b.0));
            let Some(&(_, fighter)) = slot.queue.last() else {
                continue;
            };
            slot.offered_to = Some(fighter);
            position_offers.push((fighter, owner, slot_idx as i32));
        }

        // Cookie — same non-destructive peek as the position queue.  Actor
        // is removed from `cookie_queue` only when `GrabCookie` fires.
        let cooldown_ok = (now - res.cookie_released_at) >= res.cookie_delay as f64;
        if res.cookie_holder.is_none() && cooldown_ok && !res.cookie_queue.is_empty() {
            res.cookie_queue.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(&(_, fighter)) = res.cookie_queue.last() {
                res.cookie_offered_to = Some(fighter);
                cookie_offers.push((fighter, owner));
            }
        } else if res.cookie_holder.is_some() {
            // Already held — clear any stale offer.
            res.cookie_offered_to = None;
        }
    }

    // 3. Push notifications onto each offered fighter's state.
    for (offered_to, owner, slot_idx) in position_offers {
        bevy::log::info!(
            "fight_resources_offer: slot {} of {:?} offered to {:?}",
            slot_idx,
            owner,
            offered_to
        );
        if let Ok(mut st) = state_q.get_mut(offered_to) {
            // Don't push duplicates if multiple defenders offer the same
            // (owner, slot) — in practice impossible, but cheap to guard.
            if !st
                .offered_positions
                .iter()
                .any(|&(o, s)| o == owner && s == slot_idx)
            {
                st.offered_positions.push((owner, slot_idx));
            }
        }
    }
    for (offered_to, owner) in cookie_offers {
        if let Ok(mut st) = state_q.get_mut(offered_to) {
            // Last-writer-wins; cookie is single-resource per defender so
            // there's no ambiguity in practice.
            st.cookie_offered_by = Some(owner);
        }
    }
}

/// Drains every FightSlotState's `pending_ops` and applies the
/// cross-entity mutation.  Runs AFTER the FSM tick so the FSM has had
/// a chance to enqueue requests this frame.
pub fn fight_resource_apply_system(
    transforms_q: Query<&GlobalTransform>,
    mut state_q: Query<(Entity, &mut FightSlotState)>,
    mut resources_q: Query<&mut FightResources>,
) {
    // Phase A: drain all pending_ops into a flat list so we can reborrow
    // state_q later without aliasing.
    let mut ops: Vec<(Entity, ResourceOp)> = Vec::new();
    for (e, mut st) in &mut state_q {
        if st.pending_ops.is_empty() {
            continue;
        }
        for op in st.pending_ops.drain(..) {
            ops.push((e, op));
        }
    }

    if ops.is_empty() {
        return;
    }

    // Diagnostic: dump the ops batch.  Useful when E_HAS_POSITION is
    // stuck false — confirms whether the FSM ever pushed RequestPosition,
    // and shows the slot/target each op is bound to.
    bevy::log::info!("fight_resource_apply: draining {} ops", ops.len());
    for (actor, op) in &ops {
        bevy::log::info!("  op[{:?}] = {:?}", actor, op);
    }

    // Phase B: apply.  Each ResourceOp branch is a port of one method
    // on aiFighter (RequestPosition / GrabPosition / ReleasePosition /
    // RequestCookie / etc.).
    for (actor, op) in ops {
        match op {
            ResourceOp::RequestPosition {
                target,
                slot,
                priority,
            } => {
                let slot_idx = if slot >= 0 {
                    (slot as usize) % NUM_POSITIONS
                } else {
                    // Pick the closest direction to where the attacker
                    // currently stands.  Falls back to slot 0 if either
                    // transform is missing.
                    let attacker_pos = transforms_q
                        .get(actor)
                        .map(|t| t.translation())
                        .unwrap_or(Vec3::ZERO);
                    let target_pos = transforms_q
                        .get(target)
                        .map(|t| t.translation())
                        .unwrap_or(Vec3::ZERO);
                    closest_direction(attacker_pos, target_pos) as usize
                };
                if let Ok(mut res) = resources_q.get_mut(target) {
                    let slot = &mut res.positions[slot_idx];
                    // Don't double-enqueue.
                    if !slot.queue.iter().any(|&(_, e)| e == actor)
                        && slot.holder != Some(actor)
                        && slot.offered_to != Some(actor)
                    {
                        slot.queue.push((priority, actor));
                    }
                }
            }

            ResourceOp::GrabPosition => {
                // Pull the first offer off our state.
                let Ok((_, mut st)) = state_q.get_mut(actor) else {
                    continue;
                };
                if st.offered_positions.is_empty() {
                    continue;
                }
                let (target, slot_idx) = st.offered_positions.remove(0);
                let pos_idx = (slot_idx as usize) % NUM_POSITIONS;

                // Validate the offer is still live in target's resources,
                // then grab.  Mirrors the IsAvailable() / OfferedTo check
                // in aiFightStateMachine::AGrabPosition.
                let mut grabbed = false;
                if let Ok(mut target_res) = resources_q.get_mut(target) {
                    let slot = &mut target_res.positions[pos_idx];
                    if slot.holder.is_none() && slot.offered_to == Some(actor) {
                        slot.holder = Some(actor);
                        slot.offered_to = None;
                        // Offers are non-destructive in
                        // `fight_resources_offer_system` (the actor stays in
                        // the queue while being offered) — pop them here, on
                        // the grab, so they're not re-offered next tick.
                        slot.queue.retain(|&(_, e)| e != actor);
                        grabbed = true;
                    }
                }
                if grabbed {
                    // Free any previously held position before claiming the
                    // new one (the legacy `GrabPosition` first calls
                    // `ReleasePosition`).
                    let prev_target = st.resource_target;
                    let prev_slot = st.current_position;
                    if let Some(prev_t) = prev_target {
                        if prev_slot >= 0
                            && prev_t != target
                            && let Ok(mut prev_res) = resources_q.get_mut(prev_t)
                        {
                            let prev_pos = &mut prev_res.positions[prev_slot as usize];
                            if prev_pos.holder == Some(actor) {
                                prev_pos.holder = None;
                            }
                        }
                    }

                    st.resource_target = Some(target);
                    st.current_position = slot_idx;

                    // Reserve the OPPOSITE slot on the actor's own
                    // resources for the target — mirrors
                    // `aiFighter::GrabPosition` calling
                    // `Resources->GrabOwnPosition(this, oppPos, target)`.
                    let opp = ((slot_idx + 4) & 7) as usize;
                    if let Ok(mut self_res) = resources_q.get_mut(actor) {
                        let self_slot = &mut self_res.positions[opp];
                        self_slot.grabbed_by_owner = true;
                        self_slot.holder = Some(target);
                    }
                }
            }

            ResourceOp::ReleasePosition => {
                let Ok((_, mut st)) = state_q.get_mut(actor) else {
                    continue;
                };
                let target = match st.resource_target {
                    Some(t) => t,
                    None => continue,
                };
                let slot_idx = st.current_position;
                if slot_idx < 0 {
                    continue;
                }

                if let Ok(mut t_res) = resources_q.get_mut(target) {
                    let slot = &mut t_res.positions[slot_idx as usize];
                    if slot.holder == Some(actor) {
                        slot.holder = None;
                    }
                }
                let opp = ((slot_idx as usize) + 4) & 7;
                if let Ok(mut s_res) = resources_q.get_mut(actor) {
                    let opp_slot = &mut s_res.positions[opp];
                    if opp_slot.grabbed_by_owner && opp_slot.holder == Some(target) {
                        opp_slot.holder = None;
                        opp_slot.grabbed_by_owner = false;
                    }
                }
                st.resource_target = None;
                st.current_position = -1;
                st.offered_positions.clear();
            }

            ResourceOp::RequestCookie { target, priority } => {
                if let Ok(mut res) = resources_q.get_mut(target) {
                    if !res.cookie_queue.iter().any(|&(_, e)| e == actor)
                        && res.cookie_holder != Some(actor)
                        && res.cookie_offered_to != Some(actor)
                    {
                        res.cookie_queue.push((priority, actor));
                    }
                }
            }

            ResourceOp::GrabCookie => {
                let Ok((_, mut st)) = state_q.get_mut(actor) else {
                    continue;
                };
                let Some(offered_by) = st.cookie_offered_by else {
                    continue;
                };
                if let Ok(mut res) = resources_q.get_mut(offered_by) {
                    if res.cookie_offered_to == Some(actor) {
                        res.cookie_holder = Some(actor);
                        res.cookie_offered_to = None;
                        // Pop the cookie queue here (non-destructive offers —
                        // see GrabPosition above for the matching rationale).
                        res.cookie_queue.retain(|&(_, e)| e != actor);
                        st.cookie_held = true;
                    }
                }
            }

            ResourceOp::ReleaseCookie => {
                let Ok((_, mut st)) = state_q.get_mut(actor) else {
                    continue;
                };
                let Some(target) = st.resource_target else {
                    st.cookie_held = false;
                    continue;
                };
                if let Ok(mut res) = resources_q.get_mut(target) {
                    if res.cookie_holder == Some(actor) {
                        res.cookie_holder = None;
                        res.cookie_offered_to = None;
                        // Cookie cooldown timestamp is tracked relative to
                        // `Time::elapsed_secs_f64()`; we update it next
                        // tick from `fight_resources_offer_system`.  Until
                        // then `cookie_delay` simply needs to be > the
                        // wait — defaulting to 0 means immediate offer
                        // again, which matches default legacy behavior.
                    }
                }
                st.cookie_held = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_direction_cardinals() {
        let origin = Vec3::ZERO;
        // +X = slot 0
        assert_eq!(closest_direction(Vec3::new(1.0, 0.0, 0.0), origin), 0);
        // +Z = slot 2
        assert_eq!(closest_direction(Vec3::new(0.0, 0.0, 1.0), origin), 2);
        // -X = slot 4
        assert_eq!(closest_direction(Vec3::new(-1.0, 0.0, 0.0), origin), 4);
        // -Z = slot 6
        assert_eq!(closest_direction(Vec3::new(0.0, 0.0, -1.0), origin), 6);
    }

    #[test]
    fn slot_world_pos_round_trips() {
        let target = Vec3::new(10.0, 0.0, 5.0);
        let p = slot_world_pos(target, 0, 1.0);
        assert!((p.x - 11.0).abs() < 1e-4 && (p.z - 5.0).abs() < 1e-4);
    }
}
