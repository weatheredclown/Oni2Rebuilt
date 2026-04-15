# Layout Editor (MVP+UI)

This utility is a standalone 3D layout editor scaffold intended to sit outside `rb-game`.

## Current capabilities

- Derive `layout/` and `entity/` folders from one `--path <assets_root>` argument.
- Load `layout.actors` and per-actor XML files.
- Parse `layout.et` for entity palette entries.
- Show an egui **File** menu (`Load`, `Save`, `Save As`, separator, `Quit`).
- Prompt for unsaved changes when loading a different layout.
- Prompt before overwriting an existing layout folder on `Save As`.
- Display a scrollable 2-column entity palette panel.
- Generate missing thumbnails in a background pass while the app is running.
- Display actor placeholders in 3D, select with LMB, edit transform/rotation, and save back to XML.
- Toggle debug bounds with `B`.

## Run

```bash
cargo run -p layout_editor -- --path ../oni2/zips/assets --layout tim06
```

## Controls

- **LMB**: select actor
- **RMB drag**: orbit camera
- **WASD**: pan camera focus
- **Mouse wheel**: zoom in/out around focus point
- **T**: transform mode (translate with arrows)
- **R**: rotate mode (yaw with Q/E)
- **B**: toggle debug bounds
- **Ctrl+S**: save
