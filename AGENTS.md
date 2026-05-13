# Repository Guidelines

## Project Structure & Module Organization
Oni2Rebuilt is a Cargo workspace that unifies three crates: `game/` (Bevy client, asset tooling, and gameplay systems), `server/` (Axum/DataFusion telemetry backends), and `shared/` (protobuf schemas plus shared data utilities). Keep extracted ISO data such as `RB.DAT` outside the repo and pass its directory to commands via flags instead of committing binaries.

## Important Distinctions between Bungee-developed Oni and Angel Studios-developed Oni 2
The original Oni game developed by Bungee is a well understood and deeply reversed engineers. It shared no technical overlap with this code-base, and does not share any extentions/file formats/animation/scripting conventions in common.
"Oni 1" had bsl files and other types that are irrelevant even though your corpus/internet search results may suggest
these file types and well known quirks or system behaviors when writing Oni code or mods. DO NOT CONFUSE THESE SYSTEMS.

## Build, Test, and Development Commands
- `cargo build --workspace` — compile every crate using the optimized-deps dev profile in `Cargo.toml`.
- `cargo run --bin rb-game -- --dat path/to/iso` — launch the client; add `--path custom/raw` to override assets for quick iteration.
- `cargo run -p rb-server` — start the telemetry/ingest server locally for integration loops.
- `cargo test --workspace` (or `cargo test -p rb-game`) — run Rust unit/integration tests.
- `cargo fmt && cargo clippy --workspace --all-targets` — enforce formatting and linting before opening a PR.

## Coding Style & Naming Conventions
Use stable Rust 1.78+ with edition 2024. The repo relies on default `rustfmt` (4-space indents, trailing commas), so never hand-format blocks. Modules, files, and components follow `snake_case`, while types and systems use `UpperCamelCase` (e.g., `ActiveStateSystem`). Only add the `Rb` prefix when mirroring Oni 2 structures, and keep modules small under `game/src/` (such as `systems/` and `asset/`) to preserve fast reloads.

## Testing Guidelines
Game-side tests live alongside modules (use `#[cfg(test)] mod tests`), while cross-crate checks reside in `shared/` and root fixtures such as `test_xml.rs`. Favor behavior-driven names like `handles_sleep_toggle` for clarity. When a test depends on script behavior, note the relevant entry in `docs/scroni_roadmap.md`. Run `cargo test --workspace` plus the relevant `cargo run` to validate entity playback.

## Commit & Pull Request Guidelines
Commits in this repo favor action-oriented subjects (`feat: introduce Oni2 asset parsing`, `fix: actor sleep and wake system`). Follow that tone: start with a concise verb, mention the subsystem (`game`, `server`, or `shared`), and keep body bullets for rationale. PRs should include a summary, linked issue or roadmap item, reproduction/verification steps (commands, RB.DAT path used), screenshots or GIFs for gameplay/UI tweaks, and test/format output; also call out any asset assumptions so reviewers can reproduce the build.

## Asset & Configuration Tips
Read `docs/publishing.md` before distributing binaries. Keep personal ISO paths in `.env` or shell scripts, never in tracked files. When capturing telemetry, clean up generated logs outside the repo or add them to `.gitignore` in `utils/`.
