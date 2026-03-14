# Architectural Suggestions for Rust Best Practices

Based on a review of the codebase, here are suggestions to bring the architecture closer to idiomatic Rust and Bevy best practices:

## 1. Eliminate Global State (`OnceLock`)
In `game/src/main.rs`, the application currently relies on global state for asset paths:
```rust
pub static ASSETS_PATH: OnceLock<String> = OnceLock::new();
pub static ASSETS_DAT: OnceLock<String> = OnceLock::new();
```
**Why it's an issue:** Using global static variables makes testing difficult, hides dependencies, and goes against Bevy’s data-driven architecture.
**Recommendation:** Remove these `OnceLock` variables. Instead, parse these values at startup and insert them into the Bevy app as a `Resource`. For example:
```rust
#[derive(Resource)]
pub struct AssetConfig {
    pub path: String,
    pub dat: String,
}

// In main():
// app.insert_resource(AssetConfig { path: parsed_path, dat: parsed_dat });
```

## 2. Robust CLI Parsing
The current CLI parsing in `game/src/main.rs` relies on manual slice windowing and string comparisons (`args.windows(2).find_map(...)`).
**Why it's an issue:** This approach is error-prone, hard to scale, and doesn't provide helpful features like `--help` documentation or type validation.
**Recommendation:** Use the `clap` crate with the `derive` API to define a structured CLI argument parser.
```rust
use clap::Parser;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, default_value = "oni2/zips/assets")]
    path: String,

    #[arg(long, default_value = "RB.DAT")]
    dat: String,
    // ...
}
```

## 3. Modularize `main.rs`
The `game/src/main.rs` file is becoming quite large, containing heavy setup logic like `setup_scene`, `setup_formation_scene`, and `free_camera_system`.
**Why it's an issue:** Monolithic files are harder to navigate, maintain, and review.
**Recommendation:** Extract these functions into dedicated modules (e.g., `scene.rs`, `formation.rs`, `camera/free_cam.rs`) or wrap them in Bevy `Plugin`s to encapsulate their logic and setup steps cleanly.

## 4. Refactor VFS Initialization
The virtual file system implementation (`game/src/filesystem/vfs.rs`) utilizes a static instance `static VFS_INSTANCE: std::sync::OnceLock<Box<dyn Vfs>> = std::sync::OnceLock::new();`.
**Why it's an issue:** Similar to the asset path configurations, relying on a global singleton for file system access can restrict parallel processing and complicate dependency injection or testing.
**Recommendation:** Pass the `Vfs` instance as a Bevy `Resource` to systems that require file access, avoiding global access entirely.

## 5. Improve Error Handling
There are patterns of implicit or unhandled errors (or using `unwrap`/`expect` on operations like file loading in `vfs.rs`).
**Why it's an issue:** Panicking on recoverable errors (like a missing file) can crash the game ungracefully.
**Recommendation:** Use the `?` operator and crates like `anyhow` or `thiserror` to propagate errors up the call stack, and handle them explicitly at the application boundary or within specific Bevy systems.
