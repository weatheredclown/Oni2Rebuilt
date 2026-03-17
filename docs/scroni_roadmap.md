# ScrOni Subsystems Roadmap: M03_A01_Blast_Chambers

This roadmap outlines the steps to reverse-engineer and implement the remaining `ScrOni` commands found in the Blast Chambers layout. Our goal is to prevent the VM from crashing on unimplemented commands by properly parsing them into the AST, and then either executing them or safely stubbing them out.

## Phase 1: Audio (Stub)
We will parse these commands so they don't crash, but we will safely ignore their execution for now until the audio system is fully implemented.
- `sound`
- `ambientsound`

## Phase 2: Camera & Cinematics
Reverse-engineer the arguments and syntax for camera control during cutscenes.
- `cameramovetopoint`
- `cameratrackactor`
- `cameratrackpoint`
- `camerashake`
- `camerasetfov`
- `camerafollowactor`

## Phase 3: Combat & AI Behaviors
Examine how AI scripts command actors to handle weapons and positioning.
- `setattacktable`
- `holsterweapon`
- `drawweapon`
- `look`
- `takecover`
- `setcrouch`
- `form`

## Phase 4: Messaging
- `sendaction`

## Phase 5: VFX & Engine Feedback
These are mostly visual/tactile effects that can be mapped to Bevy events.
- `makeexplosion`
- `makefx`
- `padrumblelargemotor`
- `setshaderlocal`
- `intensity`

## Phase 6: Script Flow
Commands that alter the high-level execution state of the game or scripts.
- `endscript`
- `endgrapple`
- `levelcomplete`

## Phase 7: Entity Management
Commands that manipulate items, objects, and entity lifecycles.
- `remove`
- `setunbreakable`
- `pickup`
- `dropoff`
- `clear`

## Phase 8: HUD
- `sethud`
- `controlhead`

---
**Strategy for each phase:**
1. **Asset Analysis:** Use `grep_search` on the `.oni` scripts to find every instance of the command to understand its syntax (arguments, optional modifiers).
2. **Lexer/Parser Updates:** Add the token to `tokenizer.rs`/`token.rs`, and write a `parse_*` function in `compiler.rs` to consume the tokens correctly.
3. **AST Update:** Add the corresponding variant to `ast::Stmt`.
4. **VM Execution:** Add the statement to `vm.rs`'s `exec_stmt` loop. If it's a stub, we will `info!` log it instead of panicking. If it's fully implemented, we'll map it to a `BlockingAction` or `SysRequest`.
