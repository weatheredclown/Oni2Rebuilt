# rb-game

Oni2 engine reimplementation in Rust/Bevy.

## Building

```
cargo build
```

Dependencies are compiled with optimizations even in dev builds (`[profile.dev.package."*"] opt-level = 2`) for acceptable runtime performance.

## Command Line Options

| Flag | Argument | Description |
|---|---|---|
| `--layout` | `<name>` | Skip the menu and load a layout directly (e.g. `--layout tim06`) |
| `--testanim` | `<path>` | Animation preview mode. Loads a single entity and plays the specified `.anim` file |
| `--sandbox` | | Flat ground with a single kno entity, no layout |
| `--formation` | | Spawn all known character entities in a grid for visual inspection |
| `--fog` | | Enable distance fog from layout.fog files |
| `--diagnostics` | | Enable Bevy's `LogDiagnosticsPlugin` (prints FPS, frame time, GPU stats to console every second) |

With no flags, the game starts at a layout selection menu.

### Examples

```bash
# Normal game with layout menu
cargo run

# Jump straight into a level
cargo run -- --layout tim06

# Preview a specific animation file
cargo run -- --testanim oni2/zips/assets/Entity/kno/kno_nav_run_fwd.anim

# Inspect all character models side by side
cargo run -- --formation

# Flat sandbox with one character
cargo run -- --sandbox

# Any mode with diagnostics logging
cargo run -- --layout tim06 --diagnostics
```

## In-Game Controls

### Player Movement
| Key | Action |
|---|---|
| W/A/S/D | Move forward/left/back/right |
| Space | Jump |
| Left Shift | Block |
| Left Mouse | Light attack |
| Right Mouse | Heavy attack |
| Ctrl + A/D/S | Directional attack (left/right/back) |
| E | Grab |
| F | Pick up weapon |
| Q | Drop weapon |

### Camera
| Key | Action |
|---|---|
| Tab | Toggle camera mode (MouseLook / SmartFollow) |
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
| F3 | Toggle debug bounds + physics capsule wireframes |
| F4 | Toggle debug skeleton rendering |
| F6 | Toggle prototype element visibility (capsules, weapons, HUD) |
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
