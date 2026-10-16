use bevy::app::AppExit;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::commands::{CadGeometryKind, CadObject, CommandHistory, DeleteCommand};
use crate::tools::{
    ActiveToolState, AxisInference, CadTool, CursorRayState, FocalPoint, MoveToolState,
    OffsetToolState, ScaleToolState, SelectedCadObjects, SnapAxisKind, ToolDraftState, VcbInput,
};

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadUiSet;

pub struct CadUiPlugin;

impl Plugin for CadUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_draftdown_ui.in_set(CadUiSet));
    }
}

fn draw_draftdown_ui(
    mut egui_contexts: EguiContexts,
    mut active_tool: ResMut<ActiveToolState>,
    mut draft_state: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut offset_state: ResMut<OffsetToolState>,
    mut selected: ResMut<SelectedCadObjects>,
    cursor_state: Res<CursorRayState>,
    focal_point: Res<FocalPoint>,
    inference: Res<AxisInference>,
    vcb: Res<VcbInput>,
    mut history: ResMut<CommandHistory>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut exit_writer: EventWriter<AppExit>,
) {
    let Some(ctx) = egui_contexts.try_ctx_mut() else {
        return;
    };

    egui::TopBottomPanel::bottom("draftdown_bottom_status_bar")
        .exact_height(34.0_f32)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    egui::RichText::new(format!("Tool: {}", active_tool.current.label()))
                        .strong()
                        .color(egui::Color32::from_rgb(130, 205, 255)),
                );
                ui.separator();

                if cursor_state.has_hit {
                    let p = cursor_state.snapped_pos;
                    ui.label(format!(
                        "XYZ: ({:.2}, {:.2}, {:.2}) [{}]",
                        p.x, p.y, p.z, cursor_state.plane_label
                    ));
                } else {
                    ui.label("XYZ: (--, --, --)");
                }

                ui.separator();
                let fp = focal_point.0;
                ui.label(format!("Pivot: ({:.1}, {:.1}, {:.1})", fp.x, fp.y, fp.z));

                ui.separator();
                let snap_color = match inference.snap_kind {
                    SnapAxisKind::RedX => egui::Color32::from_rgb(235, 65, 70),
                    SnapAxisKind::GreenY => egui::Color32::from_rgb(55, 210, 95),
                    SnapAxisKind::BlueZ => egui::Color32::from_rgb(65, 145, 250),
                    SnapAxisKind::ParallelNeighbor => egui::Color32::from_rgb(245, 75, 230),
                    SnapAxisKind::None => egui::Color32::from_rgb(185, 190, 200),
                };
                ui.label(
                    egui::RichText::new(inference.snap_kind.status_text())
                        .strong()
                        .color(snap_color),
                );

                if active_tool.current == CadTool::Offset && offset_state.is_dragging {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!(
                            "Offset: {:+.2} m",
                            offset_state.current_offset_distance
                        ))
                        .strong()
                        .color(egui::Color32::from_rgb(255, 175, 45)),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let vcb_display = if vcb.buffer.is_empty() {
                        if vcb.last_status.is_empty() {
                            "Type dimension + Enter".to_string()
                        } else {
                            vcb.last_status.clone()
                        }
                    } else {
                        format!("{}_", vcb.buffer)
                    };

                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(24, 28, 36))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(90, 150, 230),
                        ))
                        .inner_margin(egui::Margin::symmetric(8.0_f32, 3.0_f32))
                        .rounding(4.0_f32)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(vcb_display)
                                    .monospace()
                                    .strong()
                                    .color(egui::Color32::from_rgb(240, 245, 255)),
                            );
                        });

                    ui.label(
                        egui::RichText::new("Measurements (VCB):")
                            .strong()
                            .color(egui::Color32::from_rgb(190, 200, 215)),
                    );
                });
            });
        });

    egui::SidePanel::left("draftdown_left_toolbar")
        .resizable(false)
        .default_width(245.0_f32)
        .show(ctx, |ui| {
            ui.add_space(6.0_f32);
            ui.heading(
                egui::RichText::new("DraftDown CAD")
                    .strong()
                    .color(egui::Color32::from_rgb(230, 240, 255)),
            );
            ui.label(
                egui::RichText::new("Wayland / Hyprland 3D Workspace")
                    .small()
                    .color(egui::Color32::from_rgb(150, 160, 175)),
            );
            ui.separator();

            ui.label(egui::RichText::new("Modeling & Transform Tools").strong());
            ui.add_space(4.0_f32);

            let tool_items = [
                CadTool::Select,
                CadTool::LineDraw,
                CadTool::Block,
                CadTool::Move,
                CadTool::Scale,
                CadTool::Offset,
                CadTool::PushPullExtrude,
            ];

            for item in tool_items {
                let is_active = active_tool.current == item;
                let fill_col = if is_active {
                    egui::Color32::from_rgb(42, 105, 185)
                } else {
                    egui::Color32::from_rgb(38, 42, 52)
                };
                let btn = egui::Button::new(
                    egui::RichText::new(item.label()).color(egui::Color32::WHITE),
                )
                .fill(fill_col)
                .min_size(egui::vec2(220.0_f32, 28.0_f32));

                if ui.add(btn).clicked() && active_tool.current != item {
                    active_tool.current = item;
                    draft_state.reset();
                    move_state.active_drag = None;
                    scale_state.active_drag = None;
                    offset_state.reset_drag();
                }
            }

            ui.add_space(8.0_f32);
            ui.separator();
            ui.label(egui::RichText::new("History (Command Stack)").strong());
            ui.add_space(4.0_f32);

            ui.horizontal(|ui| {
                let undo_enabled = history.can_undo();
                let redo_enabled = history.can_redo();

                if ui
                    .add_enabled(
                        undo_enabled,
                        egui::Button::new(format!("Undo ({})", history.undo_count())),
                    )
                    .clicked()
                {
                    history.request_undo();
                }

                if ui
                    .add_enabled(
                        redo_enabled,
                        egui::Button::new(format!("Redo ({})", history.redo_count())),
                    )
                    .clicked()
                {
                    history.request_redo();
                }
            });

            if let Some(desc) = history.last_undo_description() {
                ui.label(
                    egui::RichText::new(format!("Last: {desc}"))
                        .small()
                        .color(egui::Color32::from_rgb(155, 165, 180)),
                );
            }

            ui.add_space(8.0_f32);
            ui.separator();
            ui.label(egui::RichText::new("Selection Inspector").strong());

            let total_entities = cad_objects.iter().count();
            ui.label(format!("Scene Objects: {total_entities}"));

            if let Some(sel_ent) = selected.primary() {
                if let Ok((_, obj)) = cad_objects.get(sel_ent) {
                    ui.label(
                        egui::RichText::new(format!("Selected: {}", obj.label))
                            .color(egui::Color32::from_rgb(80, 220, 255))
                            .strong(),
                    );
                    if let Some(gid) = obj.group_id {
                        ui.label(format!("Group ID: #{}", gid.0));
                    }
                    match obj.geometry {
                        CadGeometryKind::Line { start, end, .. } => {
                            ui.label(format!("Length: {:.2} m", start.distance(end)));
                        }
                        CadGeometryKind::Block { size, .. } => {
                            ui.label(format!(
                                "Size: {:.2} x {:.2} x {:.2} m",
                                size.x, size.y, size.z
                            ));
                        }
                    }

                    if ui
                        .button(
                            egui::RichText::new("Delete Selected (Del)")
                                .color(egui::Color32::from_rgb(255, 120, 120)),
                        )
                        .clicked()
                    {
                        history.execute_command(Box::new(DeleteCommand::new(vec![(
                            sel_ent,
                            obj.clone(),
                        )])));
                        selected.clear();
                    }
                }
            } else {
                ui.label(
                    egui::RichText::new("No object selected")
                        .italics()
                        .color(egui::Color32::from_rgb(140, 145, 155)),
                );
            }

            ui.add_space(8.0_f32);
            ui.separator();

            egui::CollapsingHeader::new(
                egui::RichText::new("Hotkeys Cheat Sheet").strong(),
            )
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Space : Select Tool");
                ui.label("L : Line Tool (Continuous Chain)");
                ui.label("B : 3D Block / Box Tool");
                ui.label("M : Move Tool (Translate)");
                ui.label("S : Scale Tool (8-Handle Box)");
                ui.label("F : Offset Tool (Coplanar 2D UV)");
                ui.label("Hold Shift : Lock Blue (Z) Axis");
                ui.label("MMB Drag : Orbit Targeted Object");
                ui.label("Shift + MMB : Pan Focal Pivot");
                ui.label("Scroll : Smooth Zoom");
                ui.label("0-9 / , / Enter : VCB Dimensions");
                ui.label("Del / Backspace : Delete Object");
                ui.label("Esc : Break Chain / Cancel Drag");
                ui.label("Ctrl+Z / Ctrl+Y : Undo / Redo");
                ui.label("Ctrl+Q : Exit Application");
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(8.0_f32);
                let exit_btn = egui::Button::new(
                    egui::RichText::new("Exit Application (Ctrl+Q)")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(175, 42, 48))
                .min_size(egui::vec2(220.0_f32, 30.0_f32));

                if ui.add(exit_btn).clicked() {
                    exit_writer.send(AppExit::Success);
                }
            });
        });
}