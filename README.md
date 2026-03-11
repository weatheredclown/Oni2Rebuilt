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
Bevy/Linux dependencies installed locally. From there you can## Running the game
1. Download the Oni 2 (Angel Studios) ISO from the [Oni 2 Archive](https://wiki.oni2.net/Oni_2_(Angel_Studios)).
2. Use 7zip to extract the contents of the ISO.
3. Locate the `RB.DAT` file within the extracted archive.
4. Pass the relative or absolute path of the directory containing `RB.DAT` to `rb-game` via the `--dat` flag.

```sh
cargo run --bin rb-game -- --dat path/to/extracted/iso/dir
```

Optionally, you can also inject custom raw files to override the `RB.DAT` archive using the `--path` flag:
```sh
cargo run --bin rb-game -- --dat path/to/extracted/iso/dir --path path/to/raw/assets/dir
```

## Repo Layout

```
game/    # Bevy client + asset tooling
server/  # Axum/DataFusion server utilities
shared/  # Shared protobuf + data model crate
```

Each crate is a regular Cargo package inside the workspace, so `cargo test -p
rb-game` or `cargo run -p rb-server` work as expected. See `docs/` for deeper
notes on publishing or asset preparation if you are setting this up locally.
