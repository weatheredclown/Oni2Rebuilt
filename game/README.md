# rb-game

Oni2 engine reimplementation in Rust/Bevy.

## Building

```
cargo build --workspace
```

Dependencies are compiled with optimizations even in dev builds (`[profile.dev.package."*"] opt-level = 2`) for acceptable runtime performance.

## Command Line Options

| Flag | Argument | Description |
|---|---|---|
| `--dat` | `<path>` | Path to extracted `.DAT` files (default: `RB.DAT/STREAMS.DAT` in current dir) |
| `--path` | `<path>` | Path to raw assets directory |
| `--layout` | `<name>` | Skip the menu and load a layout directly (e.g. `--layout tim06`) |
| `--testanim` | `<path>` | Animation preview mode. Path to an `.anim` file relative to the working directory. The entity model is derived automatically from the filename prefix before the first `_` (e.g. `kno_nav_run_fwd.anim` loads the `kno` entity) |
| `--testentity` | `<name>` | Spawns the specified character model for previewing in a blank scene |
| `--sandbox` | | Flat ground with a single kno entity, no layout |
| `--formation` | | Spawn all known character entities in a grid for visual inspection |
| `--fog` | | Enable distance fog from layout.fog files |
| `--diagnostics` | | Enable Bevy's `LogDiagnosticsPlugin` (prints FPS and frame time to console every second) |

With no flags, the game starts at a layout selection menu.

### Examples

```bash
# Normal game with layout menu
cargo run --bin rb-game -- --dat path/to/iso

# Jump straight into a level
cargo run --bin rb-game -- --dat path/to/iso --layout tim06

# Preview a specific animation file
cargo run --bin rb-game -- --dat path/to/iso --testanim oni2/zips/assets/Entity/kno/kno_nav_run_fwd.anim

# Check a specific character model
cargo run --bin rb-game -- --dat path/to/iso --testentity kno

# Inspect all character models side by side
cargo run --bin rb-game -- --dat path/to/iso --formation

# Flat sandbox with one character
cargo run --bin rb-game -- --dat path/to/iso --sandbox

# Override assets using the raw directory
cargo run --bin rb-game -- --dat path/to/iso --path custom/raw
```

## In-Game Controls

### Player Movement
| Key | Action |
|---|---|
| W / S | Run forward / turn and run back |
| A / D | Turn left / right |
| Left Shift + W/A/S/D | Strafe (walk forward/left/back/right without turning) |
| Q | Jump |
| F | Evade |
| Left Mouse / Space | Light attack |
| Right Mouse | Heavy attack |

### Camera
| Key | Action |
|---|---|
| Tab | Cycle camera mode (GameNavigation / GameFighting / GameTargeting) |
| F5 | Toggle FreeCam mode |
| Mouse Wheel | Zoom in/out |

#### FreeCam Controls (when in FreeCam mode)
| Key | Action |
|---|---|
| W/A/S/D | Fly forward/left/back/right |
| Left Shift | Fly up |
| Left Ctrl | Fly down |
| Right Mouse (hold) | Look around |

### Debug
| Key | Action |
|---|---|
| F2 | Toggle prototype element visibility (capsules, weapons, HUD) |
| F3 | Toggle debug bounds + physics capsule wireframes |
| F4 | Toggle debug skeleton rendering |
| F6 | Toggle procedural light grid |
| F7 | Toggle avian3d physics debug rendering |
| F8 | Toggle debug point light |
| F9 | Toggle debug fog |
| F11 | Scan player geometry and print log |
| K | Kill all active creatures (excluding player) |
| Escape | Return to menu |

### Animation Preview Mode (`--testanim`)
| Key | Action |
|---|---|
| Space | Pause / resume playback |
| Left/Right Arrow | Step one frame back/forward (when paused) |
| Up/Down Arrow | Increase/decrease animation FPS |
| L | Toggle looping |
| Right Mouse (hold) | Orbit camera |

### Formation Mode (`--formation`)
| Key | Action |
|---|---|
| W/A/S/D | Fly camera |
| Space | Fly up |
| Left Ctrl | Fly down |
| Left Shift | Speed boost |
| Right Mouse (hold) | Look around |
