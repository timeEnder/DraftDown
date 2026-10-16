#![forbid(unsafe_code)]

mod commands;
mod tools;
mod ui;

use bevy::prelude::*;
use bevy::window::{PresentMode, Window, WindowPlugin, WindowResolution};
use bevy_egui::EguiPlugin;

use crate::commands::{CadCommandExecutionSet, CadCommandsPlugin};
use crate::tools::{CadToolsPlugin, CadToolsSet};
use crate::ui::{CadUiPlugin, CadUiSet};

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.11, 0.12, 0.15)))
        .insert_resource(AmbientLight {
            color: Color::srgb(0.96, 0.97, 1.0),
            brightness: 680.0_f32,
        })
        .add_plugins(
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "DraftDown".into(),
                    name: Some("draftdown".into()),
                    resolution: WindowResolution::new(1440.0_f32, 900.0_f32),
                    present_mode: PresentMode::AutoVsync,
                    resizable: true,
                    ..default()
                }),
                ..default()
            }),
        )
        .add_plugins(EguiPlugin)
        .configure_sets(
            Update,
            (CadUiSet, CadToolsSet, CadCommandExecutionSet).chain(),
        )
        .add_plugins((CadCommandsPlugin, CadToolsPlugin, CadUiPlugin))
        .run();
}