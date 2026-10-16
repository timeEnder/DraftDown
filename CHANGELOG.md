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

## [0.1.1] — Camera-Oriented Raycasting & Damped Targeted Orbit

### Added

* [Added] - Camera-oriented `DynamicDrawingPlane` selector (`HorizontalXY`, `VerticalXZ`, `VerticalYZ`, and `CameraFacing` fallback) that automatically chooses the dominant primary axis plane based on the camera's forward vector.
* [Added] - SketchUp-style raycast-targeted camera orbit (`OrbitCamera::reanchor_to_pivot`) that casts a ray on Middle-Mouse Button (MMB) press to re-anchor `FocalPoint` directly onto the clicked `CadObject` mesh (or ground plane fallback) without camera eye teleportation.
* [Added] - Exponential mouse motion smoothing and spherical coordinate damping (`damping_sharpness`) in `orbit_camera_controller` to eliminate Wayland/Hyprland input stutter.

### Fixed

* [Fixed] - Warped and skewed 3D object placement at shallow/horizon camera angles by replacing rigid `Z=0` plane projection with dynamic axis-plane intersection and strict `1.0m` integer grid increment snapping (`snap_vec3_to_integer_grid`).
* [Fixed] - Camera upside-down inversion and polar gimbal lock by clamping vertical pitch strictly between `-85°` and `+85°` (`MAX_CAMERA_PITCH_RAD`) and keeping target azimuth continuous across `2π` boundaries.

## [0.1.2] — Concentric 2D/3D Offset Tool & VCB Wall-Thickness Support

### Added

* [Added] - Offset Tool (`CadTool::Offset`) mapped to the `[F]` hotkey and integrated into the left-side `egui::SidePanel` toolbar and Hotkeys Cheat Sheet.
* [Added] - Automatic 2D face perimeter and connected coplanar polyline chain detector (`detect_offset_contour_for_entity`) with real-time hover highlighting.
* [Added] - Concentric 2D mitered offset solver (`OffsetContour::compute_offset_segments`) supporting interactive inward/outward LMB dragging and exact decimal VCB distance inputs (e.g., `0.3` for 30cm wall thickness).
* [Added] - Reversible `CreateOffsetCommand` integrated with `CommandHistory` (`undo_stack` / `redo_stack`) for full `Ctrl+Z` / `Ctrl+Y` rollback support.

## [0.1.3] - 2026-10-16

### Fixed
- **Strict Coplanar Local-Space Offset Projection (`src/tools.rs`)**: Projected selected 3D block faces and closed coplanar line loops into an orthonormal 2D `(U, V)` local basis (`OffsetContour`), computing inward/outward parallel edge shifts and 2D miter intersections (`intersect_uv_miter`) with self-intersection clamping.
- **Vertex-to-Face Flush Binding (`src/tools.rs`)**: Added `OffsetContour::bind_uv_flush_to_plane` to eliminate normal drift and lock generated 3D offset segments strictly flush to the source face plane.
- **Cross-Module Signature Synchronization (`src/commands.rs`, `src/tools.rs`, `src/ui.rs`)**: Unified `CommandHistory::execute_command`, `Option<GroupId>` command constructors, `MoveToolState`/`ScaleToolState` drag sessions, and `VcbInput` state across all four modules, and resolved the `E0502` borrow conflict in `handle_offset_tool_interaction`.
