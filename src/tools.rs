use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::math::primitives::{Cuboid, InfinitePlane3d};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContexts;

use crate::commands::{
    build_manifold_block_mesh_with_topology, extract_world_triangles_from_bevy_mesh,
    CadGeometryKind, CadObject, CommandHistory, CreateBlockCommand, CreateOffsetCommand,
    DeleteCommand, DrawLineCommand, EditableBlockTopology, GroupId, ManifoldPolygonFace,
    MaterialColor, MoveObjectCommand, PushPullCommand, ScaleObjectCommand, SubFaceRegionKind,
};

pub const AXIS_RED_COLOR: Color = Color::srgb(0.92, 0.20, 0.22);
pub const AXIS_GREEN_COLOR: Color = Color::srgb(0.18, 0.80, 0.34);
pub const AXIS_BLUE_COLOR: Color = Color::srgb(0.20, 0.52, 0.96);
pub const DIAGONAL_YELLOW_COLOR: Color = Color::srgb(1.00, 0.92, 0.12);
pub const INFERENCE_MAGENTA_COLOR: Color = Color::srgb(0.95, 0.22, 0.88);
pub const NEON_BLUE_SELECTION_COLOR: Color = Color::srgb(0.08, 0.85, 1.00);
pub const OFFSET_PREVIEW_COLOR: Color = Color::srgb(1.00, 0.65, 0.12);
pub const PUSH_PULL_HIGHLIGHT_COLOR: Color = Color::srgb(0.15, 0.95, 0.82);
pub const SCALE_GIZMO_COLOR: Color = Color::srgb(0.98, 0.82, 0.18);

const GRID_HALF_EXTENT: i32 = 40;
const GRID_SNAP_STEP: f32 = 1.0_f32;
const SUB_GRID_SNAP_STEP: f32 = 0.25_f32;
const OFFSET_SNAP_STEP: f32 = 0.05_f32;
const EXTRUDE_SNAP_STEP: f32 = 0.10_f32;
const VERTEX_SNAP_RADIUS: f32 = 0.35_f32;
const AXIS_SNAP_COS_THRESHOLD: f32 = 0.990_f32;
const DIAGONAL_SNAP_TOLERANCE_DEG: f32 = 2.0_f32;
const PARALLEL_THRESHOLD_DEG: f32 = 2.0_f32;
const NEIGHBOR_SCAN_RADIUS: f32 = 40.0_f32;
const MAX_PLANE_RAY_DIST: f32 = 85.0_f32;
const DIAGONAL_GUIDE_HALF_SPAN: f32 = 60.0_f32;
const PITCH_LIMIT_RAD: f32 = 85.0_f32 * (std::f32::consts::PI / 180.0_f32);
const COPLANAR_EPSILON: f32 = 0.002_f32;
const MIN_UV_EDGE_LEN: f32 = 0.001_f32;
const MITER_DOT_LIMIT: f32 = -0.98_f32;
const MITER_MAX_MULTIPLIER: f32 = 4.0_f32;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadToolsSet;

pub struct CadToolsPlugin;

impl Plugin for CadToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveToolState>()
            .init_resource::<FocalPoint>()
            .init_resource::<DynamicDrawingPlane>()
            .init_resource::<CursorRayState>()
            .init_resource::<SelectedCadObjects>()
            .init_resource::<AxisInference>()
            .init_resource::<ToolDraftState>()
            .init_resource::<MoveToolState>()
            .init_resource::<ScaleToolState>()
            .init_resource::<OffsetToolState>()
            .init_resource::<PushPullToolState>()
            .init_resource::<VcbInput>()
            .init_resource::<NextGroupId>()
            .add_event::<VcbSubmitEvent>()
            .add_systems(Startup, setup_cad_environment)
            .add_systems(
                Update,
                (
                    handle_global_shortcuts,
                    capture_vcb_keyboard_input,
                    update_orbit_camera,
                    update_cursor_world_projection,
                    handle_select_and_delete_tool,
                    handle_line_tool_interaction,
                    handle_block_tool_interaction,
                    handle_move_tool_interaction,
                    handle_scale_tool_interaction,
                    handle_offset_tool_interaction,
                    handle_push_pull_tool_interaction,
                    process_vcb_commit_requests,
                    sync_block_preview_entity,
                    refresh_selection_materials,
                    render_world_grid_and_axes,
                    render_active_tool_gizmos,
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
    pub fn label(&self) -> &'static str {
        match self {
            Self::Select => "Select (Space)",
            Self::LineDraw => "Line (L)",
            Self::Block => "Block / Box (B)",
            Self::Move => "Move (M)",
            Self::Scale => "Scale (S)",
            Self::Offset => "Offset (F)",
            Self::PushPullExtrude => "Push / Pull (P)",
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.label()
    }
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ActiveToolState {
    pub current: CadTool,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct VcbInput {
    pub buffer: String,
    pub last_status: String,
}

impl VcbInput {
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

#[derive(Event, Debug, Clone)]
pub struct VcbSubmitEvent {
    pub raw_text: String,
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct FocalPoint(pub Vec3);

impl Default for FocalPoint {
    fn default() -> Self {
        Self(Vec3::ZERO)
    }
}

#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicPlaneOrientation {
    #[default]
    HorizontalXY,
    VerticalXZ,
    VerticalYZ,
    CameraFacing,
}

impl DynamicPlaneOrientation {
    pub fn label(&self) -> &'static str {
        match self {
            Self::HorizontalXY => "XY Ground Plane",
            Self::VerticalXZ => "XZ Front Plane",
            Self::VerticalYZ => "YZ Side Plane",
            Self::CameraFacing => "Camera Aligned Plane",
        }
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct DynamicDrawingPlane {
    pub orientation: DynamicPlaneOrientation,
    pub normal: Dir3,
    pub origin: Vec3,
}

impl Default for DynamicDrawingPlane {
    fn default() -> Self {
        Self {
            orientation: DynamicPlaneOrientation::HorizontalXY,
            normal: Dir3::Z,
            origin: Vec3::ZERO,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct OrbitCamera {
    pub radius: f32,
    pub target_radius: f32,
    pub azimuth: f32,
    pub target_azimuth: f32,
    pub pitch: f32,
    pub target_pitch: f32,
    pub target_focal: Vec3,
    pub smoothed_delta: Vec2,
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub zoom_sensitivity: f32,
    pub damping: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            radius: 18.0_f32,
            target_radius: 18.0_f32,
            azimuth: -0.85_f32,
            target_azimuth: -0.85_f32,
            pitch: 0.55_f32,
            target_pitch: 0.55_f32,
            target_focal: Vec3::ZERO,
            smoothed_delta: Vec2::ZERO,
            orbit_sensitivity: 0.0052_f32,
            pan_sensitivity: 0.0018_f32,
            zoom_sensitivity: 1.35_f32,
            damping: 18.0_f32,
        }
    }
}

impl OrbitCamera {
    pub fn compute_eye_position(&self, focal: Vec3) -> Vec3 {
        let clamped_pitch = self.pitch.clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        let horiz = self.radius * clamped_pitch.cos();
        let x = focal.x + horiz * self.azimuth.cos();
        let y = focal.y + horiz * self.azimuth.sin();
        let z = focal.z + self.radius * clamped_pitch.sin();
        Vec3::new(x, y, z)
    }

    pub fn reanchor_pivot(&mut self, current_focal: Vec3, new_pivot: Vec3) {
        let current_eye = self.compute_eye_position(current_focal);
        let offset = current_eye - new_pivot;
        let dist = offset.length().clamp(1.5_f32, 250.0_f32);
        let sin_pitch = (offset.z / dist).clamp(-0.996_f32, 0.996_f32);
        let new_pitch = sin_pitch.asin().clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        let new_azimuth = offset.y.atan2(offset.x);

        self.radius = dist;
        self.target_radius = dist;
        self.pitch = new_pitch;
        self.target_pitch = new_pitch;
        self.azimuth = new_azimuth;
        self.target_azimuth = new_azimuth;
        self.target_focal = new_pivot;
    }
}

#[derive(Resource, Debug, Clone, Copy)]
pub struct CursorRayState {
    pub world_pos: Vec3,
    pub snapped_pos: Vec3,
    pub vertical_z_locked: bool,
    pub plane_label: &'static str,
    pub has_hit: bool,
}

impl Default for CursorRayState {
    fn default() -> Self {
        Self {
            world_pos: Vec3::ZERO,
            snapped_pos: Vec3::ZERO,
            vertical_z_locked: false,
            plane_label: "XY Ground Plane",
            has_hit: false,
        }
    }
}

#[derive(Resource, Debug, Default)]
pub struct SelectedCadObjects {
    pub entities: Vec<Entity>,
}

impl SelectedCadObjects {
    pub fn primary(&self) -> Option<Entity> {
        self.entities.first().copied()
    }

    pub fn select_single(&mut self, entity: Entity) {
        self.entities.clear();
        self.entities.push(entity);
    }

    pub fn clear(&mut self) {
        self.entities.clear();
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.entities.contains(&entity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapAxisKind {
    #[default]
    None,
    RedX,
    GreenY,
    BlueZ,
    Diagonal45,
    ParallelNeighbor,
}

impl SnapAxisKind {
    pub fn color(&self) -> Color {
        match self {
            Self::None => Color::srgb(0.95, 0.88, 0.25),
            Self::RedX => AXIS_RED_COLOR,
            Self::GreenY => AXIS_GREEN_COLOR,
            Self::BlueZ => AXIS_BLUE_COLOR,
            Self::Diagonal45 => DIAGONAL_YELLOW_COLOR,
            Self::ParallelNeighbor => INFERENCE_MAGENTA_COLOR,
        }
    }

    pub fn status_text(&self) -> &'static str {
        match self {
            Self::None => "Free Grid Snap",
            Self::RedX => "On Red Axis (X)",
            Self::GreenY => "On Green Axis (Y)",
            Self::BlueZ => "On Blue Axis (Z)",
            Self::Diagonal45 => "45° Diagonal Snap (Yellow)",
            Self::ParallelNeighbor => "Parallel to Edge (Magenta)",
        }
    }
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AxisInference {
    pub snap_kind: SnapAxisKind,
    pub matched_neighbor_segment: Option<(Vec3, Vec3)>,
    pub diagonal_guide_ray: Option<(Vec3, Vec3)>,
}

#[derive(Resource, Debug)]
pub struct ToolDraftState {
    pub line_chain_start: Option<Vec3>,
    pub line_preview_end: Vec3,
    pub block_first_corner: Option<Vec3>,
    pub block_second_corner: Vec3,
    pub default_block_height: f32,
}

impl Default for ToolDraftState {
    fn default() -> Self {
        Self {
            line_chain_start: None,
            line_preview_end: Vec3::ZERO,
            block_first_corner: None,
            block_second_corner: Vec3::ZERO,
            default_block_height: 2.0_f32,
        }
    }
}

impl ToolDraftState {
    pub fn reset(&mut self) {
        self.line_chain_start = None;
        self.block_first_corner = None;
    }
}

#[derive(Component)]
pub struct BlockPreviewVisual;

#[derive(Debug, Clone)]
pub struct MoveDragSession {
    pub entity: Entity,
    pub start_cursor: Vec3,
    pub current_delta: Vec3,
    pub original_object: CadObject,
}

#[derive(Resource, Debug, Default)]
pub struct MoveToolState {
    pub active_drag: Option<MoveDragSession>,
}

#[derive(Debug, Clone)]
pub struct ScaleDragSession {
    pub entity: Entity,
    pub handle_index: usize,
    pub anchor_corner: Vec3,
    pub initial_handle_pos: Vec3,
    pub current_factors: Vec3,
    pub original_object: CadObject,
}

#[derive(Resource, Debug, Default)]
pub struct ScaleToolState {
    pub active_drag: Option<ScaleDragSession>,
}

#[derive(Debug, Clone)]
pub struct BlockFaceHit {
    pub t: f32,
    pub hit_point: Vec3,
    pub plane_origin: Vec3,
    pub normal: Dir3,
    pub vertices: [Vec3; 4],
}

#[derive(Debug, Clone)]
pub struct OffsetContour {
    pub plane_origin: Vec3,
    pub plane_normal: Dir3,
    pub tangent_u: Vec3,
    pub bitangent_v: Vec3,
    pub vertices_uv: Vec<Vec2>,
    pub winding_sign: f32,
    pub group_id: Option<GroupId>,
    pub snapped_vertex: Option<Vec3>,
}

impl OffsetContour {
    pub fn from_block_face_hit(face_hit: &BlockFaceHit, group_id: Option<GroupId>) -> Option<Self> {
        Self::from_coplanar_loop_3d_with_origin(
            &face_hit.vertices,
            face_hit.plane_origin,
            face_hit.normal,
            group_id,
        )
    }

    pub fn from_coplanar_loop_3d(
        vertices_3d: &[Vec3],
        explicit_normal: Option<Dir3>,
        group_id: Option<GroupId>,
    ) -> Option<Self> {
        if vertices_3d.len() < 3 {
            return None;
        }

        let plane_normal = if let Some(norm) = explicit_normal {
            norm
        } else {
            let mut newell = Vec3::ZERO;
            let count = vertices_3d.len();
            for i in 0..count {
                let curr = vertices_3d[i];
                let next = vertices_3d[(i + 1) % count];
                newell.x += (curr.y - next.y) * (curr.z + next.z);
                newell.y += (curr.z - next.z) * (curr.x + next.x);
                newell.z += (curr.x - next.x) * (curr.y + next.y);
            }
            Dir3::new(newell).ok()?
        };

        let normal_vec = plane_normal.as_vec3();
        let mut centroid = Vec3::ZERO;
        for v in vertices_3d {
            centroid += *v;
        }
        let inv_n = 1.0_f32 / (vertices_3d.len() as f32);
        let mut plane_origin = centroid * inv_n;

        if normal_vec.z.abs() > 0.999_f32 {
            plane_origin.z = vertices_3d[0].z;
        } else if normal_vec.x.abs() > 0.999_f32 {
            plane_origin.x = vertices_3d[0].x;
        } else if normal_vec.y.abs() > 0.999_f32 {
            plane_origin.y = vertices_3d[0].y;
        }

        Self::from_coplanar_loop_3d_with_origin(
            vertices_3d,
            plane_origin,
            plane_normal,
            group_id,
        )
    }

    pub fn from_coplanar_loop_3d_with_origin(
        vertices_3d: &[Vec3],
        plane_origin: Vec3,
        plane_normal: Dir3,
        group_id: Option<GroupId>,
    ) -> Option<Self> {
        if vertices_3d.len() < 3 {
            return None;
        }

        let normal_vec = plane_normal.as_vec3();

        for v in vertices_3d {
            let signed_plane_dist = (*v - plane_origin).dot(normal_vec).abs();
            if signed_plane_dist > COPLANAR_EPSILON * 10.0_f32 {
                return None;
            }
        }

        let (tangent_u, bitangent_v) = build_orthonormal_uv_basis(normal_vec);

        let mut vertices_uv = Vec::with_capacity(vertices_3d.len());
        for v in vertices_3d {
            let rel = *v - plane_origin;
            let u = rel.dot(tangent_u);
            let v_coord = rel.dot(bitangent_v);
            vertices_uv.push(Vec2::new(u, v_coord));
        }

        let signed_area_2x = compute_polygon_signed_area_2x(&vertices_uv);
        if signed_area_2x.abs() < 1e-5_f32 {
            return None;
        }
        let winding_sign = if signed_area_2x >= 0.0_f32 {
            1.0_f32
        } else {
            -1.0_f32
        };

        Some(Self {
            plane_origin,
            plane_normal,
            tangent_u,
            bitangent_v,
            vertices_uv,
            winding_sign,
            group_id,
            snapped_vertex: None,
        })
    }

    pub fn with_vertex_zero_origin(mut self, vertex_3d: Vec3) -> Self {
        let vertex_uv = self.project_world_to_uv(vertex_3d);
        let flush_vertex = self.bind_uv_flush_to_plane(vertex_uv);
        for uv in &mut self.vertices_uv {
            *uv -= vertex_uv;
        }
        self.plane_origin = flush_vertex;
        self.snapped_vertex = Some(flush_vertex);
        self
    }

    pub fn project_world_to_uv(&self, world_pt: Vec3) -> Vec2 {
        let rel = world_pt - self.plane_origin;
        Vec2::new(rel.dot(self.tangent_u), rel.dot(self.bitangent_v))
    }

    pub fn bind_uv_flush_to_plane(&self, uv: Vec2) -> Vec3 {
        let raw_3d = self.plane_origin + self.tangent_u * uv.x + self.bitangent_v * uv.y;
        let n = self.plane_normal.as_vec3();
        let residual = (raw_3d - self.plane_origin).dot(n);
        let mut flush = raw_3d - n * residual;

        if n.z.abs() > 0.999_f32 {
            flush.z = self.plane_origin.z;
        } else if n.x.abs() > 0.999_f32 {
            flush.x = self.plane_origin.x;
        } else if n.y.abs() > 0.999_f32 {
            flush.y = self.plane_origin.y;
        }
        flush
    }

    pub fn perimeter_segments_3d(&self) -> Vec<(Vec3, Vec3)> {
        let count = self.vertices_uv.len();
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let a = self.bind_uv_flush_to_plane(self.vertices_uv[i]);
            let b = self.bind_uv_flush_to_plane(self.vertices_uv[(i + 1) % count]);
            out.push((a, b));
        }
        out
    }

    pub fn vertices_3d(&self) -> Vec<Vec3> {
        self.vertices_uv
            .iter()
            .map(|uv| self.bind_uv_flush_to_plane(*uv))
            .collect()
    }

    pub fn sample_interior_dots_3d(&self) -> Vec<Vec3> {
        if self.vertices_uv.len() < 3 {
            return Vec::new();
        }
        let mut min_uv = Vec2::splat(f32::MAX);
        let mut max_uv = Vec2::splat(f32::MIN);
        for uv in &self.vertices_uv {
            min_uv = min_uv.min(*uv);
            max_uv = max_uv.max(*uv);
        }

        let span = max_uv - min_uv;
        let step_u = (span.x / 6.0_f32).clamp(0.20_f32, 0.80_f32);
        let step_v = (span.y / 6.0_f32).clamp(0.20_f32, 0.80_f32);
        let normal_offset = self.plane_normal.as_vec3() * 0.006_f32;

        let mut dots = Vec::new();
        let mut u = min_uv.x + step_u * 0.5_f32;
        while u < max_uv.x {
            let mut v = min_uv.y + step_v * 0.5_f32;
            while v < max_uv.y {
                let pt_uv = Vec2::new(u, v);
                if is_point_inside_polygon_2d(pt_uv, &self.vertices_uv) {
                    dots.push(self.bind_uv_flush_to_plane(pt_uv) + normal_offset);
                }
                v += step_v;
            }
            u += step_u;
        }
        dots
    }

    pub fn signed_offset_from_cursor(&self, cursor_on_plane_3d: Vec3) -> f32 {
        let cursor_uv = self.project_world_to_uv(cursor_on_plane_3d);
        self.signed_offset_from_uv(cursor_uv)
    }

    pub fn signed_offset_from_uv(&self, cursor_uv: Vec2) -> f32 {
        let count = self.vertices_uv.len();
        if count < 3 {
            return 0.0_f32;
        }

        if self.snapped_vertex.is_some() && cursor_uv.length() < 0.015_f32 {
            return 0.0_f32;
        }

        let mut min_dist = f32::MAX;
        for i in 0..count {
            let a = self.vertices_uv[i];
            let b = self.vertices_uv[(i + 1) % count];
            let d = point_to_segment_dist_2d(cursor_uv, a, b);
            if d < min_dist {
                min_dist = d;
            }
        }

        let inside = is_point_inside_polygon_2d(cursor_uv, &self.vertices_uv);
        if inside {
            -min_dist
        } else {
            min_dist
        }
    }

    pub fn compute_offset_segments(&self, signed_distance: f32) -> Vec<(Vec3, Vec3)> {
        let count = self.vertices_uv.len();
        if count < 3 || signed_distance.abs() < 0.005_f32 {
            return Vec::new();
        }

        let mut shifted_lines: Vec<(Vec2, Vec2)> = Vec::with_capacity(count);
        for i in 0..count {
            let p0 = self.vertices_uv[i];
            let p1 = self.vertices_uv[(i + 1) % count];
            let delta = p1 - p0;
            let len = delta.length();
            if len < MIN_UV_EDGE_LEN {
                return Vec::new();
            }
            let dir = delta / len;
            let outward_normal = Vec2::new(dir.y, -dir.x) * self.winding_sign;
            let shift = outward_normal * signed_distance;
            shifted_lines.push((p0 + shift, dir));
        }

        let mut offset_uv: Vec<Vec2> = Vec::with_capacity(count);
        for i in 0..count {
            let prev_idx = (i + count - 1) % count;
            let (p_prev, d_prev) = shifted_lines[prev_idx];
            let (p_curr, d_curr) = shifted_lines[i];
            let miter_uv = intersect_uv_miter(
                p_prev,
                d_prev,
                p_curr,
                d_curr,
                signed_distance.abs(),
            );
            offset_uv.push(miter_uv);
        }

        if signed_distance < 0.0_f32 {
            let orig_area = compute_polygon_signed_area_2x(&self.vertices_uv);
            let new_area = compute_polygon_signed_area_2x(&offset_uv);
            if orig_area * new_area <= 1e-5_f32 {
                return Vec::new();
            }
            for i in 0..count {
                let orig_edge =
                    self.vertices_uv[(i + 1) % count] - self.vertices_uv[i];
                let new_edge = offset_uv[(i + 1) % count] - offset_uv[i];
                if orig_edge.dot(new_edge) <= 0.0_f32 {
                    return Vec::new();
                }
            }
        }

        let mut segments_3d = Vec::with_capacity(count);
        for i in 0..count {
            let a_3d = self.bind_uv_flush_to_plane(offset_uv[i]);
            let b_3d = self.bind_uv_flush_to_plane(offset_uv[(i + 1) % count]);
            if a_3d.distance(b_3d) >= MIN_UV_EDGE_LEN {
                segments_3d.push((a_3d, b_3d));
            }
        }
        segments_3d
    }

    pub fn base_vertices_4(&self) -> Option<[Vec3; 4]> {
        if self.vertices_uv.len() != 4 {
            return None;
        }
        Some([
            self.bind_uv_flush_to_plane(self.vertices_uv[0]),
            self.bind_uv_flush_to_plane(self.vertices_uv[1]),
            self.bind_uv_flush_to_plane(self.vertices_uv[2]),
            self.bind_uv_flush_to_plane(self.vertices_uv[3]),
        ])
    }

    pub fn compute_offset_vertices_4(&self, signed_distance: f32) -> Option<[Vec3; 4]> {
        let segs = self.compute_offset_segments(signed_distance);
        if segs.len() != 4 {
            return None;
        }
        Some([segs[0].0, segs[1].0, segs[2].0, segs[3].0])
    }

    pub fn sample_annular_dots_3d(&self, inner_hole: &OffsetContour) -> Vec<Vec3> {
        if self.vertices_uv.len() < 3 {
            return Vec::new();
        }
        let mut min_uv = Vec2::splat(f32::MAX);
        let mut max_uv = Vec2::splat(f32::MIN);
        for uv in &self.vertices_uv {
            min_uv = min_uv.min(*uv);
            max_uv = max_uv.max(*uv);
        }

        let span = max_uv - min_uv;
        let step_u = (span.x / 7.0_f32).clamp(0.18_f32, 0.70_f32);
        let step_v = (span.y / 7.0_f32).clamp(0.18_f32, 0.70_f32);
        let normal_offset = self.plane_normal.as_vec3() * 0.006_f32;

        let mut dots = Vec::new();
        let mut u = min_uv.x + step_u * 0.5_f32;
        while u < max_uv.x {
            let mut v = min_uv.y + step_v * 0.5_f32;
            while v < max_uv.y {
                let pt_uv = Vec2::new(u, v);
                if is_point_inside_polygon_2d(pt_uv, &self.vertices_uv) {
                    let world_pt = self.bind_uv_flush_to_plane(pt_uv);
                    let hole_uv = inner_hole.project_world_to_uv(world_pt);
                    if !is_point_inside_polygon_2d(hole_uv, &inner_hole.vertices_uv) {
                        dots.push(world_pt + normal_offset);
                    }
                }
                v += step_v;
            }
            u += step_u;
        }
        dots
    }
}

fn build_orthonormal_uv_basis(normal: Vec3) -> (Vec3, Vec3) {
    let n = normal.normalize_or_zero();
    if n.z.abs() > 0.999_f32 {
        let u = Vec3::X;
        let v = if n.z >= 0.0_f32 { Vec3::Y } else { -Vec3::Y };
        return (u, v);
    }
    let up_ref = Vec3::Z;
    let u = up_ref.cross(n).normalize_or_zero();
    let v = n.cross(u).normalize_or_zero();
    (u, v)
}

fn compute_polygon_signed_area_2x(pts: &[Vec2]) -> f32 {
    let count = pts.len();
    let mut accum = 0.0_f32;
    for i in 0..count {
        let a = pts[i];
        let b = pts[(i + 1) % count];
        accum += a.x * b.y - b.x * a.y;
    }
    accum
}

fn intersect_uv_miter(
    p1: Vec2,
    d1: Vec2,
    p2: Vec2,
    d2: Vec2,
    abs_offset: f32,
) -> Vec2 {
    let cross = d1.x * d2.y - d1.y * d2.x;
    let dot = d1.dot(d2);
    if cross.abs() < 1e-5_f32 || dot < MITER_DOT_LIMIT {
        return (p1 + p2) * 0.5_f32;
    }
    let delta = p2 - p1;
    let t = (delta.x * d2.y - delta.y * d2.x) / cross;
    let candidate = p1 + d1 * t;
    let max_miter = (abs_offset * MITER_MAX_MULTIPLIER).max(0.25_f32);
    if candidate.distance(p2) > max_miter {
        (p1 + p2) * 0.5_f32
    } else {
        candidate
    }
}

fn point_to_segment_dist_2d(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-8_f32 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0_f32, 1.0_f32);
    let proj = a + ab * t;
    p.distance(proj)
}

fn is_point_inside_polygon_2d(pt: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let count = poly.len();
    let mut j = count.saturating_sub(1);
    for i in 0..count {
        let pi = poly[i];
        let pj = poly[j];
        let intersects = ((pi.y > pt.y) != (pj.y > pt.y))
            && (pt.x
                < (pj.x - pi.x) * (pt.y - pi.y) / ((pj.y - pi.y) + 1e-9_f32)
                    + pi.x);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[derive(Resource, Debug, Default)]
pub struct OffsetToolState {
    pub hovered_entity: Option<Entity>,
    pub active_entity: Option<Entity>,
    pub hovered_contour: Option<OffsetContour>,
    pub active_contour: Option<OffsetContour>,
    pub is_dragging: bool,
    pub current_offset_distance: f32,
    pub drag_uv_zero_correction: Vec2,
    pub preview_segments: Vec<(Vec3, Vec3)>,
}

impl OffsetToolState {
    pub fn reset_drag(&mut self) {
        self.active_entity = None;
        self.active_contour = None;
        self.is_dragging = false;
        self.current_offset_distance = 0.0_f32;
        self.drag_uv_zero_correction = Vec2::ZERO;
        self.preview_segments.clear();
    }
}

#[derive(Debug, Clone)]
pub struct ExtrudableFace {
    pub entity: Entity,
    pub is_block_face: bool,
    pub region_kind: SubFaceRegionKind,
    pub original_object: CadObject,
    pub original_topology: EditableBlockTopology,
    pub contour: OffsetContour,
    pub inner_hole_contour: Option<OffsetContour>,
    pub drag_anchor_3d: Vec3,
}

impl ExtrudableFace {
    pub fn build_wall_segments(&self, distance: f32) -> Vec<(Vec3, Vec3)> {
        let base_verts = self.contour.vertices_3d();
        let count = base_verts.len();
        if count < 3 || distance.abs() < 0.005_f32 {
            return Vec::new();
        }
        let shift = self.contour.plane_normal.as_vec3() * distance;
        let mut segments = Vec::with_capacity(count * 3);
        for i in 0..count {
            let b0 = base_verts[i];
            let b1 = base_verts[(i + 1) % count];
            let t0 = b0 + shift;
            let t1 = b1 + shift;
            segments.push((b0, t0));
            segments.push((t0, t1));
        }
        segments
    }
}

#[derive(Resource, Debug, Default)]
pub struct PushPullToolState {
    pub hovered_face: Option<ExtrudableFace>,
    pub active_face: Option<ExtrudableFace>,
    pub last_selected_face: Option<ExtrudableFace>,
    pub is_dragging: bool,
    pub current_extrude_distance: f32,
    pub preview_wall_segments: Vec<(Vec3, Vec3)>,
}

impl PushPullToolState {
    pub fn reset_drag(&mut self) {
        self.active_face = None;
        self.is_dragging = false;
        self.current_extrude_distance = 0.0_f32;
        self.preview_wall_segments.clear();
    }
}

#[derive(Resource, Debug)]
pub struct NextGroupId(pub u64);

impl Default for NextGroupId {
    fn default() -> Self {
        Self(1)
    }
}

impl NextGroupId {
    pub fn allocate(&mut self) -> GroupId {
        let id = GroupId(self.0);
        self.0 = self.0.saturating_add(1);
        id
    }
}

fn setup_cad_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let orbit = OrbitCamera::default();
    let eye = orbit.compute_eye_position(Vec3::ZERO);

    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Z),
            ..default()
        },
        orbit,
    ));

    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            illuminance: 13_500.0_f32,
            shadows_enabled: false,
            color: Color::srgb(1.0, 0.99, 0.96),
            ..default()
        },
        transform: Transform::from_xyz(24.0_f32, -20.0_f32, 32.0_f32)
            .looking_at(Vec3::ZERO, Vec3::Z),
        ..default()
    });

    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            illuminance: 5_500.0_f32,
            shadows_enabled: false,
            color: Color::srgb(0.85, 0.90, 1.0),
            ..default()
        },
        transform: Transform::from_xyz(-22.0_f32, 26.0_f32, 18.0_f32)
            .looking_at(Vec3::ZERO, Vec3::Z),
        ..default()
    });

    let preview_mesh = meshes.add(Cuboid::from_size(Vec3::ONE));
    let preview_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.28, 0.72, 0.98, 0.38),
        alpha_mode: AlphaMode::Blend,
        unlit: false,
        perceptual_roughness: 0.4_f32,
        ..default()
    });

    commands.spawn((
        PbrBundle {
            mesh: preview_mesh,
            material: preview_mat,
            visibility: Visibility::Hidden,
            ..default()
        },
        BlockPreviewVisual,
    ));
}

fn is_egui_consuming_pointer(egui_contexts: &mut EguiContexts) -> bool {
    if let Some(ctx) = egui_contexts.try_ctx_mut() {
        ctx.wants_pointer_input() || ctx.is_pointer_over_area()
    } else {
        false
    }
}

fn is_egui_consuming_keyboard(egui_contexts: &mut EguiContexts) -> bool {
    if let Some(ctx) = egui_contexts.try_ctx_mut() {
        ctx.wants_keyboard_input()
    } else {
        false
    }
}

fn handle_global_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mut active_tool: ResMut<ActiveToolState>,
    mut draft_state: ResMut<ToolDraftState>,
    mut move_state: ResMut<MoveToolState>,
    mut scale_state: ResMut<ScaleToolState>,
    mut offset_state: ResMut<OffsetToolState>,
    mut push_pull_state: ResMut<PushPullToolState>,
    mut vcb: ResMut<VcbInput>,
    mut history: ResMut<CommandHistory>,
    mut exit_events: EventWriter<bevy::app::AppExit>,
    mut cad_query: Query<&mut CadObject>,
    mut topologies: Query<&mut EditableBlockTopology>,
    mut transforms: Query<&mut Transform>,
    mut mesh_handles: Query<&mut Handle<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut egui_contexts: EguiContexts,
) {
    let ctrl_held =
        keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    if ctrl_held && keys.just_pressed(KeyCode::KeyQ) {
        exit_events.send(bevy::app::AppExit::Success);
        return;
    }

    if is_egui_consuming_keyboard(&mut egui_contexts) {
        return;
    }

    if ctrl_held && keys.just_pressed(KeyCode::KeyZ) {
        let shift_held =
            keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        if shift_held {
            history.request_redo();
        } else {
            history.request_undo();
        }
        return;
    }

    if ctrl_held && keys.just_pressed(KeyCode::KeyY) {
        history.request_redo();
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        draft_state.reset();

        if let Some(session) = move_state.active_drag.take() {
            restore_object_geometry(
                session.entity,
                &session.original_object,
                &mut cad_query,
                &mut transforms,
                &mut meshes,
            );
        }

        if let Some(session) = scale_state.active_drag.take() {
            restore_object_geometry(
                session.entity,
                &session.original_object,
                &mut cad_query,
                &mut transforms,
                &mut meshes,
            );
        }

        if let Some(face) = push_pull_state.active_face.take() {
            if face.is_block_face {
                if let Ok(mut topo) = topologies.get_mut(face.entity) {
                    *topo = face.original_topology.clone();
                }
                if let CadGeometryKind::Block { center, size } = face.original_object.geometry {
                    let restored_mesh = build_manifold_block_mesh_with_topology(
                        center,
                        size,
                        &face.original_topology,
                    );
                    let restored_handle = meshes.add(restored_mesh);
                    if let Ok(mut mh) = mesh_handles.get_mut(face.entity) {
                        *mh = restored_handle;
                    }
                }
                restore_object_geometry(
                    face.entity,
                    &face.original_object,
                    &mut cad_query,
                    &mut transforms,
                    &mut meshes,
                );
            }
        }
        push_pull_state.reset_drag();
        offset_state.reset_drag();
        vcb.clear();
        return;
    }

    let previous_tool = active_tool.current;
    if keys.just_pressed(KeyCode::Space) {
        active_tool.current = CadTool::Select;
    } else if keys.just_pressed(KeyCode::KeyL) {
        active_tool.current = CadTool::LineDraw;
    } else if keys.just_pressed(KeyCode::KeyB) {
        active_tool.current = CadTool::Block;
    } else if keys.just_pressed(KeyCode::KeyM) {
        active_tool.current = CadTool::Move;
    } else if keys.just_pressed(KeyCode::KeyS) {
        active_tool.current = CadTool::Scale;
    } else if keys.just_pressed(KeyCode::KeyF) {
        active_tool.current = CadTool::Offset;
    } else if keys.just_pressed(KeyCode::KeyP) {
        active_tool.current = CadTool::PushPullExtrude;
    }

    if active_tool.current != previous_tool {
        draft_state.reset();
        move_state.active_drag = None;
        scale_state.active_drag = None;
        offset_state.reset_drag();
        push_pull_state.reset_drag();
        push_pull_state.hovered_face = None;
        vcb.clear();
    }
}

fn capture_vcb_keyboard_input(
    mut key_events: EventReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut vcb: ResMut<VcbInput>,
    mut submit_writer: EventWriter<VcbSubmitEvent>,
    mut egui_contexts: EguiContexts,
) {
    if is_egui_consuming_keyboard(&mut egui_contexts) {
        return;
    }

    let ctrl_held =
        keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if ctrl_held {
        return;
    }

    for event in key_events.read() {
        if !event.state.is_pressed() {
            continue;
        }

        match &event.logical_key {
            Key::Backspace => {
                vcb.buffer.pop();
            }
            Key::Enter => {
                let trimmed = vcb.buffer.trim().to_string();
                if !trimmed.is_empty() {
                    submit_writer.send(VcbSubmitEvent {
                        raw_text: trimmed.clone(),
                    });
                    vcb.last_status = format!("Committed: {trimmed}");
                    vcb.buffer.clear();
                }
            }
            Key::Character(ch_str) => {
                for ch in ch_str.chars() {
                    let allowed = ch.is_ascii_digit()
                        || ch == '.'
                        || ch == ','
                        || ch == '-'
                        || ch == ';';
                    if allowed && vcb.buffer.len() < 32 {
                        vcb.buffer.push(ch);
                    }
                }
            }
            _ => {}
        }
    }
}

fn update_orbit_camera(
    time: Res<Time>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion_events: EventReader<MouseMotion>,
    mut scroll_events: EventReader<MouseWheel>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    mut focal_point: ResMut<FocalPoint>,
    mut camera_query: Query<(&Camera, &GlobalTransform, &mut Transform, &mut OrbitCamera)>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut egui_contexts: EguiContexts,
) {
    let pointer_busy = is_egui_consuming_pointer(&mut egui_contexts);
    let dt = time.delta_seconds().clamp(0.0005_f32, 0.1_f32);

    let mut raw_delta = Vec2::ZERO;
    for ev in motion_events.read() {
        raw_delta += ev.delta;
    }

    let mut scroll_total = 0.0_f32;
    for ev in scroll_events.read() {
        let step = match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y * 0.02_f32,
        };
        scroll_total += step;
    }

    let Ok((camera, cam_global, mut cam_transform, mut orbit)) =
        camera_query.get_single_mut()
    else {
        return;
    };

    if !pointer_busy && mouse_buttons.just_pressed(MouseButton::Middle) {
        let shift_held =
            keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        if !shift_held {
            if let Ok(window) = window_query.get_single() {
                if let Some(cursor_pos) = window.cursor_position() {
                    if let Some(ray) = camera.viewport_to_world(cam_global, cursor_pos) {
                        if let Some((_entity, hit_point)) =
                            raycast_closest_cad_object_point(ray, &cad_objects)
                        {
                            orbit.reanchor_pivot(focal_point.0, hit_point);
                            focal_point.0 = hit_point;
                        } else if let Some(ground_dist) =
                            ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Z))
                        {
                            if ground_dist > 0.0_f32 && ground_dist < MAX_PLANE_RAY_DIST {
                                let ground_pt = ray.get_point(ground_dist);
                                orbit.reanchor_pivot(focal_point.0, ground_pt);
                                focal_point.0 = ground_pt;
                            }
                        }
                    }
                }
            }
        }
    }

    let smoothing_alpha = (dt * 28.0_f32).clamp(0.0_f32, 1.0_f32);
    orbit.smoothed_delta = orbit.smoothed_delta.lerp(raw_delta, smoothing_alpha);

    if !pointer_busy && mouse_buttons.pressed(MouseButton::Middle) {
        let shift_held =
            keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        if shift_held {
            let right_dir = cam_transform.right().as_vec3();
            let up_dir = cam_transform.up().as_vec3();
            let pan_scale = orbit.radius * orbit.pan_sensitivity;
            let pan_offset = (-right_dir * orbit.smoothed_delta.x
                + up_dir * orbit.smoothed_delta.y)
                * pan_scale;
            orbit.target_focal += pan_offset;
        } else {
            orbit.target_azimuth -= orbit.smoothed_delta.x * orbit.orbit_sensitivity;
            orbit.target_pitch = (orbit.target_pitch
                + orbit.smoothed_delta.y * orbit.orbit_sensitivity)
                .clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
        }
    } else {
        orbit.smoothed_delta = Vec2::ZERO;
    }

    if !pointer_busy && scroll_total.abs() > 0.0001_f32 {
        let zoom_factor = (1.0_f32 - scroll_total * 0.11_f32 * orbit.zoom_sensitivity)
            .clamp(0.75_f32, 1.35_f32);
        orbit.target_radius =
            (orbit.target_radius * zoom_factor).clamp(1.5_f32, 220.0_f32);
    }

    let lerp_t = (1.0_f32 - (-orbit.damping * dt).exp()).clamp(0.0_f32, 1.0_f32);
    orbit.radius += (orbit.target_radius - orbit.radius) * lerp_t;
    orbit.azimuth += (orbit.target_azimuth - orbit.azimuth) * lerp_t;
    orbit.pitch = (orbit.pitch + (orbit.target_pitch - orbit.pitch) * lerp_t)
        .clamp(-PITCH_LIMIT_RAD, PITCH_LIMIT_RAD);
    focal_point.0 = focal_point.0.lerp(orbit.target_focal, lerp_t);

    let eye_pos = orbit.compute_eye_position(focal_point.0);
    *cam_transform = Transform::from_translation(eye_pos).looking_at(focal_point.0, Vec3::Z);
}

fn update_cursor_world_projection(
    keys: Res<ButtonInput<KeyCode>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    focal_point: Res<FocalPoint>,
    active_tool: Res<ActiveToolState>,
    draft_state: Res<ToolDraftState>,
    move_state: Res<MoveToolState>,
    mut drawing_plane: ResMut<DynamicDrawingPlane>,
    mut cursor_state: ResMut<CursorRayState>,
) {
    let Ok(window) = window_query.get_single() else {
        cursor_state.has_hit = false;
        return;
    };
    let Some(cursor_px) = window.cursor_position() else {
        cursor_state.has_hit = false;
        return;
    };
    let Ok((camera, cam_global)) = camera_query.get_single() else {
        cursor_state.has_hit = false;
        return;
    };
    let Some(ray) = camera.viewport_to_world(cam_global, cursor_px) else {
        cursor_state.has_hit = false;
        return;
    };

    let shift_held =
        keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    cursor_state.vertical_z_locked = shift_held;

    let anchor_point: Option<Vec3> = match active_tool.current {
        CadTool::LineDraw => draft_state.line_chain_start,
        CadTool::Block => draft_state.block_first_corner,
        CadTool::Move => move_state.active_drag.as_ref().map(|s| s.start_cursor),
        _ => None,
    };

    if shift_held {
        if let Some(anchor) = anchor_point {
            let cam_forward = cam_global.forward().as_vec3();
            let horiz_normal =
                Vec3::new(cam_forward.x, cam_forward.y, 0.0_f32).normalize_or(Vec3::X);
            if let Ok(plane_dir) = Dir3::new(horiz_normal) {
                if let Some(dist) =
                    ray.intersect_plane(anchor, InfinitePlane3d::new(plane_dir))
                {
                    if dist > 0.0_f32 {
                        let hit = ray.get_point(dist);
                        let raw_vert = Vec3::new(anchor.x, anchor.y, hit.z);
                        let snapped_z = snap_scalar(hit.z, GRID_SNAP_STEP);
                        cursor_state.world_pos = raw_vert;
                        cursor_state.snapped_pos =
                            Vec3::new(anchor.x, anchor.y, snapped_z);
                        cursor_state.plane_label = "Vertical Blue (Z) Lock";
                        cursor_state.has_hit = true;
                        return;
                    }
                }
            }
        }
    }

    let cam_fwd = cam_global.forward().as_vec3();
    let plane_origin = anchor_point.unwrap_or(Vec3::new(
        focal_point.0.x.round(),
        focal_point.0.y.round(),
        0.0_f32,
    ));

    let abs_x = cam_fwd.x.abs();
    let abs_y = cam_fwd.y.abs();
    let abs_z = cam_fwd.z.abs();

    if abs_z >= 0.24_f32 {
        drawing_plane.orientation = DynamicPlaneOrientation::HorizontalXY;
        drawing_plane.normal = Dir3::Z;
        drawing_plane.origin = Vec3::new(plane_origin.x, plane_origin.y, plane_origin.z);
    } else if abs_y >= abs_x {
        drawing_plane.orientation = DynamicPlaneOrientation::VerticalXZ;
        drawing_plane.normal = Dir3::Y;
        drawing_plane.origin = plane_origin;
    } else {
        drawing_plane.orientation = DynamicPlaneOrientation::VerticalYZ;
        drawing_plane.normal = Dir3::X;
        drawing_plane.origin = plane_origin;
    }

    let mut hit_point: Option<Vec3> = None;
    if let Some(dist) = ray.intersect_plane(
        drawing_plane.origin,
        InfinitePlane3d::new(drawing_plane.normal),
    ) {
        if dist > 0.0_f32 && dist <= MAX_PLANE_RAY_DIST {
            hit_point = Some(ray.get_point(dist));
        }
    }

    if hit_point.is_none() {
        if let Ok(cam_dir) = Dir3::new(-cam_fwd) {
            drawing_plane.orientation = DynamicPlaneOrientation::CameraFacing;
            drawing_plane.normal = cam_dir;
            drawing_plane.origin = anchor_point.unwrap_or(focal_point.0);
            if let Some(dist) = ray.intersect_plane(
                drawing_plane.origin,
                InfinitePlane3d::new(drawing_plane.normal),
            ) {
                if dist > 0.0_f32 {
                    hit_point = Some(ray.get_point(dist));
                }
            }
        }
    }

    if let Some(pt) = hit_point {
        cursor_state.world_pos = pt;
        cursor_state.snapped_pos = match drawing_plane.orientation {
            DynamicPlaneOrientation::HorizontalXY => Vec3::new(
                snap_scalar(pt.x, GRID_SNAP_STEP),
                snap_scalar(pt.y, GRID_SNAP_STEP),
                snap_scalar(drawing_plane.origin.z, GRID_SNAP_STEP),
            ),
            DynamicPlaneOrientation::VerticalXZ => Vec3::new(
                snap_scalar(pt.x, GRID_SNAP_STEP),
                snap_scalar(drawing_plane.origin.y, GRID_SNAP_STEP),
                snap_scalar(pt.z, GRID_SNAP_STEP),
            ),
            DynamicPlaneOrientation::VerticalYZ => Vec3::new(
                snap_scalar(drawing_plane.origin.x, GRID_SNAP_STEP),
                snap_scalar(pt.y, GRID_SNAP_STEP),
                snap_scalar(pt.z, GRID_SNAP_STEP),
            ),
            DynamicPlaneOrientation::CameraFacing => Vec3::new(
                snap_scalar(pt.x, GRID_SNAP_STEP),
                snap_scalar(pt.y, GRID_SNAP_STEP),
                snap_scalar(pt.z, GRID_SNAP_STEP),
            ),
        };
        cursor_state.plane_label = drawing_plane.orientation.label();
        cursor_state.has_hit = true;
    } else {
        cursor_state.has_hit = false;
    }
}

fn snap_scalar(value: f32, step: f32) -> f32 {
    if step <= 0.0_f32 {
        return value;
    }
    (value / step).round() * step
}

fn collect_object_edges(obj: &CadObject) -> Vec<(Vec3, Vec3)> {
    match obj.geometry {
        CadGeometryKind::Line { start, end, .. } => vec![(start, end)],
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5_f32;
            let min = center - half;
            let max = center + half;
            let c000 = Vec3::new(min.x, min.y, min.z);
            let c100 = Vec3::new(max.x, min.y, min.z);
            let c110 = Vec3::new(max.x, max.y, min.z);
            let c010 = Vec3::new(min.x, max.y, min.z);
            let c001 = Vec3::new(min.x, min.y, max.z);
            let c101 = Vec3::new(max.x, min.y, max.z);
            let c111 = Vec3::new(max.x, max.y, max.z);
            let c011 = Vec3::new(min.x, max.y, max.z);
            vec![
                (c000, c100),
                (c100, c110),
                (c110, c010),
                (c010, c000),
                (c001, c101),
                (c101, c111),
                (c111, c011),
                (c011, c001),
                (c000, c001),
                (c100, c101),
                (c110, c111),
                (c010, c011),
            ]
        }
    }
}

fn evaluate_diagonal_45_snap(anchor: Vec3, raw_target: Vec3) -> Option<(Vec3, Vec3)> {
    let delta = raw_target - anchor;
    if delta.z.abs() > 0.25_f32 {
        return None;
    }
    let horiz = Vec2::new(delta.x, delta.y);
    let horiz_len = horiz.length();
    if horiz_len < 0.12_f32 {
        return None;
    }

    let angle_deg = horiz.y.atan2(horiz.x).to_degrees().rem_euclid(360.0_f32);
    let target_angles_deg = [45.0_f32, 135.0_f32, 225.0_f32, 315.0_f32];

    for target_deg in target_angles_deg {
        if (angle_deg - target_deg).abs() <= DIAGONAL_SNAP_TOLERANCE_DEG {
            let rad = target_deg.to_radians();
            let diag_dir = Vec3::new(rad.cos(), rad.sin(), 0.0_f32).normalize_or_zero();
            let proj_len =
                snap_scalar(delta.dot(diag_dir), SUB_GRID_SNAP_STEP).max(SUB_GRID_SNAP_STEP);
            let snapped_pt = anchor + diag_dir * proj_len;
            return Some((snapped_pt, diag_dir));
        }
    }

    None
}

fn evaluate_axis_and_neighbor_snapping(
    anchor: Vec3,
    raw_target: Vec3,
    grid_snapped_target: Vec3,
    vertical_locked: bool,
    enable_diagonal_45: bool,
    excluded_entity: Option<Entity>,
    cad_objects: &Query<(Entity, &CadObject)>,
) -> (
    Vec3,
    SnapAxisKind,
    Option<(Vec3, Vec3)>,
    Option<(Vec3, Vec3)>,
) {
    if vertical_locked {
        let z_snapped = snap_scalar(raw_target.z, SUB_GRID_SNAP_STEP);
        return (
            Vec3::new(anchor.x, anchor.y, z_snapped),
            SnapAxisKind::BlueZ,
            None,
            None,
        );
    }

    let delta = raw_target - anchor;
    let dist = delta.length();
    if dist < 0.08_f32 {
        return (grid_snapped_target, SnapAxisKind::None, None, None);
    }

    let dir = delta / dist;
    let snapped_len = snap_scalar(dist, GRID_SNAP_STEP).max(GRID_SNAP_STEP);

    if dir.x.abs() >= AXIS_SNAP_COS_THRESHOLD {
        let sign = dir.x.signum();
        return (
            Vec3::new(anchor.x + sign * snapped_len, anchor.y, anchor.z),
            SnapAxisKind::RedX,
            None,
            None,
        );
    }
    if dir.y.abs() >= AXIS_SNAP_COS_THRESHOLD {
        let sign = dir.y.signum();
        return (
            Vec3::new(anchor.x, anchor.y + sign * snapped_len, anchor.z),
            SnapAxisKind::GreenY,
            None,
            None,
        );
    }
    if dir.z.abs() >= AXIS_SNAP_COS_THRESHOLD {
        let sign = dir.z.signum();
        return (
            Vec3::new(anchor.x, anchor.y, anchor.z + sign * snapped_len),
            SnapAxisKind::BlueZ,
            None,
            None,
        );
    }

    if enable_diagonal_45 {
        if let Some((diag_pt, diag_dir)) = evaluate_diagonal_45_snap(anchor, raw_target) {
            return (
                diag_pt,
                SnapAxisKind::Diagonal45,
                None,
                Some((anchor, diag_dir)),
            );
        }
    }

    let cos_2deg = PARALLEL_THRESHOLD_DEG.to_radians().cos();
    let mut best_dot = cos_2deg;
    let mut best_dir: Option<Vec3> = None;
    let mut best_edge: Option<(Vec3, Vec3)> = None;

    for (entity, obj) in cad_objects.iter() {
        if Some(entity) == excluded_entity {
            continue;
        }
        for (edge_a, edge_b) in collect_object_edges(obj) {
            let mid = (edge_a + edge_b) * 0.5_f32;
            if mid.distance(anchor) > NEIGHBOR_SCAN_RADIUS {
                continue;
            }
            let edge_vec = edge_b - edge_a;
            let edge_len = edge_vec.length();
            if edge_len < 0.05_f32 {
                continue;
            }
            let edge_dir = edge_vec / edge_len;

            let is_primary_axis = edge_dir.x.abs() > 0.999_f32
                || edge_dir.y.abs() > 0.999_f32
                || edge_dir.z.abs() > 0.999_f32;
            if is_primary_axis {
                continue;
            }

            let alignment = dir.dot(edge_dir);
            if alignment.abs() >= best_dot {
                best_dot = alignment.abs();
                let oriented_dir = if alignment >= 0.0_f32 {
                    edge_dir
                } else {
                    -edge_dir
                };
                best_dir = Some(oriented_dir);
                best_edge = Some((edge_a, edge_b));
            }
        }
    }

    if let (Some(aligned_dir), Some(neighbor_seg)) = (best_dir, best_edge) {
        let projected_len = snap_scalar(delta.dot(aligned_dir), SUB_GRID_SNAP_STEP)
            .max(SUB_GRID_SNAP_STEP);
        let snapped_pt = anchor + aligned_dir * projected_len;
        return (
            snapped_pt,
            SnapAxisKind::ParallelNeighbor,
            Some(neighbor_seg),
            None,
        );
    }

    (grid_snapped_target, SnapAxisKind::None, None, None)
}

fn handle_select_and_delete_tool(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut selected: ResMut<SelectedCadObjects>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if !is_egui_consuming_keyboard(&mut egui_contexts)
        && (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
    {
        if !selected.entities.is_empty() {
            let mut to_delete = Vec::new();
            for ent in selected.entities.drain(..) {
                if let Ok((e, obj)) = cad_objects.get(ent) {
                    to_delete.push((e, obj.clone()));
                }
            }
            if !to_delete.is_empty() {
                history.execute_command(Box::new(DeleteCommand::new(to_delete)));
                return;
            }
        }
    }

    if active_tool.current != CadTool::Select {
        return;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) {
        let Ok(window) = window_query.get_single() else {
            return;
        };
        let Some(cursor_pos) = window.cursor_position() else {
            return;
        };
        let Ok((camera, cam_global)) = camera_query.get_single() else {
            return;
        };
        let Some(ray) = camera.viewport_to_world(cam_global, cursor_pos) else {
            return;
        };

        if let Some((hit_ent, _)) = raycast_closest_cad_object_point(ray, &cad_objects) {
            selected.select_single(hit_ent);
        } else {
            selected.clear();
        }
    }
}

fn handle_line_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    cursor_state: Res<CursorRayState>,
    cad_objects: Query<(Entity, &CadObject)>,
    mut draft_state: ResMut<ToolDraftState>,
    mut inference: ResMut<AxisInference>,
    mut next_group: ResMut<NextGroupId>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::LineDraw {
        return;
    }

    if !cursor_state.has_hit {
        return;
    }

    if let Some(start_pt) = draft_state.line_chain_start {
        let (snapped_end, snap_kind, matched_seg, diag_ray) =
            evaluate_axis_and_neighbor_snapping(
                start_pt,
                cursor_state.world_pos,
                cursor_state.snapped_pos,
                cursor_state.vertical_z_locked,
                true,
                None,
                &cad_objects,
            );
        draft_state.line_preview_end = snapped_end;
        inference.snap_kind = snap_kind;
        inference.matched_neighbor_segment = matched_seg;
        inference.diagonal_guide_ray = diag_ray;
    } else {
        draft_state.line_preview_end = cursor_state.snapped_pos;
        inference.snap_kind = SnapAxisKind::None;
        inference.matched_neighbor_segment = None;
        inference.diagonal_guide_ray = None;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) {
        if let Some(start_pt) = draft_state.line_chain_start {
            let end_pt = draft_state.line_preview_end;
            if start_pt.distance(end_pt) > 0.05_f32 {
                let group_id = next_group.allocate();
                let line_color = match inference.snap_kind {
                    SnapAxisKind::RedX => AXIS_RED_COLOR,
                    SnapAxisKind::GreenY => AXIS_GREEN_COLOR,
                    SnapAxisKind::BlueZ => AXIS_BLUE_COLOR,
                    SnapAxisKind::Diagonal45 => DIAGONAL_YELLOW_COLOR,
                    SnapAxisKind::ParallelNeighbor => INFERENCE_MAGENTA_COLOR,
                    SnapAxisKind::None => Color::srgb(0.90, 0.92, 0.95),
                };
                history.execute_command(Box::new(DrawLineCommand::new(
                    start_pt,
                    end_pt,
                    line_color,
                    Some(group_id),
                )));
                draft_state.line_chain_start = Some(end_pt);
            }
        } else {
            draft_state.line_chain_start = Some(cursor_state.snapped_pos);
            draft_state.line_preview_end = cursor_state.snapped_pos;
        }
    }
}

fn handle_block_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    cursor_state: Res<CursorRayState>,
    mut draft_state: ResMut<ToolDraftState>,
    mut next_group: ResMut<NextGroupId>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::Block {
        return;
    }

    if draft_state.default_block_height <= 0.01_f32 {
        draft_state.default_block_height = 2.0_f32;
    }

    if cursor_state.has_hit {
        draft_state.block_second_corner = cursor_state.snapped_pos;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) || !cursor_state.has_hit {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) {
        if let Some(first_pt) = draft_state.block_first_corner {
            let mut second_pt = draft_state.block_second_corner;
            if (second_pt.x - first_pt.x).abs() < 0.25_f32 {
                second_pt.x = first_pt.x + 1.0_f32;
            }
            if (second_pt.y - first_pt.y).abs() < 0.25_f32 {
                second_pt.y = first_pt.y + 1.0_f32;
            }
            second_pt.z = first_pt.z + draft_state.default_block_height.max(0.25_f32);
            let group_id = next_group.allocate();
            history.execute_command(Box::new(CreateBlockCommand::from_corners(
                first_pt,
                second_pt,
                Color::srgb(0.76, 0.80, 0.86),
                Some(group_id),
            )));
            draft_state.block_first_corner = None;
        } else {
            draft_state.block_first_corner = Some(cursor_state.snapped_pos);
            draft_state.block_second_corner = cursor_state.snapped_pos;
        }
    }
}

fn sync_block_preview_entity(
    active_tool: Res<ActiveToolState>,
    draft_state: Res<ToolDraftState>,
    push_pull_state: Res<PushPullToolState>,
    mut preview_query: Query<(&mut Transform, &mut Visibility), With<BlockPreviewVisual>>,
) {
    let Ok((mut tf, mut vis)) = preview_query.get_single_mut() else {
        return;
    };

    if active_tool.current == CadTool::Block {
        if let Some(first) = draft_state.block_first_corner {
            let second = draft_state.block_second_corner;
            let height = draft_state.default_block_height.max(0.25_f32);
            let min_x = first.x.min(second.x);
            let max_x = first.x.max(second.x).max(min_x + 0.5_f32);
            let min_y = first.y.min(second.y);
            let max_y = first.y.max(second.y).max(min_y + 0.5_f32);
            let base_z = first.z.min(second.z);

            let size = Vec3::new(max_x - min_x, max_y - min_y, height);
            let center = Vec3::new(
                (min_x + max_x) * 0.5_f32,
                (min_y + max_y) * 0.5_f32,
                base_z + height * 0.5_f32,
            );
            tf.translation = center;
            tf.scale = size;
            *vis = Visibility::Visible;
            return;
        }
    }

    if active_tool.current == CadTool::PushPullExtrude && push_pull_state.is_dragging {
        if let Some(ref face) = push_pull_state.active_face {
            if !face.is_block_face && push_pull_state.current_extrude_distance.abs() >= 0.02_f32 {
                let shift =
                    face.contour.plane_normal.as_vec3() * push_pull_state.current_extrude_distance;
                let mut min_pt = Vec3::splat(f32::MAX);
                let mut max_pt = Vec3::splat(f32::MIN);
                for v in face.contour.vertices_3d() {
                    let t = v + shift;
                    min_pt = min_pt.min(v).min(t);
                    max_pt = max_pt.max(v).max(t);
                }
                tf.translation = (min_pt + max_pt) * 0.5_f32;
                tf.scale = (max_pt - min_pt).max(Vec3::splat(0.05_f32));
                *vis = Visibility::Visible;
                return;
            }
        }
    }

    *vis = Visibility::Hidden;
}

fn handle_move_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    cursor_state: Res<CursorRayState>,
    mut selected: ResMut<SelectedCadObjects>,
    mut move_state: ResMut<MoveToolState>,
    mut inference: ResMut<AxisInference>,
    mut cad_objects: Query<(Entity, &mut CadObject)>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::Move {
        return;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) && move_state.active_drag.is_none() {
        let Ok(window) = window_query.get_single() else {
            return;
        };
        let Some(cursor_pos) = window.cursor_position() else {
            return;
        };
        let Ok((camera, cam_global)) = camera_query.get_single() else {
            return;
        };
        let Some(ray) = camera.viewport_to_world(cam_global, cursor_pos) else {
            return;
        };

        let mut hit_entity: Option<Entity> = None;
        let mut closest_t = f32::MAX;
        for (entity, obj) in cad_objects.iter() {
            if let Some(t) = ray_intersects_cad_object(ray, obj) {
                if t < closest_t {
                    closest_t = t;
                    hit_entity = Some(entity);
                }
            }
        }

        if let Some(ent) = hit_entity {
            if let Ok((_, obj)) = cad_objects.get(ent) {
                selected.select_single(ent);
                move_state.active_drag = Some(MoveDragSession {
                    entity: ent,
                    start_cursor: cursor_state.snapped_pos,
                    current_delta: Vec3::ZERO,
                    original_object: obj.clone(),
                });
            }
        }
    }

    if mouse_buttons.pressed(MouseButton::Left) {
        if let Some(session_snapshot) = move_state.active_drag.clone() {
            if cursor_state.has_hit {
                let immutable_snapshot: Vec<(Entity, CadObject)> = cad_objects
                    .iter()
                    .map(|(e, o)| (e, o.clone()))
                    .collect();

                let (snapped_pt, snap_kind, neighbor_seg) =
                    evaluate_axis_and_neighbor_from_slice(
                        session_snapshot.start_cursor,
                        cursor_state.world_pos,
                        cursor_state.snapped_pos,
                        cursor_state.vertical_z_locked,
                        Some(session_snapshot.entity),
                        &immutable_snapshot,
                    );

                let delta = snapped_pt - session_snapshot.start_cursor;
                if let Some(ref mut active) = move_state.active_drag {
                    active.current_delta = delta;
                }
                inference.snap_kind = snap_kind;
                inference.matched_neighbor_segment = neighbor_seg;
                inference.diagonal_guide_ray = None;

                let mut preview_obj = session_snapshot.original_object.clone();
                preview_obj.geometry = preview_obj.geometry.translated(delta);
                apply_live_object_preview(
                    session_snapshot.entity,
                    &preview_obj,
                    &mut cad_objects,
                    &mut transforms,
                    &mut meshes,
                );
            }
        }
    }

    if mouse_buttons.just_released(MouseButton::Left) {
        if let Some(session) = move_state.active_drag.take() {
            let final_delta = session.current_delta;
            let before_geom = session.original_object.geometry;
            let after_geom = before_geom.translated(final_delta);

            apply_live_object_preview(
                session.entity,
                &session.original_object,
                &mut cad_objects,
                &mut transforms,
                &mut meshes,
            );

            if final_delta.length() > 0.01_f32 {
                history.execute_command(Box::new(MoveObjectCommand::new(
                    session.entity,
                    before_geom,
                    after_geom,
                )));
            }
        }
    }
}

fn evaluate_axis_and_neighbor_from_slice(
    anchor: Vec3,
    raw_target: Vec3,
    grid_snapped_target: Vec3,
    vertical_locked: bool,
    excluded_entity: Option<Entity>,
    objects: &[(Entity, CadObject)],
) -> (Vec3, SnapAxisKind, Option<(Vec3, Vec3)>) {
    if vertical_locked {
        let z_snapped = snap_scalar(raw_target.z, SUB_GRID_SNAP_STEP);
        return (
            Vec3::new(anchor.x, anchor.y, z_snapped),
            SnapAxisKind::BlueZ,
            None,
        );
    }

    let delta = raw_target - anchor;
    let dist = delta.length();
    if dist < 0.08_f32 {
        return (grid_snapped_target, SnapAxisKind::None, None);
    }

    let dir = delta / dist;
    let snapped_len = snap_scalar(dist, GRID_SNAP_STEP).max(GRID_SNAP_STEP);

    if dir.x.abs() >= AXIS_SNAP_COS_THRESHOLD {
        return (
            Vec3::new(anchor.x + dir.x.signum() * snapped_len, anchor.y, anchor.z),
            SnapAxisKind::RedX,
            None,
        );
    }
    if dir.y.abs() >= AXIS_SNAP_COS_THRESHOLD {
        return (
            Vec3::new(anchor.x, anchor.y + dir.y.signum() * snapped_len, anchor.z),
            SnapAxisKind::GreenY,
            None,
        );
    }
    if dir.z.abs() >= AXIS_SNAP_COS_THRESHOLD {
        return (
            Vec3::new(anchor.x, anchor.y, anchor.z + dir.z.signum() * snapped_len),
            SnapAxisKind::BlueZ,
            None,
        );
    }

    let cos_2deg = PARALLEL_THRESHOLD_DEG.to_radians().cos();
    let mut best_dot = cos_2deg;
    let mut best_dir: Option<Vec3> = None;
    let mut best_edge: Option<(Vec3, Vec3)> = None;

    for (entity, obj) in objects {
        if Some(*entity) == excluded_entity {
            continue;
        }
        for (edge_a, edge_b) in collect_object_edges(obj) {
            let mid = (edge_a + edge_b) * 0.5_f32;
            if mid.distance(anchor) > NEIGHBOR_SCAN_RADIUS {
                continue;
            }
            let edge_vec = edge_b - edge_a;
            let edge_len = edge_vec.length();
            if edge_len < 0.05_f32 {
                continue;
            }
            let edge_dir = edge_vec / edge_len;
            let is_primary_axis = edge_dir.x.abs() > 0.999_f32
                || edge_dir.y.abs() > 0.999_f32
                || edge_dir.z.abs() > 0.999_f32;
            if is_primary_axis {
                continue;
            }
            let alignment = dir.dot(edge_dir);
            if alignment.abs() >= best_dot {
                best_dot = alignment.abs();
                best_dir = Some(if alignment >= 0.0_f32 {
                    edge_dir
                } else {
                    -edge_dir
                });
                best_edge = Some((edge_a, edge_b));
            }
        }
    }

    if let (Some(aligned_dir), Some(seg)) = (best_dir, best_edge) {
        let projected_len = snap_scalar(delta.dot(aligned_dir), SUB_GRID_SNAP_STEP)
            .max(SUB_GRID_SNAP_STEP);
        return (
            anchor + aligned_dir * projected_len,
            SnapAxisKind::ParallelNeighbor,
            Some(seg),
        );
    }

    (grid_snapped_target, SnapAxisKind::None, None)
}

fn handle_scale_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    cursor_state: Res<CursorRayState>,
    mut selected: ResMut<SelectedCadObjects>,
    mut scale_state: ResMut<ScaleToolState>,
    mut cad_objects: Query<(Entity, &mut CadObject)>,
    mut transforms: Query<&mut Transform>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::Scale {
        return;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    if mouse_buttons.just_pressed(MouseButton::Left) && scale_state.active_drag.is_none() {
        let Ok(window) = window_query.get_single() else {
            return;
        };
        let Some(cursor_pos) = window.cursor_position() else {
            return;
        };
        let Ok((camera, cam_global)) = camera_query.get_single() else {
            return;
        };
        let Some(ray) = camera.viewport_to_world(cam_global, cursor_pos) else {
            return;
        };

        if let Some(selected_ent) = selected.primary() {
            if let Ok((_, selected_obj)) = cad_objects.get(selected_ent) {
                let corners = compute_object_bbox_corners(selected_obj);
                if let Some((handle_idx, handle_pos)) = pick_bbox_handle(ray, &corners) {
                    let opposite_idx = 7_usize.saturating_sub(handle_idx);
                    scale_state.active_drag = Some(ScaleDragSession {
                        entity: selected_ent,
                        handle_index: handle_idx,
                        anchor_corner: corners[opposite_idx],
                        initial_handle_pos: handle_pos,
                        current_factors: Vec3::ONE,
                        original_object: selected_obj.clone(),
                    });
                    return;
                }
            }
        }

        let mut hit_entity: Option<Entity> = None;
        let mut closest_t = f32::MAX;
        for (ent, obj) in cad_objects.iter() {
            if let Some(t) = ray_intersects_cad_object(ray, obj) {
                if t < closest_t {
                    closest_t = t;
                    hit_entity = Some(ent);
                }
            }
        }
        if let Some(ent) = hit_entity {
            selected.select_single(ent);
        } else {
            selected.clear();
        }
    }

    if mouse_buttons.pressed(MouseButton::Left) {
        if let Some(session_snapshot) = scale_state.active_drag.clone() {
            if cursor_state.has_hit {
                let anchor = session_snapshot.anchor_corner;
                let init_span = session_snapshot.initial_handle_pos - anchor;
                let curr_span = cursor_state.snapped_pos - anchor;

                let sx = if init_span.x.abs() > 0.1_f32 {
                    (curr_span.x / init_span.x).abs().clamp(0.1_f32, 20.0_f32)
                } else {
                    1.0_f32
                };
                let sy = if init_span.y.abs() > 0.1_f32 {
                    (curr_span.y / init_span.y).abs().clamp(0.1_f32, 20.0_f32)
                } else {
                    1.0_f32
                };
                let sz = if init_span.z.abs() > 0.1_f32 {
                    (curr_span.z / init_span.z).abs().clamp(0.1_f32, 20.0_f32)
                } else {
                    1.0_f32
                };

                let factors = Vec3::new(sx, sy, sz);
                if let Some(ref mut active) = scale_state.active_drag {
                    let _ = active.handle_index;
                    active.current_factors = factors;
                }

                let mut preview_obj = session_snapshot.original_object.clone();
                preview_obj.geometry = preview_obj
                    .geometry
                    .scaled_from_anchor(anchor, factors);
                apply_live_object_preview(
                    session_snapshot.entity,
                    &preview_obj,
                    &mut cad_objects,
                    &mut transforms,
                    &mut meshes,
                );
            }
        }
    }

    if mouse_buttons.just_released(MouseButton::Left) {
        if let Some(session) = scale_state.active_drag.take() {
            let factors = session.current_factors;
            let before_geom = session.original_object.geometry;
            let after_geom = before_geom.scaled_from_anchor(session.anchor_corner, factors);

            apply_live_object_preview(
                session.entity,
                &session.original_object,
                &mut cad_objects,
                &mut transforms,
                &mut meshes,
            );

            if (factors - Vec3::ONE).length() > 0.01_f32 {
                history.execute_command(Box::new(ScaleObjectCommand::new(
                    session.entity,
                    before_geom,
                    after_geom,
                )));
            }
        }
    }
}

pub fn compute_object_bbox_corners(obj: &CadObject) -> [Vec3; 8] {
    let (min, max) = match obj.geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let pad = Vec3::splat(radius.max(0.15_f32));
            (start.min(end) - pad, start.max(end) + pad)
        }
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5_f32;
            (center - half, center + half)
        }
    };
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

fn pick_bbox_handle(ray: Ray3d, corners: &[Vec3; 8]) -> Option<(usize, Vec3)> {
    let mut best_idx: Option<usize> = None;
    let mut best_t = f32::MAX;
    for (idx, corner) in corners.iter().enumerate() {
        if let Some(t) = ray_intersects_sphere(ray, *corner, 0.28_f32) {
            if t < best_t {
                best_t = t;
                best_idx = Some(idx);
            }
        }
    }
    best_idx.map(|i| (i, corners[i]))
}

pub fn raycast_block_face_exact(
    ray: Ray3d,
    center: Vec3,
    size: Vec3,
) -> Option<BlockFaceHit> {
    let half = size * 0.5_f32;
    let min = center - half;
    let max = center + half;
    let ray_dir = ray.direction.as_vec3();

    let candidate_faces: [(Dir3, Vec3, Vec3, Vec3, f32, f32, [Vec3; 4]); 6] = [
        (
            Dir3::Z,
            Vec3::new(center.x, center.y, max.z),
            Vec3::X,
            Vec3::Y,
            half.x,
            half.y,
            [
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Z,
            Vec3::new(center.x, center.y, min.z),
            Vec3::X,
            Vec3::NEG_Y,
            half.x,
            half.y,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, min.y, min.z),
            ],
        ),
        (
            Dir3::X,
            Vec3::new(max.x, center.y, center.z),
            Vec3::Y,
            Vec3::Z,
            half.y,
            half.z,
            [
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(max.x, min.y, max.z),
            ],
        ),
        (
            Dir3::NEG_X,
            Vec3::new(min.x, center.y, center.z),
            Vec3::NEG_Y,
            Vec3::Z,
            half.y,
            half.z,
            [
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::Y,
            Vec3::new(center.x, max.y, center.z),
            Vec3::NEG_X,
            Vec3::Z,
            half.x,
            half.z,
            [
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, max.y, max.z),
                Vec3::new(max.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Y,
            Vec3::new(center.x, min.y, center.z),
            Vec3::X,
            Vec3::Z,
            half.x,
            half.z,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
        ),
    ];

    let mut best_hit: Option<BlockFaceHit> = None;

    for (normal, plane_origin, u_axis, v_axis, half_u, half_v, vertices) in candidate_faces {
        let denom = ray_dir.dot(normal.as_vec3());
        if denom >= -1e-6_f32 {
            continue;
        }
        let t = (plane_origin - ray.origin).dot(normal.as_vec3()) / denom;
        if t <= 0.001_f32 {
            continue;
        }
        let hit_pt = ray.get_point(t);
        let rel = hit_pt - plane_origin;
        let u_dist = rel.dot(u_axis).abs();
        let v_dist = rel.dot(v_axis).abs();

        if u_dist <= half_u + 1e-3_f32 && v_dist <= half_v + 1e-3_f32 {
            let replace = match &best_hit {
                Some(prev) => t < prev.t,
                None => true,
            };
            if replace {
                best_hit = Some(BlockFaceHit {
                    t,
                    hit_point: hit_pt,
                    plane_origin,
                    normal,
                    vertices,
                });
            }
        }
    }

    best_hit
}

fn enumerate_block_faces(
    center: Vec3,
    size: Vec3,
) -> [(Dir3, Vec3, Vec3, Vec3, f32, f32, [Vec3; 4]); 6] {
    let half = size * 0.5_f32;
    let min = center - half;
    let max = center + half;
    [
        (
            Dir3::Z,
            Vec3::new(center.x, center.y, max.z),
            Vec3::X,
            Vec3::Y,
            half.x,
            half.y,
            [
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Z,
            Vec3::new(center.x, center.y, min.z),
            Vec3::X,
            Vec3::NEG_Y,
            half.x,
            half.y,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, min.y, min.z),
            ],
        ),
        (
            Dir3::X,
            Vec3::new(max.x, center.y, center.z),
            Vec3::Y,
            Vec3::Z,
            half.y,
            half.z,
            [
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(max.x, min.y, max.z),
            ],
        ),
        (
            Dir3::NEG_X,
            Vec3::new(min.x, center.y, center.z),
            Vec3::NEG_Y,
            Vec3::Z,
            half.y,
            half.z,
            [
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::Y,
            Vec3::new(center.x, max.y, center.z),
            Vec3::NEG_X,
            Vec3::Z,
            half.x,
            half.z,
            [
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, max.y, max.z),
                Vec3::new(max.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Y,
            Vec3::new(center.x, min.y, center.z),
            Vec3::X,
            Vec3::Z,
            half.x,
            half.z,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
        ),
    ]
}

fn raycast_triangle_moller_trumbore(
    ray: Ray3d,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
) -> Option<f32> {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let dir = ray.direction.as_vec3();
    let h = dir.cross(edge2);
    let a = edge1.dot(h);
    if a.abs() < 1e-7_f32 {
        return None;
    }
    let f = 1.0_f32 / a;
    let s = ray.origin - v0;
    let u = f * s.dot(h);
    if !(0.0_f32..=1.0_f32).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = f * dir.dot(q);
    if v < 0.0_f32 || u + v > 1.0_f32 {
        return None;
    }
    let t = f * edge2.dot(q);
    if t > 0.001_f32 {
        Some(t)
    } else {
        None
    }
}

fn quantize_uv_key(uv: Vec2) -> (i64, i64) {
    let scale = 1000.0_f32;
    ((uv.x * scale).round() as i64, (uv.y * scale).round() as i64)
}

fn simplify_collinear_polygon_2d(loop_uv: &[Vec2]) -> Vec<Vec2> {
    let n = loop_uv.len();
    if n <= 3 {
        return loop_uv.to_vec();
    }
    let mut simplified = Vec::with_capacity(n);
    for i in 0..n {
        let prev = loop_uv[(i + n - 1) % n];
        let curr = loop_uv[i];
        let next = loop_uv[(i + 1) % n];
        let d1 = (curr - prev).normalize_or_zero();
        let d2 = (next - curr).normalize_or_zero();
        if d1.dot(d2) < 0.9992_f32 {
            simplified.push(curr);
        }
    }
    if simplified.len() >= 3 {
        simplified
    } else {
        loop_uv.to_vec()
    }
}

pub fn reconstruct_coplanar_outer_boundary_loop(
    coplanar_triangles: &[(Vec3, Vec3, Vec3)],
    plane_origin: Vec3,
    plane_normal: Dir3,
    group_id: Option<GroupId>,
) -> Option<OffsetContour> {
    if coplanar_triangles.is_empty() {
        return None;
    }

    let n_vec = plane_normal.as_vec3();
    let (tangent_u, bitangent_v) = build_orthonormal_uv_basis(n_vec);

    let project_uv = |pt: Vec3| -> Vec2 {
        let rel = pt - plane_origin;
        Vec2::new(rel.dot(tangent_u), rel.dot(bitangent_v))
    };

    let mut key_to_uv: std::collections::HashMap<(i64, i64), Vec2> =
        std::collections::HashMap::new();
    let mut directed_edges: Vec<((i64, i64), (i64, i64))> =
        Vec::with_capacity(coplanar_triangles.len() * 3);

    for (a, b, c) in coplanar_triangles {
        let mut ua = project_uv(*a);
        let mut ub = project_uv(*b);
        let mut uc = project_uv(*c);

        let area_2x = (ub.x - ua.x) * (uc.y - ua.y) - (ub.y - ua.y) * (uc.x - ua.x);
        if area_2x.abs() < 1e-6_f32 {
            continue;
        }
        if area_2x < 0.0_f32 {
            std::mem::swap(&mut ub, &mut uc);
        }

        let ka = quantize_uv_key(ua);
        let kb = quantize_uv_key(ub);
        let kc = quantize_uv_key(uc);

        key_to_uv.entry(ka).or_insert(ua);
        key_to_uv.entry(kb).or_insert(ub);
        key_to_uv.entry(kc).or_insert(uc);

        if ka != kb {
            directed_edges.push((ka, kb));
        }
        if kb != kc {
            directed_edges.push((kb, kc));
        }
        if kc != ka {
            directed_edges.push((kc, ka));
        }
    }

    let mut edge_set: std::collections::HashSet<((i64, i64), (i64, i64))> =
        std::collections::HashSet::with_capacity(directed_edges.len());
    for e in &directed_edges {
        edge_set.insert(*e);
    }

    // Cancel all interior shared twin half-edges (u -> v where v -> u also exists)
    let mut boundary_edges: Vec<((i64, i64), (i64, i64))> = Vec::new();
    for &(u, v) in &directed_edges {
        if !edge_set.contains(&(v, u)) {
            boundary_edges.push((u, v));
        }
    }

    if boundary_edges.len() < 3 {
        return None;
    }

    let mut used = vec![false; boundary_edges.len()];
    let mut best_loop_uv: Option<Vec<Vec2>> = None;
    let mut best_abs_area = 0.0_f32;

    for start_idx in 0..boundary_edges.len() {
        if used[start_idx] {
            continue;
        }
        used[start_idx] = true;
        let (seed_u, mut curr_v) = boundary_edges[start_idx];
        let mut chain_keys = vec![seed_u, curr_v];

        for _ in 0..boundary_edges.len() {
            if curr_v == seed_u {
                break;
            }
            let mut found_next = None;
            for (idx, &(u, v)) in boundary_edges.iter().enumerate() {
                if !used[idx] && u == curr_v {
                    used[idx] = true;
                    found_next = Some(v);
                    break;
                }
            }
            if let Some(next_k) = found_next {
                curr_v = next_k;
                if curr_v != seed_u {
                    chain_keys.push(curr_v);
                }
            } else {
                break;
            }
        }

        if curr_v == seed_u && chain_keys.len() >= 3 {
            let raw_uv: Vec<Vec2> = chain_keys
                .iter()
                .filter_map(|k| key_to_uv.get(k).copied())
                .collect();
            let simplified_uv = simplify_collinear_polygon_2d(&raw_uv);
            if simplified_uv.len() >= 3 {
                let area = compute_polygon_signed_area_2x(&simplified_uv).abs();
                if area > best_abs_area {
                    best_abs_area = area;
                    best_loop_uv = Some(simplified_uv);
                }
            }
        }
    }

    let outer_uv = best_loop_uv?;
    let world_loop: Vec<Vec3> = outer_uv
        .iter()
        .map(|uv| plane_origin + tangent_u * uv.x + bitangent_v * uv.y)
        .collect();

    OffsetContour::from_coplanar_loop_3d_with_origin(
        &world_loop,
        plane_origin,
        plane_normal,
        group_id,
    )
}

pub fn detect_offset_contour_for_entity(
    ray: Ray3d,
    entity: Entity,
    obj: &CadObject,
    transform: &Transform,
    mesh: Option<&Mesh>,
    topology: Option<&EditableBlockTopology>,
    all_line_segments: &[(Vec3, Vec3)],
) -> Option<(f32, OffsetContour)> {
    match obj.geometry {
        CadGeometryKind::Block { center, size } => {
            // 1. First check if the entity has a live Bevy Mesh and raycast its actual transformed triangles
            if let Some(bevy_mesh) = mesh {
                let tris = extract_world_triangles_from_bevy_mesh(bevy_mesh, transform);
                let ray_dir = ray.direction.as_vec3();

                let mut closest_t = f32::MAX;
                let mut hit_tri_idx: Option<usize> = None;

                for (idx, (a, b, c, n)) in tris.iter().enumerate() {
                    if ray_dir.dot(*n) >= -1e-6_f32 {
                        continue;
                    }
                    if let Some(t) = raycast_triangle_moller_trumbore(ray, *a, *b, *c) {
                        if t < closest_t {
                            closest_t = t;
                            hit_tri_idx = Some(idx);
                        }
                    }
                }

                if let Some(seed_idx) = hit_tri_idx {
                    let hit_point = ray.get_point(closest_t);
                    let seed_normal_vec = tris[seed_idx].3;
                    if let Ok(hit_normal) = Dir3::new(seed_normal_vec) {
                        // If EditableBlockTopology has an exact sub-face (e.g. InnerLoop vs OuterFrame on the same plane),
                        // check if the hit point lands directly inside a specific ManifoldPolygonFace without holes first.
                        if let Some(topo) = topology {
                            let scale_aware_tf = *transform;
                            let is_unit_scale =
                                (scale_aware_tf.scale - Vec3::ONE).length() < 0.005_f32;
                            if is_unit_scale {
                                for face in &topo.faces {
                                    if face.normal.as_vec3().dot(seed_normal_vec) > 0.99_f32
                                        && face.inner_holes.is_empty()
                                    {
                                        if let Some(c) = OffsetContour::from_coplanar_loop_3d(
                                            &face.outer_loop,
                                            Some(face.normal),
                                            obj.group_id,
                                        ) {
                                            let plane_dist = (hit_point - c.plane_origin)
                                                .dot(seed_normal_vec)
                                                .abs();
                                            if plane_dist <= 0.008_f32 {
                                                let uv = c.project_world_to_uv(hit_point);
                                                if is_point_inside_polygon_2d(uv, &c.vertices_uv) {
                                                    return Some((closest_t, c));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Filter all triangles in the live Mesh that lie strictly on the same 3D infinite plane
                        let plane_tol = 0.006_f32;
                        let mut coplanar_indices: Vec<usize> = Vec::new();
                        for (idx, (a, b, c, n)) in tris.iter().enumerate() {
                            if n.dot(seed_normal_vec) < 0.992_f32 {
                                continue;
                            }
                            let da = (*a - hit_point).dot(seed_normal_vec).abs();
                            let db = (*b - hit_point).dot(seed_normal_vec).abs();
                            let dc = (*c - hit_point).dot(seed_normal_vec).abs();
                            if da <= plane_tol && db <= plane_tol && dc <= plane_tol {
                                coplanar_indices.push(idx);
                            }
                        }

                        // Flood-fill connected coplanar triangles starting from seed_idx
                        let quant3 = |p: Vec3| -> (i64, i64, i64) {
                            (
                                (p.x * 500.0_f32).round() as i64,
                                (p.y * 500.0_f32).round() as i64,
                                (p.z * 500.0_f32).round() as i64,
                            )
                        };

                        let mut connected_tris: Vec<(Vec3, Vec3, Vec3)> = Vec::new();
                        let mut visited = vec![false; coplanar_indices.len()];
                        let mut active_verts: std::collections::HashSet<(i64, i64, i64)> =
                            std::collections::HashSet::new();

                        let (sa, sb, sc, _) = tris[seed_idx];
                        active_verts.insert(quant3(sa));
                        active_verts.insert(quant3(sb));
                        active_verts.insert(quant3(sc));

                        loop {
                            let mut added_any = false;
                            for (pos, &tri_i) in coplanar_indices.iter().enumerate() {
                                if visited[pos] {
                                    continue;
                                }
                                let (a, b, c, _) = tris[tri_i];
                                let ka = quant3(a);
                                let kb = quant3(b);
                                let kc = quant3(c);
                                let shared_count = usize::from(active_verts.contains(&ka))
                                    + usize::from(active_verts.contains(&kb))
                                    + usize::from(active_verts.contains(&kc));
                                if shared_count >= 2 || tri_i == seed_idx {
                                    visited[pos] = true;
                                    active_verts.insert(ka);
                                    active_verts.insert(kb);
                                    active_verts.insert(kc);
                                    connected_tris.push((a, b, c));
                                    added_any = true;
                                }
                            }
                            if !added_any {
                                break;
                            }
                        }

                        if let Some(contour) = reconstruct_coplanar_outer_boundary_loop(
                            &connected_tris,
                            hit_point,
                            hit_normal,
                            obj.group_id,
                        ) {
                            return Some((closest_t, contour));
                        }
                    }
                }
            }

            // Fallback to analytical box face raycast if Mesh asset is not yet ready
            let scaled_size = size * transform.scale.abs();
            if let Some(face_hit) = raycast_block_face_exact(ray, transform.translation, scaled_size)
            {
                let contour = OffsetContour::from_block_face_hit(&face_hit, obj.group_id)?;
                return Some((face_hit.t, contour));
            }
            let _ = entity;
            None
        }
        CadGeometryKind::Line { start, end, radius } => {
            let pick_radius = (radius * 2.5_f32).max(0.18_f32);
            let t = ray_intersects_segment_capsule(ray, start, end, pick_radius)?;
            let ordered_loop =
                assemble_closed_loop_from_segments(start, end, all_line_segments)?;
            let contour =
                OffsetContour::from_coplanar_loop_3d(&ordered_loop, None, obj.group_id)?;
            Some((t, contour))
        }
    }
}

fn collect_object_snap_vertices_world(
    obj: &CadObject,
    transform: &Transform,
    mesh: Option<&Mesh>,
) -> Vec<Vec3> {
    if let Some(bevy_mesh) = mesh {
        if matches!(obj.geometry, CadGeometryKind::Block { .. }) {
            let tris = extract_world_triangles_from_bevy_mesh(bevy_mesh, transform);
            let mut unique: Vec<Vec3> = Vec::new();
            for (a, b, c, _) in tris {
                for v in [a, b, c, (a + b) * 0.5_f32, (b + c) * 0.5_f32, (c + a) * 0.5_f32] {
                    if !unique.iter().any(|u| u.distance(v) < 0.01_f32) {
                        unique.push(v);
                    }
                }
            }
            if !unique.is_empty() {
                return unique;
            }
        }
    }

    match obj.geometry {
        CadGeometryKind::Line { start, end, .. } => {
            vec![start, end, (start + end) * 0.5_f32]
        }
        CadGeometryKind::Block { .. } => {
            let corners = compute_object_bbox_corners(obj);
            let edges = collect_object_edges(obj);
            let mut pts = Vec::with_capacity(corners.len() + edges.len());
            pts.extend_from_slice(&corners);
            for (a, b) in edges {
                pts.push((a + b) * 0.5_f32);
            }
            pts
        }
    }
}

fn resolve_vertex_parent_face_contour(
    ray: Ray3d,
    cad_objects: &Query<(Entity, &CadObject)>,
    transforms: &Query<&Transform>,
    mesh_handles: &Query<&Handle<Mesh>>,
    topologies: &Query<&EditableBlockTopology>,
    meshes: &Assets<Mesh>,
    all_lines: &[(Vec3, Vec3)],
) -> Option<(Entity, OffsetContour, f32)> {
    let mut closest_vertex_t = f32::MAX;
    let mut matched_entity: Option<Entity> = None;
    let mut matched_obj: Option<CadObject> = None;
    let mut snapped_vertex_3d = Vec3::ZERO;

    for (entity, obj) in cad_objects.iter() {
        let tf = transforms.get(entity).copied().unwrap_or_default();
        let mesh_opt = mesh_handles.get(entity).ok().and_then(|h| meshes.get(h));
        for v_pt in collect_object_snap_vertices_world(obj, &tf, mesh_opt) {
            if let Some(t) = ray_intersects_sphere(ray, v_pt, VERTEX_SNAP_RADIUS) {
                if t < closest_vertex_t {
                    closest_vertex_t = t;
                    matched_entity = Some(entity);
                    matched_obj = Some(obj.clone());
                    snapped_vertex_3d = v_pt;
                }
            }
        }
    }

    let parent_ent = matched_entity?;
    let parent_obj = matched_obj?;
    let tf = transforms.get(parent_ent).copied().unwrap_or_default();
    let mesh_opt = mesh_handles
        .get(parent_ent)
        .ok()
        .and_then(|h| meshes.get(h));
    let topo_opt = topologies.get(parent_ent).ok();

    if let Some((_t, direct_contour)) = detect_offset_contour_for_entity(
        ray,
        parent_ent,
        &parent_obj,
        &tf,
        mesh_opt,
        topo_opt,
        all_lines,
    ) {
        let dist_to_plane = (snapped_vertex_3d - direct_contour.plane_origin)
            .dot(direct_contour.plane_normal.as_vec3())
            .abs();
        if dist_to_plane <= 0.04_f32 {
            return Some((
                parent_ent,
                direct_contour.with_vertex_zero_origin(snapped_vertex_3d),
                closest_vertex_t,
            ));
        }
    }

    if let Some(bevy_mesh) = mesh_opt {
        let tris = extract_world_triangles_from_bevy_mesh(bevy_mesh, &tf);
        let ray_dir = ray.direction.as_vec3();
        let mut best_score = f32::NEG_INFINITY;
        let mut best_normal: Option<Dir3> = None;

        for (a, b, c, n) in &tris {
            let touches = a.distance(snapped_vertex_3d) < 0.03_f32
                || b.distance(snapped_vertex_3d) < 0.03_f32
                || c.distance(snapped_vertex_3d) < 0.03_f32
                || ((*a + *b) * 0.5_f32).distance(snapped_vertex_3d) < 0.03_f32
                || ((*b + *c) * 0.5_f32).distance(snapped_vertex_3d) < 0.03_f32
                || ((*c + *a) * 0.5_f32).distance(snapped_vertex_3d) < 0.03_f32;
            if !touches {
                continue;
            }
            let facing = -ray_dir.dot(*n);
            if facing <= 0.01_f32 {
                continue;
            }
            let horiz_bonus = if n.z > 0.9_f32 { 0.20_f32 } else { 0.0_f32 };
            let score = facing + horiz_bonus;
            if score > best_score {
                best_score = score;
                best_normal = Dir3::new(*n).ok();
            }
        }

        if let Some(chosen_normal) = best_normal {
            let n_vec = chosen_normal.as_vec3();
            let coplanar_tris: Vec<(Vec3, Vec3, Vec3)> = tris
                .into_iter()
                .filter(|(a, b, c, n)| {
                    n.dot(n_vec) > 0.992_f32
                        && (*a - snapped_vertex_3d).dot(n_vec).abs() <= 0.008_f32
                        && (*b - snapped_vertex_3d).dot(n_vec).abs() <= 0.008_f32
                        && (*c - snapped_vertex_3d).dot(n_vec).abs() <= 0.008_f32
                })
                .map(|(a, b, c, _)| (a, b, c))
                .collect();

            if let Some(contour) = reconstruct_coplanar_outer_boundary_loop(
                &coplanar_tris,
                snapped_vertex_3d,
                chosen_normal,
                parent_obj.group_id,
            ) {
                return Some((
                    parent_ent,
                    contour.with_vertex_zero_origin(snapped_vertex_3d),
                    closest_vertex_t,
                ));
            }
        }
    }

    None
}

fn detect_hovered_coplanar_contour(
    ray: Ray3d,
    cad_objects: &Query<(Entity, &CadObject)>,
    transforms: &Query<&Transform>,
    mesh_handles: &Query<&Handle<Mesh>>,
    topologies: &Query<&EditableBlockTopology>,
    meshes: &Assets<Mesh>,
) -> Option<(Entity, OffsetContour)> {
    let all_lines: Vec<(Vec3, Vec3)> = cad_objects
        .iter()
        .filter_map(|(_, o)| match o.geometry {
            CadGeometryKind::Line { start, end, .. } => Some((start, end)),
            CadGeometryKind::Block { .. } => None,
        })
        .collect();

    let vertex_resolved = resolve_vertex_parent_face_contour(
        ray,
        cad_objects,
        transforms,
        mesh_handles,
        topologies,
        meshes,
        &all_lines,
    );

    let mut best_surface_t = f32::MAX;
    let mut best_surface_contour: Option<(Entity, OffsetContour)> = None;

    for (entity, obj) in cad_objects.iter() {
        let tf = transforms.get(entity).copied().unwrap_or_default();
        let mesh_opt = mesh_handles.get(entity).ok().and_then(|h| meshes.get(h));
        let topo_opt = topologies.get(entity).ok();

        if let Some((t, contour)) = detect_offset_contour_for_entity(
            ray,
            entity,
            obj,
            &tf,
            mesh_opt,
            topo_opt,
            &all_lines,
        ) {
            if t < best_surface_t {
                best_surface_t = t;
                best_surface_contour = Some((entity, contour));
            }
        }
    }

    if let Some((v_ent, v_contour, v_t)) = vertex_resolved {
        if v_t <= best_surface_t + VERTEX_SNAP_RADIUS {
            return Some((v_ent, v_contour));
        }
    }

    best_surface_contour
}

fn handle_offset_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    cad_objects: Query<(Entity, &CadObject)>,
    transforms: Query<&Transform>,
    mesh_handles: Query<&Handle<Mesh>>,
    topologies: Query<&EditableBlockTopology>,
    meshes: Res<Assets<Mesh>>,
    mut selected: ResMut<SelectedCadObjects>,
    mut offset_state: ResMut<OffsetToolState>,
    mut vcb: ResMut<VcbInput>,
    mut next_group: ResMut<NextGroupId>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::Offset {
        if offset_state.is_dragging
            || offset_state.hovered_contour.is_some()
            || !offset_state.preview_segments.is_empty()
        {
            offset_state.hovered_contour = None;
            offset_state.reset_drag();
        }
        return;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    let Ok(window) = window_query.get_single() else {
        return;
    };
    let Ok((camera, cam_global)) = camera_query.get_single() else {
        return;
    };

    let cursor_ray = window
        .cursor_position()
        .and_then(|pos| camera.viewport_to_world(cam_global, pos));

    if !offset_state.is_dragging {
        offset_state.hovered_entity = None;
        offset_state.hovered_contour = None;
        if let Some(ray) = cursor_ray {
            if let Some((hit_ent, contour)) = detect_hovered_coplanar_contour(
                ray,
                &cad_objects,
                &transforms,
                &mesh_handles,
                &topologies,
                &meshes,
            ) {
                offset_state.hovered_entity = Some(hit_ent);
                offset_state.hovered_contour = Some(contour.clone());
                if mouse_buttons.just_pressed(MouseButton::Left) {
                    selected.select_single(hit_ent);
                    let uv_zero_correction = if contour.snapped_vertex.is_some() {
                        intersect_ray_with_plane(
                            ray,
                            contour.plane_origin,
                            contour.plane_normal,
                            MAX_PLANE_RAY_DIST * 2.0_f32,
                        )
                        .map(|hit_3d| -contour.project_world_to_uv(hit_3d))
                        .unwrap_or(Vec2::ZERO)
                    } else {
                        Vec2::ZERO
                    };
                    offset_state.active_entity = Some(hit_ent);
                    offset_state.active_contour = Some(contour);
                    offset_state.is_dragging = true;
                    offset_state.current_offset_distance = 0.0_f32;
                    offset_state.drag_uv_zero_correction = uv_zero_correction;
                    offset_state.preview_segments.clear();
                    if vcb.buffer.is_empty() {
                        vcb.last_status = "Offset: 0.00 m".to_string();
                    }
                }
            }
        }
        return;
    }

    if mouse_buttons.pressed(MouseButton::Left) {
        if let Some(ray) = cursor_ray {
            let uv_correction = offset_state.drag_uv_zero_correction;
            let computed_preview = if let Some(ref contour) = offset_state.active_contour {
                intersect_ray_with_plane(
                    ray,
                    contour.plane_origin,
                    contour.plane_normal,
                    MAX_PLANE_RAY_DIST * 2.0_f32,
                )
                .map(|hit_3d| {
                    let adjusted_uv = contour.project_world_to_uv(hit_3d) + uv_correction;
                    let raw_offset = contour.signed_offset_from_uv(adjusted_uv);
                    let snapped_offset =
                        (raw_offset / OFFSET_SNAP_STEP).round() * OFFSET_SNAP_STEP;
                    let segments = contour.compute_offset_segments(snapped_offset);
                    (snapped_offset, segments)
                })
            } else {
                None
            };

            if let Some((snapped_offset, segments)) = computed_preview {
                offset_state.current_offset_distance = snapped_offset;
                offset_state.preview_segments = segments;
                if vcb.buffer.is_empty() {
                    vcb.last_status = format!("Offset: {:+.2} m", snapped_offset);
                }
            }
        }
        return;
    }

    if mouse_buttons.just_released(MouseButton::Left) {
        let final_segments = offset_state.preview_segments.clone();
        let dist = offset_state.current_offset_distance;
        let target_ent = offset_state.active_entity;
        let active_contour = offset_state.active_contour.clone();
        let group_opt = active_contour
            .as_ref()
            .and_then(|c| c.group_id)
            .or_else(|| Some(next_group.allocate()));
        offset_state.reset_drag();

        if dist.abs() >= 0.02_f32 && final_segments.len() >= 3 {
            if let (Some(ent), Some(contour)) = (target_ent, active_contour) {
                let is_block = cad_objects
                    .get(ent)
                    .map(|(_, o)| matches!(o.geometry, CadGeometryKind::Block { .. }))
                    .unwrap_or(false);
                if is_block {
                    if let (Some(base_v4), Some(offset_v4)) = (
                        contour.base_vertices_4(),
                        contour.compute_offset_vertices_4(dist),
                    ) {
                        history.execute_command(Box::new(CreateOffsetCommand::for_block_face(
                            ent,
                            contour.plane_normal,
                            base_v4,
                            offset_v4,
                            dist,
                            Color::srgb(0.95, 0.78, 0.22),
                            group_opt,
                        )));
                        return;
                    }
                }
            }

            history.execute_command(Box::new(CreateOffsetCommand::new(
                final_segments,
                Color::srgb(0.95, 0.78, 0.22),
                group_opt,
            )));
        }
    }
}

fn intersect_ray_with_plane(
    ray: Ray3d,
    plane_origin: Vec3,
    plane_normal: Dir3,
    max_dist: f32,
) -> Option<Vec3> {
    if let Some(t) = ray.intersect_plane(plane_origin, InfinitePlane3d::new(plane_normal)) {
        if t > 0.0_f32 && t <= max_dist {
            return Some(ray.get_point(t));
        }
    }
    if let Ok(flipped) = Dir3::new(-plane_normal.as_vec3()) {
        if let Some(t) = ray.intersect_plane(plane_origin, InfinitePlane3d::new(flipped)) {
            if t > 0.0_f32 && t <= max_dist {
                return Some(ray.get_point(t));
            }
        }
    }
    None
}

fn detect_offset_contour_for_object(
    hit_obj: &CadObject,
    all_line_segments: &[(Vec3, Vec3)],
) -> Option<OffsetContour> {
    match hit_obj.geometry {
        CadGeometryKind::Block { center, size } => {
            let half = size * 0.5_f32;
            let min = center - half;
            let max = center + half;
            let top_hit = BlockFaceHit {
                t: 1.0_f32,
                hit_point: Vec3::new(center.x, center.y, max.z),
                plane_origin: Vec3::new(center.x, center.y, max.z),
                normal: Dir3::Z,
                vertices: [
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
            };
            OffsetContour::from_block_face_hit(&top_hit, hit_obj.group_id)
        }
        CadGeometryKind::Line { start, end, .. } => {
            let ordered_loop = assemble_closed_loop_from_segments(start, end, all_line_segments)?;
            OffsetContour::from_coplanar_loop_3d(&ordered_loop, None, hit_obj.group_id)
        }
    }
}

fn assemble_closed_loop_from_segments(
    seed_start: Vec3,
    seed_end: Vec3,
    segments: &[(Vec3, Vec3)],
) -> Option<Vec<Vec3>> {
    let eps = 0.08_f32;
    let mut used = vec![false; segments.len()];
    for (idx, (s, e)) in segments.iter().enumerate() {
        if (s.distance(seed_start) < eps && e.distance(seed_end) < eps)
            || (s.distance(seed_end) < eps && e.distance(seed_start) < eps)
        {
            used[idx] = true;
            break;
        }
    }

    let mut chain = vec![seed_start, seed_end];
    for _ in 0..32 {
        let current_tip = *chain.last()?;
        if chain.len() >= 3 && current_tip.distance(seed_start) < eps {
            chain.pop();
            return Some(chain);
        }

        let mut found_next: Option<Vec3> = None;
        for (idx, (s, e)) in segments.iter().enumerate() {
            if used[idx] {
                continue;
            }
            if s.distance(current_tip) < eps {
                used[idx] = true;
                found_next = Some(*e);
                break;
            } else if e.distance(current_tip) < eps {
                used[idx] = true;
                found_next = Some(*s);
                break;
            }
        }

        if let Some(next_pt) = found_next {
            chain.push(next_pt);
        } else {
            break;
        }
    }

    None
}

fn raycast_manifold_polygon_face(
    ray: Ray3d,
    face: &ManifoldPolygonFace,
    group_id: Option<GroupId>,
) -> Option<(
    f32,
    Vec3,
    SubFaceRegionKind,
    OffsetContour,
    Option<OffsetContour>,
)> {
    let normal = face.normal;
    if ray.direction.as_vec3().dot(normal.as_vec3()) >= -1e-6_f32 {
        return None;
    }

    let outer_contour =
        OffsetContour::from_coplanar_loop_3d(&face.outer_loop, Some(normal), group_id)?;

    let hit_pt = intersect_ray_with_plane(
        ray,
        outer_contour.plane_origin,
        normal,
        MAX_PLANE_RAY_DIST * 2.0_f32,
    )?;

    let uv_out = outer_contour.project_world_to_uv(hit_pt);
    if !is_point_inside_polygon_2d(uv_out, &outer_contour.vertices_uv) {
        return None;
    }

    if let Some(last_hole) = face.inner_holes.last() {
        let hole_contour =
            OffsetContour::from_coplanar_loop_3d(last_hole, Some(normal), group_id)?;
        let uv_hole = hole_contour.project_world_to_uv(hit_pt);
        if is_point_inside_polygon_2d(uv_hole, &hole_contour.vertices_uv) {
            return None;
        }
        let t = ray.origin.distance(hit_pt);
        return Some((
            t,
            hit_pt,
            SubFaceRegionKind::OuterFrame,
            outer_contour,
            Some(hole_contour),
        ));
    }

    let t = ray.origin.distance(hit_pt);
    Some((
        t,
        hit_pt,
        SubFaceRegionKind::InnerLoop,
        outer_contour,
        None,
    ))
}

fn detect_hovered_extrudable_face(
    ray: Ray3d,
    cad_objects: &Query<(Entity, &mut CadObject)>,
    topologies: &Query<&mut EditableBlockTopology>,
) -> Option<ExtrudableFace> {
    let mut all_lines: Vec<(Vec3, Vec3)> = Vec::new();
    for (_ent, obj) in cad_objects.iter() {
        if let CadGeometryKind::Line { start, end, .. } = obj.geometry {
            all_lines.push((start, end));
        }
    }

    let mut best_block_t = f32::MAX;
    let mut best_block_face: Option<ExtrudableFace> = None;

    for (entity, obj) in cad_objects.iter() {
        if let CadGeometryKind::Block { center, size } = obj.geometry {
            let mut topo = topologies.get(entity).cloned().unwrap_or_default();
            topo.ensure_initialized(center, size);

            for face_poly in &topo.faces {
                if let Some((t, hit_pt, region_kind, contour, hole_contour)) =
                    raycast_manifold_polygon_face(ray, face_poly, obj.group_id)
                {
                    if t < best_block_t {
                        best_block_t = t;
                        let classified_kind = if topo.faces.len() <= 6
                            && face_poly.inner_holes.is_empty()
                        {
                            SubFaceRegionKind::WholeFace
                        } else {
                            region_kind
                        };
                        best_block_face = Some(ExtrudableFace {
                            entity,
                            is_block_face: true,
                            region_kind: classified_kind,
                            original_object: obj.clone(),
                            original_topology: topo.clone(),
                            contour,
                            inner_hole_contour: hole_contour,
                            drag_anchor_3d: hit_pt,
                        });
                    }
                }
            }
        }
    }

    let mut best_loop_t = f32::MAX;
    let mut best_loop_face: Option<ExtrudableFace> = None;

    for (entity, obj) in cad_objects.iter() {
        if let CadGeometryKind::Line { start, end, .. } = obj.geometry {
            if let Some(ordered_loop) =
                assemble_closed_loop_from_segments(start, end, &all_lines)
            {
                if let Some(contour) =
                    OffsetContour::from_coplanar_loop_3d(&ordered_loop, None, obj.group_id)
                {
                    if let Some(hit_pt) = intersect_ray_with_plane(
                        ray,
                        contour.plane_origin,
                        contour.plane_normal,
                        MAX_PLANE_RAY_DIST * 2.0_f32,
                    ) {
                        let uv = contour.project_world_to_uv(hit_pt);
                        if is_point_inside_polygon_2d(uv, &contour.vertices_uv) {
                            let t = ray.origin.distance(hit_pt);
                            if t < best_loop_t {
                                best_loop_t = t;
                                best_loop_face = Some(ExtrudableFace {
                                    entity,
                                    is_block_face: false,
                                    region_kind: SubFaceRegionKind::WholeFace,
                                    original_object: obj.clone(),
                                    original_topology: EditableBlockTopology::default(),
                                    contour,
                                    inner_hole_contour: None,
                                    drag_anchor_3d: hit_pt,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    match (best_block_face, best_loop_face) {
        (Some(block_f), Some(loop_f)) => {
            if best_loop_t + 0.02_f32 < best_block_t {
                Some(loop_f)
            } else {
                Some(block_f)
            }
        }
        (Some(block_f), None) => Some(block_f),
        (None, Some(loop_f)) => Some(loop_f),
        (None, None) => None,
    }
}

fn compute_extruded_block_state_for_face(
    face: &ExtrudableFace,
    distance: f32,
) -> (CadGeometryKind, EditableBlockTopology) {
    let normal = face.contour.plane_normal;
    let mut next_geom = face.original_object.geometry;
    let mut next_topo = face.original_topology.clone();

    if let CadGeometryKind::Block { center, size } = next_geom {
        next_topo.ensure_initialized(center, size);
    }

    if let Some(target_v4) = face.contour.base_vertices_4() {
        next_topo.extrude_face_region(normal, target_v4, face.region_kind, distance);
    }

    if let Some((new_center, new_size)) = next_topo.compute_bounds() {
        next_geom = CadGeometryKind::Block {
            center: new_center,
            size: new_size,
        };
    } else {
        next_geom = next_geom.extruded_along_face(normal, distance);
    }

    (next_geom, next_topo)
}

fn apply_live_manifold_subface_preview(
    entity: Entity,
    preview_geom: CadGeometryKind,
    preview_topo: &EditableBlockTopology,
    cad_objects: &mut Query<(Entity, &mut CadObject)>,
    topologies: &mut Query<&mut EditableBlockTopology>,
    transforms: &mut Query<&mut Transform>,
    mesh_handles: &mut Query<&mut Handle<Mesh>>,
    meshes: &mut Assets<Mesh>,
) {
    if let Ok((_, mut obj)) = cad_objects.get_mut(entity) {
        obj.geometry = preview_geom;
    }
    if let Ok(mut topo) = topologies.get_mut(entity) {
        *topo = preview_topo.clone();
    }
    if let CadGeometryKind::Block { center, size } = preview_geom {
        let new_mesh = build_manifold_block_mesh_with_topology(center, size, preview_topo);
        let new_handle = meshes.add(new_mesh);
        if let Ok(mut mh) = mesh_handles.get_mut(entity) {
            *mh = new_handle;
        }
        if let Ok(mut tf) = transforms.get_mut(entity) {
            tf.translation = center;
            tf.rotation = Quat::IDENTITY;
            tf.scale = Vec3::ONE;
        }
    }
}

fn compute_extrude_distance_from_ray(
    ray: Ray3d,
    camera_forward: Vec3,
    anchor_3d: Vec3,
    face_normal: Dir3,
) -> Option<f32> {
    let n = face_normal.as_vec3();
    let plane_vec = (camera_forward - n * camera_forward.dot(n)).try_normalize().or_else(|| {
        let fallback = if n.z.abs() < 0.9_f32 { Vec3::Z } else { Vec3::X };
        (fallback - n * fallback.dot(n)).try_normalize()
    })?;
    let drag_plane_normal = Dir3::new(-plane_vec).ok()?;
    let hit_pt = intersect_ray_with_plane(
        ray,
        anchor_3d,
        drag_plane_normal,
        MAX_PLANE_RAY_DIST * 2.5_f32,
    )?;
    let raw_dist = (hit_pt - anchor_3d).dot(n);
    Some(snap_scalar(raw_dist, EXTRUDE_SNAP_STEP))
}

fn handle_push_pull_tool_interaction(
    active_tool: Res<ActiveToolState>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window_query: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    mut selected: ResMut<SelectedCadObjects>,
    mut push_pull_state: ResMut<PushPullToolState>,
    mut vcb: ResMut<VcbInput>,
    mut cad_objects: Query<(Entity, &mut CadObject)>,
    mut topologies: Query<&mut EditableBlockTopology>,
    mut transforms: Query<&mut Transform>,
    mut mesh_handles: Query<&mut Handle<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut next_group: ResMut<NextGroupId>,
    mut history: ResMut<CommandHistory>,
    mut egui_contexts: EguiContexts,
) {
    if active_tool.current != CadTool::PushPullExtrude {
        if push_pull_state.is_dragging
            || push_pull_state.hovered_face.is_some()
            || !push_pull_state.preview_wall_segments.is_empty()
        {
            push_pull_state.hovered_face = None;
            push_pull_state.reset_drag();
        }
        return;
    }

    if is_egui_consuming_pointer(&mut egui_contexts) {
        return;
    }

    let Ok(window) = window_query.get_single() else {
        return;
    };
    let Ok((camera, cam_global)) = camera_query.get_single() else {
        return;
    };

    let cursor_ray = window
        .cursor_position()
        .and_then(|pos| camera.viewport_to_world(cam_global, pos));

    if !push_pull_state.is_dragging {
        push_pull_state.hovered_face = None;
        if let Some(ray) = cursor_ray {
            if let Some(face) =
                detect_hovered_extrudable_face(ray, &cad_objects, &topologies)
            {
                push_pull_state.hovered_face = Some(face.clone());
                if mouse_buttons.just_pressed(MouseButton::Left) {
                    selected.select_single(face.entity);
                    push_pull_state.last_selected_face = Some(face.clone());
                    push_pull_state.active_face = Some(face);
                    push_pull_state.is_dragging = true;
                    push_pull_state.current_extrude_distance = 0.0_f32;
                    push_pull_state.preview_wall_segments.clear();
                }
            }
        }
        return;
    }

    if mouse_buttons.pressed(MouseButton::Left) {
        if let Some(ray) = cursor_ray {
            let cam_fwd = cam_global.forward().as_vec3();
            let maybe_update = push_pull_state.active_face.as_ref().and_then(|face| {
                let dist = compute_extrude_distance_from_ray(
                    ray,
                    cam_fwd,
                    face.drag_anchor_3d,
                    face.contour.plane_normal,
                )?;
                let walls = face.build_wall_segments(dist);
                Some((face.clone(), dist, walls))
            });

            if let Some((face_snapshot, dist, walls)) = maybe_update {
                push_pull_state.current_extrude_distance = dist;
                push_pull_state.preview_wall_segments = walls;
                if vcb.buffer.is_empty() {
                    vcb.last_status = format!(
                        "{}: {:+.2} m",
                        face_snapshot.region_kind.label(),
                        dist
                    );
                }

                if face_snapshot.is_block_face && dist.abs() >= 0.01_f32 {
                    let (preview_geom, preview_topo) =
                        compute_extruded_block_state_for_face(&face_snapshot, dist);
                    apply_live_manifold_subface_preview(
                        face_snapshot.entity,
                        preview_geom,
                        &preview_topo,
                        &mut cad_objects,
                        &mut topologies,
                        &mut transforms,
                        &mut mesh_handles,
                        &mut meshes,
                    );
                }
            }
        }
        return;
    }

    if mouse_buttons.just_released(MouseButton::Left) {
        let finished_face = push_pull_state.active_face.take();
        let dist = push_pull_state.current_extrude_distance;
        push_pull_state.reset_drag();

        if let Some(face) = finished_face {
            if face.is_block_face {
                apply_live_manifold_subface_preview(
                    face.entity,
                    face.original_object.geometry,
                    &face.original_topology,
                    &mut cad_objects,
                    &mut topologies,
                    &mut transforms,
                    &mut mesh_handles,
                    &mut meshes,
                );
            }

            if dist.abs() >= 0.04_f32 {
                let group_opt = face
                    .contour
                    .group_id
                    .or_else(|| Some(next_group.allocate()));

                if face.is_block_face {
                    let before_geom = face.original_object.geometry;
                    let before_topo = face.original_topology.clone();
                    let (after_geom, after_topo) =
                        compute_extruded_block_state_for_face(&face, dist);

                    history.execute_command(Box::new(PushPullCommand::for_block_subface(
                        face.entity,
                        before_geom,
                        after_geom,
                        before_topo,
                        after_topo.clone(),
                        face.contour.plane_normal,
                        dist,
                        face.original_object.material_color.0,
                        group_opt,
                    )));

                    let shift = face.contour.plane_normal.as_vec3() * dist;
                    let shifted_verts: Vec<Vec3> = face
                        .contour
                        .vertices_3d()
                        .into_iter()
                        .map(|v| v + shift)
                        .collect();
                    if let Some(updated_contour) = OffsetContour::from_coplanar_loop_3d(
                        &shifted_verts,
                        Some(face.contour.plane_normal),
                        group_opt,
                    ) {
                        let mut updated_obj = face.original_object.clone();
                        updated_obj.geometry = after_geom;
                        push_pull_state.last_selected_face = Some(ExtrudableFace {
                            entity: face.entity,
                            is_block_face: true,
                            region_kind: face.region_kind,
                            original_object: updated_obj,
                            original_topology: after_topo,
                            contour: updated_contour,
                            inner_hole_contour: face.inner_hole_contour.clone(),
                            drag_anchor_3d: face.drag_anchor_3d + shift,
                        });
                    }
                } else {
                    let initial_boundary_loop = face.contour.vertices_3d();
                    history.execute_command(Box::new(PushPullCommand::for_closed_face_loop(
                        &initial_boundary_loop,
                        face.contour.plane_normal,
                        dist,
                        Color::srgb(0.74, 0.80, 0.88),
                        group_opt,
                    )));
                }
            }
        }
    }
}

fn process_vcb_commit_requests(
    mut submit_events: EventReader<VcbSubmitEvent>,
    active_tool: Res<ActiveToolState>,
    cursor_state: Res<CursorRayState>,
    selected: Res<SelectedCadObjects>,
    mut cad_objects: Query<(Entity, &mut CadObject)>,
    mut topologies: Query<&mut EditableBlockTopology>,
    mut transforms: Query<&mut Transform>,
    mut mesh_handles: Query<&mut Handle<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut draft_state: ResMut<ToolDraftState>,
    move_state: Res<MoveToolState>,
    mut offset_state: ResMut<OffsetToolState>,
    mut push_pull_state: ResMut<PushPullToolState>,
    mut next_group: ResMut<NextGroupId>,
    mut history: ResMut<CommandHistory>,
) {
    for ev in submit_events.read() {
        let text = ev.raw_text.trim();
        if text.is_empty() {
            continue;
        }

        let parts: Vec<f32> = text
            .split([',', ';'])
            .filter_map(|token| token.trim().parse::<f32>().ok())
            .collect();

        if parts.is_empty() {
            continue;
        }

        match active_tool.current {
            CadTool::LineDraw => {
                if let Some(start_pt) = draft_state.line_chain_start {
                    let end_pt = if parts.len() >= 3 {
                        start_pt + Vec3::new(parts[0], parts[1], parts[2])
                    } else {
                        let length = parts[0].abs().max(0.05_f32);
                        let raw_dir = (draft_state.line_preview_end - start_pt)
                            .try_normalize()
                            .unwrap_or(Vec3::X);
                        start_pt + raw_dir * length
                    };
                    let group_id = next_group.allocate();
                    history.execute_command(Box::new(DrawLineCommand::new(
                        start_pt,
                        end_pt,
                        Color::srgb(0.90, 0.92, 0.95),
                        Some(group_id),
                    )));
                    draft_state.line_chain_start = Some(end_pt);
                    draft_state.line_preview_end = end_pt;
                }
            }
            CadTool::Block => {
                let origin = draft_state
                    .block_first_corner
                    .unwrap_or(cursor_state.snapped_pos);
                let dims = if parts.len() >= 3 {
                    Vec3::new(
                        parts[0].abs().max(0.1_f32),
                        parts[1].abs().max(0.1_f32),
                        parts[2].abs().max(0.1_f32),
                    )
                } else if parts.len() == 2 {
                    Vec3::new(
                        parts[0].abs().max(0.1_f32),
                        parts[1].abs().max(0.1_f32),
                        draft_state.default_block_height.max(1.0_f32),
                    )
                } else {
                    let s = parts[0].abs().max(0.1_f32);
                    Vec3::splat(s)
                };
                draft_state.default_block_height = dims.z;
                let group_id = next_group.allocate();
                history.execute_command(Box::new(
                    CreateBlockCommand::from_corner_and_dimensions(
                        origin,
                        dims,
                        Color::srgb(0.76, 0.80, 0.86),
                        Some(group_id),
                    ),
                ));
                draft_state.block_first_corner = None;
            }
            CadTool::Move => {
                if let Some(selected_ent) = selected.primary() {
                    if let Ok((_, obj)) = cad_objects.get(selected_ent) {
                        let delta = if parts.len() >= 3 {
                            Vec3::new(parts[0], parts[1], parts[2])
                        } else {
                            let dist = parts[0];
                            let drag_dir = move_state
                                .active_drag
                                .as_ref()
                                .map(|s| s.current_delta)
                                .unwrap_or(Vec3::ZERO);
                            let dir = if drag_dir.length() > 0.01_f32 {
                                drag_dir.normalize_or_zero()
                            } else {
                                Vec3::X
                            };
                            dir * dist
                        };
                        if delta.length() > 0.005_f32 {
                            let before = obj.geometry;
                            let after = before.translated(delta);
                            history.execute_command(Box::new(MoveObjectCommand::new(
                                selected_ent,
                                before,
                                after,
                            )));
                        }
                    }
                }
            }
            CadTool::Scale => {
                if let Some(selected_ent) = selected.primary() {
                    if let Ok((_, obj)) = cad_objects.get(selected_ent) {
                        let corners = compute_object_bbox_corners(obj);
                        let anchor = corners[0];
                        let factors = if parts.len() >= 3 {
                            Vec3::new(
                                parts[0].abs().clamp(0.05_f32, 50.0_f32),
                                parts[1].abs().clamp(0.05_f32, 50.0_f32),
                                parts[2].abs().clamp(0.05_f32, 50.0_f32),
                            )
                        } else {
                            let uniform = parts[0].abs().clamp(0.05_f32, 50.0_f32);
                            Vec3::splat(uniform)
                        };
                        let before = obj.geometry;
                        let after = before.scaled_from_anchor(anchor, factors);
                        history.execute_command(Box::new(ScaleObjectCommand::new(
                            selected_ent,
                            before,
                            after,
                        )));
                    }
                }
            }
            CadTool::Offset => {
                let distance = parts[0];
                if distance.abs() >= 0.005_f32 {
                    let all_lines: Vec<(Vec3, Vec3)> = cad_objects
                        .iter()
                        .filter_map(|(_, o)| match o.geometry {
                            CadGeometryKind::Line { start, end, .. } => Some((start, end)),
                            CadGeometryKind::Block { .. } => None,
                        })
                        .collect();
                    let target_ent = offset_state
                        .active_entity
                        .or(offset_state.hovered_entity)
                        .or_else(|| selected.primary());
                    let maybe_contour = offset_state
                        .active_contour
                        .clone()
                        .or_else(|| offset_state.hovered_contour.clone())
                        .or_else(|| {
                            let sel_ent = selected.primary()?;
                            let (_, obj) = cad_objects.get(sel_ent).ok()?;
                            detect_offset_contour_for_object(obj, &all_lines)
                        });

                    if let Some(contour) = maybe_contour {
                        let signed_dist = if offset_state.current_offset_distance < 0.0_f32
                            && distance > 0.0_f32
                        {
                            -distance
                        } else {
                            distance
                        };
                        let segments = contour.compute_offset_segments(signed_dist);
                        let group_opt = contour
                            .group_id
                            .or_else(|| Some(next_group.allocate()));
                        offset_state.reset_drag();

                        if let Some(ent) = target_ent {
                            let is_block = cad_objects
                                .get(ent)
                                .map(|(_, o)| matches!(o.geometry, CadGeometryKind::Block { .. }))
                                .unwrap_or(false);
                            if is_block {
                                if let (Some(base_v4), Some(offset_v4)) = (
                                    contour.base_vertices_4(),
                                    contour.compute_offset_vertices_4(signed_dist),
                                ) {
                                    history.execute_command(Box::new(
                                        CreateOffsetCommand::for_block_face(
                                            ent,
                                            contour.plane_normal,
                                            base_v4,
                                            offset_v4,
                                            signed_dist,
                                            Color::srgb(0.95, 0.78, 0.22),
                                            group_opt,
                                        ),
                                    ));
                                    continue;
                                }
                            }
                        }

                        if segments.len() >= 3 {
                            history.execute_command(Box::new(CreateOffsetCommand::new(
                                segments,
                                Color::srgb(0.95, 0.78, 0.22),
                                group_opt,
                            )));
                        }
                    }
                }
            }
            CadTool::PushPullExtrude => {
                let distance = parts[0];
                if distance.abs() >= 0.01_f32 {
                    let was_dragging = push_pull_state.is_dragging;
                    let signed_dist = if was_dragging
                        && push_pull_state.current_extrude_distance < 0.0_f32
                        && distance > 0.0_f32
                    {
                        -distance
                    } else {
                        distance
                    };

                    let target_face = push_pull_state
                        .active_face
                        .clone()
                        .or_else(|| push_pull_state.hovered_face.clone())
                        .or_else(|| push_pull_state.last_selected_face.clone());

                    if let Some(mut face) = target_face {
                        if was_dragging && face.is_block_face {
                            apply_live_manifold_subface_preview(
                                face.entity,
                                face.original_object.geometry,
                                &face.original_topology,
                                &mut cad_objects,
                                &mut topologies,
                                &mut transforms,
                                &mut mesh_handles,
                                &mut meshes,
                            );
                        } else if face.is_block_face {
                            if let Ok((_, latest_obj)) = cad_objects.get(face.entity) {
                                face.original_object = latest_obj.clone();
                            }
                            if let Ok(latest_topo) = topologies.get(face.entity) {
                                face.original_topology = latest_topo.clone();
                            }
                        }
                        push_pull_state.reset_drag();

                        let group_opt = face
                            .contour
                            .group_id
                            .or_else(|| Some(next_group.allocate()));

                        if face.is_block_face {
                            let before_geom = face.original_object.geometry;
                            let before_topo = face.original_topology.clone();
                            let (after_geom, after_topo) =
                                compute_extruded_block_state_for_face(&face, signed_dist);

                            history.execute_command(Box::new(PushPullCommand::for_block_subface(
                                face.entity,
                                before_geom,
                                after_geom,
                                before_topo,
                                after_topo.clone(),
                                face.contour.plane_normal,
                                signed_dist,
                                face.original_object.material_color.0,
                                group_opt,
                            )));

                            let shift = face.contour.plane_normal.as_vec3() * signed_dist;
                            let shifted_verts: Vec<Vec3> = face
                                .contour
                                .vertices_3d()
                                .into_iter()
                                .map(|v| v + shift)
                                .collect();
                            if let Some(updated_contour) = OffsetContour::from_coplanar_loop_3d(
                                &shifted_verts,
                                Some(face.contour.plane_normal),
                                group_opt,
                            ) {
                                let mut updated_obj = face.original_object.clone();
                                updated_obj.geometry = after_geom;
                                push_pull_state.last_selected_face = Some(ExtrudableFace {
                                    entity: face.entity,
                                    is_block_face: true,
                                    region_kind: face.region_kind,
                                    original_object: updated_obj,
                                    original_topology: after_topo,
                                    contour: updated_contour,
                                    inner_hole_contour: face.inner_hole_contour.clone(),
                                    drag_anchor_3d: face.drag_anchor_3d + shift,
                                });
                            }
                        } else {
                            let base_verts = face.contour.vertices_3d();
                            history.execute_command(Box::new(
                                PushPullCommand::for_closed_face_loop(
                                    &base_verts,
                                    face.contour.plane_normal,
                                    signed_dist,
                                    Color::srgb(0.74, 0.80, 0.88),
                                    group_opt,
                                ),
                            ));
                        }
                    } else {
                        let height = distance.abs().max(0.25_f32);
                        let origin =
                            cursor_state.snapped_pos - Vec3::new(0.5_f32, 0.5_f32, 0.0_f32);
                        let group_id = next_group.allocate();
                        history.execute_command(Box::new(
                            CreateBlockCommand::from_corner_and_dimensions(
                                origin,
                                Vec3::new(1.0_f32, 1.0_f32, height),
                                Color::srgb(0.68, 0.78, 0.88),
                                Some(group_id),
                            ),
                        ));
                    }
                }
            }
            CadTool::Select => {}
        }
    }
}

fn apply_live_object_preview(
    entity: Entity,
    updated_obj: &CadObject,
    cad_objects: &mut Query<(Entity, &mut CadObject)>,
    transforms: &mut Query<&mut Transform>,
    meshes: &mut Assets<Mesh>,
) {
    let old_size = cad_objects
        .get(entity)
        .ok()
        .and_then(|(_, o)| match o.geometry {
            CadGeometryKind::Block { size, .. } => Some(size),
            CadGeometryKind::Line { .. } => None,
        });

    if let Ok((_, mut obj_comp)) = cad_objects.get_mut(entity) {
        *obj_comp = updated_obj.clone();
    }
    if let Ok(mut tf) = transforms.get_mut(entity) {
        match updated_obj.geometry {
            CadGeometryKind::Line { start, end, radius } => {
                let delta = end - start;
                let len = delta.length().max(0.001_f32);
                let dir = delta / len;
                tf.translation = (start + end) * 0.5_f32;
                tf.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
                let _ = (radius, meshes);
            }
            CadGeometryKind::Block { center, size } => {
                tf.translation = center;
                if let Some(prev) = old_size {
                    let ratio = Vec3::new(
                        (size.x / prev.x.max(0.05_f32)).clamp(0.05_f32, 50.0_f32),
                        (size.y / prev.y.max(0.05_f32)).clamp(0.05_f32, 50.0_f32),
                        (size.z / prev.z.max(0.05_f32)).clamp(0.05_f32, 50.0_f32),
                    );
                    tf.scale *= ratio;
                }
            }
        }
    }
}

fn restore_object_geometry(
    entity: Entity,
    orig_obj: &CadObject,
    cad_objects: &mut Query<&mut CadObject>,
    transforms: &mut Query<&mut Transform>,
    meshes: &mut Assets<Mesh>,
) {
    if let Ok(mut obj_comp) = cad_objects.get_mut(entity) {
        *obj_comp = orig_obj.clone();
    }
    if let Ok(mut tf) = transforms.get_mut(entity) {
        match orig_obj.geometry {
            CadGeometryKind::Line { start, end, .. } => {
                let delta = end - start;
                let len = delta.length().max(0.001_f32);
                let dir = delta / len;
                tf.translation = (start + end) * 0.5_f32;
                tf.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
            }
            CadGeometryKind::Block { center, .. } => {
                tf.translation = center;
                tf.scale = Vec3::ONE;
            }
        }
    }
    let _ = (meshes, MaterialColor(Color::WHITE));
}

fn refresh_selection_materials(
    active_tool: Res<ActiveToolState>,
    selected: Res<SelectedCadObjects>,
    push_pull_state: Res<PushPullToolState>,
    cad_query: Query<(Entity, &CadObject, &Handle<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let hovered_push_pull_ent = if active_tool.current == CadTool::PushPullExtrude {
        push_pull_state
            .active_face
            .as_ref()
            .or(push_pull_state.hovered_face.as_ref())
            .map(|f| f.entity)
    } else {
        None
    };

    for (entity, obj, mat_handle) in cad_query.iter() {
        if let Some(mat) = materials.get_mut(mat_handle) {
            if Some(entity) == hovered_push_pull_ent {
                mat.base_color = obj.material_color.0;
                mat.emissive = LinearRgba::new(0.08_f32, 0.55_f32, 0.48_f32, 1.0_f32);
            } else if selected.contains(entity) {
                mat.base_color = NEON_BLUE_SELECTION_COLOR;
                mat.emissive = LinearRgba::new(0.05_f32, 0.45_f32, 0.65_f32, 1.0_f32);
            } else {
                mat.base_color = obj.material_color.0;
                mat.emissive = LinearRgba::BLACK;
            }
        }
    }
}

fn render_world_grid_and_axes(mut gizmos: Gizmos, focal_point: Res<FocalPoint>) {
    let grid_color = Color::srgba(0.32, 0.35, 0.40, 0.42);
    let sub_major_color = Color::srgba(0.44, 0.48, 0.55, 0.62);
    let extent = GRID_HALF_EXTENT as f32;

    for i in -GRID_HALF_EXTENT..=GRID_HALF_EXTENT {
        let coord = i as f32;
        if i == 0 {
            continue;
        }
        let line_col = if i % 5 == 0 {
            sub_major_color
        } else {
            grid_color
        };
        gizmos.line(
            Vec3::new(coord, -extent, 0.0_f32),
            Vec3::new(coord, extent, 0.0_f32),
            line_col,
        );
        gizmos.line(
            Vec3::new(-extent, coord, 0.0_f32),
            Vec3::new(extent, coord, 0.0_f32),
            line_col,
        );
    }

    gizmos.line(
        Vec3::new(-extent, 0.0_f32, 0.0_f32),
        Vec3::new(extent, 0.0_f32, 0.0_f32),
        AXIS_RED_COLOR,
    );
    gizmos.line(
        Vec3::new(0.0_f32, -extent, 0.0_f32),
        Vec3::new(0.0_f32, extent, 0.0_f32),
        AXIS_GREEN_COLOR,
    );
    gizmos.line(
        Vec3::new(0.0_f32, 0.0_f32, -extent * 0.5_f32),
        Vec3::new(0.0_f32, 0.0_f32, extent),
        AXIS_BLUE_COLOR,
    );

    let pivot = focal_point.0;
    let pivot_cross = 0.22_f32;
    let pivot_col = Color::srgba(0.95, 0.85, 0.30, 0.75);
    gizmos.line(
        pivot - Vec3::X * pivot_cross,
        pivot + Vec3::X * pivot_cross,
        pivot_col,
    );
    gizmos.line(
        pivot - Vec3::Y * pivot_cross,
        pivot + Vec3::Y * pivot_cross,
        pivot_col,
    );
    gizmos.line(
        pivot - Vec3::Z * pivot_cross,
        pivot + Vec3::Z * pivot_cross,
        pivot_col,
    );
}

fn render_active_tool_gizmos(
    mut gizmos: Gizmos,
    active_tool: Res<ActiveToolState>,
    cursor_state: Res<CursorRayState>,
    selected: Res<SelectedCadObjects>,
    inference: Res<AxisInference>,
    draft_state: Res<ToolDraftState>,
    move_state: Res<MoveToolState>,
    offset_state: Res<OffsetToolState>,
    push_pull_state: Res<PushPullToolState>,
    cad_objects: Query<(&CadObject, Option<&EditableBlockTopology>)>,
) {
    if cursor_state.has_hit {
        let c = cursor_state.snapped_pos;
        let marker_col = if cursor_state.vertical_z_locked {
            AXIS_BLUE_COLOR
        } else {
            Color::srgb(0.98, 0.92, 0.28)
        };
        gizmos.sphere(c, Quat::IDENTITY, 0.08_f32, marker_col);
    }

    if active_tool.current == CadTool::LineDraw {
        if let Some(start_pt) = draft_state.line_chain_start {
            let end_pt = draft_state.line_preview_end;
            let preview_col = inference.snap_kind.color();
            gizmos.line(start_pt, end_pt, preview_col);
            gizmos.sphere(start_pt, Quat::IDENTITY, 0.09_f32, AXIS_GREEN_COLOR);
            gizmos.sphere(end_pt, Quat::IDENTITY, 0.09_f32, preview_col);

            if let Some((na, nb)) = inference.matched_neighbor_segment {
                render_dashed_segment(&mut gizmos, na, nb, INFERENCE_MAGENTA_COLOR);
            }

            if let Some((ray_origin, ray_dir)) = inference.diagonal_guide_ray {
                let guide_a = ray_origin - ray_dir * DIAGONAL_GUIDE_HALF_SPAN;
                let guide_b = ray_origin + ray_dir * DIAGONAL_GUIDE_HALF_SPAN;
                render_dashed_segment(&mut gizmos, guide_a, guide_b, DIAGONAL_YELLOW_COLOR);
            }
        }
    }

    if active_tool.current == CadTool::Move {
        if let Some(ref session) = move_state.active_drag {
            let start = session.start_cursor;
            let current = start + session.current_delta;
            let col = inference.snap_kind.color();
            render_dashed_segment(&mut gizmos, start, current, col);
            if let Some((na, nb)) = inference.matched_neighbor_segment {
                render_dashed_segment(&mut gizmos, na, nb, INFERENCE_MAGENTA_COLOR);
            }
        }
    }

    let seam_edge_color = Color::srgb(0.18, 0.22, 0.28);
    for (_obj, maybe_topo) in cad_objects.iter() {
        if let Some(topo) = maybe_topo {
            for face_poly in &topo.faces {
                for k in 0..4 {
                    let next_k = (k + 1) % 4;
                    gizmos.line(
                        face_poly.outer_loop[k],
                        face_poly.outer_loop[next_k],
                        seam_edge_color,
                    );
                }
                for hole in &face_poly.inner_holes {
                    for k in 0..4 {
                        let next_k = (k + 1) % 4;
                        gizmos.line(hole[k], hole[next_k], seam_edge_color);
                    }
                }
            }
        }
    }

    if active_tool.current == CadTool::Scale {
        if let Some(selected_ent) = selected.primary() {
            if let Ok((obj, _)) = cad_objects.get(selected_ent) {
                let corners = compute_object_bbox_corners(obj);
                let edges = [
                    (0, 1),
                    (1, 3),
                    (3, 2),
                    (2, 0),
                    (4, 5),
                    (5, 7),
                    (7, 6),
                    (6, 4),
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 7),
                ];
                for (a, b) in edges {
                    gizmos.line(corners[a], corners[b], SCALE_GIZMO_COLOR);
                }
                for corner in corners {
                    gizmos.sphere(corner, Quat::IDENTITY, 0.14_f32, SCALE_GIZMO_COLOR);
                }
            }
        }
    }

    if active_tool.current == CadTool::Offset {
        let base_contour = offset_state
            .active_contour
            .as_ref()
            .or(offset_state.hovered_contour.as_ref());

        if let Some(contour) = base_contour {
            for (a, b) in contour.perimeter_segments_3d() {
                gizmos.line(a, b, OFFSET_PREVIEW_COLOR);
                gizmos.sphere(a, Quat::IDENTITY, 0.05_f32, OFFSET_PREVIEW_COLOR);
            }
            if let Some(snapped_v) = contour.snapped_vertex {
                gizmos.sphere(snapped_v, Quat::IDENTITY, 0.11_f32, OFFSET_PREVIEW_COLOR);
            }
        }

        if offset_state.is_dragging {
            for (a, b) in &offset_state.preview_segments {
                gizmos.line(*a, *b, OFFSET_PREVIEW_COLOR);
                gizmos.sphere(*a, Quat::IDENTITY, 0.055_f32, OFFSET_PREVIEW_COLOR);
            }
        }
    }

    if active_tool.current == CadTool::PushPullExtrude {
        let target_face = push_pull_state
            .active_face
            .as_ref()
            .or(push_pull_state.hovered_face.as_ref());

        if let Some(face) = target_face {
            for (a, b) in face.contour.perimeter_segments_3d() {
                gizmos.line(a, b, PUSH_PULL_HIGHLIGHT_COLOR);
            }
            if let Some(ref hole) = face.inner_hole_contour {
                for (a, b) in hole.perimeter_segments_3d() {
                    gizmos.line(a, b, PUSH_PULL_HIGHLIGHT_COLOR);
                }
                for dot_pt in face.contour.sample_annular_dots_3d(hole) {
                    gizmos.sphere(dot_pt, Quat::IDENTITY, 0.032_f32, PUSH_PULL_HIGHLIGHT_COLOR);
                }
            } else {
                for dot_pt in face.contour.sample_interior_dots_3d() {
                    gizmos.sphere(dot_pt, Quat::IDENTITY, 0.032_f32, PUSH_PULL_HIGHLIGHT_COLOR);
                }
            }
            let normal_tip =
                face.contour.plane_origin + face.contour.plane_normal.as_vec3() * 0.65_f32;
            gizmos.line(face.contour.plane_origin, normal_tip, PUSH_PULL_HIGHLIGHT_COLOR);
            gizmos.sphere(normal_tip, Quat::IDENTITY, 0.06_f32, PUSH_PULL_HIGHLIGHT_COLOR);
        }

        if push_pull_state.is_dragging {
            for (a, b) in &push_pull_state.preview_wall_segments {
                gizmos.line(*a, *b, OFFSET_PREVIEW_COLOR);
            }
        }
    }
}

fn render_dashed_segment(gizmos: &mut Gizmos, a: Vec3, b: Vec3, color: Color) {
    let total = b - a;
    let len = total.length();
    if len < 0.05_f32 {
        return;
    }
    let dir = total / len;
    let dash_len = 0.22_f32;
    let gap_len = 0.14_f32;
    let step = dash_len + gap_len;
    let mut dist = 0.0_f32;
    while dist < len {
        let seg_start = a + dir * dist;
        let seg_end = a + dir * (dist + dash_len).min(len);
        gizmos.line(seg_start, seg_end, color);
        dist += step;
    }
}

fn raycast_closest_cad_object_point(
    ray: Ray3d,
    cad_objects: &Query<(Entity, &CadObject)>,
) -> Option<(Entity, Vec3)> {
    let mut closest_t = f32::MAX;
    let mut closest_entity: Option<Entity> = None;

    for (entity, obj) in cad_objects.iter() {
        if let Some(t) = ray_intersects_cad_object(ray, obj) {
            if t < closest_t {
                closest_t = t;
                closest_entity = Some(entity);
            }
        }
    }

    closest_entity.map(|ent| (ent, ray.get_point(closest_t)))
}

pub fn ray_intersects_cad_object(ray: Ray3d, obj: &CadObject) -> Option<f32> {
    match obj.geometry {
        CadGeometryKind::Block { center, size } => {
            if let Some(face_hit) = raycast_block_face_exact(ray, center, size) {
                return Some(face_hit.t);
            }
            let half = size * 0.5_f32;
            ray_intersects_aabb(ray, center - half, center + half)
        }
        CadGeometryKind::Line { start, end, radius } => {
            let pick_radius = (radius * 2.5_f32).max(0.18_f32);
            ray_intersects_segment_capsule(ray, start, end, pick_radius)
        }
    }
}

fn ray_intersects_aabb(ray: Ray3d, min: Vec3, max: Vec3) -> Option<f32> {
    let dir = ray.direction.as_vec3();
    let orig = ray.origin;

    let mut t_near = f32::NEG_INFINITY;
    let mut t_far = f32::INFINITY;

    for axis in 0..3 {
        let o = orig[axis];
        let d = dir[axis];
        let b_min = min[axis];
        let b_max = max[axis];

        if d.abs() < 1e-7_f32 {
            if o < b_min || o > b_max {
                return None;
            }
        } else {
            let inv_d = 1.0_f32 / d;
            let mut t1 = (b_min - o) * inv_d;
            let mut t2 = (b_max - o) * inv_d;
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

    if t_far < 0.0_f32 {
        return None;
    }
    Some(if t_near >= 0.0_f32 { t_near } else { t_far })
}

fn ray_intersects_sphere(ray: Ray3d, center: Vec3, radius: f32) -> Option<f32> {
    let oc = ray.origin - center;
    let dir = ray.direction.as_vec3();
    let half_b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = half_b * half_b - c;
    if disc < 0.0_f32 {
        return None;
    }
    let root = disc.sqrt();
    let t1 = -half_b - root;
    if t1 > 0.0_f32 {
        return Some(t1);
    }
    let t2 = -half_b + root;
    if t2 > 0.0_f32 {
        Some(t2)
    } else {
        None
    }
}

fn ray_intersects_segment_capsule(
    ray: Ray3d,
    seg_a: Vec3,
    seg_b: Vec3,
    threshold: f32,
) -> Option<f32> {
    let u = ray.direction.as_vec3();
    let v = seg_b - seg_a;
    let w = ray.origin - seg_a;

    let a = u.dot(u);
    let b = u.dot(v);
    let c = v.dot(v);
    let d = u.dot(w);
    let e = v.dot(w);
    let denom = a * c - b * b;

    let (s_ray, t_seg) = if denom < 1e-6_f32 {
        (0.0_f32, (e / c.max(1e-6_f32)).clamp(0.0_f32, 1.0_f32))
    } else {
        let t_unclamped = (a * e - b * d) / denom;
        let t_clamped = t_unclamped.clamp(0.0_f32, 1.0_f32);
        let s_computed = (b * t_clamped - d) / a.max(1e-6_f32);
        (s_computed.max(0.0_f32), t_clamped)
    };

    if s_ray <= 0.0_f32 {
        return None;
    }

    let pt_on_ray = ray.origin + u * s_ray;
    let pt_on_seg = seg_a + v * t_seg;
    if pt_on_ray.distance(pt_on_seg) <= threshold {
        Some(s_ray)
    } else {
        None
    }
}