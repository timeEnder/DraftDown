use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::{
    app::AppExit,
    input::{
        keyboard::{Key, KeyboardInput},
        mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
        ButtonState,
    },
    pbr::NotShadowCaster,
    prelude::*,
    render::{
        mesh::{Indices, PrimitiveTopology},
        render_asset::RenderAssetUsages,
    },
    window::PrimaryWindow,
};

use crate::{
    commands::{
        CadGeometryKind, CadObject, Command, CommandHistory, CreateBlockCommand,
        CreateOffsetCommand, DeleteCommand, DrawLineCommand, GroupId, HistoryAction,
        MoveObjectCommand, PendingHistoryActions, ScaleObjectCommand,
    },
    ui::UiInputCapture,
};

pub const AXIS_RED_COLOR: Color = Color::srgb(0.92, 0.16, 0.16);
pub const AXIS_GREEN_COLOR: Color = Color::srgb(0.18, 0.80, 0.24);
pub const AXIS_BLUE_COLOR: Color = Color::srgb(0.16, 0.46, 0.96);
pub const INFERENCE_MAGENTA_COLOR: Color = Color::srgb(0.95, 0.20, 0.85);
pub const DEFAULT_LINE_PREVIEW_COLOR: Color = Color::srgb(0.98, 0.84, 0.20);
pub const NEON_BLUE_SELECTION_COLOR: Color = Color::srgb(0.05, 0.88, 1.0);
pub const SCALE_BBOX_COLOR: Color = Color::srgb(1.0, 0.78, 0.15);
pub const SCALE_HANDLE_COLOR: Color = Color::srgb(0.15, 0.95, 0.55);
pub const SCALE_ACTIVE_HANDLE_COLOR: Color = Color::srgb(1.0, 0.30, 0.20);
pub const OFFSET_HIGHLIGHT_COLOR: Color = Color::srgb(0.20, 0.95, 0.88);
pub const OFFSET_PREVIEW_COLOR: Color = Color::srgb(1.0, 0.58, 0.16);

const GRID_SNAP_STEP: f32 = 1.0_f32;
const NEIGHBOR_SCAN_RADIUS: f32 = 40.0_f32;
const PARALLEL_THRESHOLD_DEG: f32 = 2.0_f32;
const PITCH_LIMIT_RAD: f32 = 85.0_f32 * (PI / 180.0_f32);
const MAX_PLANE_RAY_DIST: f32 = 85.0_f32;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadToolsSet;

pub struct CadToolsPlugin;

impl Plugin for CadToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveToolState>()
            .init_resource::<CursorRayState>()
            .init_resource::<ToolDraftState>()
            .init_resource::<MoveToolState>()
            .init_resource::<ScaleToolState>()
            .init_resource::<OffsetToolState>()
            .init_resource::<VcbInput>()
            .init_resource::<SelectedCadObjects>()
            .init_resource::<FocalPoint>()
            .add_systems(
                Startup,
                (
                    init_cad_scene_and_lights,
                    init_orbit_camera,
                    init_block_preview_mesh,
                    populate_starter_geometry,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    process_keyboard_and_vcb_events,
                    compute_raycast_and_snapping,
                    update_orbit_camera_motion,
                    process_viewport_pointer_actions,
                    refresh_selection_materials,
                    sync_block_preview_mesh,
                    draw_viewport_gizmos,
                )
                    .chain()
                    .in_set(CadToolsSet),
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CadTool {
    #[default]
    Select,
    LineDraw,
    Block,
    Move,
    Scale,
    Offset,
    PushPullExtrude,
}

impl CadTool {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select (Space)",
            Self::LineDraw => "Line (L)",
            Self::Block => "Block (B)",
            Self::Move => "Move (M)",
            Self::Scale => "Scale (S)",
            Self::Offset => "Offset (F)",
            Self::PushPullExtrude => "Push/Pull",
        }
    }

    pub const fn vcb_label(self) -> &'static str {
        match self {
            Self::Select => "Measurements",
            Self::LineDraw => "Length",
            Self::Block => "Dimensions (W,D,H)",
            Self::Move => "Move Distance",
            Self::Scale => "Scale Factor",
            Self::Offset => "Offset Distance",
            Self::PushPullExtrude => "Extrude Height",
        }
    }

    pub const fn status_hint(self) -> &'static str {
        match self {
            Self::Select => "LMB: Select Object | MMB on Object: Targeted Orbit | Del: Delete",
            Self::LineDraw => "Click to chain lines | Hold Shift: Vertical Z | Magenta: 2° Parallel Guide",
            Self::Block => "1st Click: Corner | 2nd Click: Commit Block | Or type 'W,D,H' + Enter",
            Self::Move => "LMB Drag on object: Translate | Hold Shift: Vertical Z | Release: Commit",
            Self::Scale => "LMB Drag any of the 8 corner handles | Or type Scale Factor + Enter",
            Self::Offset => {
                "Hover face/chain -> LMB Drag inward/outward or type decimal offset (e.g. 0.3) + Enter"
            }
            Self::PushPullExtrude => "Click to extrude 3D column | Type height + Enter",
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct ActiveToolState {
    pub current: CadTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicDrawingPlane {
    #[default]
    HorizontalXY,
    VerticalXZ,
    VerticalYZ,
    CameraFacing,
}

impl DynamicDrawingPlane {
    pub fn choose_for_view(forward: Vec3, has_anchor: bool) -> Self {
        let ax = forward.x.abs();
        let ay = forward.y.abs();
        let az = forward.z.abs();

        if az >= 0.32 && (!has_anchor || az >= ax.max(ay) * 0.55) {
            Self::HorizontalXY
        } else if ax >= ay && ax >= az {
            Self::VerticalYZ
        } else if ay >= ax && ay >= az {
            Self::VerticalXZ
        } else {
            Self::HorizontalXY
        }
    }

    pub fn normal_dir(self, cam_forward: Dir3) -> Dir3 {
        match self {
            Self::HorizontalXY => Dir3::Z,
            Self::VerticalXZ => Dir3::Y,
            Self::VerticalYZ => Dir3::X,
            Self::CameraFacing => -cam_forward,
        }
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct FocalPoint(pub Vec3);

impl Default for FocalPoint {
    fn default() -> Self {
        Self(Vec3::new(1.0, 1.0, 0.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AxisInference {
    #[default]
    None,
    RedX,
    GreenY,
    BlueZ,
    ParallelNeighbor {
        edge_start: Vec3,
        edge_end: Vec3,
        direction: Vec3,
    },
    FlushTopFace {
        elevation_z: f32,
        face_center: Vec3,
    },
}

impl AxisInference {
    pub const fn preview_color(self) -> Color {
        match self {
            Self::None => DEFAULT_LINE_PREVIEW_COLOR,
            Self::RedX => AXIS_RED_COLOR,
            Self::GreenY => AXIS_GREEN_COLOR,
            Self::BlueZ => AXIS_BLUE_COLOR,
            Self::ParallelNeighbor { .. } | Self::FlushTopFace { .. } => INFERENCE_MAGENTA_COLOR,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Grid Snap (1m)",
            Self::RedX => "On Red Axis (X)",
            Self::GreenY => "On Green Axis (Y)",
            Self::BlueZ => "On Blue Axis (Z / Vertical)",
            Self::ParallelNeighbor { .. } => "Parallel to Neighbor Edge (<2° Magenta)",
            Self::FlushTopFace { .. } => "Flush with Top Face (Magenta)",
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct CursorRayState {
    pub raw_ground_hit: Vec3,
    pub snapped_pos: Vec3,
    pub is_valid_hit: bool,
    pub inference: AxisInference,
    pub vertical_z_locked: bool,
    pub active_plane: DynamicDrawingPlane,
}

#[derive(Resource, Debug)]
pub struct ToolDraftState {
    pub active_start_point: Option<Vec3>,
    pub current_preview_end: Vec3,
    pub default_block_height: f32,
}

impl Default for ToolDraftState {
    fn default() -> Self {
        Self {
            active_start_point: None,
            current_preview_end: Vec3::ZERO,
            default_block_height: 2.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveMoveDrag {
    pub entity: Entity,
    pub grab_anchor: Vec3,
    pub current_offset: Vec3,
    pub initial_geometry: CadGeometryKind,
    pub initial_transform: Transform,
}

#[derive(Resource, Debug, Default)]
pub struct MoveToolState {
    pub active_drag: Option<ActiveMoveDrag>,
}

#[derive(Debug, Clone)]
pub struct ActiveScaleDrag {
    pub entity: Entity,
    pub handle_index: usize,
    pub opposite_anchor: Vec3,
    pub initial_handle_pos: Vec3,
    pub current_scale_factors: Vec3,
    pub initial_geometry: CadGeometryKind,
    pub initial_transform: Transform,
}

#[derive(Resource, Debug, Default)]
pub struct ScaleToolState {
    pub active_drag: Option<ActiveScaleDrag>,
    pub hovered_handle: Option<usize>,
}

/// Represents a detected 2D contour (either a closed Block face perimeter or a connected chain of lines)
/// that can be offset inwards or outwards.
#[derive(Debug, Clone)]
pub struct OffsetContour {
    pub segments: Vec<(Vec3, Vec3)>,
    pub center: Vec3,
    pub plane_z: f32,
    pub group_id: Option<GroupId>,
}

impl OffsetContour {
    /// Generates a concentric replica of `self.segments` offset by `signed_distance`
    /// (`> 0.0` = outward away from `self.center`, `< 0.0` = inward toward `self.center`).
    pub fn compute_offset_segments(&self, signed_distance: f32) -> Vec<(Vec3, Vec3)> {
        if self.segments.is_empty() || signed_distance.abs() <= 0.001 {
            return Vec::new();
        }

        // Compute outward-pointing 2D normal for each segment on the X/Y plane
        let shifted_lines: Vec<(Vec3, Vec3, Vec2)> = self
            .segments
            .iter()
            .filter_map(|&(a, b)| {
                let d2 = Vec2::new(b.x - a.x, b.y - a.y);
                let len = d2.length();
                if len <= 0.001 {
                    return None;
                }
                let tangent = d2 / len;
                let mut normal = Vec2::new(-tangent.y, tangent.x);
                let mid = (a + b) * 0.5;
                let from_center = Vec2::new(mid.x - self.center.x, mid.y - self.center.y);
                if normal.dot(from_center) < 0.0 {
                    normal = -normal;
                }
                let shift = Vec3::new(
                    normal.x * signed_distance,
                    normal.y * signed_distance,
                    0.0,
                );
                Some((a + shift, b + shift, tangent))
            })
            .collect();

        let count = shifted_lines.len();
        if count == 0 {
            return Vec::new();
        }
        if count == 1 {
            return vec![(shifted_lines[0].0, shifted_lines[0].1)];
        }

        // Check if the original contour forms a closed loop
        let first_pt = self.segments.first().map(|s| s.0).unwrap_or(Vec3::ZERO);
        let last_pt = self.segments.last().map(|s| s.1).unwrap_or(Vec3::ZERO);
        let is_closed = first_pt.distance(last_pt) <= 0.08;

        // Intersect adjacent shifted lines in 2D to form clean miter corners
        let mut out_segments = Vec::with_capacity(count);
        for i in 0..count {
            let (curr_a, curr_b, curr_t) = shifted_lines[i];

            let start_vertex = if i == 0 && !is_closed {
                curr_a
            } else {
                let prev_idx = if i == 0 { count - 1 } else { i - 1 };
                let (prev_a, prev_b, prev_t) = shifted_lines[prev_idx];
                intersect_2d_lines(prev_a, prev_b, prev_t, curr_a, curr_t)
            };

            let end_vertex = if i + 1 == count && !is_closed {
                curr_b
            } else {
                let next_idx = (i + 1) % count;
                let (next_a, _, next_t) = shifted_lines[next_idx];
                intersect_2d_lines(curr_a, curr_b, curr_t, next_a, next_t)
            };

            if start_vertex.distance(end_vertex) > 0.02 {
                out_segments.push((start_vertex, end_vertex));
            }
        }

        out_segments
    }
}

fn intersect_2d_lines(p1: Vec3, p1_end: Vec3, d1: Vec2, p2: Vec3, d2: Vec2) -> Vec3 {
    let cross = d1.x * d2.y - d1.y * d2.x;
    if cross.abs() < 1e-4 {
        return p1_end;
    }
    let delta = Vec2::new(p2.x - p1.x, p2.y - p1.y);
    let t = (delta.x * d2.y - delta.y * d2.x) / cross;
    // Clamp extreme miter spikes
    let clamped_t = t.clamp(-25.0, 25.0);
    Vec3::new(p1.x + d1.x * clamped_t, p1.y + d1.y * clamped_t, p1.z)
}

#[derive(Resource, Debug, Default)]
pub struct OffsetToolState {
    pub hovered_contour: Option<OffsetContour>,
    pub active_contour: Option<OffsetContour>,
    pub is_dragging: bool,
    pub grab_radius: f32,
    pub current_offset_distance: f32,
}

#[derive(Resource, Debug, Default)]
pub struct VcbInput {
    pub buffer: String,
    pub last_status_message: Option<String>,
}

#[derive(Resource, Debug, Default)]
pub struct SelectedCadObjects {
    pub entities: Vec<Entity>,
}

#[derive(Component, Debug)]
pub struct BlockPreviewVisual;

#[derive(Component, Debug)]
pub struct OrbitCamera {
    pub radius: f32,
    pub target_radius: f32,
    pub azimuth: f32,
    pub target_azimuth: f32,
    pub elevation: f32,
    pub target_elevation: f32,
    pub target_focus: Vec3,
    pub smoothed_delta: Vec2,
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub zoom_sensitivity: f32,
    pub damping: f32,
    pub min_radius: f32,
    pub max_radius: f32,
    pub is_mmb_dragging: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let focus = Vec3::new(1.0, 1.0, 0.0);
        Self {
            radius: 16.0,
            target_radius: 16.0,
            azimuth: -1.15,
            target_azimuth: -1.15,
            elevation: 0.52,
            target_elevation: 0.52,
            target_focus: focus,
            smoothed_delta: Vec2::ZERO,
            orbit_sensitivity: 0.0052,
            pan_sensitivity: 0.0017,
            zoom_sensitivity: 0.12,
            damping: 18.0,
            min_radius: 0.75,
            max_radius: 300.0,
            is_mmb_dragging: false,
        }
    }
}

impl OrbitCamera {
    pub fn build_transform(&self, focus: Vec3) -> Transform {
        let el = self.elevation.clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        let r = self.radius.clamp(self.min_radius, self.max_radius);
        let h = r * el.cos();
        let eye = focus + Vec3::new(h * self.azimuth.cos(), h * self.azimuth.sin(), r * el.sin());
        Transform::from_translation(eye).looking_at(focus, Vec3::Z)
    }

    pub fn reanchor_pivot(&mut self, eye: Vec3, pivot: Vec3) {
        let vec = eye - pivot;
        let dist = vec.length().clamp(self.min_radius, self.max_radius);
        if dist <= 0.001 {
            return;
        }
        let el = (vec.z / dist)
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        let az = vec.y.atan2(vec.x);

        self.target_focus = pivot;
        self.radius = dist;
        self.target_radius = dist;
        self.azimuth = az;
        self.target_azimuth = az;
        self.elevation = el;
        self.target_elevation = el;
    }
}

fn init_orbit_camera(mut commands: Commands, focal_point: Res<FocalPoint>) {
    let mut cam = OrbitCamera::default();
    cam.target_focus = focal_point.0;
    let tr = cam.build_transform(focal_point.0);

    commands.spawn((
        Camera3dBundle {
            transform: tr,
            projection: Projection::Perspective(PerspectiveProjection {
                fov: 45.0_f32.to_radians(),
                near: 0.05,
                far: 1200.0,
                ..default()
            }),
            ..default()
        },
        cam,
    ));
}

#[allow(clippy::too_many_arguments)]
fn update_orbit_camera_motion(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    ui_capture: Res<UiInputCapture>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut focal_point: ResMut<FocalPoint>,
    mut motion_events: EventReader<MouseMotion>,
    mut scroll_events: EventReader<MouseWheel>,
    mut camera_query: Query<(&Camera, &GlobalTransform, &mut OrbitCamera, &mut Transform)>,
) {
    let mut raw_delta = Vec2::ZERO;
    for ev in motion_events.read() {
        raw_delta += ev.delta;
    }

    let mut wheel = 0.0_f32;
    for ev in scroll_events.read() {
        let mult = match ev.unit {
            MouseScrollUnit::Line => 1.0,
            MouseScrollUnit::Pixel => 0.025,
        };
        wheel += ev.y * mult;
    }

    let Ok((camera, cam_global, mut orbit, mut transform)) = camera_query.get_single_mut() else {
        return;
    };

    let shift_held = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if mouse_buttons.just_pressed(MouseButton::Middle) && !ui_capture.egui_wants_pointer {
        orbit.is_mmb_dragging = true;
        orbit.smoothed_delta = Vec2::ZERO;

        if !shift_held {
            if let Ok(window) = window_query.get_single() {
                let screen_pt = window
                    .cursor_position()
                    .unwrap_or_else(|| Vec2::new(window.width() * 0.5, window.height() * 0.5));

                if let Some(ray) = camera.viewport_to_world(cam_global, screen_pt) {
                    let mut best_hit: Option<f32> = None;
                    for (_, obj) in cad_objects.iter() {
                        if let Some(d) = ray_intersects_cad_object(&ray, obj) {
                            if best_hit.map(|b| d < b).unwrap_or(true) {
                                best_hit = Some(d);
                            }
                        }
                    }

                    let pivot = if let Some(d) = best_hit {
                        Some(ray.get_point(d))
                    } else if let Some(gd) =
                        ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Z))
                    {
                        if gd > 0.05 && gd <= MAX_PLANE_RAY_DIST {
                            Some(ray.get_point(gd))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    if let Some(target_pt) = pivot {
                        orbit.reanchor_pivot(transform.translation, target_pt);
                        focal_point.0 = target_pt;
                    }
                }
            }
        }
    }

    if !mouse_buttons.pressed(MouseButton::Middle) {
        orbit.is_mmb_dragging = false;
        orbit.smoothed_delta = Vec2::ZERO;
    }

    let dt = time.delta_seconds().clamp(0.0005, 0.1);
    let blend = (1.0 - (-28.0 * dt).exp()).clamp(0.15, 1.0);
    orbit.smoothed_delta = orbit.smoothed_delta.lerp(raw_delta, blend);

    if orbit.is_mmb_dragging && orbit.smoothed_delta.length_squared() > 0.0001 {
        let d = orbit.smoothed_delta;
        if shift_held {
            let right = transform.rotation * Vec3::X;
            let up = transform.rotation * Vec3::Y;
            let scale = orbit.target_radius.max(1.0) * orbit.pan_sensitivity;
            orbit.target_focus += (-d.x * scale * right) + (d.y * scale * up);
        } else {
            orbit.target_azimuth -= d.x * orbit.orbit_sensitivity;
            orbit.target_elevation = (orbit.target_elevation + d.y * orbit.orbit_sensitivity)
                .clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        }
    }

    if !ui_capture.egui_wants_pointer && wheel.abs() > f32::EPSILON {
        let factor = (1.0 - wheel * orbit.zoom_sensitivity).clamp(0.5, 1.8);
        orbit.target_radius =
            (orbit.target_radius * factor).clamp(orbit.min_radius, orbit.max_radius);
    }

    let alpha = (1.0 - (-orbit.damping * dt).exp()).clamp(0.05, 1.0);
    orbit.azimuth += (orbit.target_azimuth - orbit.azimuth) * alpha;
    orbit.elevation = (orbit.elevation + (orbit.target_elevation - orbit.elevation) * alpha)
        .clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
    orbit.radius = (orbit.radius + (orbit.target_radius - orbit.radius) * alpha)
        .clamp(orbit.min_radius, orbit.max_radius);
    focal_point.0 = focal_point.0.lerp(orbit.target_focus, alpha);

    if !orbit.is_mmb_dragging && (orbit.target_azimuth - orbit.azimuth).abs() < 1e-4 {
        orbit.azimuth %= TAU;
        orbit.target_azimuth = orbit.azimuth;
    }

    *transform = orbit.build_transform(focal_point.0);
}

fn init_cad_scene_and_lights(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let floor_mesh = meshes.add(Plane3d::default().mesh().size(400.0, 400.0));
    let floor_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.19, 0.21, 0.24, 0.88),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.92,
        reflectance: 0.08,
        double_sided: true,
        cull_mode: None,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: floor_mesh,
            material: floor_mat,
            transform: Transform {
                translation: Vec3::new(0.0, 0.0, -0.004),
                rotation: Quat::from_rotation_x(FRAC_PI_2),
                scale: Vec3::ONE,
            },
            ..default()
        },
        NotShadowCaster,
    ));

    let unlit_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: meshes.add(create_ground_grid_mesh(100, 1.0, 5)),
            material: unlit_mat.clone(),
            transform: Transform::from_xyz(0.0, 0.0, -0.001),
            ..default()
        },
        NotShadowCaster,
    ));

    let axis_len = 120.0_f32;
    let cyl = meshes.add(Cylinder::new(0.024, axis_len));

    for (color, pos, rot) in [
        (
            AXIS_RED_COLOR,
            Vec3::new(axis_len * 0.5, 0.0, 0.0),
            Quat::from_rotation_z(-FRAC_PI_2),
        ),
        (
            AXIS_GREEN_COLOR,
            Vec3::new(0.0, axis_len * 0.5, 0.0),
            Quat::IDENTITY,
        ),
        (
            AXIS_BLUE_COLOR,
            Vec3::new(0.0, 0.0, axis_len * 0.5),
            Quat::from_rotation_x(FRAC_PI_2),
        ),
    ] {
        commands.spawn((
            PbrBundle {
                mesh: cyl.clone(),
                material: materials.add(StandardMaterial {
                    base_color: color,
                    unlit: true,
                    ..default()
                }),
                transform: Transform {
                    translation: pos,
                    rotation: rot,
                    scale: Vec3::ONE,
                },
                ..default()
            },
            NotShadowCaster,
        ));
    }

    commands.spawn((
        PbrBundle {
            mesh: meshes.add(create_axes_line_mesh(axis_len)),
            material: unlit_mat,
            ..default()
        },
        NotShadowCaster,
    ));

    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            color: Color::srgb(1.0, 0.98, 0.95),
            illuminance: 16_000.0,
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(22.0, -18.0, 34.0).looking_at(Vec3::ZERO, Vec3::Z),
        ..default()
    });

    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            color: Color::srgb(0.85, 0.90, 1.0),
            illuminance: 5_500.0,
            shadows_enabled: false,
            ..default()
        },
        transform: Transform::from_xyz(-20.0, 24.0, 18.0).looking_at(Vec3::ZERO, Vec3::Z),
        ..default()
    });
}

fn init_block_preview_mesh(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        PbrBundle {
            mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            material: materials.add(StandardMaterial {
                base_color: Color::srgba(0.24, 0.68, 0.98, 0.34),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            }),
            visibility: Visibility::Hidden,
            ..default()
        },
        NotShadowCaster,
        BlockPreviewVisual,
    ));
}

fn populate_starter_geometry(world: &mut World) {
    let initial: Vec<Box<dyn Command + Send + Sync>> = vec![
        Box::new(CreateBlockCommand::from_corner_and_dimensions(
            Vec3::new(-3.0, 1.0, 0.0),
            Vec3::new(2.0, 2.0, 2.0),
            Color::srgb(0.76, 0.80, 0.86),
            Some(GroupId(1)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(4.0, 1.0, 0.0),
            AXIS_RED_COLOR,
            Some(GroupId(2)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(4.0, 1.0, 0.0),
            Vec3::new(4.0, 4.0, 0.0),
            AXIS_GREEN_COLOR,
            Some(GroupId(2)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(4.0, 4.0, 0.0),
            Vec3::new(1.0, 4.0, 0.0),
            AXIS_RED_COLOR,
            Some(GroupId(2)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(1.0, 4.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            AXIS_GREEN_COLOR,
            Some(GroupId(2)),
        )),
    ];

    for mut cmd in initial {
        cmd.execute(world);
        if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
            history.undo_stack.push(cmd);
            history.redo_stack.clear();
        }
    }
}

fn create_ground_grid_mesh(half_count: i32, step: f32, major_interval: i32) -> Mesh {
    let span = half_count as f32 * step;
    let mut pts: Vec<[f32; 3]> = Vec::new();
    let mut cols: Vec<[f32; 4]> = Vec::new();
    let minor = [0.34, 0.37, 0.42, 0.52];
    let major = [0.50, 0.54, 0.60, 0.82];

    for i in -half_count..=half_count {
        let d = i as f32 * step;
        let col = if major_interval > 0 && i % major_interval == 0 {
            major
        } else {
            minor
        };

        if i == 0 {
            pts.extend([[-span, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -span, 0.0], [0.0, 0.0, 0.0]]);
            cols.extend([major, major, major, major]);
            continue;
        }

        pts.extend([[-span, d, 0.0], [span, d, 0.0], [d, -span, 0.0], [d, span, 0.0]]);
        cols.extend([col, col, col, col]);
    }

    let idx: Vec<u32> = (0..pts.len() as u32).collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::LineList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pts);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, cols);
    mesh.insert_indices(Indices::U32(idx));
    mesh
}

fn create_axes_line_mesh(len: f32) -> Mesh {
    let r = AXIS_RED_COLOR.to_linear().to_f32_array();
    let g = AXIS_GREEN_COLOR.to_linear().to_f32_array();
    let b = AXIS_BLUE_COLOR.to_linear().to_f32_array();
    let mut mesh = Mesh::new(
        PrimitiveTopology::LineList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [0.0, 0.0, 0.0],
            [len, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, len, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 0.0, len],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![r, r, g, g, b, b]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 3, 4, 5]));
    mesh
}

fn snap_to_integer_grid(pt: Vec3) -> Vec3 {
    Vec3::new(
        (pt.x / GRID_SNAP_STEP).round() * GRID_SNAP_STEP,
        (pt.y / GRID_SNAP_STEP).round() * GRID_SNAP_STEP,
        (pt.z / GRID_SNAP_STEP).round() * GRID_SNAP_STEP,
    )
}

/// Discovers a closed 2D face perimeter (from a hovered Block) or a connected chain of `CadObject`
/// lines connected to the hovered/selected line entity.
fn detect_offset_contour_for_entity(
    seed_entity: Entity,
    cad_objects: &Query<(Entity, &CadObject, &mut Transform)>,
    selected: &SelectedCadObjects,
) -> Option<OffsetContour> {
    let Ok((_, seed_obj, _)) = cad_objects.get(seed_entity) else {
        return None;
    };

    match seed_obj.geometry {
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5;
            let top_z = center.z + half.z;
            let c0 = Vec3::new(center.x - half.x, center.y - half.y, top_z);
            let c1 = Vec3::new(center.x + half.x, center.y - half.y, top_z);
            let c2 = Vec3::new(center.x + half.x, center.y + half.y, top_z);
            let c3 = Vec3::new(center.x - half.x, center.y + half.y, top_z);
            Some(OffsetContour {
                segments: vec![(c0, c1), (c1, c2), (c2, c3), (c3, c0)],
                center: Vec3::new(center.x, center.y, top_z),
                plane_z: top_z,
                group_id: seed_obj.group_id,
            })
        }
        CadGeometryKind::Line { start, end, .. } => {
            let plane_z = start.z;
            let mut pool: Vec<(Entity, Vec3, Vec3)> = Vec::new();

            for (ent, obj, _) in cad_objects.iter() {
                if let CadGeometryKind::Line {
                    start: s, end: e, ..
                } = obj.geometry
                {
                    if (s.z - plane_z).abs() <= 0.15 && (e.z - plane_z).abs() <= 0.15 {
                        pool.push((ent, s, e));
                    }
                }
            }

            // If multiple lines are explicitly selected, prioritize the selected chain
            let mut chain: Vec<(Vec3, Vec3)> = Vec::new();
            let mut used: Vec<Entity> = vec![seed_entity];
            chain.push((start, end));

            let mut head = start;
            let mut tail = end;

            // Grow the connected polyline chain forward and backward
            loop {
                let mut extended = false;
                for &(ent, s, e) in &pool {
                    if used.contains(&ent) {
                        continue;
                    }
                    if selected.entities.len() > 1 && !selected.entities.contains(&ent) {
                        continue;
                    }
                    if tail.distance(s) <= 0.08 {
                        chain.push((s, e));
                        tail = e;
                        used.push(ent);
                        extended = true;
                        break;
                    } else if tail.distance(e) <= 0.08 {
                        chain.push((e, s));
                        tail = s;
                        used.push(ent);
                        extended = true;
                        break;
                    } else if head.distance(e) <= 0.08 {
                        chain.insert(0, (s, e));
                        head = s;
                        used.push(ent);
                        extended = true;
                        break;
                    } else if head.distance(s) <= 0.08 {
                        chain.insert(0, (e, s));
                        head = e;
                        used.push(ent);
                        extended = true;
                        break;
                    }
                }
                if !extended {
                    break;
                }
            }

            let mut sum = Vec3::ZERO;
            for &(s, e) in &chain {
                sum += (s + e) * 0.5;
            }
            let avg_center = sum / (chain.len() as f32);

            Some(OffsetContour {
                segments: chain,
                center: avg_center,
                plane_z,
                group_id: seed_obj.group_id,
            })
        }
    }
}

fn evaluate_axis_and_neighbor_snapping<'a>(
    anchor: Vec3,
    raw_pt: Vec3,
    snapped_pt: Vec3,
    excluded: Option<Entity>,
    objects: impl Iterator<Item = (Entity, &'a CadObject)>,
) -> (Vec3, AxisInference) {
    let delta = raw_pt - anchor;
    let dist = delta.length();
    if dist <= 0.15 {
        return (snapped_pt, AxisInference::None);
    }

    let dir = delta / dist;
    let cos_limit = 0.985_f32;

    if dir.x.abs() >= cos_limit {
        let sx = (raw_pt.x / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
        return (Vec3::new(sx, anchor.y, anchor.z), AxisInference::RedX);
    }
    if dir.y.abs() >= cos_limit {
        let sy = (raw_pt.y / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
        return (Vec3::new(anchor.x, sy, anchor.z), AxisInference::GreenY);
    }
    if dir.z.abs() >= cos_limit {
        let sz = (raw_pt.z / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
        return (Vec3::new(anchor.x, anchor.y, sz), AxisInference::BlueZ);
    }

    let mut best_match: Option<(f32, Vec3, Vec3, Vec3)> = None;
    for (ent, obj) in objects {
        if Some(ent) == excluded {
            continue;
        }
        if obj.geometry.centroid().distance(anchor) > NEIGHBOR_SCAN_RADIUS {
            continue;
        }
        for (es, ee) in obj.geometry.edge_segments() {
            let ev = ee - es;
            let el = ev.length();
            if el <= 0.05 {
                continue;
            }
            let ed = ev / el;
            if ed.x.abs() > 0.999 || ed.y.abs() > 0.999 || ed.z.abs() > 0.999 {
                continue;
            }
            let cos_a = dir.dot(ed).abs().clamp(0.0, 1.0);
            let deg = cos_a.acos().to_degrees();
            if deg <= PARALLEL_THRESHOLD_DEG
                && best_match.map(|(b, _, _, _)| deg < b).unwrap_or(true)
            {
                best_match = Some((deg, es, ee, ed));
            }
        }
    }

    if let Some((_, es, ee, ed)) = best_match {
        let raw_proj = delta.dot(ed);
        let snapped_proj = (raw_proj / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
        let final_proj = if snapped_proj.abs() < GRID_SNAP_STEP {
            raw_proj.signum() * GRID_SNAP_STEP
        } else {
            snapped_proj
        };
        return (
            anchor + ed * final_proj,
            AxisInference::ParallelNeighbor {
                edge_start: es,
                edge_end: ee,
                direction: ed,
            },
        );
    }

    (snapped_pt, AxisInference::None)
}

#[allow(clippy::too_many_arguments)]
fn compute_raycast_and_snapping(
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    tool_state: Res<ActiveToolState>,
    move_state: Res<MoveToolState>,
    scale_state: Res<ScaleToolState>,
    selected: Res<SelectedCadObjects>,
    focal_point: Res<FocalPoint>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut cursor_ray: ResMut<CursorRayState>,
    mut draft: ResMut<ToolDraftState>,
) {
    let Ok(window) = window_query.get_single() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Ok((camera, cam_transform)) = camera_query.get_single() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Some(screen_pos) = window.cursor_position() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Some(ray) = camera.viewport_to_world(cam_transform, screen_pos) else {
        cursor_ray.is_valid_hit = false;
        return;
    };

    let shift_held = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mmb_held = mouse_buttons.pressed(MouseButton::Middle);
    let cam_fwd = cam_transform.forward();

    let active_anchor: Option<(Vec3, Option<Entity>)> = match tool_state.current {
        CadTool::LineDraw => draft.active_start_point.map(|p| (p, None)),
        CadTool::Move => move_state
            .active_drag
            .as_ref()
            .map(|d| (d.grab_anchor, Some(d.entity))),
        _ => None,
    };

    if shift_held && !mmb_held {
        if let Some((anchor, excluded)) = active_anchor {
            let eye = cam_transform.translation();
            let horiz_to_eye = Vec3::new(eye.x - anchor.x, eye.y - anchor.y, 0.0);
            let p_norm = Dir3::new(horiz_to_eye).unwrap_or(Dir3::X);

            if let Some(d) = ray.intersect_plane(anchor, InfinitePlane3d::new(p_norm)) {
                let hit = ray.get_point(d);
                let mut sz = (hit.z / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
                let mut inf = AxisInference::BlueZ;

                for (ent, obj) in cad_objects.iter() {
                    if Some(ent) == excluded {
                        continue;
                    }
                    if let CadGeometryKind::Block { center, size } = obj.geometry {
                        let top_z = center.z + size.z * 0.5;
                        if (hit.z - top_z).abs() <= 0.35 {
                            sz = top_z;
                            inf = AxisInference::FlushTopFace {
                                elevation_z: top_z,
                                face_center: Vec3::new(center.x, center.y, top_z),
                            };
                            break;
                        }
                    }
                }

                let vert_pt = Vec3::new(anchor.x, anchor.y, sz);
                cursor_ray.raw_ground_hit = hit;
                cursor_ray.snapped_pos = vert_pt;
                cursor_ray.is_valid_hit = true;
                cursor_ray.vertical_z_locked = true;
                cursor_ray.inference = inf;
                cursor_ray.active_plane = DynamicDrawingPlane::VerticalXZ;
                draft.current_preview_end = vert_pt;
                return;
            }
        }
    }

    cursor_ray.vertical_z_locked = false;

    if tool_state.current == CadTool::Scale {
        if let Some(ref drag) = scale_state.active_drag {
            if let Some(d) =
                ray.intersect_plane(drag.initial_handle_pos, InfinitePlane3d::new(*cam_fwd))
            {
                let hit = ray.get_point(d);
                cursor_ray.raw_ground_hit = hit;
                cursor_ray.snapped_pos = hit;
                cursor_ray.is_valid_hit = true;
                cursor_ray.inference = AxisInference::None;
                cursor_ray.active_plane = DynamicDrawingPlane::CameraFacing;
                draft.current_preview_end = hit;
                return;
            }
        }
    }

    let mut top_face_match: Option<(f32, Vec3, f32, Vec3)> = None;
    if active_anchor.is_none() {
        for (ent, obj) in cad_objects.iter() {
            if tool_state.current == CadTool::Move && selected.entities.contains(&ent) {
                continue;
            }
            if let CadGeometryKind::Block { center, size } = obj.geometry {
                let top_z = center.z + size.z * 0.5;
                let hx = size.x * 0.5;
                let hy = size.y * 0.5;
                let origin = Vec3::new(center.x, center.y, top_z);
                if let Some(d) = ray.intersect_plane(origin, InfinitePlane3d::new(Vec3::Z)) {
                    if d <= MAX_PLANE_RAY_DIST {
                        let p = ray.get_point(d);
                        if (p.x - center.x).abs() <= hx + 0.15 && (p.y - center.y).abs() <= hy + 0.15
                        {
                            if top_face_match.map(|(b, _, _, _)| d < b).unwrap_or(true) {
                                top_face_match =
                                    Some((d, p, top_z, Vec3::new(center.x, center.y, top_z)));
                            }
                        }
                    }
                }
            }
        }
    }

    let plane_origin = match active_anchor {
        Some((anchor, _)) => anchor,
        None => draft
            .active_start_point
            .or_else(|| top_face_match.map(|(_, _, tz, _)| Vec3::new(focal_point.0.x, focal_point.0.y, tz)))
            .unwrap_or(Vec3::new(focal_point.0.x, focal_point.0.y, 0.0)),
    };

    let mut plane_mode =
        DynamicDrawingPlane::choose_for_view(Vec3::from(cam_fwd), active_anchor.is_some());
    let norm = plane_mode.normal_dir(cam_fwd);

    let raw_hit = if let Some(d) = ray.intersect_plane(plane_origin, InfinitePlane3d::new(norm)) {
        if d > 0.05 && d <= MAX_PLANE_RAY_DIST {
            ray.get_point(d)
        } else {
            plane_mode = DynamicDrawingPlane::CameraFacing;
            let fb_origin = active_anchor
                .map(|(a, _)| a)
                .or(draft.active_start_point)
                .unwrap_or(focal_point.0);
            let Some(fb_d) = ray.intersect_plane(fb_origin, InfinitePlane3d::new(-cam_fwd)) else {
                cursor_ray.is_valid_hit = false;
                return;
            };
            let mut pt = ray.get_point(fb_d);
            if active_anchor.is_none() && draft.active_start_point.is_none() {
                pt.z = pt.z.max(0.0);
            }
            pt
        }
    } else {
        plane_mode = DynamicDrawingPlane::CameraFacing;
        let fb_origin = active_anchor
            .map(|(a, _)| a)
            .or(draft.active_start_point)
            .unwrap_or(focal_point.0);
        let Some(fb_d) = ray.intersect_plane(fb_origin, InfinitePlane3d::new(-cam_fwd)) else {
            cursor_ray.is_valid_hit = false;
            return;
        };
        let mut pt = ray.get_point(fb_d);
        if active_anchor.is_none() && draft.active_start_point.is_none() {
            pt.z = pt.z.max(0.0);
        }
        pt
    };

    let mut candidate = snap_to_integer_grid(raw_hit);
    if plane_mode == DynamicDrawingPlane::HorizontalXY {
        candidate.z = plane_origin.z;
    }

    let mut inference = if let Some((_, _, tz, fc)) = top_face_match {
        candidate.z = tz;
        AxisInference::FlushTopFace {
            elevation_z: tz,
            face_center: fc,
        }
    } else {
        AxisInference::None
    };

    if let Some((anchor, excluded)) = active_anchor {
        let (snapped_pt, det_inf) = evaluate_axis_and_neighbor_snapping(
            anchor,
            raw_hit,
            candidate,
            excluded,
            cad_objects.iter(),
        );
        candidate = snapped_pt;
        if det_inf != AxisInference::None {
            inference = det_inf;
        }
    }

    cursor_ray.raw_ground_hit = raw_hit;
    cursor_ray.snapped_pos = candidate;
    cursor_ray.is_valid_hit = true;
    cursor_ray.inference = inference;
    cursor_ray.active_plane = plane_mode;
    draft.current_preview_end = candidate;
}

#[allow(clippy::too_many_arguments)]
fn process_keyboard_and_vcb_events(
    mut keyboard_events: EventReader<KeyboardInput>,
    keyboard_state: Res<ButtonInput<KeyCode>>,
    ui_capture: Res<UiInputCapture>,
    cursor_ray: Res<CursorRayState>,
    mut tool_state: ResMut<ActiveToolState>,
    mut vcb: ResMut<VcbInput>,
    mut draft: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut offset_state: ResMut<OffsetToolState>,
    mut selected: ResMut<SelectedCadObjects>,
    mut pending_actions: ResMut<PendingHistoryActions>,
    mut cad_query: Query<(Entity, &CadObject, &mut Transform)>,
    mut app_exit_events: EventWriter<AppExit>,
) {
    if ui_capture.egui_wants_keyboard {
        return;
    }

    let ctrl_held = keyboard_state.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift_held = keyboard_state.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    for event in keyboard_events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }

        if ctrl_held && event.key_code == KeyCode::KeyQ {
            app_exit_events.send(AppExit::Success);
            continue;
        }

        if ctrl_held {
            if event.key_code == KeyCode::KeyZ {
                abort_active_drags(&mut move_state, &mut scale_state, &mut offset_state, &mut cad_query);
                draft.active_start_point = None;
                if shift_held {
                    pending_actions.queue.push(HistoryAction::Redo);
                } else {
                    pending_actions.queue.push(HistoryAction::Undo);
                }
                continue;
            } else if event.key_code == KeyCode::KeyY {
                abort_active_drags(&mut move_state, &mut scale_state, &mut offset_state, &mut cad_query);
                draft.active_start_point = None;
                pending_actions.queue.push(HistoryAction::Redo);
                continue;
            }
        }

        // Tool Hotkeys: [Space], [L], [B], [M], [S], [F] (Offset Tool)
        if !ctrl_held {
            let next_tool = match event.key_code {
                KeyCode::Space => Some(CadTool::Select),
                KeyCode::KeyL => Some(CadTool::LineDraw),
                KeyCode::KeyB => Some(CadTool::Block),
                KeyCode::KeyM => Some(CadTool::Move),
                KeyCode::KeyS => Some(CadTool::Scale),
                KeyCode::KeyF => Some(CadTool::Offset),
                _ => None,
            };

            if let Some(tool) = next_tool {
                abort_active_drags(&mut move_state, &mut scale_state, &mut offset_state, &mut cad_query);
                tool_state.current = tool;
                draft.active_start_point = None;
                vcb.buffer.clear();
                vcb.last_status_message = Some(format!("Activated {}", tool.label()));
                continue;
            }
        }

        if event.key_code == KeyCode::Escape || matches!(event.logical_key, Key::Escape) {
            abort_active_drags(&mut move_state, &mut scale_state, &mut offset_state, &mut cad_query);
            draft.active_start_point = None;
            vcb.buffer.clear();
            vcb.last_status_message = Some(String::from("Operation canceled"));
            continue;
        }

        if event.key_code == KeyCode::Backspace || matches!(event.logical_key, Key::Backspace) {
            if !vcb.buffer.is_empty() {
                vcb.buffer.pop();
                continue;
            }
        }

        if event.key_code == KeyCode::Delete
            || matches!(event.logical_key, Key::Delete)
            || (event.key_code == KeyCode::Backspace && vcb.buffer.is_empty())
        {
            if !selected.entities.is_empty() {
                abort_active_drags(&mut move_state, &mut scale_state, &mut offset_state, &mut cad_query);
                let mut snapshots = Vec::new();
                for &ent in &selected.entities {
                    if let Ok((e, obj, _)) = cad_query.get(ent) {
                        snapshots.push((e, obj.clone()));
                    }
                }
                selected.entities.clear();
                if !snapshots.is_empty() {
                    let count = snapshots.len();
                    pending_actions.queue.push(HistoryAction::Execute(Box::new(
                        DeleteCommand::new(snapshots),
                    )));
                    vcb.last_status_message =
                        Some(format!("Deleted {count} selected CAD object(s)"));
                }
            }
            continue;
        }

        if event.key_code == KeyCode::Enter
            || event.key_code == KeyCode::NumpadEnter
            || matches!(event.logical_key, Key::Enter)
        {
            let text = vcb.buffer.trim().to_string();
            vcb.buffer.clear();
            if !text.is_empty() {
                apply_vcb_command(
                    &text,
                    tool_state.current,
                    &cursor_ray,
                    &selected,
                    &mut draft,
                    &mut move_state,
                    &mut scale_state,
                    &mut offset_state,
                    &mut cad_query,
                    &mut vcb,
                    &mut pending_actions,
                );
            }
            continue;
        }

        if !ctrl_held {
            if let Key::Character(ref s) = event.logical_key {
                for ch in s.chars() {
                    if ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == '-' {
                        vcb.buffer.push(ch);
                    }
                }
            }
        }
    }
}

fn abort_active_drags(
    move_state: &mut MoveToolState,
    scale_state: &mut ScaleToolState,
    offset_state: &mut OffsetToolState,
    cad_query: &mut Query<(Entity, &CadObject, &mut Transform)>,
) {
    if let Some(drag) = move_state.active_drag.take() {
        if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
            *tr = drag.initial_transform;
        }
    }
    if let Some(drag) = scale_state.active_drag.take() {
        if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
            *tr = drag.initial_transform;
        }
    }
    offset_state.is_dragging = false;
    offset_state.active_contour = None;
    offset_state.current_offset_distance = 0.0;
}

#[allow(clippy::too_many_arguments)]
fn apply_vcb_command(
    input_text: &str,
    active_tool: CadTool,
    cursor_ray: &CursorRayState,
    selected: &SelectedCadObjects,
    draft: &mut ToolDraftState,
    move_state: &mut MoveToolState,
    scale_state: &mut ScaleToolState,
    offset_state: &mut OffsetToolState,
    cad_query: &mut Query<(Entity, &CadObject, &mut Transform)>,
    vcb: &mut VcbInput,
    pending_actions: &mut PendingHistoryActions,
) {
    let tokens: Vec<&str> = input_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    match active_tool {
        CadTool::LineDraw => {
            let Some(start) = draft.active_start_point else {
                vcb.last_status_message =
                    Some(String::from("Click a start point before entering line length"));
                return;
            };
            if tokens.len() == 1 {
                if let Ok(len) = tokens[0].parse::<f32>() {
                    if len.abs() > 0.001 {
                        let raw_vec = draft.current_preview_end - start;
                        let dir = if cursor_ray.vertical_z_locked {
                            if raw_vec.z < 0.0 { -Vec3::Z } else { Vec3::Z }
                        } else {
                            raw_vec.try_normalize().unwrap_or(Vec3::X)
                        };
                        let end_pt = start + dir * len;
                        let col = cursor_ray.inference.preview_color();
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(
                            DrawLineCommand::new(start, end_pt, col, None),
                        )));
                        draft.active_start_point = Some(end_pt);
                        draft.current_preview_end = end_pt;
                        vcb.last_status_message = Some(format!("Committed {len:.2}m line"));
                    }
                }
            }
        }
        CadTool::Block => {
            let origin = draft.active_start_point.unwrap_or(cursor_ray.snapped_pos);
            if tokens.len() == 3 {
                if let (Ok(w), Ok(d), Ok(h)) = (
                    tokens[0].parse::<f32>(),
                    tokens[1].parse::<f32>(),
                    tokens[2].parse::<f32>(),
                ) {
                    if w.abs() > 0.001 && d.abs() > 0.001 && h.abs() > 0.001 {
                        let cmd = CreateBlockCommand::from_corner_and_dimensions(
                            origin,
                            Vec3::new(w, d, h),
                            Color::srgb(0.78, 0.82, 0.88),
                            None,
                        );
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));
                        draft.active_start_point = None;
                        vcb.last_status_message =
                            Some(format!("Spawned Block ({w:.2}m x {d:.2}m x {h:.2}m)"));
                    }
                }
            }
        }
        CadTool::Move => {
            let ent_opt = move_state
                .active_drag
                .as_ref()
                .map(|d| d.entity)
                .or_else(|| selected.entities.first().copied());
            let Some(entity) = ent_opt else {
                return;
            };
            if tokens.len() == 1 {
                if let Ok(dist) = tokens[0].parse::<f32>() {
                    let dir = move_state
                        .active_drag
                        .as_ref()
                        .and_then(|d| d.current_offset.try_normalize())
                        .unwrap_or(Vec3::X);
                    let delta = dir * dist;
                    let before_geom = if let Some(drag) = move_state.active_drag.take() {
                        if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
                            *tr = drag.initial_transform;
                        }
                        Some(drag.initial_geometry)
                    } else {
                        cad_query.get(entity).ok().map(|(_, o, _)| o.geometry.clone())
                    };
                    if let Some(before) = before_geom {
                        let after = before.translated(delta);
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(
                            MoveObjectCommand::new(entity, before, after),
                        )));
                        vcb.last_status_message = Some(format!("Moved object {dist:.2}m"));
                    }
                }
            }
        }
        CadTool::Scale => {
            if tokens.len() == 1 {
                if let Ok(s) = tokens[0].parse::<f32>() {
                    if s.abs() > 0.01 {
                        let factors = Vec3::splat(s.abs());
                        if let Some(drag) = scale_state.active_drag.take() {
                            if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
                                *tr = drag.initial_transform;
                            }
                            let after = drag
                                .initial_geometry
                                .scaled_from_anchor(drag.opposite_anchor, factors);
                            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                ScaleObjectCommand::new(drag.entity, drag.initial_geometry, after),
                            )));
                            vcb.last_status_message = Some(format!("Scaled object by {s:.2}x"));
                        } else if let Some(&entity) = selected.entities.first() {
                            if let Ok((_, obj, _)) = cad_query.get(entity) {
                                let before = obj.geometry.clone();
                                let (anchor, _) = before.bounds_min_max();
                                let after = before.scaled_from_anchor(anchor, factors);
                                pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                    ScaleObjectCommand::new(entity, before, after),
                                )));
                                vcb.last_status_message = Some(format!("Scaled object by {s:.2}x"));
                            }
                        }
                    }
                }
            }
        }
        CadTool::Offset => {
            // VCB decimal offset distance (e.g. "0.3" or "-0.3")
            if tokens.len() == 1 {
                if let Ok(typed_dist) = tokens[0].parse::<f32>() {
                    if typed_dist.abs() > 0.001 {
                        let target_contour = offset_state
                            .active_contour
                            .take()
                            .or_else(|| offset_state.hovered_contour.clone())
                            .or_else(|| {
                                selected.entities.first().and_then(|&ent| {
                                    detect_offset_contour_for_entity(ent, cad_query, selected)
                                })
                            });

                        offset_state.is_dragging = false;

                        if let Some(contour) = target_contour {
                            // If dragging inward when a positive number is typed, preserve drag direction sign
                            let direction_sign = if typed_dist < 0.0 {
                                1.0
                            } else if offset_state.current_offset_distance < -0.001 {
                                -1.0
                            } else {
                                1.0
                            };
                            let final_dist = typed_dist * direction_sign;
                            let segs = contour.compute_offset_segments(final_dist);
                            if !segs.is_empty() {
                                pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                    CreateOffsetCommand::new(
                                        segs,
                                        OFFSET_PREVIEW_COLOR,
                                        contour.group_id,
                                    ),
                                )));
                                vcb.last_status_message = Some(format!(
                                    "Committed concentric offset at {final_dist:+.2}m"
                                ));
                            }
                        } else {
                            vcb.last_status_message = Some(String::from(
                                "Hover or select a face/line chain before entering offset distance",
                            ));
                        }
                    }
                }
            }
        }
        CadTool::PushPullExtrude => {
            if tokens.len() == 1 {
                if let Ok(h) = tokens[0].parse::<f32>() {
                    if h.abs() > 0.01 {
                        draft.default_block_height = h;
                    }
                }
            }
        }
        CadTool::Select => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn process_viewport_pointer_actions(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    ui_capture: Res<UiInputCapture>,
    tool_state: Res<ActiveToolState>,
    cursor_ray: Res<CursorRayState>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    mut cad_query: Query<(Entity, &CadObject, &mut Transform)>,
    mut draft: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut offset_state: ResMut<OffsetToolState>,
    mut selected: ResMut<SelectedCadObjects>,
    mut vcb: ResMut<VcbInput>,
    mut pending_actions: ResMut<PendingHistoryActions>,
) {
    let Ok(window) = window_query.get_single() else {
        return;
    };
    let Ok((camera, cam_transform)) = camera_query.get_single() else {
        return;
    };
    let current_ray = window
        .cursor_position()
        .and_then(|pos| camera.viewport_to_world(cam_transform, pos));

    match tool_state.current {
        // --------------------------------------------------------------------
        // OFFSET TOOL (`[F]`): Hover highlights perimeter -> LMB Drag offsets inward/outward -> Release commits `CreateOffsetCommand`
        // --------------------------------------------------------------------
        CadTool::Offset => {
            if offset_state.is_dragging {
                if let Some(ref contour) = offset_state.active_contour {
                    if let Some(ref ray) = current_ray {
                        let plane_pt = Vec3::new(0.0, 0.0, contour.plane_z);
                        if let Some(d) =
                            ray.intersect_plane(plane_pt, InfinitePlane3d::new(Vec3::Z))
                        {
                            let hit = ray.get_point(d);
                            let curr_r = Vec2::new(
                                hit.x - contour.center.x,
                                hit.y - contour.center.y,
                            )
                            .length();
                            let raw_offset = curr_r - offset_state.grab_radius;
                            // Snap to 0.05m increments during interactive drag
                            offset_state.current_offset_distance =
                                (raw_offset / 0.05_f32).round() * 0.05_f32;
                        }
                    }
                }

                if mouse_buttons.just_released(MouseButton::Left) {
                    offset_state.is_dragging = false;
                    let dist = offset_state.current_offset_distance;
                    if let Some(contour) = offset_state.active_contour.take() {
                        if dist.abs() > 0.02 {
                            let segs = contour.compute_offset_segments(dist);
                            if !segs.is_empty() {
                                pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                    CreateOffsetCommand::new(
                                        segs,
                                        OFFSET_PREVIEW_COLOR,
                                        contour.group_id,
                                    ),
                                )));
                                vcb.last_status_message =
                                    Some(format!("Created offset contour ({dist:+.2}m)"));
                            }
                        }
                    }
                }
                return;
            }

            // Update hovered perimeter contour when not actively dragging
            offset_state.hovered_contour = None;
            if let Some(ref ray) = current_ray {
                if let Some((hit_ent, _)) = find_closest_cad_object(ray, &cad_query) {
                    offset_state.hovered_contour =
                        detect_offset_contour_for_entity(hit_ent, &cad_query, &selected);
                } else if let Some(&sel_ent) = selected.entities.first() {
                    offset_state.hovered_contour =
                        detect_offset_contour_for_entity(sel_ent, &cad_query, &selected);
                }

                if !ui_capture.egui_wants_pointer && mouse_buttons.just_pressed(MouseButton::Left) {
                    if let Some(contour) = offset_state.hovered_contour.clone() {
                        let plane_pt = Vec3::new(0.0, 0.0, contour.plane_z);
                        let grab_r = ray
                            .intersect_plane(plane_pt, InfinitePlane3d::new(Vec3::Z))
                            .map(|d| {
                                let p = ray.get_point(d);
                                Vec2::new(p.x - contour.center.x, p.y - contour.center.y).length()
                            })
                            .unwrap_or(1.0);

                        offset_state.grab_radius = grab_r;
                        offset_state.current_offset_distance = 0.0;
                        offset_state.active_contour = Some(contour);
                        offset_state.is_dragging = true;
                    }
                }
            }
        }

        CadTool::Move => {
            if let Some(ref mut drag) = move_state.active_drag {
                if cursor_ray.is_valid_hit {
                    let offset = draft.current_preview_end - drag.grab_anchor;
                    drag.current_offset = offset;
                    if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
                        tr.translation = drag.initial_transform.translation + offset;
                    }
                }

                if mouse_buttons.just_released(MouseButton::Left) {
                    if let Some(done) = move_state.active_drag.take() {
                        if let Ok((_, _, mut tr)) = cad_query.get_mut(done.entity) {
                            *tr = done.initial_transform;
                        }
                        if done.current_offset.length() > 0.01 {
                            let after = done.initial_geometry.translated(done.current_offset);
                            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                MoveObjectCommand::new(done.entity, done.initial_geometry, after),
                            )));
                            vcb.last_status_message =
                                Some(format!("Moved object {:.2}m", done.current_offset.length()));
                        }
                    }
                }
                return;
            }

            if !ui_capture.egui_wants_pointer && mouse_buttons.just_pressed(MouseButton::Left) {
                let Some(ray) = current_ray else {
                    return;
                };
                if let Some((hit_ent, _)) = find_closest_cad_object(&ray, &cad_query) {
                    selected.entities.clear();
                    selected.entities.push(hit_ent);
                    if let Ok((_, obj, tr)) = cad_query.get(hit_ent) {
                        let anchor = snap_to_integer_grid(obj.geometry.centroid());
                        move_state.active_drag = Some(ActiveMoveDrag {
                            entity: hit_ent,
                            grab_anchor: anchor,
                            current_offset: Vec3::ZERO,
                            initial_geometry: obj.geometry.clone(),
                            initial_transform: *tr,
                        });
                    }
                }
            }
        }

        CadTool::Scale => {
            scale_state.hovered_handle = None;

            if let Some(ref mut drag) = scale_state.active_drag {
                if cursor_ray.is_valid_hit {
                    let init_v = drag.initial_handle_pos - drag.opposite_anchor;
                    let curr_v = draft.current_preview_end - drag.opposite_anchor;
                    let axis_ratio = |i_c: f32, c_c: f32| -> f32 {
                        if i_c.abs() > 0.08 {
                            let snapped = (c_c / GRID_SNAP_STEP).round() * GRID_SNAP_STEP;
                            let num = if snapped.abs() >= 0.5 { snapped } else { c_c };
                            (num / i_c).abs().clamp(0.10, 20.0)
                        } else {
                            1.0
                        }
                    };
                    let factors = Vec3::new(
                        axis_ratio(init_v.x, curr_v.x),
                        axis_ratio(init_v.y, curr_v.y),
                        axis_ratio(init_v.z, curr_v.z),
                    );
                    drag.current_scale_factors = factors;

                    let preview = drag
                        .initial_geometry
                        .scaled_from_anchor(drag.opposite_anchor, factors);
                    if let Ok((_, _, mut tr)) = cad_query.get_mut(drag.entity) {
                        tr.translation = preview.centroid();
                        tr.scale = factors;
                    }
                }

                if mouse_buttons.just_released(MouseButton::Left) {
                    if let Some(done) = scale_state.active_drag.take() {
                        if let Ok((_, _, mut tr)) = cad_query.get_mut(done.entity) {
                            *tr = done.initial_transform;
                        }
                        if (done.current_scale_factors - Vec3::ONE).length() > 0.01 {
                            let after = done.initial_geometry.scaled_from_anchor(
                                done.opposite_anchor,
                                done.current_scale_factors,
                            );
                            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                ScaleObjectCommand::new(done.entity, done.initial_geometry, after),
                            )));
                        }
                    }
                }
                return;
            }

            let Some(ray) = current_ray else {
                return;
            };

            if let Some(&sel_ent) = selected.entities.first() {
                if let Ok((_, obj, tr)) = cad_query.get(sel_ent) {
                    let corners = obj.geometry.bbox_vertices();
                    let hit_h = pick_bbox_handle(&ray, &corners);
                    scale_state.hovered_handle = hit_h;

                    if !ui_capture.egui_wants_pointer
                        && mouse_buttons.just_pressed(MouseButton::Left)
                    {
                        if let Some(h_idx) = hit_h {
                            scale_state.active_drag = Some(ActiveScaleDrag {
                                entity: sel_ent,
                                handle_index: h_idx,
                                opposite_anchor: corners[7 - h_idx],
                                initial_handle_pos: corners[h_idx],
                                current_scale_factors: Vec3::ONE,
                                initial_geometry: obj.geometry.clone(),
                                initial_transform: *tr,
                            });
                            return;
                        }
                    }
                }
            }

            if !ui_capture.egui_wants_pointer && mouse_buttons.just_pressed(MouseButton::Left) {
                if let Some((hit_ent, _)) = find_closest_cad_object(&ray, &cad_query) {
                    selected.entities.clear();
                    selected.entities.push(hit_ent);
                }
            }
        }

        CadTool::Select => {
            if ui_capture.egui_wants_pointer || !mouse_buttons.just_pressed(MouseButton::Left) {
                return;
            }
            draft.active_start_point = None;
            let Some(ray) = current_ray else {
                return;
            };
            let hit = find_closest_cad_object(&ray, &cad_query);
            if !keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
                selected.entities.clear();
            }
            if let Some((ent, _)) = hit {
                if !selected.entities.contains(&ent) {
                    selected.entities.push(ent);
                }
            }
        }

        CadTool::LineDraw => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target = draft.current_preview_end;
            if let Some(start) = draft.active_start_point {
                if start.distance(target) > 0.05 {
                    let col = cursor_ray.inference.preview_color();
                    pending_actions.queue.push(HistoryAction::Execute(Box::new(
                        DrawLineCommand::new(start, target, col, None),
                    )));
                    draft.active_start_point = Some(target);
                }
            } else {
                draft.active_start_point = Some(target);
            }
        }

        CadTool::Block => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target = draft.current_preview_end;
            if let Some(c0) = draft.active_start_point.take() {
                let dx = target.x - c0.x;
                let dy = target.y - c0.y;
                let ex = if dx.abs() < GRID_SNAP_STEP {
                    c0.x + dx.signum().max(1.0) * GRID_SNAP_STEP
                } else {
                    target.x
                };
                let ey = if dy.abs() < GRID_SNAP_STEP {
                    c0.y + dy.signum().max(1.0) * GRID_SNAP_STEP
                } else {
                    target.y
                };
                let c1 = Vec3::new(ex, ey, c0.z + draft.default_block_height);
                pending_actions.queue.push(HistoryAction::Execute(Box::new(
                    CreateBlockCommand::from_corners(
                        c0,
                        c1,
                        Color::srgb(0.78, 0.82, 0.88),
                        None,
                    ),
                )));
            } else {
                draft.active_start_point = Some(target);
            }
        }

        CadTool::PushPullExtrude => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target = draft.current_preview_end;
            draft.active_start_point = None;
            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                CreateBlockCommand::from_corner_and_dimensions(
                    target,
                    Vec3::new(1.0, 1.0, draft.default_block_height),
                    Color::srgb(0.82, 0.85, 0.90),
                    None,
                ),
            )));
        }
    }
}

fn find_closest_cad_object(
    ray: &Ray3d,
    cad_query: &Query<(Entity, &CadObject, &mut Transform)>,
) -> Option<(Entity, f32)> {
    let mut best: Option<(Entity, f32)> = None;
    for (ent, obj, _) in cad_query.iter() {
        if let Some(d) = ray_intersects_cad_object(ray, obj) {
            if best.map(|(_, b)| d < b).unwrap_or(true) {
                best = Some((ent, d));
            }
        }
    }
    best
}

fn pick_bbox_handle(ray: &Ray3d, corners: &[Vec3; 8]) -> Option<usize> {
    let origin = ray.origin;
    let dir = Vec3::from(ray.direction);
    let mut best: Option<(usize, f32)> = None;

    for (idx, &pt) in corners.iter().enumerate() {
        let t = (pt - origin).dot(dir).max(0.0);
        let closest = origin + dir * t;
        if closest.distance(pt) <= 0.24 && best.map(|(_, bt)| t < bt).unwrap_or(true) {
            best = Some((idx, t));
        }
    }
    best.map(|(i, _)| i)
}

fn ray_intersects_cad_object(ray: &Ray3d, obj: &CadObject) -> Option<f32> {
    let ro = ray.origin;
    let rd = Vec3::from(ray.direction);

    match obj.geometry {
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5;
            let b_min = center - half;
            let b_max = center + half;
            let mut t_near = f32::NEG_INFINITY;
            let mut t_far = f32::INFINITY;

            for axis in 0..3 {
                let o = ro[axis];
                let d = rd[axis];
                if d.abs() < 1e-6 {
                    if o < b_min[axis] || o > b_max[axis] {
                        return None;
                    }
                } else {
                    let inv = 1.0 / d;
                    let mut t1 = (b_min[axis] - o) * inv;
                    let mut t2 = (b_max[axis] - o) * inv;
                    if t1 > t2 {
                        std::mem::swap(&mut t1, &mut t2);
                    }
                    t_near = t_near.max(t1);
                    t_far = t_far.min(t2);
                    if t_near > t_far {
                        return None;
                    }
                }
            }
            if t_far >= 0.0 {
                Some(t_near.max(0.0))
            } else {
                None
            }
        }
        CadGeometryKind::Line { start, end, radius } => {
            let u = end - start;
            let w = start - ro;
            let a = u.dot(u);
            let b = u.dot(rd);
            let c = rd.dot(rd);
            let d = u.dot(w);
            let e = rd.dot(w);
            let denom = a * c - b * b;

            let (s, t) = if denom.abs() < 1e-6 {
                (0.0_f32, (e / c).max(0.0))
            } else {
                (
                    ((b * e - c * d) / denom).clamp(0.0, 1.0),
                    ((a * e - b * d) / denom).max(0.0),
                )
            };

            let seg_pt = start + u * s;
            let ray_pt = ro + rd * t;
            if seg_pt.distance(ray_pt) <= (radius * 4.5).max(0.14) {
                Some(t)
            } else {
                None
            }
        }
    }
}

fn refresh_selection_materials(
    selected: Res<SelectedCadObjects>,
    cad_objects: Query<(Entity, &CadObject, &Handle<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (ent, obj, mat_h) in cad_objects.iter() {
        let Some(mat) = materials.get_mut(mat_h) else {
            continue;
        };
        if selected.entities.contains(&ent) {
            mat.base_color = NEON_BLUE_SELECTION_COLOR;
            mat.emissive = LinearRgba::rgb(0.02, 0.45, 0.65);
        } else {
            mat.base_color = obj.material_color.0;
            mat.emissive = LinearRgba::BLACK;
        }
    }
}

fn sync_block_preview_mesh(
    tool_state: Res<ActiveToolState>,
    draft: Res<ToolDraftState>,
    cursor_ray: Res<CursorRayState>,
    mut preview_q: Query<(&mut Transform, &mut Visibility), With<BlockPreviewVisual>>,
) {
    let Ok((mut tr, mut vis)) = preview_q.get_single_mut() else {
        return;
    };

    if tool_state.current == CadTool::Block && cursor_ray.is_valid_hit {
        if let Some(c0) = draft.active_start_point {
            let dx = draft.current_preview_end.x - c0.x;
            let dy = draft.current_preview_end.y - c0.y;
            let ex = if dx.abs() < GRID_SNAP_STEP {
                c0.x + dx.signum().max(1.0) * GRID_SNAP_STEP
            } else {
                draft.current_preview_end.x
            };
            let ey = if dy.abs() < GRID_SNAP_STEP {
                c0.y + dy.signum().max(1.0) * GRID_SNAP_STEP
            } else {
                draft.current_preview_end.y
            };
            let c1 = Vec3::new(ex, ey, c0.z + draft.default_block_height);
            let lo = c0.min(c1);
            let hi = c0.max(c1);
            tr.translation = (lo + hi) * 0.5;
            tr.scale = (hi - lo).max(Vec3::new(GRID_SNAP_STEP, GRID_SNAP_STEP, 0.10));
            *vis = Visibility::Visible;
            return;
        }
    }
    *vis = Visibility::Hidden;
}

fn render_dashed_segment(
    gizmos: &mut Gizmos,
    a: Vec3,
    b: Vec3,
    dash: f32,
    gap: f32,
    col: Color,
) {
    let vec = b - a;
    let total = vec.length();
    if total <= 0.001 {
        return;
    }
    let dir = vec / total;
    let step = (dash + gap).max(0.05);
    let mut pos = 0.0_f32;
    while pos < total {
        gizmos.line(a + dir * pos, a + dir * (pos + dash).min(total), col);
        pos += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_viewport_gizmos(
    mut gizmos: Gizmos,
    focal_point: Res<FocalPoint>,
    tool_state: Res<ActiveToolState>,
    cursor_ray: Res<CursorRayState>,
    draft: Res<ToolDraftState>,
    move_state: Res<MoveToolState>,
    scale_state: Res<ScaleToolState>,
    offset_state: Res<OffsetToolState>,
    selected: Res<SelectedCadObjects>,
    cad_objects: Query<&CadObject>,
) {
    let step = 5.0_f32;
    let half_span = 25_i32;
    let cx = (focal_point.0.x / step).round() * step;
    let cy = (focal_point.0.y / step).round() * step;
    let max_d = half_span as f32 * step;
    let g_col = Color::srgba(0.38, 0.42, 0.48, 0.26);

    for i in -half_span..=half_span {
        let off = i as f32 * step;
        let x = cx + off;
        let y = cy + off;
        if y.abs() > 0.01 {
            gizmos.line(Vec3::new(cx - max_d, y, -0.002), Vec3::new(cx + max_d, y, -0.002), g_col);
        }
        if x.abs() > 0.01 {
            gizmos.line(Vec3::new(x, cy - max_d, -0.002), Vec3::new(x, cy + max_d, -0.002), g_col);
        }
    }

    gizmos.sphere(
        focal_point.0,
        Quat::IDENTITY,
        0.09,
        Color::srgba(0.98, 0.78, 0.22, 0.75),
    );

    if cursor_ray.is_valid_hit {
        gizmos.sphere(
            draft.current_preview_end,
            Quat::IDENTITY,
            0.075,
            cursor_ray.inference.preview_color(),
        );

        match cursor_ray.inference {
            AxisInference::ParallelNeighbor {
                edge_start,
                edge_end,
                direction,
            } => {
                gizmos.line(edge_start, edge_end, INFERENCE_MAGENTA_COLOR);
                let span = 45.0_f32;
                let mid = (edge_start + edge_end) * 0.5;
                render_dashed_segment(
                    &mut gizmos,
                    mid - direction * span,
                    mid + direction * span,
                    0.35,
                    0.20,
                    INFERENCE_MAGENTA_COLOR,
                );
                render_dashed_segment(
                    &mut gizmos,
                    draft.current_preview_end - direction * span,
                    draft.current_preview_end + direction * span,
                    0.35,
                    0.20,
                    INFERENCE_MAGENTA_COLOR,
                );
            }
            AxisInference::FlushTopFace {
                elevation_z,
                face_center,
            } => {
                render_dashed_segment(
                    &mut gizmos,
                    face_center,
                    Vec3::new(
                        draft.current_preview_end.x,
                        draft.current_preview_end.y,
                        elevation_z,
                    ),
                    0.25,
                    0.15,
                    INFERENCE_MAGENTA_COLOR,
                );
            }
            _ => {}
        }
    }

    if tool_state.current == CadTool::LineDraw {
        if let Some(start) = draft.active_start_point {
            gizmos.sphere(start, Quat::IDENTITY, 0.08, Color::srgb(1.0, 0.92, 0.30));
            if cursor_ray.is_valid_hit {
                gizmos.line(
                    start,
                    draft.current_preview_end,
                    cursor_ray.inference.preview_color(),
                );
            }
        }
    }

    if tool_state.current == CadTool::Move {
        if let Some(ref drag) = move_state.active_drag {
            let dest = drag.grab_anchor + drag.current_offset;
            let col = cursor_ray.inference.preview_color();
            gizmos.sphere(drag.grab_anchor, Quat::IDENTITY, 0.08, col);
            render_dashed_segment(&mut gizmos, drag.grab_anchor, dest, 0.25, 0.15, col);
        }
    }

    if tool_state.current == CadTool::Scale {
        if let Some(&sel_ent) = selected.entities.first() {
            if let Ok(obj) = cad_objects.get(sel_ent) {
                let geom = if let Some(ref drag) = scale_state.active_drag {
                    drag.initial_geometry
                        .scaled_from_anchor(drag.opposite_anchor, drag.current_scale_factors)
                } else {
                    obj.geometry.clone()
                };
                let (lo, hi) = geom.bounds_min_max();
                gizmos.cuboid(
                    Transform {
                        translation: (lo + hi) * 0.5,
                        rotation: Quat::IDENTITY,
                        scale: (hi - lo).max(Vec3::splat(0.10)),
                    },
                    SCALE_BBOX_COLOR,
                );
                for (idx, &pt) in geom.bbox_vertices().iter().enumerate() {
                    let active = scale_state
                        .active_drag
                        .as_ref()
                        .map(|d| d.handle_index == idx)
                        .unwrap_or(false)
                        || scale_state.hovered_handle == Some(idx);
                    gizmos.cuboid(
                        Transform {
                            translation: pt,
                            rotation: Quat::IDENTITY,
                            scale: Vec3::splat(if active { 0.18 } else { 0.13 }),
                        },
                        if active {
                            SCALE_ACTIVE_HANDLE_COLOR
                        } else {
                            SCALE_HANDLE_COLOR
                        },
                    );
                }
            }
        }
    }

    // Offset Tool (`[F]`): Highlight source perimeter & live concentric offset preview
    if tool_state.current == CadTool::Offset {
        let contour_ref = offset_state
            .active_contour
            .as_ref()
            .or(offset_state.hovered_contour.as_ref());

        if let Some(contour) = contour_ref {
            for &(a, b) in &contour.segments {
                gizmos.line(a, b, OFFSET_HIGHLIGHT_COLOR);
                gizmos.sphere(a, Quat::IDENTITY, 0.055, OFFSET_HIGHLIGHT_COLOR);
            }
            gizmos.sphere(contour.center, Quat::IDENTITY, 0.07, OFFSET_HIGHLIGHT_COLOR);

            if offset_state.is_dragging && offset_state.current_offset_distance.abs() > 0.01 {
                let preview_segs =
                    contour.compute_offset_segments(offset_state.current_offset_distance);
                for (pa, pb) in preview_segs {
                    gizmos.line(pa, pb, OFFSET_PREVIEW_COLOR);
                    gizmos.sphere(pa, Quat::IDENTITY, 0.05, OFFSET_PREVIEW_COLOR);
                }
            }
        }
    }
}
