# Layout Editor (MVP)

This utility is a standalone 3D layout editor scaffold intended to sit outside `rb-game`.

## Goals covered in this MVP

- Load `layout.actors` and per-actor XML files.
- Parse `layout.et` for entity palette entries.
- Discover pre-cached `thumbnail.png` files under each entity folder.
- Display a 3D scene with actor placeholders.
- Orbit camera controls (RMB drag + mouse wheel, WASD pan).
- Select actors with left click.
- Sticky transform (`T`) and rotate (`R`) mode toggles.
- Arrow-key translation and `Q/E` yaw rotation editing.
- Save updated actor transforms back to XML plus `layout.actors`.

## Run

```bash
cargo run -p layout_editor -- --layout <path/to/layouts/tim06> --entities <path/to/entity>
```

## Controls

- **LMB**: select actor
- **RMB drag**: orbit camera
- **WASD**: pan camera focus
- **Mouse wheel**: zoom in/out around focus point
- **T**: transform mode (translate with arrows)
- **R**: rotate mode (yaw with Q/E)
- **Ctrl+S**: save
- **Ctrl+N**: new in-memory layout
