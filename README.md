# Oni2Rebuilt

`Oni2Rebuilt` is a Rust workspace that gathers the three crates needed to work on
this reverse engineered Oni 2 prototype:

- `game/` &mdash; the Bevy-based client that can load Oni 2 assets and test
  gameplay systems.
- `server/` &mdash; Axum + DataFusion backend services for ingesting telemetry
  and gameplay data.
- `shared/` &mdash; protobuf definitions plus the utilities that are shared by
  the game and the server.

## Getting Started

```
cargo build --workspace
```

The workspace uses Rust 1.78+ (edition 2024) and expects that you have the
Bevy/Linux dependencies installed locally. From there you can run either binary:

```
cargo run -p rb-game -- --help
cargo run -p rb-server
```

Assets are intentionally kept out of the repository. Point the game to your
local Oni 2 dump through the CLI options or configuration files already present
in the `game` crate.

## Repo Layout

```
game/    # Bevy client + asset tooling
server/  # Axum/DataFusion server utilities
shared/  # Shared protobuf + data model crate
```

Each crate is a regular Cargo package inside the workspace, so `cargo test -p
rb-game` or `cargo run -p rb-server` work as expected. See `docs/` for deeper
notes on publishing or asset preparation if you are setting this up locally.
