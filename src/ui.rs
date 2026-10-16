use bevy::{app::AppExit, prelude::*};
use bevy_egui::{egui, EguiContexts};

use crate::{
    commands::{CadObject, CommandHistory, DeleteCommand, HistoryAction, PendingHistoryActions},
    tools::{
        ActiveToolState, AxisInference, CadTool, CursorRayState, FocalPoint, MoveToolState,
        OffsetToolState, ScaleToolState, SelectedCadObjects, ToolDraftState, VcbInput,
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
            .add_systems(Update, draw_draftdown_ui.in_set(CadUiSet));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_draftdown_ui(
    mut contexts: EguiContexts,
    mut tool_state: ResMut<ActiveToolState>,
    mut ui_capture: ResMut<UiInputCapture>,
    mut draft: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut offset_state: ResMut<OffsetToolState>,
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
        return;
    };

    let mut visuals = egui::Visuals::dark();
    visuals.window_rounding = egui::Rounding::same(4.0);
    visuals.panel_fill = egui::Color32::from_rgba_unmultiplied(24, 26, 31, 246);
    ctx.set_visuals(visuals);

    egui::TopBottomPanel::bottom("draftdown_status_bar")
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
                    "Pivot: ({:+.1}, {:+.1}, {:+.1})",
                    fp.x, fp.y, fp.z
                ));
                ui.separator();

                let inf_color = match cursor_ray.inference {
                    AxisInference::None => egui::Color32::from_rgb(180, 185, 195),
                    AxisInference::RedX => egui::Color32::from_rgb(235, 65, 65),
                    AxisInference::GreenY => egui::Color32::from_rgb(65, 210, 80),
                    AxisInference::BlueZ => egui::Color32::from_rgb(75, 145, 255),
                    AxisInference::ParallelNeighbor { .. }
                    | AxisInference::FlushTopFace { .. } => egui::Color32::from_rgb(245, 65, 220),
                };
                ui.colored_label(inf_color, cursor_ray.inference.label());
                ui.separator();

                if let Some(ref msg) = vcb.last_status_message {
                    ui.weak(msg);
                } else {
                    ui.weak(tool_state.current.status_hint());
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let vcb_display = if vcb.buffer.is_empty() {
                        match tool_state.current {
                            CadTool::LineDraw => draft
                                .active_start_point
                                .map(|s| format!("{:.2} m", s.distance(draft.current_preview_end)))
                                .unwrap_or_default(),
                            CadTool::Block => draft
                                .active_start_point
                                .map(|s| {
                                    format!(
                                        "{:.2}, {:.2}, {:.2}",
                                        (draft.current_preview_end.x - s.x).abs(),
                                        (draft.current_preview_end.y - s.y).abs(),
                                        draft.default_block_height
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
                            CadTool::Offset => {
                                format!("{:+.2} m", offset_state.current_offset_distance)
                            }
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
                                egui::RichText::new(vcb_display)
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

    egui::SidePanel::left("draftdown_left_toolbar")
        .resizable(false)
        .default_width(265.0)
        .show(ctx, |ui| {
            egui::TopBottomPanel::bottom("draftdown_toolbar_footer")
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| {
                    ui.add_space(4.0);
                    ui.separator();
                    let exit_btn = egui::Button::new(
                        egui::RichText::new("Exit Application (Ctrl+Q)")
                            .color(egui::Color32::from_rgb(255, 230, 230))
                            .strong(),
                    )
                    .fill(egui::Color32::from_rgb(145, 34, 38))
                    .min_size(egui::vec2(ui.available_width(), 34.0));

                    if ui.add(exit_btn).clicked() {
                        app_exit_events.send(AppExit::Success);
                    }
                    ui.add_space(4.0);
                });

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("DraftDown");
                ui.small("Z-Up 3D CAD Workspace");
                ui.separator();

                ui.label(egui::RichText::new("CAD Tools").strong());
                ui.add_space(4.0);

                let tool_list = [
                    CadTool::Select,
                    CadTool::LineDraw,
                    CadTool::Block,
                    CadTool::Move,
                    CadTool::Scale,
                    CadTool::Offset,
                    CadTool::PushPullExtrude,
                ];

                for item in tool_list {
                    let active = tool_state.current == item;
                    let btn = egui::Button::new(item.label())
                        .selected(active)
                        .min_size(egui::vec2(ui.available_width(), 30.0));

                    if ui.add(btn).clicked() {
                        tool_state.current = item;
                        draft.active_start_point = None;
                        move_state.active_drag = None;
                        scale_state.active_drag = None;
                        offset_state.is_dragging = false;
                        offset_state.active_contour = None;
                        vcb.buffer.clear();
                    }
                    ui.add_space(2.0);
                }

                ui.add_space(6.0);
                ui.separator();
                ui.label(egui::RichText::new("Command History").strong());
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let half = (ui.available_width() - ui.spacing().item_spacing.x) * 0.5;
                    let u_btn = egui::Button::new(format!("Undo ({})", history.undo_stack.len()))
                        .min_size(egui::vec2(half, 28.0));
                    if ui.add_enabled(history.can_undo(), u_btn).clicked() {
                        draft.active_start_point = None;
                        pending_actions.queue.push(HistoryAction::Undo);
                    }

                    let r_btn = egui::Button::new(format!("Redo ({})", history.redo_stack.len()))
                        .min_size(egui::vec2(half, 28.0));
                    if ui.add_enabled(history.can_redo(), r_btn).clicked() {
                        draft.active_start_point = None;
                        pending_actions.queue.push(HistoryAction::Redo);
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                ui.label(egui::RichText::new("Selection & Groups").strong());
                let total_count = cad_objects.iter().count();
                let sel_count = selected.entities.len();
                ui.small(format!("Objects: {total_count} | Selected: {sel_count}"));

                let del_btn = egui::Button::new("Delete Selected (Del)")
                    .min_size(egui::vec2(ui.available_width(), 26.0));
                if ui.add_enabled(sel_count > 0, del_btn).clicked() {
                    let mut snaps = Vec::new();
                    for &ent in &selected.entities {
                        if let Ok((e, o)) = cad_objects.get(ent) {
                            snaps.push((e, o.clone()));
                        }
                    }
                    selected.entities.clear();
                    if !snaps.is_empty() {
                        pending_actions
                            .queue
                            .push(HistoryAction::Execute(Box::new(DeleteCommand::new(snaps))));
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                egui::CollapsingHeader::new(
                    egui::RichText::new("Hotkeys Cheat Sheet").strong(),
                )
                .default_open(true)
                .show(ui, |ui| {
                    let hotkeys = [
                        ("[Spacebar]", "Select Tool"),
                        ("[L]", "Line Draw Tool"),
                        ("[B]", "Block / Box Tool"),
                        ("[M]", "Move Tool"),
                        ("[S]", "Scale / Resize Tool"),
                        ("[F]", "Offset Tool"),
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

                    for (k, desc) in hotkeys {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(115, 205, 255),
                                egui::RichText::new(k).monospace().small(),
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
