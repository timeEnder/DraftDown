use bevy::{app::AppExit, prelude::*};
use bevy_egui::{egui, EguiContexts};

use crate::{
    commands::{CadObject, CommandHistory, DeleteCommand, HistoryAction, PendingHistoryActions},
    tools::{
        ActiveToolState, AxisInference, CadTool, CursorRayState, FocalPoint, MoveToolState,
        ScaleToolState, SelectedCadObjects, ToolDraftState, VcbInput,
    },
};

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadUiSet;

#[derive(Resource, Debug, Default)]
pub struct UiInputCapture {
    pub egui_wants_pointer: bool,
    pub egui_wants_keyboard: bool,
}

pub struct CadUiPlugin;

impl Plugin for CadUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiInputCapture>()
            .add_systems(Update, render_immediate_ui.in_set(CadUiSet));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_immediate_ui(
    mut contexts: EguiContexts,
    mut tool_state: ResMut<ActiveToolState>,
    mut ui_capture: ResMut<UiInputCapture>,
    mut draft: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut vcb: ResMut<VcbInput>,
    mut selected: ResMut<SelectedCadObjects>,
    mut pending_actions: ResMut<PendingHistoryActions>,
    mut app_exit_events: EventWriter<AppExit>,
    cursor_ray: Res<CursorRayState>,
    focal_point: Res<FocalPoint>,
    history: Res<CommandHistory>,
    cad_objects: Query<(Entity, &CadObject)>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else {
        warn!("Primary egui context unavailable this frame.");
        return;
    };

    let mut visuals = egui::Visuals::dark();
    visuals.window_rounding = egui::Rounding::same(4.0);
    visuals.panel_fill = egui::Color32::from_rgba_unmultiplied(24, 26, 31, 246);
    ctx.set_visuals(visuals);

    // ========================================================================
    // 1. BOTTOM STATUS BAR & SKETCHUP VCB ("MEASUREMENTS") BOX
    // ========================================================================
    egui::TopBottomPanel::bottom("sketchcraft_status_bar")
        .exact_height(32.0)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;

                ui.colored_label(
                    egui::Color32::from_rgb(100, 195, 255),
                    format!("Tool: {}", tool_state.current.label()),
                );
                ui.separator();

                let pos = draft.current_preview_end;
                ui.monospace(format!(
                    "XYZ: ({:+6.2}, {:+6.2}, {:+6.2})",
                    pos.x, pos.y, pos.z
                ));
                ui.separator();

                let fp = focal_point.0;
                ui.monospace(format!(
                    "Target: ({:+.1}, {:+.1}, {:+.1})",
                    fp.x, fp.y, fp.z
                ));
                ui.separator();

                let inference_color = match cursor_ray.inference {
                    AxisInference::None => egui::Color32::from_rgb(180, 185, 195),
                    AxisInference::RedX => egui::Color32::from_rgb(235, 65, 65),
                    AxisInference::GreenY => egui::Color32::from_rgb(65, 210, 80),
                    AxisInference::BlueZ => egui::Color32::from_rgb(75, 145, 255),
                    AxisInference::ParallelNeighbor { .. }
                    | AxisInference::FlushTopFace { .. } => egui::Color32::from_rgb(245, 65, 220),
                };
                ui.colored_label(inference_color, cursor_ray.inference.label());
                ui.separator();

                if let Some(ref msg) = vcb.last_status_message {
                    ui.weak(msg);
                } else {
                    ui.weak(tool_state.current.status_hint());
                }

                // Bottom-Right SketchUp "Measurements" (VCB) Box
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let display_value = if vcb.buffer.is_empty() {
                        match tool_state.current {
                            CadTool::LineDraw => draft
                                .active_start_point
                                .map(|start| {
                                    format!("{:.2} m", start.distance(draft.current_preview_end))
                                })
                                .unwrap_or_default(),
                            CadTool::Block => draft
                                .active_start_point
                                .map(|start| {
                                    let dx = (draft.current_preview_end.x - start.x).abs();
                                    let dy = (draft.current_preview_end.y - start.y).abs();
                                    format!(
                                        "{:.2}, {:.2}, {:.2}",
                                        dx, dy, draft.default_block_height
                                    )
                                })
                                .unwrap_or_default(),
                            CadTool::Move => move_state
                                .active_drag
                                .as_ref()
                                .map(|d| format!("{:.2} m", d.current_offset.length()))
                                .unwrap_or_default(),
                            CadTool::Scale => scale_state
                                .active_drag
                                .as_ref()
                                .map(|d| {
                                    let f = d.current_scale_factors;
                                    format!("{:.2}, {:.2}, {:.2}", f.x, f.y, f.z)
                                })
                                .unwrap_or_else(|| String::from("1.00")),
                            _ => String::new(),
                        }
                    } else {
                        format!("{}|", vcb.buffer)
                    };

                    egui::Frame::canvas(ui.style())
                        .fill(egui::Color32::from_rgb(14, 16, 20))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if vcb.buffer.is_empty() {
                                egui::Color32::from_rgb(70, 76, 88)
                            } else {
                                egui::Color32::from_rgb(100, 195, 255)
                            },
                        ))
                        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                        .show(ui, |ui| {
                            ui.set_min_width(145.0);
                            ui.monospace(
                                egui::RichText::new(display_value)
                                    .color(egui::Color32::from_rgb(240, 245, 255)),
                            );
                        });

                    ui.label(
                        egui::RichText::new(format!("{}:", tool_state.current.vcb_label()))
                            .strong(),
                    );
                });
            });
        });

    // ========================================================================
    // 2. LEFT-SIDE VERTICAL TOOLBAR PANEL (`egui::SidePanel`)
    // ========================================================================
    egui::SidePanel::left("sketchcraft_left_toolbar")
        .resizable(false)
        .default_width(265.0)
        .show(ctx, |ui| {
            // Pin the Exit button cleanly at the bottom of the panel first
            egui::TopBottomPanel::bottom("sketchcraft_toolbar_footer")
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| {
                    ui.add_space(4.0);
                    ui.separator();
                    let exit_button = egui::Button::new(
                        egui::RichText::new("Exit Application (Ctrl+Q)")
                            .color(egui::Color32::from_rgb(255, 230, 230))
                            .strong(),
                    )
                    .fill(egui::Color32::from_rgb(145, 34, 38))
                    .min_size(egui::vec2(ui.available_width(), 34.0));

                    if ui.add(exit_button).clicked() {
                        app_exit_events.send(AppExit::Success);
                    }
                    ui.add_space(4.0);
                });

            // Scrollable main toolbar body + permanently visible Hotkeys Cheat Sheet
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("sketchcraft");
                ui.small("Z-Up 3D CAD Workspace");
                ui.separator();

                // Modeling & Transform Tools
                ui.label(egui::RichText::new("CAD Tools").strong());
                ui.add_space(4.0);

                let tools = [
                    CadTool::Select,
                    CadTool::LineDraw,
                    CadTool::Block,
                    CadTool::Move,
                    CadTool::Scale,
                    CadTool::PushPullExtrude,
                ];

                for candidate in tools {
                    let is_selected = tool_state.current == candidate;
                    let button = egui::Button::new(candidate.label())
                        .selected(is_selected)
                        .min_size(egui::vec2(ui.available_width(), 30.0));

                    if ui.add(button).clicked() {
                        tool_state.current = candidate;
                        draft.active_start_point = None;
                        move_state.active_drag = None;
                        scale_state.active_drag = None;
                        vcb.buffer.clear();
                    }
                    ui.add_space(2.0);
                }

                if draft.active_start_point.is_some()
                    || move_state.active_drag.is_some()
                    || scale_state.active_drag.is_some()
                {
                    ui.add_space(4.0);
                    if ui.button("Break Chain / Cancel (Esc)").clicked() {
                        draft.active_start_point = None;
                        move_state.active_drag = None;
                        scale_state.active_drag = None;
                        vcb.buffer.clear();
                    }
                }

                // Command History (Undo / Redo)
                ui.add_space(6.0);
                ui.separator();
                ui.label(egui::RichText::new("Command History").strong());
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let half_width = (ui.available_width() - ui.spacing().item_spacing.x) * 0.5;

                    let undo_btn =
                        egui::Button::new(format!("Undo ({})", history.undo_stack.len()))
                            .min_size(egui::vec2(half_width, 28.0));
                    if ui.add_enabled(history.can_undo(), undo_btn).clicked() {
                        draft.active_start_point = None;
                        move_state.active_drag = None;
                        scale_state.active_drag = None;
                        pending_actions.queue.push(HistoryAction::Undo);
                    }

                    let redo_btn =
                        egui::Button::new(format!("Redo ({})", history.redo_stack.len()))
                            .min_size(egui::vec2(half_width, 28.0));
                    if ui.add_enabled(history.can_redo(), redo_btn).clicked() {
                        draft.active_start_point = None;
                        move_state.active_drag = None;
                        scale_state.active_drag = None;
                        pending_actions.queue.push(HistoryAction::Redo);
                    }
                });

                if let Some(last_cmd) = history.undo_stack.last() {
                    ui.small(format!("Last: {}", last_cmd.description()));
                } else {
                    ui.small("Last: None");
                }

                // Selection & Group Inspector
                ui.add_space(6.0);
                ui.separator();
                ui.label(egui::RichText::new("Selection & Groups").strong());
                ui.add_space(2.0);

                let total_objects = cad_objects.iter().count();
                let selected_count = selected.entities.len();
                ui.small(format!(
                    "Scene Objects: {total_objects} | Selected: {selected_count}"
                ));

                if let Some(&first_entity) = selected.entities.first() {
                    if let Ok((_, cad_obj)) = cad_objects.get(first_entity) {
                        ui.small(format!("Label: {}", cad_obj.label));
                        let group_text = match cad_obj.group_id {
                            Some(gid) => format!("Group #{}", gid.0),
                            None => String::from("Ungrouped"),
                        };
                        let rgba = cad_obj.material_color.0.to_srgba();
                        ui.small(format!(
                            "{group_text} | RGB({:.0}, {:.0}, {:.0})",
                            rgba.red * 255.0,
                            rgba.green * 255.0,
                            rgba.blue * 255.0
                        ));
                    }
                }

                let delete_btn = egui::Button::new("Delete Selected (Del)")
                    .min_size(egui::vec2(ui.available_width(), 26.0));
                if ui.add_enabled(selected_count > 0, delete_btn).clicked() {
                    let mut snapshots = Vec::new();
                    for &entity in &selected.entities {
                        if let Ok((ent, obj)) = cad_objects.get(entity) {
                            snapshots.push((ent, obj.clone()));
                        }
                    }
                    if !snapshots.is_empty() {
                        selected.entities.clear();
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(
                            DeleteCommand::new(snapshots),
                        )));
                        vcb.last_status_message =
                            Some(String::from("Deleted selected CAD objects"));
                    }
                }

                // Restored & Permanently Open Hotkeys Cheat Sheet
                ui.add_space(6.0);
                ui.separator();
                egui::CollapsingHeader::new(
                    egui::RichText::new("Hotkeys Cheat Sheet").strong(),
                )
                .default_open(true)
                .show(ui, |ui| {
                    let shortcuts = [
                        ("[Spacebar]", "Select Tool"),
                        ("[L]", "Line Draw Tool"),
                        ("[B]", "Block / Box Tool"),
                        ("[M]", "Move Tool"),
                        ("[S]", "Scale / Resize Tool"),
                        ("[Delete / Backspace]", "Delete Selected Object"),
                        ("[Shift + MMB Drag]", "Pan Camera Field"),
                        ("[MMB Drag]", "Orbit Camera View"),
                        ("[Mouse Scroll]", "Zoom In / Out"),
                        (
                            "[Hold Shift while drawing/moving]",
                            "Lock to Vertical Blue (Z) Axis",
                        ),
                        ("[Ctrl + Z]", "Undo Last Action"),
                        ("[Ctrl + Y]", "Redo Action"),
                        ("[Escape]", "Cancel Operation / Break Chain"),
                        ("[Ctrl + Q]", "Exit Application cleanly"),
                    ];

                    for (key, desc) in shortcuts {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(115, 205, 255),
                                egui::RichText::new(key).monospace().small(),
                            );
                            ui.small(format!("- {desc}"));
                        });
                    }
                });
            });
        });

    ui_capture.egui_wants_pointer = ctx.wants_pointer_input() || ctx.is_pointer_over_area();
    ui_capture.egui_wants_keyboard = ctx.wants_keyboard_input();
}
