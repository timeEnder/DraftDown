# DraftDown — Open-Source AI-Assisted 3D CAD built in Rust

**DraftDown** is a lightweight, fast 3D CAD modeling workspace engineered via Vibe Coding methodologies and explicitly optimized for Linux, native Wayland, and the Hyprland tiling compositor environment.

Built on a strict Z-Up coordinate architecture (`X` = Red Width, `Y` = Green Depth, `Z` = Blue Vertical Elevation), DraftDown delivers SketchUp-inspired direct 3D modeling workflows, precision keyboard dimension entry, and real-time geometric inference without hardware cursor-locking bugs on Wayland compositors.

## Tech Stack

* **Language:** Rust (2021 Edition, `#![forbid(unsafe_code)]`)
* **3D Engine & ECS:** [Bevy v0.14](https://bevy.org/news/bevy-0-14/)
* **Immediate-Mode GUI:** [bevy_egui v0.28](https://crates.io/crates/bevy_egui/0.28.0)
* **Graphics & Windowing Pipeline:** `wgpu` (Vulkan backend) + `winit` (Native Wayland `xdg_toplevel` protocol)

## Core Features Showcase

* **Continuous Polyline Chaining (`[L]`):** Click to place connected 3D line segments continuously where each committed endpoint becomes the next start point. Press `Escape` at any time to break the active chain.
* **Primary Axis Snapping & Color Feedback:** Dynamic line previews automatically snap parallel to the **Red (`X`)**, **Green (`Y`)**, and **Blue (`Z`)** axes, changing the live preview wire color to match the locked axis.
* **Shift-Key Vertical Blue (`Z`) Axis Drawing & Translation:** Hold `Shift` while drawing lines or moving objects to lock horizontal `X`/`Y` coordinates to the anchor point and project cursor motion onto a camera-facing vertical plane for strict vertical `Z`-axis elevation control.
* **Keyboard Dimension Input — VCB / Measurements Box:** Live raw keyboard character capture (`VcbInput { buffer: String }`) rendered in the bottom-right corner of the status bar. Type exact line lengths (e.g., `5.4`), 3D offsets (`dx,dy,dz`), block dimensions (`3,4,2`), or scale factors (`1.5`) and press `Enter` to commit.
* **3D Block / Box Tool (`[B]`):** Two-click ground-plane corner-to-corner block creation with a live translucent 3D bounding-box preview (`AlphaMode::Blend` + wireframe gizmo) and full VCB `"width,depth,height"` (`W,D,H`) input integration wrapped in `CreateBlockCommand`.
* **Raycast Object Selection (`[Spacebar]`):** 3D ray-AABB and ray-segment intersection picking that highlights selected `CadObject` entities with a bright **Neon Blue** emissive material override while displaying object labels, `GroupId(u64)`, and `MaterialColor(Color)` metadata.
* **Move Tool (`[M]`):** Click and drag any `CadObject` to translate it in 3D space with real-time Red/Green/Blue axis snapping, `Shift` vertical `Z`-locking, parallel neighbor inference, VCB distance support, and reversible `MoveObjectCommand` execution.
* **Scale Tool (`[S]`):** Wraps the selected `CadObject` in a 3D bounding box with **8 interactive corner vertex handles**. Drag any handle relative to its opposite anchor corner or type a VCB scale factor (`1.5` or `1.5,1.5,2.0`) to resize mesh geometry via `ScaleObjectCommand`.
* **Reversible Entity Deletion (`[Delete]` / `[Backspace]`):** Deletes selected `CadObject` entities via `DeleteCommand`, storing full geometric, material, and group snapshots so deleted objects are cleanly restored upon Undo.
* **Command Pattern Undo/Redo Architecture (`[Ctrl + Z]` / `[Ctrl + Y]`):** Central `CommandHistory` resource managing `undo_stack` and `redo_stack` trait objects (`Box<dyn Command + Send + Sync>`) executed through an exclusive `&mut World` system.
* **Targeted Camera Panning & Wayland-Safe Orbit Navigation:** Orbits (`MMB Drag`), pans (`Shift + MMB Drag`), and smoothly zooms (`Mouse Scroll`) around a dynamic 3D `FocalPoint` resource using raw `MouseMotion` deltas—explicitly avoiding `CursorGrabMode::Locked` to prevent Wayland/Hyprland cursor warping glitches.
* **Perfected Parallel Neighbor & Top-Face Inference Engine:** Scans edges of adjacent `CadObject` entities within a 40m proximity radius. When the active drawing or translation vector aligns within `2.0°` of a neighbor edge (or snaps flush with an existing block's top face), DraftDown locks onto the vector and renders an infinite dashed **Magenta** alignment guide line across the viewport.
* **Collapsible Hotkeys Cheat Sheet & Clean Wayland Exit (`[Ctrl + Q]`):** Built-in shortcut reference panel in the left toolbar and a prominent **Exit Application** button that dispatches `bevy::app::AppExit::Success`.
* **Camera-Oriented Dynamic Drawing Plane & Strict Integer Snapping:** Automatically projects cursor rays onto the dominant primary axis plane (`XY`, `XZ`, or `YZ`) or a camera-facing plane at steep horizon angles, snapping 3D coordinates strictly to `1.0m` integer grid increments (or exact typed VCB dimensions) to eliminate warped geometry.
* **Raycast-Targeted Camera Orbit & Damped Smoothing:** Clicking `MMB` casts a ray into the 3D scene to lock the orbit `FocalPoint` directly onto the clicked `CadObject` (or ground fallback) without camera jumps, paired with exponential mouse delta smoothing and a strict `-85°` to `+85°` pitch lock.
* **Offset Tool (`[F]`):** Hover over any existing 3D block face or connected coplanar edge chain to highlight its perimeter, then drag inward/outward or type an exact decimal offset distance (e.g., `0.3` for 30cm wall thickness) into the VCB and press `Enter` to generate a concentric mitered edge loop via `CreateOffsetCommand`.

## Project Architecture

```text
draftdown/
├── Cargo.toml
├── LICENSE
├── README.md
├── CHANGELOG.md
└── src/
    ├── main.rs      # Application entry point, Wayland WindowPlugin setup & plugin orchestration
    ├── ui.rs        # bevy_egui SidePanel toolbar, VCB status bar, inspector & Hotkeys Cheat Sheet
    ├── commands.rs  # Command trait, CommandHistory stacks, CadObject metadata & reversible commands
    └── tools.rs     # 3D environment, FocalPoint camera, raycasting, 2° inference & tool state machines
```

## Coding Standards & Safety Guarantees

1. **Zero Unsafe Code:** Every binary compilation enforces `#![forbid(unsafe_code)]` at the crate root (`src/main.rs`).
2. **Zero-Crash Error Handling:** Direct calls to `panic!`, `.unwrap()`, and `.expect()` are strictly prohibited across the entire repository. Systems use pattern matching (`let Some(...) = ... else { return; }`), clamped mathematical bounds, and structured `warn!` diagnostics.
3. **Wayland-First Windowing:** Configures `Window::name` (`xdg_toplevel` `app_id`) and raw pointer motion deltas for predictable behavior under tiling Wayland compositors such as Hyprland and Sway.

## Quick Start (Arch Linux / Hyprland)

### 1. Install System Dependencies

```bash
sudo pacman -S --needed base-devel rust cargo wayland wayland-protocols \
    libxkbcommon vulkan-icd-loader vulkan-tools pkgconf alsa-lib udev
```

### 2. Configure `Cargo.toml`

Ensure your `Cargo.toml` enables the `wayland` feature on Bevy v0.14:

```toml
[package]
name = "draftdown"
version = "0.1.0"
edition = "2021"

[dependencies]
bevy = { version = "0.14", features = ["wayland"] }
bevy_egui = "0.28"
```

### 3. Build and Launch on Native Wayland

```bash
WINIT_UNIX_BACKEND=wayland cargo run
```

## Keyboard Shortcuts Reference

| Shortcut | Action |
| :--- | :--- |
| `Spacebar` | Activate **Select Tool** |
| `L` | Activate **Line Draw Tool** (Continuous Polyline) |
| `B` | Activate **Block / Box Tool** |
| `M` | Activate **Move Tool** (3D Translation) |
| `S` | Activate **Scale / Resize Tool** (8-Handle Bounding Box) |
| `Delete` / `Backspace` | Delete Selected `CadObject` Entity |
| `Hold Shift` *(drawing/moving)* | Lock to Vertical **Blue (`Z`) Axis** |
| `MMB Drag` | Orbit Camera Around Dynamic `FocalPoint` |
| `Shift + MMB Drag` | Pan Camera and `FocalPoint` Target |
| `Mouse Scroll` | Smooth Zoom In / Out Toward `FocalPoint` |
| `Ctrl + Z` | Undo Last Command |
| `Ctrl + Y` / `Ctrl + Shift + Z` | Redo Command |
| `Escape` | Cancel Active Operation / Break Line Chain |
| `Ctrl + Q` | Exit Application Cleanly |
| F | Offset |
| `P` | Activate **Push/Pull** face extrusion tool (real-time drag or VCB distance) |

## License

Distributed under the **MIT License**. See [`LICENSE`](./LICENSE) for full legal text.
