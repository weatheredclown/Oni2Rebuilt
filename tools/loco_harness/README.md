# Locomotion harness — rb (C++) vs rb-game (Rust)

Drives the player through the same timed stick/button script in both builds
and compares how each blends its navigation animations frame by frame.

```
tools/loco_harness/run.sh [moves.pad] [secs]
```

It installs the `zz_loco_harness` layout (a flat plane + Konoko) into the
asset tree, runs `rb.exe` and `rb-game.exe` unattended, removes the layout
again, and prints a per-step comparison:

```
 step        stick  btn |   C++ fam settle flips      v    sub |  Rust fam settle flips      v    sub | match
 11.5  (+1.0,+0.0) 0008 |  STRAFE_R   0.04     1   0.01  -1.00 |  STRAFE_R   0.04     1   0.00  -0.98 | ok
```

* **fam** — what the legs settle into: STAND / FWD / BACK / STRAFE_L / STRAFE_R
* **settle** — seconds from the step's start until that family is reached
* **flips** — family changes inside the step (legs swapping cycles = kinks)
* **v / sub** — main gait value and strafe sub-blender value, tail average

Raw traces stay in `out/cpp.log` and `out/rust.log` (`[gait]` lines).

## Pad script (`moves.pad`)

`start dur lx ly [buttons_hex]` per line, seconds of game time; `#` comments.
Stick in the AGE convention: +lx right, +ly down/back, camera-relative.
Buttons use the ioPad bits — `0008` is R1 (fight mode / lock-on: strafe and
back-pedal with the heading held).

## Hooks in each build

| | C++ (`repos/rb`) | Rust (`Oni2Rebuilt`) |
|---|---|---|
| scripted input | `-padscript <file>` — `age/src/input/pad.cpp` | `--padscript <file>` — `game/src/player/padscript.rs` |
| gait trace | `-gaitlog` — `rb/src/animator/animator.cpp`, `crGaitBlender::DebugDescribe` | `--gaitlog` — `oni2_loader/animation.rs`, `gait_blender::describe` |

rb.exe needs rebuilding (`age\tools\build\build_rbmain.bat`) after pulling
the C++ hooks.
