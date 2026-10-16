# Changelog

All notable changes to **DraftDown** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## — Initial Prototype (AI-Assisted Modular Framework)

### Added

* [Added] - Basic 3D Viewport engine initialization, infinite floor grid, and RGB primary orientation coordinate axes.
* [Added] - Continuous line drawing tool state machine with Esc key chain-breaking and real-time axis color snapping.
* [Added] - Vertical Blue (Z) Axis drawing plane toggle triggered by holding down the Shift key.
* [Added] - VCB (Value Control Box) keyboard dimension capture resource with Enter key parameter string parsing.
* [Added] - Procedural 3D Solid Block (Box) creation tool with support for exact "width,depth,height" typing inputs.
* [Added] - Viewport Raycast selection engine (Spacebar hotkey) highlighting `CadObject` meshes in neon blue.
* [Added] - Reversible object deletion command mapped to the Delete/Backspace keys.
* [Added] - Central Command Pattern history manager with `undo_stack` and `redo_stack` resources for full rollback support.
* [Added] - Fixed-focus targeted camera panning (Shift+MMB) and parallel proximity vector edge sniffing (Magenta guide vectors).
* [Added] - Collapsible UI GUI Hotkeys Cheat Sheet panel and a clean native "Exit Application" button mapped to Ctrl+Q.
* [Added] - Interactive 3D Move tool (`M` hotkey, `MoveObjectCommand`) and 8-handle bounding-box Scale tool (`S` hotkey, `ScaleObjectCommand`) with VCB input integration.
* [Added] - Modular four-file Rust architecture (`src/main.rs`, `src/ui.rs`, `src/commands.rs`, `src/tools.rs`) with `GroupId(u64)` and `MaterialColor(Color)` component metadata on `CadObject`.

### Changed

* [Changed] - Officially rebranded the project from `sketchcraft` to **DraftDown** for open-source repository uniqueness.
