# Bevy 0.19 upgrade — reference & plan

Reference notes for upgrading the engine dependency stack from Bevy **0.18.1** to
**0.19**. Captured 2026-07 from live crates.io metadata (sparse index dep
requirements, not guesses). Re-verify versions before acting — the ecosystem
moves.

## Current state

`game/Cargo.toml`:

```toml
bevy = { version = "0.18", features = ["wav"] }   # resolves to 0.18.1
avian3d = "0.5"
bevy_hanabi = "0.18.0"
bevy-tnua = "0.29"
bevy-tnua-avian3d = "0.10"
```

## Dependency constraints (from crates.io sparse index)

Each ecosystem crate pins a specific Bevy (and avian, for the tnua crates), so a
Bevy upgrade is really an "upgrade the whole graph to a co-resolving set" task.

| Crate               | Version   | requires bevy | requires avian3d |
|---------------------|-----------|---------------|------------------|
| avian3d             | 0.5.0     | ^0.18.0       | —                |
| avian3d             | 0.6.0/0.6.1 | ^0.18.0     | —                |
| avian3d             | 0.7.0     | ^0.19.0       | —                |
| bevy_hanabi         | 0.18.0    | ^0.18         | —                |
| bevy_hanabi         | 0.19.0    | ^0.19         | —                |
| bevy-tnua           | 0.29/0.30 | ^0.18         | ^0.5             |
| bevy-tnua           | 0.31.0    | ^0.18         | ^0.6             |
| bevy-tnua           | 0.32.0    | ^0.19         | ^0.7             |
| bevy-tnua-avian3d   | 0.10.0    | ^0.18         | ^0.5             |
| bevy-tnua-avian3d   | 0.11.0/0.11.1 | ^0.18     | ^0.6             |
| bevy-tnua-avian3d   | 0.12.0    | ^0.19         | ^0.7             |

Key fact: **avian 0.6 still targets bevy 0.18.** That lets us decouple the avian
API churn from the Bevy churn (see staging below).

## Target set — Bevy 0.19

Fully consistent; no version juggling (tnua 0.32 + tnua-avian3d 0.12 both pin
`avian ^0.7`, and avian 0.7 pins `bevy ^0.19`):

```toml
bevy = { version = "0.19", features = ["wav"] }   # verify "wav" feature still exists in 0.19
avian3d = "0.7"
bevy_hanabi = "0.19"
bevy-tnua = "0.32"
bevy-tnua-avian3d = "0.12"
```

## Where the work concentrates

- **avian3d 0.5 → 0.7 is the heavy lift** — *two* minors, and avian renames
  contact-graph / manifold / solver APIs between minors. It hits our physics
  code directly:
  - `game/src/oni2_loader/physics.rs` — `ContactGraph`, `ContactPair`,
    `ContactPairFlags` (`GENERATE_CONSTRAINTS`), `iter_active_touching_mut` /
    `iter_sleeping_touching_mut`, manifold `penetration`, `octree_one_way_contact_system`, `character_separation_system`.
  - `CollisionLayers` / `PhysicsLayer` (`GameLayer`) in `game/src/combat/bundles.rs`.
  - `Position` / `LinearVelocity` mutation, `SleepingDisabled`, `RigidBody`,
    `ShapeCaster` in the creature bundles (`combat/bundles.rs`,
    `mover/components.rs`) and the grapple/reposition code (`fight/systems.rs`).
  - `PhysicsSchedule` / `NarrowPhaseSystems::Last` / `PhysicsSet::StepSimulation`
    scheduling in `oni2_loader/mod.rs`.
- **Bevy 0.18 → 0.19** — one minor; expect the usual schedule/rendering/ECS
  renames from the migration guide.
- **bevy-tnua 0.29 → 0.32** — three minors (only relevant when `RB_MOVER=tnua`).
- **bevy_hanabi 0.18 → 0.19** — one minor (GPU particle effect API).

## Recommended staging (isolate one variable at a time)

### Stage 1 — stay on Bevy 0.18, bump avian to 0.6

Absorb the avian 0.5→0.6 API changes in isolation; if physics breaks, it's avian.

```toml
bevy = { version = "0.18", features = ["wav"] }
avian3d = "0.6"
bevy_hanabi = "0.18"
bevy-tnua = "0.31"          # bevy ^0.18, avian ^0.6
bevy-tnua-avian3d = "0.11"  # bevy ^0.18, avian ^0.6
```

Must compile + run + pass the test suite before moving on.

### Stage 2 — Bevy 0.18 → 0.19 (+ avian 0.6 → 0.7)

The target set above. Now any break is attributable to Bevy (or the avian
0.6→0.7 delta), not a tangle of both.

## Process notes

- Do it on a **dedicated branch / throwaway git worktree**, not mid-feature —
  the avian changes touch many physics call sites and you want a clean bisect.
- Skim the migration guides first: Bevy 0.18→0.19, and avian 0.5→0.6 and
  0.6→0.7 CHANGELOGs (the contact-graph/manifold renames are the ones that bite).
- Verify the `"wav"` Bevy feature name survives into 0.19.
- Motivation beyond currency: newer Bevy/wgpu has improved GPU **surface/device-loss
  handling**, which is directly relevant to the "crash on laptop sleep" issue
  (Bevy 0.18's `prepare_windows` only recovers from `SurfaceError::Outdated` and
  `panic!`s on `Lost`/`Other`).

## How to re-check versions later

```bash
cargo search bevy avian3d bevy_hanabi bevy-tnua bevy-tnua-avian3d
# authoritative per-version bevy/avian requirements (sparse index):
curl -s https://index.crates.io/av/ia/avian3d          | python3 -c "import sys,json;[print(json.loads(l)['vers'],[d['req'] for d in json.loads(l).get('deps',[]) if d['name']=='bevy']) for l in sys.stdin if l.strip()]"
# (paths: be/vy/bevy_hanabi, be/vy/bevy-tnua, be/vy/bevy-tnua-avian3d)
```
