use std::f32::consts::{FRAC_PI_2, TAU};

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
        CadGeometryKind, CadObject, Command, CommandHistory, CreateBlockCommand, DeleteCommand,
        DrawLineCommand, GroupId, HistoryAction, MoveObjectCommand, PendingHistoryActions,
        ScaleObjectCommand,
    },
    ui::UiInputCapture,
};

// ============================================================================
// SKETCHUP & INFERENCE COLOR CONSTANTS
// ============================================================================

pub const AXIS_RED_COLOR: Color = Color::srgb(0.92, 0.16, 0.16);
pub const AXIS_GREEN_COLOR: Color = Color::srgb(0.18, 0.80, 0.24);
pub const AXIS_BLUE_COLOR: Color = Color::srgb(0.16, 0.46, 0.96);
pub const INFERENCE_MAGENTA_COLOR: Color = Color::srgb(0.95, 0.20, 0.85);
pub const DEFAULT_LINE_PREVIEW_COLOR: Color = Color::srgb(0.98, 0.84, 0.20);
pub const NEON_BLUE_SELECTION_COLOR: Color = Color::srgb(0.05, 0.88, 1.0);
pub const SCALE_BBOX_COLOR: Color = Color::srgb(1.0, 0.78, 0.15);
pub const SCALE_HANDLE_COLOR: Color = Color::srgb(0.15, 0.95, 0.55);
pub const SCALE_ACTIVE_HANDLE_COLOR: Color = Color::srgb(1.0, 0.30, 0.20);

/// Maximum proximity radius (in meters) when scanning neighbor `CadObject` edges for parallel inference.
const NEIGHBOR_PROXIMITY_RADIUS: f32 = 40.0;
/// Maximum angular deviation in degrees (2.0°) for snapping parallel to a neighbor's edge.
const PARALLEL_SNAP_MAX_ANGLE_DEG: f32 = 2.0;

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
            .init_resource::<VcbInput>()
            .init_resource::<SelectedCadObjects>()
            .init_resource::<FocalPoint>()
            .add_systems(
                Startup,
                (
                    setup_cad_environment_and_lighting,
                    setup_orbit_camera,
                    setup_block_preview_entity,
                    seed_initial_cad_scene,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    capture_keyboard_vcb_and_shortcuts,
                    update_mouse_raycasting_and_inference,
                    orbit_camera_controller,
                    handle_viewport_tool_interactions,
                    sync_selection_highlighting,
                    update_transparent_block_preview,
                    render_infinite_grid_and_tool_gizmos,
                )
                    .chain()
                    .in_set(CadToolsSet),
            );
    }
}

// ============================================================================
// TOOL STATE MACHINE & INFERENCE RESOURCES
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CadTool {
    #[default]
    Select,
    LineDraw,
    Block,
    Move,
    Scale,
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
            Self::PushPullExtrude => "Extrude Height",
        }
    }

    pub const fn status_hint(self) -> &'static str {
        match self {
            Self::Select => {
                "LMB: Select Object (Neon Blue) | Del/Backspace: Delete | M: Move | S: Scale"
            }
            Self::LineDraw => {
                "Click to chain lines | Hold Shift: Vertical Blue (Z) | Magenta: 2° Parallel Guide | Esc: Break"
            }
            Self::Block => {
                "1st Click: Corner | 2nd Click: Commit Block | Or type 'W,D,H' (e.g. 3,4,2) + Enter"
            }
            Self::Move => {
                "LMB Drag on object: Translate with R/G/B & Magenta snapping | Hold Shift: Vertical Z | Release: Commit"
            }
            Self::Scale => {
                "LMB Drag any of the 8 bounding-box corner handles to resize | Or type Scale Factor (e.g. 1.5) + Enter"
            }
            Self::PushPullExtrude => {
                "Click to extrude 3D column | Type height + Enter | Space: Select Tool"
            }
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct ActiveToolState {
    pub current: CadTool,
}

/// Dynamic focal target vector around which the camera orbits and zooms,
/// shifted in 3D space via `Shift + Middle-Mouse Button` dragging.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FocalPoint(pub Vec3);

impl Default for FocalPoint {
    fn default() -> Self {
        Self(Vec3::new(1.5, 1.5, 0.5))
    }
}

/// Tracks primary axis snapping (Red/Green/Blue) and advanced neighbor inference (Magenta).
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
            Self::None => "Free",
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

/// Active drag state for the Move Tool (`CadTool::Move`).
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

/// Active handle drag state for the Scale Tool (`CadTool::Scale`).
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

/// SketchUp-style Value Control Box (VCB / Measurements) buffer.
#[derive(Resource, Debug, Default)]
pub struct VcbInput {
    pub buffer: String,
    pub last_status_message: Option<String>,
}

/// Tracks currently selected `CadObject` entities for neon-blue highlighting, Move, Scale, and Delete.
#[derive(Resource, Debug, Default)]
pub struct SelectedCadObjects {
    pub entities: Vec<Entity>,
}

#[derive(Component, Debug)]
pub struct BlockPreviewVisual;

// ============================================================================
// HYPRLAND-COMPATIBLE ORBIT CAMERA AROUND DYNAMIC `FocalPoint`
// ============================================================================

#[derive(Component, Debug)]
pub struct OrbitCamera {
    pub radius: f32,
    pub azimuth: f32,
    pub elevation: f32,
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub zoom_sensitivity: f32,
    pub min_radius: f32,
    pub max_radius: f32,
    pub is_mmb_dragging: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            radius: 16.0,
            azimuth: -1.15,
            elevation: 0.52,
            orbit_sensitivity: 0.0055,
            pan_sensitivity: 0.0018,
            zoom_sensitivity: 0.12,
            min_radius: 0.75,
            max_radius: 300.0,
            is_mmb_dragging: false,
        }
    }
}

impl OrbitCamera {
    pub fn compute_transform(&self, focal_point: Vec3) -> Transform {
        let clamped_el = self.elevation.clamp(-FRAC_PI_2 + 0.04, FRAC_PI_2 - 0.04);
        let clamped_radius = self.radius.clamp(self.min_radius, self.max_radius);

        let horiz = clamped_radius * clamped_el.cos();
        let offset = Vec3::new(
            horiz * self.azimuth.cos(),
            horiz * self.azimuth.sin(),
            clamped_radius * clamped_el.sin(),
        );
        let eye = focal_point + offset;
        Transform::from_translation(eye).looking_at(focal_point, Vec3::Z)
    }
}

fn setup_orbit_camera(mut commands: Commands, focal_point: Res<FocalPoint>) {
    let orbit = OrbitCamera::default();
    let initial_transform = orbit.compute_transform(focal_point.0);

    commands.spawn((
        Camera3dBundle {
            transform: initial_transform,
            projection: Projection::Perspective(PerspectiveProjection {
                fov: 45.0_f32.to_radians(),
                near: 0.05,
                far: 1200.0,
                ..default()
            }),
            ..default()
        },
        orbit,
    ));
}

fn orbit_camera_controller(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    ui_capture: Res<UiInputCapture>,
    mut focal_point: ResMut<FocalPoint>,
    mut motion_events: EventReader<MouseMotion>,
    mut scroll_events: EventReader<MouseWheel>,
    mut camera_query: Query<(&mut OrbitCamera, &mut Transform)>,
) {
    let mut delta = Vec2::ZERO;
    for ev in motion_events.read() {
        delta += ev.delta;
    }

    let mut scroll = 0.0_f32;
    for ev in scroll_events.read() {
        let scale = match ev.unit {
            MouseScrollUnit::Line => 1.0,
            MouseScrollUnit::Pixel => 0.025,
        };
        scroll += ev.y * scale;
    }

    let Ok((mut orbit, mut transform)) = camera_query.get_single_mut() else {
        return;
    };

    if mouse_buttons.just_pressed(MouseButton::Middle) && !ui_capture.egui_wants_pointer {
        orbit.is_mmb_dragging = true;
    }
    if !mouse_buttons.pressed(MouseButton::Middle) {
        orbit.is_mmb_dragging = false;
    }

    let shift_held = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if orbit.is_mmb_dragging && delta.length_squared() > 0.0 {
        if shift_held {
            let local_right = transform.rotation * Vec3::X;
            let local_up = transform.rotation * Vec3::Y;
            let pan_scale = orbit.radius.max(1.0) * orbit.pan_sensitivity;
            let pan_offset =
                (-delta.x * pan_scale * local_right) + (delta.y * pan_scale * local_up);
            focal_point.0 += pan_offset;
        } else {
            orbit.azimuth = (orbit.azimuth - delta.x * orbit.orbit_sensitivity) % TAU;
            orbit.elevation = (orbit.elevation + delta.y * orbit.orbit_sensitivity)
                .clamp(-FRAC_PI_2 + 0.04, FRAC_PI_2 - 0.04);
        }
    }

    if !ui_capture.egui_wants_pointer && scroll.abs() > f32::EPSILON {
        let zoom_factor = (1.0 - scroll * orbit.zoom_sensitivity).clamp(0.5, 1.8);
        orbit.radius = (orbit.radius * zoom_factor).clamp(orbit.min_radius, orbit.max_radius);
    }

    *transform = orbit.compute_transform(focal_point.0);
}

// ============================================================================
// 3D ENVIRONMENT, GROUND GRID, SKETCHUP AXES & INITIAL SCENE SETUP
// ============================================================================

fn setup_cad_environment_and_lighting(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let ground_mesh = meshes.add(Plane3d::default().mesh().size(400.0, 400.0));
    let ground_material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.19, 0.21, 0.24, 0.88),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.92,
        metallic: 0.0,
        reflectance: 0.08,
        double_sided: true,
        cull_mode: None,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: ground_mesh,
            material: ground_material,
            transform: Transform {
                translation: Vec3::new(0.0, 0.0, -0.004),
                rotation: Quat::from_rotation_x(FRAC_PI_2),
                scale: Vec3::ONE,
            },
            ..default()
        },
        NotShadowCaster,
    ));

    let grid_mesh = meshes.add(build_xy_ground_grid_mesh(100, 1.0, 5));
    let unlit_vertex_color_mat = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: grid_mesh,
            material: unlit_vertex_color_mat.clone(),
            transform: Transform::from_xyz(0.0, 0.0, -0.001),
            ..default()
        },
        NotShadowCaster,
    ));

    let axis_length = 120.0_f32;
    let axis_radius = 0.024_f32;
    let axis_cylinder_mesh = meshes.add(Cylinder::new(axis_radius, axis_length));

    let x_mat = materials.add(StandardMaterial {
        base_color: AXIS_RED_COLOR,
        unlit: true,
        ..default()
    });
    let y_mat = materials.add(StandardMaterial {
        base_color: AXIS_GREEN_COLOR,
        unlit: true,
        ..default()
    });
    let z_mat = materials.add(StandardMaterial {
        base_color: AXIS_BLUE_COLOR,
        unlit: true,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: axis_cylinder_mesh.clone(),
            material: x_mat,
            transform: Transform {
                translation: Vec3::new(axis_length * 0.5, 0.0, 0.0),
                rotation: Quat::from_rotation_z(-FRAC_PI_2),
                scale: Vec3::ONE,
            },
            ..default()
        },
        NotShadowCaster,
    ));

    commands.spawn((
        PbrBundle {
            mesh: axis_cylinder_mesh.clone(),
            material: y_mat,
            transform: Transform {
                translation: Vec3::new(0.0, axis_length * 0.5, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            ..default()
        },
        NotShadowCaster,
    ));

    commands.spawn((
        PbrBundle {
            mesh: axis_cylinder_mesh,
            material: z_mat,
            transform: Transform {
                translation: Vec3::new(0.0, 0.0, axis_length * 0.5),
                rotation: Quat::from_rotation_x(FRAC_PI_2),
                scale: Vec3::ONE,
            },
            ..default()
        },
        NotShadowCaster,
    ));

    let axes_line_mesh = meshes.add(build_coordinate_axes_line_mesh(axis_length));
    commands.spawn((
        PbrBundle {
            mesh: axes_line_mesh,
            material: unlit_vertex_color_mat,
            transform: Transform::IDENTITY,
            ..default()
        },
        NotShadowCaster,
    ));

    commands.spawn((
        PbrBundle {
            mesh: meshes.add(Sphere::new(0.06)),
            material: materials.add(StandardMaterial {
                base_color: Color::srgb(0.96, 0.96, 0.98),
                unlit: true,
                ..default()
            }),
            transform: Transform::IDENTITY,
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

fn setup_block_preview_entity(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let unit_cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let preview_material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.24, 0.68, 0.98, 0.34),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: unit_cube,
            material: preview_material,
            visibility: Visibility::Hidden,
            ..default()
        },
        NotShadowCaster,
        BlockPreviewVisual,
    ));
}

fn seed_initial_cad_scene(world: &mut World) {
    let starter_commands: Vec<Box<dyn Command + Send + Sync>> = vec![
        Box::new(CreateBlockCommand::from_corner_and_dimensions(
            Vec3::new(-3.0, 1.0, 0.0),
            Vec3::new(2.0, 2.0, 1.5),
            Color::srgb(0.76, 0.80, 0.86),
            Some(GroupId(1)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(4.0, 3.0, 0.0),
            Color::srgb(0.92, 0.62, 0.22),
            Some(GroupId(1)),
        )),
        Box::new(DrawLineCommand::new(
            Vec3::new(4.0, 3.0, 0.0),
            Vec3::new(4.0, 3.0, 2.0),
            AXIS_BLUE_COLOR,
            Some(GroupId(1)),
        )),
    ];

    for mut cmd in starter_commands {
        cmd.execute(world);
        if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
            history.undo_stack.push(cmd);
            history.redo_stack.clear();
        }
    }
}

fn build_xy_ground_grid_mesh(half_extents: i32, spacing: f32, major_every: i32) -> Mesh {
    let extent = half_extents as f32 * spacing;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();

    let minor_color = [0.34, 0.37, 0.42, 0.52];
    let major_color = [0.50, 0.54, 0.60, 0.82];

    for i in -half_extents..=half_extents {
        let offset = i as f32 * spacing;
        let is_major = major_every > 0 && (i % major_every == 0);
        let color = if is_major { major_color } else { minor_color };

        if i == 0 {
            positions.push([-extent, 0.0, 0.0]);
            positions.push([0.0, 0.0, 0.0]);
            colors.push(major_color);
            colors.push(major_color);

            positions.push([0.0, -extent, 0.0]);
            positions.push([0.0, 0.0, 0.0]);
            colors.push(major_color);
            colors.push(major_color);
            continue;
        }

        positions.push([-extent, offset, 0.0]);
        positions.push([extent, offset, 0.0]);
        colors.push(color);
        colors.push(color);

        positions.push([offset, -extent, 0.0]);
        positions.push([offset, extent, 0.0]);
        colors.push(color);
        colors.push(color);
    }

    let indices: Vec<u32> = (0..positions.len() as u32).collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::LineList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn build_coordinate_axes_line_mesh(length: f32) -> Mesh {
    let r = AXIS_RED_COLOR.to_linear().to_f32_array();
    let g = AXIS_GREEN_COLOR.to_linear().to_f32_array();
    let b = AXIS_BLUE_COLOR.to_linear().to_f32_array();

    let positions = vec![
        [0.0, 0.0, 0.0],
        [length, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, length, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, length],
    ];
    let colors = vec![r, r, g, g, b, b];
    let indices = vec![0, 1, 2, 3, 4, 5];

    let mut mesh = Mesh::new(
        PrimitiveTopology::LineList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// ============================================================================
// RAYCASTING, SHIFT VERTICAL Z-LOCK & 2-DEGREE PARALLEL INFERENCE ENGINE
// ============================================================================

/// Evaluates primary axis alignment (Red/Green/Blue) and scans all neighbor `CadObject`
/// edges within `NEIGHBOR_PROXIMITY_RADIUS` to snap within `2.0°` (`PARALLEL_SNAP_MAX_ANGLE_DEG`).
fn compute_snapped_vector_and_inference<'a>(
    anchor: Vec3,
    raw_target: Vec3,
    excluded_entity: Option<Entity>,
    cad_objects: impl Iterator<Item = (Entity, &'a CadObject)>,
) -> (Vec3, AxisInference) {
    let delta = raw_target - anchor;
    let len = delta.length();
    if len <= 0.04 {
        return (raw_target, AxisInference::None);
    }

    let dir = delta / len;
    let primary_cos_threshold = 0.990_f32;

    if dir.x.abs() >= primary_cos_threshold {
        return (
            Vec3::new(raw_target.x, anchor.y, anchor.z),
            AxisInference::RedX,
        );
    }
    if dir.y.abs() >= primary_cos_threshold {
        return (
            Vec3::new(anchor.x, raw_target.y, anchor.z),
            AxisInference::GreenY,
        );
    }
    if dir.z.abs() >= primary_cos_threshold {
        return (
            Vec3::new(anchor.x, anchor.y, raw_target.z),
            AxisInference::BlueZ,
        );
    }

    // Neighbor Parallel Inference: check if `dir` is within 2.0 degrees of any neighbor's edge
    let mut best_neighbor_match: Option<(f32, Vec3, Vec3, Vec3)> = None;

    for (entity, obj) in cad_objects {
        if Some(entity) == excluded_entity {
            continue;
        }
        if obj.geometry.center().distance(anchor) > NEIGHBOR_PROXIMITY_RADIUS {
            continue;
        }

        for (edge_start, edge_end) in obj.geometry.edges() {
            let edge_vec = edge_end - edge_start;
            let edge_len = edge_vec.length();
            if edge_len <= 0.05 {
                continue;
            }
            let edge_dir = edge_vec / edge_len;

            // Skip edges that are already purely along global X, Y, or Z
            if edge_dir.x.abs() > 0.999 || edge_dir.y.abs() > 0.999 || edge_dir.z.abs() > 0.999 {
                continue;
            }

            let cos_angle = dir.dot(edge_dir).abs().clamp(0.0, 1.0);
            let angle_deg = cos_angle.acos().to_degrees();

            if angle_deg <= PARALLEL_SNAP_MAX_ANGLE_DEG {
                let is_better = best_neighbor_match
                    .map(|(best_deg, _, _, _)| angle_deg < best_deg)
                    .unwrap_or(true);
                if is_better {
                    best_neighbor_match = Some((angle_deg, edge_start, edge_end, edge_dir));
                }
            }
        }
    }

    if let Some((_, edge_start, edge_end, edge_dir)) = best_neighbor_match {
        let projected_len = delta.dot(edge_dir);
        let snapped_end = anchor + edge_dir * projected_len;
        return (
            snapped_end,
            AxisInference::ParallelNeighbor {
                edge_start,
                edge_end,
                direction: edge_dir,
            },
        );
    }

    (raw_target, AxisInference::None)
}

#[allow(clippy::too_many_arguments)]
fn update_mouse_raycasting_and_inference(
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    tool_state: Res<ActiveToolState>,
    move_state: Res<MoveToolState>,
    scale_state: Res<ScaleToolState>,
    selected: Res<SelectedCadObjects>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut cursor_ray: ResMut<CursorRayState>,
    mut draft: ResMut<ToolDraftState>,
) {
    let Ok(window) = window_query.get_single() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.get_single() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Some(cursor_screen_pos) = window.cursor_position() else {
        cursor_ray.is_valid_hit = false;
        return;
    };
    let Some(ray) = camera.viewport_to_world(camera_transform, cursor_screen_pos) else {
        cursor_ray.is_valid_hit = false;
        return;
    };

    let shift_held = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mmb_pressed = mouse_buttons.pressed(MouseButton::Middle);
    let snap_step = 0.05_f32;

    // Determine active reference anchor (either LineDraw start point or MoveTool grab anchor)
    let active_anchor: Option<(Vec3, Option<Entity>)> = match tool_state.current {
        CadTool::LineDraw => draft.active_start_point.map(|pt| (pt, None)),
        CadTool::Move => move_state
            .active_drag
            .as_ref()
            .map(|drag| (drag.grab_anchor, Some(drag.entity))),
        _ => None,
    };

    // 1. Shift-Locked Vertical Blue (Z) Axis Plane for LineDraw and Move tools
    if shift_held && !mmb_pressed {
        if let Some((anchor, excluded_entity)) = active_anchor {
            let cam_pos = camera_transform.translation();
            let to_cam_xy = Vec3::new(cam_pos.x - anchor.x, cam_pos.y - anchor.y, 0.0);
            let plane_normal = Dir3::new(to_cam_xy).unwrap_or(Dir3::X);

            if let Some(distance) = ray.intersect_plane(anchor, InfinitePlane3d::new(plane_normal))
            {
                let hit_point = ray.get_point(distance);
                let mut snapped_z = (hit_point.z / snap_step).round() * snap_step;
                let mut inference = AxisInference::BlueZ;

                // Snap flush with any neighbor Block top face
                for (ent, obj) in cad_objects.iter() {
                    if Some(ent) == excluded_entity {
                        continue;
                    }
                    if let CadGeometryKind::Block { center, size } = obj.geometry {
                        let top_z = center.z + size.z * 0.5;
                        if (snapped_z - top_z).abs() <= 0.14 {
                            snapped_z = top_z;
                            inference = AxisInference::FlushTopFace {
                                elevation_z: top_z,
                                face_center: Vec3::new(center.x, center.y, top_z),
                            };
                            break;
                        }
                    }
                }

                let vertical_pos = Vec3::new(anchor.x, anchor.y, snapped_z);
                cursor_ray.raw_ground_hit = hit_point;
                cursor_ray.snapped_pos = vertical_pos;
                cursor_ray.is_valid_hit = true;
                cursor_ray.vertical_z_locked = true;
                cursor_ray.inference = inference;
                draft.current_preview_end = vertical_pos;
                return;
            }
        }
    }

    cursor_ray.vertical_z_locked = false;

    // 2. If dragging a 3D Scale handle, project ray onto camera-facing plane through handle
    if tool_state.current == CadTool::Scale {
        if let Some(ref drag) = scale_state.active_drag {
            let cam_forward = camera_transform.forward();
            if let Some(dist) =
                ray.intersect_plane(drag.initial_handle_pos, InfinitePlane3d::new(*cam_forward))
            {
                let hit = ray.get_point(dist);
                cursor_ray.raw_ground_hit = hit;
                cursor_ray.snapped_pos = hit;
                cursor_ray.is_valid_hit = true;
                cursor_ray.inference = AxisInference::None;
                draft.current_preview_end = hit;
                return;
            }
        }
    }

    // 3. Flush Top-Face detection when hovering over existing Blocks
    let mut best_top_face_hit: Option<(f32, Vec3, f32, Vec3)> = None;
    if active_anchor.is_none() {
        for (ent, obj) in cad_objects.iter() {
            if tool_state.current == CadTool::Move && selected.entities.contains(&ent) {
                continue;
            }
            if let CadGeometryKind::Block { center, size } = obj.geometry {
                let top_z = center.z + size.z * 0.5;
                let half_x = size.x * 0.5;
                let half_y = size.y * 0.5;
                let plane_origin = Vec3::new(center.x, center.y, top_z);

                if let Some(dist) = ray.intersect_plane(plane_origin, InfinitePlane3d::new(Vec3::Z))
                {
                    let pt = ray.get_point(dist);
                    if (pt.x - center.x).abs() <= half_x + 0.08
                        && (pt.y - center.y).abs() <= half_y + 0.08
                    {
                        let is_closer = best_top_face_hit
                            .map(|(best_d, _, _, _)| dist < best_d)
                            .unwrap_or(true);
                        if is_closer {
                            best_top_face_hit =
                                Some((dist, pt, top_z, Vec3::new(center.x, center.y, top_z)));
                        }
                    }
                }
            }
        }
    }

    let reference_z = match active_anchor {
        Some((anchor, _)) => anchor.z,
        None => draft
            .active_start_point
            .map(|s| s.z)
            .or_else(|| best_top_face_hit.map(|(_, _, tz, _)| tz))
            .unwrap_or(0.0),
    };
    let plane_origin = Vec3::new(0.0, 0.0, reference_z);

    let Some(distance) = ray.intersect_plane(plane_origin, InfinitePlane3d::new(Vec3::Z)) else {
        cursor_ray.is_valid_hit = false;
        return;
    };

    let raw_hit = ray.get_point(distance);
    let mut candidate = Vec3::new(
        (raw_hit.x / snap_step).round() * snap_step,
        (raw_hit.y / snap_step).round() * snap_step,
        reference_z,
    );

    let mut inference = if let Some((_, _, elevation_z, face_center)) = best_top_face_hit {
        AxisInference::FlushTopFace {
            elevation_z,
            face_center,
        }
    } else {
        AxisInference::None
    };

    // 4. Evaluate Red/Green/Blue Axis & 2-Degree Parallel Neighbor Inference for LineDraw and Move
    if let Some((anchor, excluded_entity)) = active_anchor {
        let (snapped_pt, detected_inference) = compute_snapped_vector_and_inference(
            anchor,
            candidate,
            excluded_entity,
            cad_objects.iter(),
        );
        candidate = snapped_pt;
        if detected_inference != AxisInference::None {
            inference = detected_inference;
        }
    }

    cursor_ray.raw_ground_hit = raw_hit;
    cursor_ray.snapped_pos = candidate;
    cursor_ray.is_valid_hit = true;
    cursor_ray.inference = inference;
    draft.current_preview_end = candidate;
}

// ============================================================================
// KEYBOARD VCB INPUT & HOTKEYS (SPACE, L, B, M, S, DEL/BACKSPACE, ESC, CTRL+Z/Y/Q)
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn capture_keyboard_vcb_and_shortcuts(
    mut keyboard_events: EventReader<KeyboardInput>,
    keyboard_state: Res<ButtonInput<KeyCode>>,
    ui_capture: Res<UiInputCapture>,
    cursor_ray: Res<CursorRayState>,
    mut tool_state: ResMut<ActiveToolState>,
    mut vcb: ResMut<VcbInput>,
    mut draft: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
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

        // Ctrl + Q -> Exit Application cleanly
        if ctrl_held && event.key_code == KeyCode::KeyQ {
            app_exit_events.send(AppExit::Success);
            continue;
        }

        // Ctrl + Z / Ctrl + Y -> Undo / Redo
        if ctrl_held {
            if event.key_code == KeyCode::KeyZ {
                draft.active_start_point = None;
                move_state.active_drag = None;
                scale_state.active_drag = None;
                if shift_held {
                    pending_actions.queue.push(HistoryAction::Redo);
                } else {
                    pending_actions.queue.push(HistoryAction::Undo);
                }
                continue;
            } else if event.key_code == KeyCode::KeyY {
                draft.active_start_point = None;
                move_state.active_drag = None;
                scale_state.active_drag = None;
                pending_actions.queue.push(HistoryAction::Redo);
                continue;
            }
        }

        // Single-key tool hotkeys: [Space], [L], [B], [M], [S]
        if !ctrl_held {
            let switched_tool = match event.key_code {
                KeyCode::Space => Some(CadTool::Select),
                KeyCode::KeyL => Some(CadTool::LineDraw),
                KeyCode::KeyB => Some(CadTool::Block),
                KeyCode::KeyM => Some(CadTool::Move),
                KeyCode::KeyS => Some(CadTool::Scale),
                _ => None,
            };

            if let Some(new_tool) = switched_tool {
                cancel_in_progress_drags(&mut move_state, &mut scale_state, &mut cad_query);
                tool_state.current = new_tool;
                draft.active_start_point = None;
                vcb.buffer.clear();
                vcb.last_status_message = Some(format!("Activated {}", new_tool.label()));
                continue;
            }
        }

        // [Escape] -> Cancel current operation / break continuous line chain
        if event.key_code == KeyCode::Escape || matches!(event.logical_key, Key::Escape) {
            cancel_in_progress_drags(&mut move_state, &mut scale_state, &mut cad_query);
            draft.active_start_point = None;
            vcb.buffer.clear();
            vcb.last_status_message = Some(String::from("Operation canceled / chain broken"));
            continue;
        }

        // [Backspace] -> Edit VCB buffer if non-empty; otherwise delete selected object(s)
        if event.key_code == KeyCode::Backspace || matches!(event.logical_key, Key::Backspace) {
            if !vcb.buffer.is_empty() {
                vcb.buffer.pop();
                continue;
            }
        }

        // [Delete / Backspace (when VCB empty)] -> Delete Selected Object(s) via `DeleteCommand`
        if event.key_code == KeyCode::Delete
            || matches!(event.logical_key, Key::Delete)
            || (event.key_code == KeyCode::Backspace && vcb.buffer.is_empty())
        {
            if !selected.entities.is_empty() {
                cancel_in_progress_drags(&mut move_state, &mut scale_state, &mut cad_query);
                let mut snapshots = Vec::new();
                for &entity in &selected.entities {
                    if let Ok((ent, obj, _)) = cad_query.get(entity) {
                        snapshots.push((ent, obj.clone()));
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

        // [Enter] -> Parse VCB buffer and commit command
        if event.key_code == KeyCode::Enter
            || event.key_code == KeyCode::NumpadEnter
            || matches!(event.logical_key, Key::Enter)
        {
            let input_text = vcb.buffer.trim().to_string();
            vcb.buffer.clear();
            if !input_text.is_empty() {
                execute_parsed_vcb_input(
                    &input_text,
                    tool_state.current,
                    &cursor_ray,
                    &selected,
                    &mut draft,
                    &mut move_state,
                    &mut scale_state,
                    &mut cad_query,
                    &mut vcb,
                    &mut pending_actions,
                );
            }
            continue;
        }

        // Append valid numeric / dimension characters ('0'-'9', '.', ',', '-') to VCB buffer
        if !ctrl_held {
            if let Key::Character(ref text) = event.logical_key {
                for ch in text.chars() {
                    if ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == '-' {
                        vcb.buffer.push(ch);
                    }
                }
            }
        }
    }
}

fn cancel_in_progress_drags(
    move_state: &mut MoveToolState,
    scale_state: &mut ScaleToolState,
    cad_query: &mut Query<(Entity, &CadObject, &mut Transform)>,
) {
    if let Some(drag) = move_state.active_drag.take() {
        if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
            *transform = drag.initial_transform;
        }
    }
    if let Some(drag) = scale_state.active_drag.take() {
        if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
            *transform = drag.initial_transform;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_parsed_vcb_input(
    input_text: &str,
    active_tool: CadTool,
    cursor_ray: &CursorRayState,
    selected: &SelectedCadObjects,
    draft: &mut ToolDraftState,
    move_state: &mut MoveToolState,
    scale_state: &mut ScaleToolState,
    cad_query: &mut Query<(Entity, &CadObject, &mut Transform)>,
    vcb: &mut VcbInput,
    pending_actions: &mut PendingHistoryActions,
) {
    let parts: Vec<&str> = input_text
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

            if parts.len() == 1 {
                if let Ok(target_length) = parts[0].parse::<f32>() {
                    if target_length.abs() > 0.001 {
                        let raw_delta = draft.current_preview_end - start;
                        let direction = if cursor_ray.vertical_z_locked {
                            if raw_delta.z < 0.0 {
                                -Vec3::Z
                            } else {
                                Vec3::Z
                            }
                        } else {
                            raw_delta.try_normalize().unwrap_or(Vec3::X)
                        };

                        let exact_end = start + direction * target_length;
                        let line_color = cursor_ray.inference.preview_color();

                        pending_actions.queue.push(HistoryAction::Execute(Box::new(
                            DrawLineCommand::new(start, exact_end, line_color, None),
                        )));

                        draft.active_start_point = Some(exact_end);
                        draft.current_preview_end = exact_end;
                        vcb.last_status_message =
                            Some(format!("Committed {:.2}m line segment", target_length));
                        return;
                    }
                }
            } else if parts.len() == 3 {
                if let (Ok(dx), Ok(dy), Ok(dz)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                ) {
                    let exact_end = start + Vec3::new(dx, dy, dz);
                    if exact_end.distance(start) > 0.001 {
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(
                            DrawLineCommand::new(
                                start,
                                exact_end,
                                DEFAULT_LINE_PREVIEW_COLOR,
                                None,
                            ),
                        )));
                        draft.active_start_point = Some(exact_end);
                        draft.current_preview_end = exact_end;
                        vcb.last_status_message =
                            Some(format!("Committed offset line ({dx:.2}, {dy:.2}, {dz:.2})"));
                        return;
                    }
                }
            }

            vcb.last_status_message = Some(format!("Invalid Line VCB input: '{input_text}'"));
        }
        CadTool::Block => {
            let start_corner = draft.active_start_point.unwrap_or(cursor_ray.snapped_pos);

            if parts.len() == 3 {
                if let (Ok(w), Ok(d), Ok(h)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                ) {
                    if w.abs() > 0.001 && d.abs() > 0.001 && h.abs() > 0.001 {
                        let cmd = CreateBlockCommand::from_corner_and_dimensions(
                            start_corner,
                            Vec3::new(w, d, h),
                            Color::srgb(0.78, 0.82, 0.88),
                            None,
                        );
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));
                        draft.active_start_point = None;
                        vcb.last_status_message = Some(format!(
                            "Spawned Block ({w:.2}m x {d:.2}m x {h:.2}m)"
                        ));
                        return;
                    }
                }
            } else if parts.len() == 1 {
                if let Ok(h) = parts[0].parse::<f32>() {
                    if h.abs() > 0.01 {
                        draft.default_block_height = h;
                        vcb.last_status_message = Some(format!("Set block height to {h:.2}m"));
                        return;
                    }
                }
            }

            vcb.last_status_message = Some(format!(
                "Block VCB expects 'width,depth,height' (e.g. 3,4,2), got '{input_text}'"
            ));
        }
        CadTool::Move => {
            // Support exact VCB distance or (dx,dy,dz) offset during Move
            let target_entity = move_state
                .active_drag
                .as_ref()
                .map(|d| d.entity)
                .or_else(|| selected.entities.first().copied());

            let Some(entity) = target_entity else {
                vcb.last_status_message =
                    Some(String::from("Select an object before entering a Move distance"));
                return;
            };

            let offset = if parts.len() == 1 {
                if let Ok(dist) = parts[0].parse::<f32>() {
                    let dir = move_state
                        .active_drag
                        .as_ref()
                        .and_then(|d| d.current_offset.try_normalize())
                        .unwrap_or(Vec3::X);
                    Some(dir * dist)
                } else {
                    None
                }
            } else if parts.len() == 3 {
                if let (Ok(dx), Ok(dy), Ok(dz)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                ) {
                    Some(Vec3::new(dx, dy, dz))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(delta) = offset {
                let before_geom = if let Some(drag) = move_state.active_drag.take() {
                    if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
                        *transform = drag.initial_transform;
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
                    vcb.last_status_message = Some(format!(
                        "Moved object by ({:+.2}, {:+.2}, {:+.2})m",
                        delta.x, delta.y, delta.z
                    ));
                }
            }
        }
        CadTool::Scale => {
            // Typing a single scale factor (e.g. "1.5") or "sx,sy,sz" scales the object and commits `ScaleObjectCommand`
            let scale_vec = if parts.len() == 1 {
                parts[0]
                    .parse::<f32>()
                    .ok()
                    .filter(|&s| s.abs() > 0.01)
                    .map(Vec3::splat)
            } else if parts.len() == 3 {
                if let (Ok(sx), Ok(sy), Ok(sz)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                ) {
                    if sx.abs() > 0.01 && sy.abs() > 0.01 && sz.abs() > 0.01 {
                        Some(Vec3::new(sx.abs(), sy.abs(), sz.abs()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let Some(factors) = scale_vec else {
                vcb.last_status_message = Some(format!(
                    "Invalid Scale factor '{input_text}' (try e.g. 1.5 or 1.5,1.5,2.0)"
                ));
                return;
            };

            if let Some(drag) = scale_state.active_drag.take() {
                if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
                    *transform = drag.initial_transform;
                }
                let after = drag
                    .initial_geometry
                    .scaled_from_anchor(drag.opposite_anchor, factors);
                pending_actions.queue.push(HistoryAction::Execute(Box::new(
                    ScaleObjectCommand::new(drag.entity, drag.initial_geometry, after),
                )));
                vcb.last_status_message = Some(format!(
                    "Scaled object by ({:.2}, {:.2}, {:.2})",
                    factors.x, factors.y, factors.z
                ));
            } else if let Some(&entity) = selected.entities.first() {
                if let Ok((_, cad_obj, _)) = cad_query.get(entity) {
                    let before = cad_obj.geometry.clone();
                    let (min_anchor, _) = before.aabb_min_max();
                    let after = before.scaled_from_anchor(min_anchor, factors);
                    pending_actions.queue.push(HistoryAction::Execute(Box::new(
                        ScaleObjectCommand::new(entity, before, after),
                    )));
                    vcb.last_status_message = Some(format!(
                        "Scaled selected object by ({:.2}, {:.2}, {:.2})",
                        factors.x, factors.y, factors.z
                    ));
                }
            } else {
                vcb.last_status_message =
                    Some(String::from("Select an object first to apply VCB Scale"));
            }
        }
        CadTool::PushPullExtrude => {
            if parts.len() == 1 {
                if let Ok(h) = parts[0].parse::<f32>() {
                    if h.abs() > 0.01 {
                        draft.default_block_height = h;
                        let base = cursor_ray.snapped_pos - Vec3::new(0.6, 0.6, 0.0);
                        let cmd = CreateBlockCommand::from_corner_and_dimensions(
                            base,
                            Vec3::new(1.2, 1.2, h),
                            Color::srgb(0.82, 0.85, 0.90),
                            None,
                        );
                        pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));
                        vcb.last_status_message =
                            Some(format!("Extruded column of height {h:.2}m"));
                    }
                }
            }
        }
        CadTool::Select => {
            vcb.last_status_message =
                Some(String::from("Switch to Line, Block, Move, or Scale to use VCB"));
        }
    }
}

// ============================================================================
// VIEWPORT INTERACTIONS (SELECT, LINE, BLOCK, MOVE DRAG, SCALE 8-HANDLE DRAG)
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn handle_viewport_tool_interactions(
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
    mut selected: ResMut<SelectedCadObjects>,
    mut vcb: ResMut<VcbInput>,
    mut pending_actions: ResMut<PendingHistoryActions>,
) {
    let Ok(window) = window_query.get_single() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.get_single() else {
        return;
    };
    let current_ray = window
        .cursor_position()
        .and_then(|pos| camera.viewport_to_world(camera_transform, pos));

    match tool_state.current {
        // --------------------------------------------------------------------
        // MOVE TOOL (Click LMB on object -> Drag in 3D with R/G/B/Z/Magenta snap -> Release commits MoveObjectCommand)
        // --------------------------------------------------------------------
        CadTool::Move => {
            if let Some(ref mut drag) = move_state.active_drag {
                if cursor_ray.is_valid_hit {
                    let offset = draft.current_preview_end - drag.grab_anchor;
                    drag.current_offset = offset;
                    if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
                        transform.translation = drag.initial_transform.translation + offset;
                    }
                }

                if mouse_buttons.just_released(MouseButton::Left) {
                    if let Some(finished_drag) = move_state.active_drag.take() {
                        // Restore transform before executing command through CommandHistory
                        if let Ok((_, _, mut transform)) = cad_query.get_mut(finished_drag.entity) {
                            *transform = finished_drag.initial_transform;
                        }
                        if finished_drag.current_offset.length() > 0.01 {
                            let after = finished_drag
                                .initial_geometry
                                .translated(finished_drag.current_offset);
                            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                MoveObjectCommand::new(
                                    finished_drag.entity,
                                    finished_drag.initial_geometry,
                                    after,
                                ),
                            )));
                            vcb.last_status_message = Some(format!(
                                "Moved object {:.2}m",
                                finished_drag.current_offset.length()
                            ));
                        }
                    }
                }
                return;
            }

            if !ui_capture.egui_wants_pointer && mouse_buttons.just_pressed(MouseButton::Left) {
                let Some(ray) = current_ray else {
                    return;
                };
                if let Some((hit_entity, _)) = pick_closest_cad_object(&ray, &cad_query) {
                    selected.entities.clear();
                    selected.entities.push(hit_entity);

                    if let Ok((_, cad_obj, transform)) = cad_query.get(hit_entity) {
                        let grab_anchor = if cursor_ray.is_valid_hit {
                            Vec3::new(
                                cursor_ray.snapped_pos.x,
                                cursor_ray.snapped_pos.y,
                                cad_obj.geometry.center().z,
                            )
                        } else {
                            cad_obj.geometry.center()
                        };

                        move_state.active_drag = Some(ActiveMoveDrag {
                            entity: hit_entity,
                            grab_anchor,
                            current_offset: Vec3::ZERO,
                            initial_geometry: cad_obj.geometry.clone(),
                            initial_transform: *transform,
                        });
                    }
                }
            }
        }

        // --------------------------------------------------------------------
        // SCALE TOOL (8 Clickable Bounding-Box Control Handles + VCB Factor Support)
        // --------------------------------------------------------------------
        CadTool::Scale => {
            scale_state.hovered_handle = None;

            if let Some(ref mut drag) = scale_state.active_drag {
                if cursor_ray.is_valid_hit {
                    let initial_vec = drag.initial_handle_pos - drag.opposite_anchor;
                    let current_vec = draft.current_preview_end - drag.opposite_anchor;

                    let compute_factor = |init_c: f32, curr_c: f32| -> f32 {
                        if init_c.abs() > 0.08 {
                            (curr_c / init_c).abs().clamp(0.10, 20.0)
                        } else {
                            1.0
                        }
                    };

                    let factors = Vec3::new(
                        compute_factor(initial_vec.x, current_vec.x),
                        compute_factor(initial_vec.y, current_vec.y),
                        compute_factor(initial_vec.z, current_vec.z),
                    );
                    drag.current_scale_factors = factors;

                    // Live visual scale preview on the entity's Transform
                    let preview_geom = drag
                        .initial_geometry
                        .scaled_from_anchor(drag.opposite_anchor, factors);
                    if let Ok((_, _, mut transform)) = cad_query.get_mut(drag.entity) {
                        transform.translation = preview_geom.center();
                        transform.scale = factors;
                    }
                }

                if mouse_buttons.just_released(MouseButton::Left) {
                    if let Some(finished_drag) = scale_state.active_drag.take() {
                        if let Ok((_, _, mut transform)) = cad_query.get_mut(finished_drag.entity) {
                            *transform = finished_drag.initial_transform;
                        }
                        if (finished_drag.current_scale_factors - Vec3::ONE).length() > 0.01 {
                            let after = finished_drag.initial_geometry.scaled_from_anchor(
                                finished_drag.opposite_anchor,
                                finished_drag.current_scale_factors,
                            );
                            pending_actions.queue.push(HistoryAction::Execute(Box::new(
                                ScaleObjectCommand::new(
                                    finished_drag.entity,
                                    finished_drag.initial_geometry,
                                    after,
                                ),
                            )));
                            vcb.last_status_message = Some(format!(
                                "Scaled object by ({:.2}, {:.2}, {:.2})",
                                finished_drag.current_scale_factors.x,
                                finished_drag.current_scale_factors.y,
                                finished_drag.current_scale_factors.z
                            ));
                        }
                    }
                }
                return;
            }

            // Detect hover or click on the 8 bounding-box vertex handles of the selected object
            let Some(ray) = current_ray else {
                return;
            };

            if let Some(&sel_entity) = selected.entities.first() {
                if let Ok((_, cad_obj, transform)) = cad_query.get(sel_entity) {
                    let corners = cad_obj.geometry.corners_8();
                    let handle_hit = pick_closest_scale_handle(&ray, &corners);
                    scale_state.hovered_handle = handle_hit;

                    if !ui_capture.egui_wants_pointer
                        && mouse_buttons.just_pressed(MouseButton::Left)
                    {
                        if let Some(h_idx) = handle_hit {
                            let opposite_idx = 7 - h_idx;
                            scale_state.active_drag = Some(ActiveScaleDrag {
                                entity: sel_entity,
                                handle_index: h_idx,
                                opposite_anchor: corners[opposite_idx],
                                initial_handle_pos: corners[h_idx],
                                current_scale_factors: Vec3::ONE,
                                initial_geometry: cad_obj.geometry.clone(),
                                initial_transform: *transform,
                            });
                            return;
                        }
                    }
                }
            }

            // If clicking LMB on another object while in Scale mode, select that object
            if !ui_capture.egui_wants_pointer && mouse_buttons.just_pressed(MouseButton::Left) {
                if let Some((hit_entity, _)) = pick_closest_cad_object(&ray, &cad_query) {
                    selected.entities.clear();
                    selected.entities.push(hit_entity);
                }
            }
        }

        // --------------------------------------------------------------------
        // SELECT TOOL
        // --------------------------------------------------------------------
        CadTool::Select => {
            if ui_capture.egui_wants_pointer || !mouse_buttons.just_pressed(MouseButton::Left) {
                return;
            }
            draft.active_start_point = None;
            let Some(ray) = current_ray else {
                return;
            };

            let closest_hit = pick_closest_cad_object(&ray, &cad_query);
            let shift_held = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            if !shift_held {
                selected.entities.clear();
            }
            if let Some((hit_entity, _)) = closest_hit {
                if !selected.entities.contains(&hit_entity) {
                    selected.entities.push(hit_entity);
                }
            }
        }

        // --------------------------------------------------------------------
        // LINE DRAW TOOL (Continuous Polyline Chain)
        // --------------------------------------------------------------------
        CadTool::LineDraw => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target_point = draft.current_preview_end;

            if let Some(start_point) = draft.active_start_point {
                if start_point.distance(target_point) > 0.02 {
                    let color = cursor_ray.inference.preview_color();
                    let cmd = DrawLineCommand::new(start_point, target_point, color, None);
                    pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));

                    // Continuous Line Chain: end point becomes the start point of the next line
                    draft.active_start_point = Some(target_point);
                }
            } else {
                draft.active_start_point = Some(target_point);
            }
        }

        // --------------------------------------------------------------------
        // 3D BLOCK (BOX) TOOL
        // --------------------------------------------------------------------
        CadTool::Block => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target_point = draft.current_preview_end;

            if let Some(start_corner) = draft.active_start_point.take() {
                let end_corner = Vec3::new(
                    target_point.x,
                    target_point.y,
                    start_corner.z + draft.default_block_height,
                );
                let cmd = CreateBlockCommand::from_corners(
                    start_corner,
                    end_corner,
                    Color::srgb(0.78, 0.82, 0.88),
                    None,
                );
                pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));
            } else {
                draft.active_start_point = Some(target_point);
            }
        }

        // --------------------------------------------------------------------
        // PUSH/PULL EXTRUDE TOOL
        // --------------------------------------------------------------------
        CadTool::PushPullExtrude => {
            if ui_capture.egui_wants_pointer
                || !mouse_buttons.just_pressed(MouseButton::Left)
                || !cursor_ray.is_valid_hit
            {
                return;
            }
            let target_point = draft.current_preview_end;
            draft.active_start_point = None;
            let corner = Vec3::new(target_point.x - 0.6, target_point.y - 0.6, target_point.z);
            let cmd = CreateBlockCommand::from_corner_and_dimensions(
                corner,
                Vec3::new(1.2, 1.2, draft.default_block_height),
                Color::srgb(0.82, 0.85, 0.90),
                None,
            );
            pending_actions.queue.push(HistoryAction::Execute(Box::new(cmd)));
        }
    }
}

fn pick_closest_cad_object(
    ray: &Ray3d,
    cad_query: &Query<(Entity, &CadObject, &mut Transform)>,
) -> Option<(Entity, f32)> {
    let mut closest_hit: Option<(Entity, f32)> = None;
    for (entity, cad_obj, _) in cad_query.iter() {
        if let Some(hit_dist) = ray_hits_cad_object(ray, cad_obj) {
            let is_closer = closest_hit
                .map(|(_, best_d)| hit_dist < best_d)
                .unwrap_or(true);
            if is_closer {
                closest_hit = Some((entity, hit_dist));
            }
        }
    }
    closest_hit
}

fn pick_closest_scale_handle(ray: &Ray3d, corners: &[Vec3; 8]) -> Option<usize> {
    let ray_origin = ray.origin;
    let ray_dir = Vec3::from(ray.direction);
    let mut best_handle: Option<(usize, f32)> = None;
    let pick_radius = 0.22_f32;

    for (idx, &corner) in corners.iter().enumerate() {
        let to_corner = corner - ray_origin;
        let t = to_corner.dot(ray_dir).max(0.0);
        let closest_pt = ray_origin + ray_dir * t;
        let dist = closest_pt.distance(corner);
        if dist <= pick_radius {
            let is_closer = best_handle.map(|(_, best_t)| t < best_t).unwrap_or(true);
            if is_closer {
                best_handle = Some((idx, t));
            }
        }
    }

    best_handle.map(|(idx, _)| idx)
}

fn ray_hits_cad_object(ray: &Ray3d, cad_obj: &CadObject) -> Option<f32> {
    let ray_origin = ray.origin;
    let ray_dir = Vec3::from(ray.direction);

    match cad_obj.geometry {
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5;
            let min_pt = center - half;
            let max_pt = center + half;

            let mut t_min = f32::NEG_INFINITY;
            let mut t_max = f32::INFINITY;

            for axis in 0..3 {
                let origin_a = ray_origin[axis];
                let dir_a = ray_dir[axis];
                let min_a = min_pt[axis];
                let max_a = max_pt[axis];

                if dir_a.abs() < 1e-6 {
                    if origin_a < min_a || origin_a > max_a {
                        return None;
                    }
                } else {
                    let inv_d = 1.0 / dir_a;
                    let mut t1 = (min_a - origin_a) * inv_d;
                    let mut t2 = (max_a - origin_a) * inv_d;
                    if t1 > t2 {
                        std::mem::swap(&mut t1, &mut t2);
                    }
                    t_min = t_min.max(t1);
                    t_max = t_max.min(t2);
                    if t_min > t_max {
                        return None;
                    }
                }
            }

            if t_max >= 0.0 {
                Some(t_min.max(0.0))
            } else {
                None
            }
        }
        CadGeometryKind::Line { start, end, radius } => {
            let u = end - start;
            let v = ray_dir;
            let w = start - ray_origin;
            let a = u.dot(u);
            let b = u.dot(v);
            let c = v.dot(v);
            let d = u.dot(w);
            let e = v.dot(w);
            let denom = a * c - b * b;

            let (s_clamped, t_ray) = if denom.abs() < 1e-6 {
                (0.0_f32, (e / c).max(0.0))
            } else {
                let s = ((b * e - c * d) / denom).clamp(0.0, 1.0);
                let t = ((a * e - b * d) / denom).max(0.0);
                (s, t)
            };

            let point_on_seg = start + u * s_clamped;
            let point_on_ray = ray_origin + v * t_ray;
            let pick_tolerance = (radius * 4.5).max(0.14);

            if point_on_seg.distance(point_on_ray) <= pick_tolerance {
                Some(t_ray)
            } else {
                None
            }
        }
    }
}

fn sync_selection_highlighting(
    selected: Res<SelectedCadObjects>,
    cad_objects: Query<(Entity, &CadObject, &Handle<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, cad_obj, mat_handle) in cad_objects.iter() {
        let Some(material) = materials.get_mut(mat_handle) else {
            continue;
        };

        if selected.entities.contains(&entity) {
            material.base_color = NEON_BLUE_SELECTION_COLOR;
            material.emissive = LinearRgba::rgb(0.02, 0.45, 0.65);
        } else {
            material.base_color = cad_obj.material_color.0;
            material.emissive = LinearRgba::BLACK;
        }
    }
}

fn update_transparent_block_preview(
    tool_state: Res<ActiveToolState>,
    draft: Res<ToolDraftState>,
    cursor_ray: Res<CursorRayState>,
    mut preview_query: Query<(&mut Transform, &mut Visibility), With<BlockPreviewVisual>>,
) {
    let Ok((mut transform, mut visibility)) = preview_query.get_single_mut() else {
        return;
    };

    if tool_state.current == CadTool::Block && cursor_ray.is_valid_hit {
        if let Some(start_corner) = draft.active_start_point {
            let end_corner = Vec3::new(
                draft.current_preview_end.x,
                draft.current_preview_end.y,
                start_corner.z + draft.default_block_height,
            );
            let min_pt = start_corner.min(end_corner);
            let max_pt = start_corner.max(end_corner);
            let size = Vec3::new(
                (max_pt.x - min_pt.x).max(0.05),
                (max_pt.y - min_pt.y).max(0.05),
                (max_pt.z - min_pt.z).max(0.05),
            );
            let center = (min_pt + max_pt) * 0.5;

            transform.translation = center;
            transform.rotation = Quat::IDENTITY;
            transform.scale = size;
            *visibility = Visibility::Visible;
            return;
        }
    }

    *visibility = Visibility::Hidden;
}

// ============================================================================
// GIZMO RENDERING: INFINITE GRID, DASHED MAGENTA GUIDES & 8 SCALE HANDLES
// ============================================================================

/// Helper that renders a 3D dashed line between `start` and `end` using `Gizmos`.
fn draw_dashed_gizmo_line(
    gizmos: &mut Gizmos,
    start: Vec3,
    end: Vec3,
    dash_length: f32,
    gap_length: f32,
    color: Color,
) {
    let delta = end - start;
    let total_len = delta.length();
    if total_len <= 0.001 {
        return;
    }
    let dir = delta / total_len;
    let step = (dash_length + gap_length).max(0.05);
    let mut dist = 0.0_f32;

    while dist < total_len {
        let seg_start = start + dir * dist;
        let seg_end = start + dir * (dist + dash_length).min(total_len);
        gizmos.line(seg_start, seg_end, color);
        dist += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_infinite_grid_and_tool_gizmos(
    mut gizmos: Gizmos,
    focal_point: Res<FocalPoint>,
    tool_state: Res<ActiveToolState>,
    cursor_ray: Res<CursorRayState>,
    draft: Res<ToolDraftState>,
    move_state: Res<MoveToolState>,
    scale_state: Res<ScaleToolState>,
    selected: Res<SelectedCadObjects>,
    cad_objects: Query<&CadObject>,
) {
    // 1. Camera-following outer infinite X/Y ground grid centered around `FocalPoint`
    let step = 5.0_f32;
    let half_span = 25_i32;
    let center_x = (focal_point.0.x / step).round() * step;
    let center_y = (focal_point.0.y / step).round() * step;
    let max_dist = half_span as f32 * step;
    let outer_grid_color = Color::srgba(0.38, 0.42, 0.48, 0.26);

    for i in -half_span..=half_span {
        let offset = i as f32 * step;
        let x = center_x + offset;
        let y = center_y + offset;

        if y.abs() > 0.01 {
            gizmos.line(
                Vec3::new(center_x - max_dist, y, -0.002),
                Vec3::new(center_x + max_dist, y, -0.002),
                outer_grid_color,
            );
        }
        if x.abs() > 0.01 {
            gizmos.line(
                Vec3::new(x, center_y - max_dist, -0.002),
                Vec3::new(x, center_y + max_dist, -0.002),
                outer_grid_color,
            );
        }
    }

    // 2. Dynamic `FocalPoint` marker
    gizmos.sphere(
        focal_point.0,
        Quat::IDENTITY,
        0.075,
        Color::srgba(0.95, 0.75, 0.25, 0.65),
    );

    // 3. Cursor snap sphere & Infinite Magenta Dashed Parallel Alignment Guides
    if cursor_ray.is_valid_hit {
        gizmos.sphere(
            draft.current_preview_end,
            Quat::IDENTITY,
            0.065,
            cursor_ray.inference.preview_color(),
        );

        match cursor_ray.inference {
            AxisInference::ParallelNeighbor {
                edge_start,
                edge_end,
                direction,
            } => {
                // Highlight the referenced neighbor edge in solid Magenta
                gizmos.line(edge_start, edge_end, INFERENCE_MAGENTA_COLOR);

                // Extend a dashed Magenta guide line from the neighbor's edge and through the viewport cursor
                let guide_span = 45.0_f32;
                let edge_mid = (edge_start + edge_end) * 0.5;
                draw_dashed_gizmo_line(
                    &mut gizmos,
                    edge_mid - direction * guide_span,
                    edge_mid + direction * guide_span,
                    0.35,
                    0.20,
                    INFERENCE_MAGENTA_COLOR,
                );
                draw_dashed_gizmo_line(
                    &mut gizmos,
                    draft.current_preview_end - direction * guide_span,
                    draft.current_preview_end + direction * guide_span,
                    0.35,
                    0.20,
                    INFERENCE_MAGENTA_COLOR,
                );
            }
            AxisInference::FlushTopFace {
                elevation_z,
                face_center,
            } => {
                draw_dashed_gizmo_line(
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

    // 4. Dynamic Preview Line for Continuous Line Chains
    if tool_state.current == CadTool::LineDraw {
        if let Some(start) = draft.active_start_point {
            gizmos.sphere(start, Quat::IDENTITY, 0.075, Color::srgb(1.0, 0.92, 0.30));

            if cursor_ray.is_valid_hit {
                let preview_color = cursor_ray.inference.preview_color();
                gizmos.line(start, draft.current_preview_end, preview_color);

                if cursor_ray.vertical_z_locked || cursor_ray.inference == AxisInference::BlueZ {
                    draw_dashed_gizmo_line(
                        &mut gizmos,
                        Vec3::new(start.x, start.y, -20.0),
                        Vec3::new(start.x, start.y, 40.0),
                        0.30,
                        0.20,
                        AXIS_BLUE_COLOR,
                    );
                }
            }
        }
    }

    // 5. Move Tool Translation Vector & Vertical Blue Axis Guide
    if tool_state.current == CadTool::Move {
        if let Some(ref drag) = move_state.active_drag {
            let current_pos = drag.grab_anchor + drag.current_offset;
            let color = cursor_ray.inference.preview_color();
            gizmos.sphere(drag.grab_anchor, Quat::IDENTITY, 0.08, color);
            draw_dashed_gizmo_line(&mut gizmos, drag.grab_anchor, current_pos, 0.25, 0.15, color);

            if cursor_ray.vertical_z_locked || cursor_ray.inference == AxisInference::BlueZ {
                draw_dashed_gizmo_line(
                    &mut gizmos,
                    Vec3::new(drag.grab_anchor.x, drag.grab_anchor.y, -20.0),
                    Vec3::new(drag.grab_anchor.x, drag.grab_anchor.y, 40.0),
                    0.30,
                    0.20,
                    AXIS_BLUE_COLOR,
                );
            }
        }
    }

    // 6. Scale Tool 3D Bounding Box & 8 Interactive Vertex Control Handles
    if tool_state.current == CadTool::Scale {
        if let Some(&sel_entity) = selected.entities.first() {
            if let Ok(cad_obj) = cad_objects.get(sel_entity) {
                let active_geom = if let Some(ref drag) = scale_state.active_drag {
                    drag.initial_geometry
                        .scaled_from_anchor(drag.opposite_anchor, drag.current_scale_factors)
                } else {
                    cad_obj.geometry.clone()
                };

                let (min_p, max_p) = active_geom.aabb_min_max();
                let bbox_size = (max_p - min_p).max(Vec3::splat(0.10));
                let bbox_center = (min_p + max_p) * 0.5;

                gizmos.cuboid(
                    Transform {
                        translation: bbox_center,
                        rotation: Quat::IDENTITY,
                        scale: bbox_size,
                    },
                    SCALE_BBOX_COLOR,
                );

                let corners = active_geom.corners_8();
                for (idx, &corner_pos) in corners.iter().enumerate() {
                    let is_active = scale_state
                        .active_drag
                        .as_ref()
                        .map(|d| d.handle_index == idx)
                        .unwrap_or(false)
                        || scale_state.hovered_handle == Some(idx);

                    let handle_color = if is_active {
                        SCALE_ACTIVE_HANDLE_COLOR
                    } else {
                        SCALE_HANDLE_COLOR
                    };
                    let handle_size = if is_active { 0.18 } else { 0.13 };

                    gizmos.cuboid(
                        Transform {
                            translation: corner_pos,
                            rotation: Quat::IDENTITY,
                            scale: Vec3::splat(handle_size),
                        },
                        handle_color,
                    );
                }
            }
        }
    }

    // 7. Wireframe 3D Bounding Box Edges for the Block Tool Preview
    if tool_state.current == CadTool::Block && cursor_ray.is_valid_hit {
        if let Some(start_corner) = draft.active_start_point {
            let end_corner = Vec3::new(
                draft.current_preview_end.x,
                draft.current_preview_end.y,
                start_corner.z + draft.default_block_height,
            );
            let min_pt = start_corner.min(end_corner);
            let max_pt = start_corner.max(end_corner);
            let size = Vec3::new(
                (max_pt.x - min_pt.x).max(0.05),
                (max_pt.y - min_pt.y).max(0.05),
                (max_pt.z - min_pt.z).max(0.05),
            );
            let center = (min_pt + max_pt) * 0.5;

            gizmos.cuboid(
                Transform {
                    translation: center,
                    rotation: Quat::IDENTITY,
                    scale: size,
                },
                Color::srgb(0.35, 0.82, 1.0),
            );
        }
    }
}
