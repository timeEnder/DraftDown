#![forbid(unsafe_code)]

mod commands;
mod tools;
mod ui;

use bevy::{
    prelude::*,
    window::{PresentMode, WindowMode, WindowResolution},
};
use bevy_egui::EguiPlugin;

use crate::{
    commands::{CadCommandExecutionSet, CadCommandsPlugin},
    tools::{CadToolsPlugin, CadToolsSet},
    ui::{CadUiPlugin, CadUiSet},
};

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: String::from("sketchcraft"),
                    // Sets the native Wayland `xdg_toplevel` app_id (and X11 WM_CLASS)
                    // for Hyprland window rules and tiling compositors.
                    name: Some(String::from("sketchcraft")),
                    resolution: WindowResolution::new(1600.0, 900.0),
                    present_mode: PresentMode::AutoVsync,
                    mode: WindowMode::Windowed,
                    fit_canvas_to_parent: true,
                    prevent_default_event_handling: false,
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(EguiPlugin)
        // Global viewport background & bright ambient illumination so 3D solids are never pitch black
        .insert_resource(ClearColor(Color::srgb(0.13, 0.14, 0.17)))
        .insert_resource(AmbientLight {
            color: Color::srgb(0.96, 0.97, 1.0),
            brightness: 680.0,
        })
        // Deterministic frame execution order: UI -> CAD Tools -> Exclusive World Command Queue
        .configure_sets(
            Update,
            (
                CadUiSet,
                CadToolsSet,
                CadCommandExecutionSet,
            )
                .chain(),
        )
        // Modular CAD plugins
        .add_plugins((
            CadCommandsPlugin,
            CadToolsPlugin,
            CadUiPlugin,
        ))
        .run();
}
