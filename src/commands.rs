use std::collections::VecDeque;

use bevy::math::primitives::{Cuboid, Cylinder, Sphere};
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadCommandExecutionSet;

pub struct CadCommandsPlugin;

impl Plugin for CadCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandHistory>()
            .init_resource::<NextObjectSerial>()
            .add_systems(
                Update,
                flush_history_action_queue.in_set(CadCommandExecutionSet),
            );
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub u64);

#[derive(Component, Debug, Clone, Copy)]
pub struct MaterialColor(pub Color);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CadGeometryKind {
    Line {
        start: Vec3,
        end: Vec3,
        radius: f32,
    },
    Block {
        center: Vec3,
        size: Vec3,
    },
}

impl CadGeometryKind {
    pub fn translated(&self, delta: Vec3) -> Self {
        match *self {
            Self::Line { start, end, radius } => Self::Line {
                start: start + delta,
                end: end + delta,
                radius,
            },
            Self::Block { center, size } => Self::Block {
                center: center + delta,
                size,
            },
        }
    }

    pub fn scaled_from_anchor(&self, anchor: Vec3, factors: Vec3) -> Self {
        match *self {
            Self::Line { start, end, radius } => Self::Line {
                start: anchor + (start - anchor) * factors,
                end: anchor + (end - anchor) * factors,
                radius,
            },
            Self::Block { center, size } => Self::Block {
                center: anchor + (center - anchor) * factors,
                size: (size * factors).max(Vec3::splat(0.1_f32)),
            },
        }
    }

    pub fn extruded_along_face(&self, normal: Dir3, distance: f32) -> Self {
        match *self {
            Self::Line { start, end, radius } => {
                let shift = normal.as_vec3() * distance;
                Self::Line {
                    start: start + shift,
                    end: end + shift,
                    radius,
                }
            }
            Self::Block { center, size } => {
                let half = size * 0.5_f32;
                let mut min = center - half;
                let mut max = center + half;
                let n = normal.as_vec3();
                let min_thickness = 0.1_f32;

                if n.z > 0.9_f32 {
                    max.z = (max.z + distance).max(min.z + min_thickness);
                } else if n.z < -0.9_f32 {
                    min.z = (min.z - distance).min(max.z - min_thickness);
                } else if n.x > 0.9_f32 {
                    max.x = (max.x + distance).max(min.x + min_thickness);
                } else if n.x < -0.9_f32 {
                    min.x = (min.x - distance).min(max.x - min_thickness);
                } else if n.y > 0.9_f32 {
                    max.y = (max.y + distance).max(min.y + min_thickness);
                } else if n.y < -0.9_f32 {
                    min.y = (min.y - distance).min(max.y - min_thickness);
                }

                let new_size = (max - min).max(Vec3::splat(min_thickness));
                let new_center = (min + max) * 0.5_f32;
                Self::Block {
                    center: new_center,
                    size: new_size,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubFaceRegionKind {
    #[default]
    WholeFace,
    InnerLoop,
    OuterFrame,
}

impl SubFaceRegionKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::WholeFace => "Full Face",
            Self::InnerLoop => "Inner Sub-Face",
            Self::OuterFrame => "Outer Frame Ring",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubdividedFaceSplit {
    pub face_normal: Dir3,
    pub base_outer_loop: [Vec3; 4],
    pub base_inner_loop: [Vec3; 4],
    pub inner_extrusion_depth: f32,
    pub outer_frame_extrusion_depth: f32,
}

impl SubdividedFaceSplit {
    pub fn current_outer_loop_3d(&self) -> [Vec3; 4] {
        let shift = self.face_normal.as_vec3() * self.outer_frame_extrusion_depth;
        [
            self.base_outer_loop[0] + shift,
            self.base_outer_loop[1] + shift,
            self.base_outer_loop[2] + shift,
            self.base_outer_loop[3] + shift,
        ]
    }

    pub fn current_frame_inner_rim_3d(&self) -> [Vec3; 4] {
        let shift = self.face_normal.as_vec3() * self.outer_frame_extrusion_depth;
        [
            self.base_inner_loop[0] + shift,
            self.base_inner_loop[1] + shift,
            self.base_inner_loop[2] + shift,
            self.base_inner_loop[3] + shift,
        ]
    }

    pub fn current_inner_loop_3d(&self) -> [Vec3; 4] {
        let shift = self.face_normal.as_vec3() * self.inner_extrusion_depth;
        [
            self.base_inner_loop[0] + shift,
            self.base_inner_loop[1] + shift,
            self.base_inner_loop[2] + shift,
            self.base_inner_loop[3] + shift,
        ]
    }

    pub fn translated(&self, delta: Vec3) -> Self {
        let mut moved = self.clone();
        for v in &mut moved.base_outer_loop {
            *v += delta;
        }
        for v in &mut moved.base_inner_loop {
            *v += delta;
        }
        moved
    }

    pub fn scaled_from_anchor(&self, anchor: Vec3, factors: Vec3) -> Self {
        let mut scaled = self.clone();
        for v in &mut scaled.base_outer_loop {
            *v = anchor + (*v - anchor) * factors;
        }
        for v in &mut scaled.base_inner_loop {
            *v = anchor + (*v - anchor) * factors;
        }
        let n = self.face_normal.as_vec3().abs();
        let depth_scale = (n.x * factors.x + n.y * factors.y + n.z * factors.z).max(0.1_f32);
        scaled.inner_extrusion_depth *= depth_scale;
        scaled.outer_frame_extrusion_depth *= depth_scale;
        scaled
    }
}

#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct EditableBlockTopology {
    pub face_splits: Vec<SubdividedFaceSplit>,
}

impl EditableBlockTopology {
    pub fn find_split(&self, normal: Dir3) -> Option<&SubdividedFaceSplit> {
        self.face_splits
            .iter()
            .find(|s| s.face_normal.as_vec3().dot(normal.as_vec3()) > 0.98_f32)
    }

    pub fn find_split_mut(&mut self, normal: Dir3) -> Option<&mut SubdividedFaceSplit> {
        self.face_splits
            .iter_mut()
            .find(|s| s.face_normal.as_vec3().dot(normal.as_vec3()) > 0.98_f32)
    }

    pub fn upsert_split(&mut self, split: SubdividedFaceSplit) {
        if let Some(existing) = self.find_split_mut(split.face_normal) {
            *existing = split;
        } else {
            self.face_splits.push(split);
        }
    }

    pub fn translated(&self, delta: Vec3) -> Self {
        Self {
            face_splits: self.face_splits.iter().map(|s| s.translated(delta)).collect(),
        }
    }

    pub fn scaled_from_anchor(&self, anchor: Vec3, factors: Vec3) -> Self {
        Self {
            face_splits: self
                .face_splits
                .iter()
                .map(|s| s.scaled_from_anchor(anchor, factors))
                .collect(),
        }
    }
}

#[derive(Component, Debug, Clone)]
pub struct CadObject {
    pub label: String,
    pub group_id: Option<GroupId>,
    pub material_color: MaterialColor,
    pub geometry: CadGeometryKind,
}

#[derive(Resource, Debug)]
pub struct NextObjectSerial(pub u64);

impl Default for NextObjectSerial {
    fn default() -> Self {
        Self(1)
    }
}

impl NextObjectSerial {
    pub fn next_id(&mut self) -> u64 {
        let current = self.0;
        self.0 = self.0.saturating_add(1);
        current
    }
}

pub trait Command: Send + Sync + 'static {
    fn execute(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn description(&self) -> &'static str;
}

enum QueuedHistoryOp {
    Execute(Box<dyn Command>),
    Undo,
    Redo,
}

#[derive(Resource, Default)]
pub struct CommandHistory {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
    pending_ops: VecDeque<QueuedHistoryOp>,
}

impl CommandHistory {
    pub fn execute_command(&mut self, command: Box<dyn Command>) {
        self.pending_ops.push_back(QueuedHistoryOp::Execute(command));
    }

    pub fn request_undo(&mut self) {
        self.pending_ops.push_back(QueuedHistoryOp::Undo);
    }

    pub fn request_redo(&mut self) {
        self.pending_ops.push_back(QueuedHistoryOp::Redo);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_count(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_count(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn last_undo_description(&self) -> Option<&'static str> {
        self.undo_stack.last().map(|c| c.description())
    }

    pub fn last_redo_description(&self) -> Option<&'static str> {
        self.redo_stack.last().map(|c| c.description())
    }
}

fn flush_history_action_queue(world: &mut World) {
    loop {
        let next_op = {
            let Some(mut history) = world.get_resource_mut::<CommandHistory>() else {
                return;
            };
            history.pending_ops.pop_front()
        };

        let Some(op) = next_op else {
            break;
        };

        match op {
            QueuedHistoryOp::Execute(mut cmd) => {
                cmd.execute(world);
                if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
                    history.redo_stack.clear();
                    history.undo_stack.push(cmd);
                }
            }
            QueuedHistoryOp::Undo => {
                let popped = world
                    .get_resource_mut::<CommandHistory>()
                    .and_then(|mut h| h.undo_stack.pop());
                if let Some(mut cmd) = popped {
                    cmd.undo(world);
                    if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
                        history.redo_stack.push(cmd);
                    }
                }
            }
            QueuedHistoryOp::Redo => {
                let popped = world
                    .get_resource_mut::<CommandHistory>()
                    .and_then(|mut h| h.redo_stack.pop());
                if let Some(mut cmd) = popped {
                    cmd.execute(world);
                    if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
                        history.undo_stack.push(cmd);
                    }
                }
            }
        }
    }
}

pub fn remove_cad_entity(world: &mut World, target: Entity) {
    let child_list: Vec<Entity> = world
        .get::<Children>(target)
        .map(|children| children.iter().copied().collect())
        .unwrap_or_default();

    for child in child_list {
        let _ = world.despawn(child);
    }
    let _ = world.despawn(target);
}

fn push_quad_ccw(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
    v3: Vec3,
    expected_normal: Vec3,
) {
    let geom_normal = (v1 - v0).cross(v2 - v0).normalize_or_zero();
    let final_normal = if expected_normal.length_squared() > 0.01_f32 {
        expected_normal.normalize_or_zero()
    } else {
        geom_normal
    };

    let base_idx = positions.len() as u32;
    let n_arr = final_normal.to_array();

    if geom_normal.dot(final_normal) < 0.0_f32 {
        positions.push(v0.to_array());
        positions.push(v3.to_array());
        positions.push(v2.to_array());
        positions.push(v1.to_array());
    } else {
        positions.push(v0.to_array());
        positions.push(v1.to_array());
        positions.push(v2.to_array());
        positions.push(v3.to_array());
    }

    normals.extend_from_slice(&[n_arr, n_arr, n_arr, n_arr]);
    uvs.extend_from_slice(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    indices.extend_from_slice(&[
        base_idx,
        base_idx + 1,
        base_idx + 2,
        base_idx,
        base_idx + 2,
        base_idx + 3,
    ]);
}

fn orient_loop_ccw(loop_pts: &[Vec3; 4], normal: Vec3) -> [Vec3; 4] {
    let mut newell = Vec3::ZERO;
    for i in 0..4 {
        let curr = loop_pts[i];
        let next = loop_pts[(i + 1) % 4];
        newell.x += (curr.y - next.y) * (curr.z + next.z);
        newell.y += (curr.z - next.z) * (curr.x + next.x);
        newell.z += (curr.x - next.x) * (curr.y + next.y);
    }
    if newell.dot(normal) < 0.0_f32 {
        [loop_pts[0], loop_pts[3], loop_pts[2], loop_pts[1]]
    } else {
        *loop_pts
    }
}

fn align_inner_loop_to_outer(outer_ccw: &[Vec3; 4], inner_raw: &[Vec3; 4], normal: Vec3) -> [Vec3; 4] {
    let inner_ccw = orient_loop_ccw(inner_raw, normal);
    let mut best_shift = 0_usize;
    let mut best_dist = f32::MAX;
    for shift in 0..4 {
        let mut total_d = 0.0_f32;
        for k in 0..4 {
            total_d += outer_ccw[k].distance(inner_ccw[(k + shift) % 4]);
        }
        if total_d < best_dist {
            best_dist = total_d;
            best_shift = shift;
        }
    }
    [
        inner_ccw[best_shift % 4],
        inner_ccw[(best_shift + 1) % 4],
        inner_ccw[(best_shift + 2) % 4],
        inner_ccw[(best_shift + 3) % 4],
    ]
}

pub fn build_manifold_block_mesh_with_topology(
    center: Vec3,
    size: Vec3,
    topology: &EditableBlockTopology,
) -> Mesh {
    let clamped_size = size.max(Vec3::splat(0.05_f32));
    let half = clamped_size * 0.5_f32;
    let min = center - half;
    let max = center + half;

    let primary_faces: [(Dir3, [Vec3; 4]); 6] = [
        (
            Dir3::Z,
            [
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Z,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, min.y, min.z),
            ],
        ),
        (
            Dir3::X,
            [
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(max.x, min.y, max.z),
            ],
        ),
        (
            Dir3::NEG_X,
            [
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        ),
        (
            Dir3::Y,
            [
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, max.y, max.z),
                Vec3::new(max.x, max.y, max.z),
            ],
        ),
        (
            Dir3::NEG_Y,
            [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
        ),
    ];

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (face_normal, default_verts) in primary_faces {
        let n_vec = face_normal.as_vec3();

        if let Some(split) = topology.find_split(face_normal) {
            let base_outer = orient_loop_ccw(&split.base_outer_loop, n_vec);
            let base_inner = align_inner_loop_to_outer(&base_outer, &split.base_inner_loop, n_vec);

            let d_outer = split.outer_frame_extrusion_depth;
            let d_inner = split.inner_extrusion_depth;

            let outer_local: [Vec3; 4] = [
                base_outer[0] - center,
                base_outer[1] - center,
                base_outer[2] - center,
                base_outer[3] - center,
            ];
            let disp_outer_local: [Vec3; 4] = [
                outer_local[0] + n_vec * d_outer,
                outer_local[1] + n_vec * d_outer,
                outer_local[2] + n_vec * d_outer,
                outer_local[3] + n_vec * d_outer,
            ];
            let frame_inner_local: [Vec3; 4] = [
                (base_inner[0] - center) + n_vec * d_outer,
                (base_inner[1] - center) + n_vec * d_outer,
                (base_inner[2] - center) + n_vec * d_outer,
                (base_inner[3] - center) + n_vec * d_outer,
            ];
            let disp_inner_local: [Vec3; 4] = [
                (base_inner[0] - center) + n_vec * d_inner,
                (base_inner[1] - center) + n_vec * d_inner,
                (base_inner[2] - center) + n_vec * d_inner,
                (base_inner[3] - center) + n_vec * d_inner,
            ];

            // 1. If the outer frame ring itself was extruded or recessed (d_outer != 0),
            // stitch 4 exterior side-wall quads connecting the block's base rim to the displaced outer frame rim.
            if d_outer.abs() > 0.002_f32 {
                for k in 0..4 {
                    let next_k = (k + 1) % 4;
                    let b0 = outer_local[k];
                    let b1 = outer_local[next_k];
                    let t1 = disp_outer_local[next_k];
                    let t0 = disp_outer_local[k];
                    let edge_dir = (b1 - b0).normalize_or_zero();
                    let wall_normal = if d_outer >= 0.0_f32 {
                        edge_dir.cross(n_vec).normalize_or_zero()
                    } else {
                        -edge_dir.cross(n_vec).normalize_or_zero()
                    };
                    push_quad_ccw(
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        &mut indices,
                        b0,
                        b1,
                        t1,
                        t0,
                        wall_normal,
                    );
                }
            }

            // 2. Triangulate the Outer Frame Ring into 4 trapezoid quads (8 triangles)
            // connecting disp_outer_local[0..4] to frame_inner_local[0..4].
            for k in 0..4 {
                let next_k = (k + 1) % 4;
                push_quad_ccw(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut indices,
                    disp_outer_local[k],
                    disp_outer_local[next_k],
                    frame_inner_local[next_k],
                    frame_inner_local[k],
                    n_vec,
                );
            }

            // 3. Stitch the 4 interior/exterior step side-walls (8 triangles) connecting
            // frame_inner_local[0..4] (at d_outer) to disp_inner_local[0..4] (at d_inner).
            let delta_depth = d_inner - d_outer;
            if delta_depth.abs() > 0.002_f32 {
                for k in 0..4 {
                    let next_k = (k + 1) % 4;
                    let r0 = frame_inner_local[k];
                    let r1 = frame_inner_local[next_k];
                    let c1 = disp_inner_local[next_k];
                    let c0 = disp_inner_local[k];

                    let edge_dir = (r1 - r0).normalize_or_zero();
                    let outward_ring_normal = edge_dir.cross(n_vec).normalize_or_zero();
                    // When delta_depth > 0 (inner sub-face protrudes outward as a boss),
                    // the step walls face outward (+outward_ring_normal).
                    // When delta_depth < 0 (inner sub-face is pushed inward as a recessed pocket),
                    // the step walls face inward into the cavity (-outward_ring_normal).
                    let step_wall_normal = if delta_depth >= 0.0_f32 {
                        outward_ring_normal
                    } else {
                        -outward_ring_normal
                    };

                    push_quad_ccw(
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        &mut indices,
                        r0,
                        r1,
                        c1,
                        c0,
                        step_wall_normal,
                    );
                }
            }

            // 4. Triangulate the Inner Sub-Face cap (2 triangles) at d_inner.
            push_quad_ccw(
                &mut positions,
                &mut normals,
                &mut uvs,
                &mut indices,
                disp_inner_local[0],
                disp_inner_local[1],
                disp_inner_local[2],
                disp_inner_local[3],
                n_vec,
            );
        } else {
            let v0 = default_verts[0] - center;
            let v1 = default_verts[1] - center;
            let v2 = default_verts[2] - center;
            let v3 = default_verts[3] - center;
            push_quad_ccw(
                &mut positions,
                &mut normals,
                &mut uvs,
                &mut indices,
                v0,
                v1,
                v2,
                v3,
                n_vec,
            );
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

pub fn rebuild_block_entity_manifold_mesh(world: &mut World, target: Entity) {
    let Some((center, size, topo)) = world.get_entity(target).and_then(|ent| {
        let obj = ent.get::<CadObject>()?;
        let CadGeometryKind::Block { center, size } = obj.geometry else {
            return None;
        };
        let topo = ent
            .get::<EditableBlockTopology>()
            .cloned()
            .unwrap_or_default();
        Some((center, size, topo))
    }) else {
        return;
    };

    let new_mesh = build_manifold_block_mesh_with_topology(center, size, &topo);
    let new_mesh_handle = {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        meshes.add(new_mesh)
    };

    if let Some(mut ent_mut) = world.get_entity_mut(target) {
        if let Some(mut tf) = ent_mut.get_mut::<Transform>() {
            tf.translation = center;
            tf.rotation = Quat::IDENTITY;
            tf.scale = Vec3::ONE;
        }
        if let Some(mut mh) = ent_mut.get_mut::<Handle<Mesh>>() {
            *mh = new_mesh_handle;
        }
    }
}

pub fn instantiate_cad_entity(world: &mut World, cad_obj: CadObject) -> Entity {
    instantiate_cad_entity_with_topology(world, cad_obj, EditableBlockTopology::default())
}

pub fn instantiate_cad_entity_with_topology(
    world: &mut World,
    cad_obj: CadObject,
    topology: EditableBlockTopology,
) -> Entity {
    match cad_obj.geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let delta = end - start;
            let length = delta.length().max(0.001_f32);
            let midpoint = (start + end) * 0.5_f32;
            let dir = delta / length;
            let rotation = Quat::from_rotation_arc(Vec3::Y, dir);

            let (cyl_mesh, cap_mesh, line_mat) = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let cyl = meshes.add(Cylinder::new(radius, length));
                let cap = meshes.add(Sphere::new(radius * 1.25_f32));
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                let mat = materials.add(StandardMaterial {
                    base_color: cad_obj.material_color.0,
                    perceptual_roughness: 0.35_f32,
                    metallic: 0.05_f32,
                    ..default()
                });
                (cyl, cap, mat)
            };

            let half_len = length * 0.5_f32;
            let parent_entity = world
                .spawn((
                    PbrBundle {
                        mesh: cyl_mesh,
                        material: line_mat.clone(),
                        transform: Transform {
                            translation: midpoint,
                            rotation,
                            scale: Vec3::ONE,
                        },
                        ..default()
                    },
                    cad_obj.material_color,
                    cad_obj,
                ))
                .id();

            let cap_a = world
                .spawn(PbrBundle {
                    mesh: cap_mesh.clone(),
                    material: line_mat.clone(),
                    transform: Transform::from_xyz(0.0_f32, -half_len, 0.0_f32),
                    ..default()
                })
                .id();

            let cap_b = world
                .spawn(PbrBundle {
                    mesh: cap_mesh,
                    material: line_mat,
                    transform: Transform::from_xyz(0.0_f32, half_len, 0.0_f32),
                    ..default()
                })
                .id();

            world
                .entity_mut(parent_entity)
                .push_children(&[cap_a, cap_b]);
            parent_entity
        }
        CadGeometryKind::Block { center, size } => {
            let clamped = size.max(Vec3::splat(0.05_f32));
            let built_mesh = build_manifold_block_mesh_with_topology(center, clamped, &topology);
            let (box_mesh, box_mat) = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let m = meshes.add(built_mesh);
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                let mat = materials.add(StandardMaterial {
                    base_color: cad_obj.material_color.0,
                    perceptual_roughness: 0.48_f32,
                    metallic: 0.08_f32,
                    ..default()
                });
                (m, mat)
            };

            world
                .spawn((
                    PbrBundle {
                        mesh: box_mesh,
                        material: box_mat,
                        transform: Transform::from_translation(center),
                        ..default()
                    },
                    cad_obj.material_color,
                    cad_obj,
                    topology,
                ))
                .id()
        }
    }
}

pub fn apply_geometry_to_existing_entity(
    world: &mut World,
    target: Entity,
    new_geometry: CadGeometryKind,
) {
    let existing_children: Vec<Entity> = world
        .get::<Children>(target)
        .map(|c| c.iter().copied().collect())
        .unwrap_or_default();

    match new_geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let delta = end - start;
            let length = delta.length().max(0.001_f32);
            let midpoint = (start + end) * 0.5_f32;
            let dir = delta / length;
            let rotation = Quat::from_rotation_arc(Vec3::Y, dir);
            let half_len = length * 0.5_f32;

            let new_cyl_mesh = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                meshes.add(Cylinder::new(radius, length))
            };

            if let Some(mut ent_mut) = world.get_entity_mut(target) {
                if let Some(mut obj) = ent_mut.get_mut::<CadObject>() {
                    obj.geometry = new_geometry;
                }
                if let Some(mut tf) = ent_mut.get_mut::<Transform>() {
                    tf.translation = midpoint;
                    tf.rotation = rotation;
                    tf.scale = Vec3::ONE;
                }
                if let Some(mut mesh_handle) = ent_mut.get_mut::<Handle<Mesh>>() {
                    *mesh_handle = new_cyl_mesh;
                }
            }

            if let Some(first_child) = existing_children.first().copied() {
                if let Some(mut tf) = world.get_mut::<Transform>(first_child) {
                    tf.translation = Vec3::new(0.0_f32, -half_len, 0.0_f32);
                }
            }
            if let Some(second_child) = existing_children.get(1).copied() {
                if let Some(mut tf) = world.get_mut::<Transform>(second_child) {
                    tf.translation = Vec3::new(0.0_f32, half_len, 0.0_f32);
                }
            }
        }
        CadGeometryKind::Block { center, size } => {
            if let Some(mut ent_mut) = world.get_entity_mut(target) {
                if let Some(mut obj) = ent_mut.get_mut::<CadObject>() {
                    obj.geometry = CadGeometryKind::Block {
                        center,
                        size: size.max(Vec3::splat(0.05_f32)),
                    };
                }
            }
            rebuild_block_entity_manifold_mesh(world, target);
        }
    }
}

pub struct DrawLineCommand {
    start: Vec3,
    end: Vec3,
    radius: f32,
    color: Color,
    group_id: Option<GroupId>,
    spawned_entity: Option<Entity>,
}

impl DrawLineCommand {
    pub fn new(start: Vec3, end: Vec3, color: Color, group_id: Option<GroupId>) -> Self {
        Self {
            start,
            end,
            radius: 0.045_f32,
            color,
            group_id,
            spawned_entity: None,
        }
    }
}

impl Command for DrawLineCommand {
    fn execute(&mut self, world: &mut World) {
        let serial = world
            .get_resource_mut::<NextObjectSerial>()
            .map(|mut s| s.next_id())
            .unwrap_or(1);

        let obj = CadObject {
            label: format!("Line #{serial}"),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry: CadGeometryKind::Line {
                start: self.start,
                end: self.end,
                radius: self.radius,
            },
        };
        self.spawned_entity = Some(instantiate_cad_entity(world, obj));
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned_entity.take() {
            remove_cad_entity(world, entity);
        }
    }

    fn description(&self) -> &'static str {
        "Draw Line Segment"
    }
}

pub struct CreateBlockCommand {
    center: Vec3,
    size: Vec3,
    color: Color,
    group_id: Option<GroupId>,
    spawned_entity: Option<Entity>,
}

impl CreateBlockCommand {
    pub fn from_corners(
        a: Vec3,
        b: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let min_x = a.x.min(b.x);
        let max_x = a.x.max(b.x).max(min_x + 0.25_f32);
        let min_y = a.y.min(b.y);
        let max_y = a.y.max(b.y).max(min_y + 0.25_f32);
        let min_z = a.z.min(b.z);
        let max_z = if (a.z - b.z).abs() < 0.1_f32 {
            min_z + 2.0_f32
        } else {
            a.z.max(b.z)
        };

        let size = Vec3::new(max_x - min_x, max_y - min_y, (max_z - min_z).max(0.25_f32));
        let center = Vec3::new(
            (min_x + max_x) * 0.5_f32,
            (min_y + max_y) * 0.5_f32,
            min_z + size.z * 0.5_f32,
        );

        Self {
            center,
            size,
            color,
            group_id,
            spawned_entity: None,
        }
    }

    pub fn from_corner_and_dimensions(
        origin: Vec3,
        dims: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let clamped_dims = Vec3::new(
            dims.x.abs().max(0.1_f32),
            dims.y.abs().max(0.1_f32),
            dims.z.abs().max(0.1_f32),
        );
        let center = origin + clamped_dims * 0.5_f32;
        Self {
            center,
            size: clamped_dims,
            color,
            group_id,
            spawned_entity: None,
        }
    }
}

impl Command for CreateBlockCommand {
    fn execute(&mut self, world: &mut World) {
        let serial = world
            .get_resource_mut::<NextObjectSerial>()
            .map(|mut s| s.next_id())
            .unwrap_or(1);

        let obj = CadObject {
            label: format!("Block #{serial}"),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry: CadGeometryKind::Block {
                center: self.center,
                size: self.size,
            },
        };
        self.spawned_entity = Some(instantiate_cad_entity(world, obj));
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned_entity.take() {
            remove_cad_entity(world, entity);
        }
    }

    fn description(&self) -> &'static str {
        "Create 3D Block"
    }
}

pub struct CreateOffsetCommand {
    target_block_entity: Option<Entity>,
    face_split: Option<SubdividedFaceSplit>,
    previous_topology: Option<EditableBlockTopology>,
    fallback_segments: Vec<(Vec3, Vec3)>,
    color: Color,
    group_id: Option<GroupId>,
    spawned_entities: Vec<Entity>,
}

impl CreateOffsetCommand {
    pub fn for_block_face(
        parent_entity: Entity,
        face_normal: Dir3,
        base_face_vertices: [Vec3; 4],
        offset_vertices: [Vec3; 4],
        signed_distance: f32,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let (base_outer_loop, base_inner_loop) = if signed_distance <= 0.0_f32 {
            (base_face_vertices, offset_vertices)
        } else {
            (offset_vertices, base_face_vertices)
        };

        let split = SubdividedFaceSplit {
            face_normal,
            base_outer_loop,
            base_inner_loop,
            inner_extrusion_depth: 0.0_f32,
            outer_frame_extrusion_depth: 0.0_f32,
        };

        Self {
            target_block_entity: Some(parent_entity),
            face_split: Some(split),
            previous_topology: None,
            fallback_segments: Vec::new(),
            color,
            group_id,
            spawned_entities: Vec::new(),
        }
    }

    pub fn new(
        segments: Vec<(Vec3, Vec3)>,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        Self {
            target_block_entity: None,
            face_split: None,
            previous_topology: None,
            fallback_segments: segments,
            color,
            group_id,
            spawned_entities: Vec::new(),
        }
    }
}

impl Command for CreateOffsetCommand {
    fn execute(&mut self, world: &mut World) {
        if let (Some(parent_entity), Some(split)) =
            (self.target_block_entity, self.face_split.clone())
        {
            let existing_topo = world
                .get::<EditableBlockTopology>(parent_entity)
                .cloned()
                .unwrap_or_default();
            self.previous_topology = Some(existing_topo.clone());

            let mut updated_topo = existing_topo;
            updated_topo.upsert_split(split);

            if let Some(mut ent_mut) = world.get_entity_mut(parent_entity) {
                ent_mut.insert(updated_topo);
            }
            rebuild_block_entity_manifold_mesh(world, parent_entity);
            return;
        }

        self.spawned_entities.clear();
        let resolved_group = self.group_id.unwrap_or(GroupId(1));

        for (start, end) in self.fallback_segments.clone() {
            if start.distance(end) < 0.005_f32 {
                continue;
            }
            let serial = world
                .get_resource_mut::<NextObjectSerial>()
                .map(|mut s| s.next_id())
                .unwrap_or(1);

            let obj = CadObject {
                label: format!("Offset Edge #{serial}"),
                group_id: Some(resolved_group),
                material_color: MaterialColor(self.color),
                geometry: CadGeometryKind::Line {
                    start,
                    end,
                    radius: 0.042_f32,
                },
            };
            let ent = instantiate_cad_entity(world, obj);
            self.spawned_entities.push(ent);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(parent_entity) = self.target_block_entity {
            if let Some(prev_topo) = self.previous_topology.clone() {
                if let Some(mut ent_mut) = world.get_entity_mut(parent_entity) {
                    ent_mut.insert(prev_topo);
                }
                rebuild_block_entity_manifold_mesh(world, parent_entity);
            }
            return;
        }

        for ent in self.spawned_entities.drain(..) {
            remove_cad_entity(world, ent);
        }
    }

    fn description(&self) -> &'static str {
        "Subdivide Face with Offset Loop"
    }
}

pub fn build_stitched_extrusion_mesh(
    initial_boundary_loop: &[Vec3],
    face_normal: Dir3,
    extrude_distance: f32,
) -> (Mesh, Vec3, Vec3) {
    let signed_dist = if extrude_distance.abs() < 0.05_f32 {
        if extrude_distance >= 0.0_f32 {
            0.05_f32
        } else {
            -0.05_f32
        }
    } else {
        extrude_distance
    };

    let extrude_vec = face_normal.as_vec3() * signed_dist;
    let axis_dir = if signed_dist >= 0.0_f32 {
        face_normal.as_vec3()
    } else {
        -face_normal.as_vec3()
    }
    .normalize_or_zero();

    let mut ordered_base: Vec<Vec3> = initial_boundary_loop.to_vec();
    let count = ordered_base.len();

    if count < 3 {
        let fallback_size = Vec3::ONE;
        return (
            Mesh::from(Cuboid::from_size(fallback_size)),
            Vec3::ZERO,
            fallback_size,
        );
    }

    let mut newell = Vec3::ZERO;
    for i in 0..count {
        let curr = ordered_base[i];
        let next = ordered_base[(i + 1) % count];
        newell.x += (curr.y - next.y) * (curr.z + next.z);
        newell.y += (curr.z - next.z) * (curr.x + next.x);
        newell.z += (curr.x - next.x) * (curr.y + next.y);
    }
    if newell.dot(axis_dir) < 0.0_f32 {
        ordered_base.reverse();
    }

    let mut min_pt = Vec3::splat(f32::MAX);
    let mut max_pt = Vec3::splat(f32::MIN);
    for b in &ordered_base {
        let t = *b + extrude_vec;
        min_pt = min_pt.min(*b).min(t);
        max_pt = max_pt.max(*b).max(t);
    }

    let solid_center = (min_pt + max_pt) * 0.5_f32;
    let solid_size = (max_pt - min_pt).max(Vec3::splat(0.05_f32));

    let local_base: Vec<Vec3> = ordered_base.iter().map(|p| *p - solid_center).collect();
    let local_top: Vec<Vec3> = ordered_base
        .iter()
        .map(|p| (*p + extrude_vec) - solid_center)
        .collect();

    let cap_triangles = count.saturating_sub(2);
    let total_vertices = count * 4 + count * 2;
    let total_indices = count * 6 + cap_triangles * 6;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(total_vertices);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(total_vertices);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(total_vertices);
    let mut indices: Vec<u32> = Vec::with_capacity(total_indices);

    for i in 0..count {
        let next_i = (i + 1) % count;
        let b0 = local_base[i];
        let b1 = local_base[next_i];
        let t1 = local_top[next_i];
        let t0 = local_top[i];

        let edge_vec = b1 - b0;
        let up_vec = t0 - b0;
        let outward_normal = edge_vec.cross(up_vec).normalize_or_zero();
        push_quad_ccw(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut indices,
            b0,
            b1,
            t1,
            t0,
            outward_normal,
        );
    }

    let top_base_idx = positions.len() as u32;
    let top_n_arr = axis_dir.to_array();
    for v in &local_top {
        positions.push(v.to_array());
        normals.push(top_n_arr);
        uvs.push([v.x, v.y]);
    }
    for k in 1..(count - 1) {
        indices.push(top_base_idx);
        indices.push(top_base_idx + k as u32);
        indices.push(top_base_idx + (k + 1) as u32);
    }

    let bot_base_idx = positions.len() as u32;
    let bot_n_arr = (-axis_dir).to_array();
    for v in &local_base {
        positions.push(v.to_array());
        normals.push(bot_n_arr);
        uvs.push([v.x, v.y]);
    }
    for k in 1..(count - 1) {
        indices.push(bot_base_idx);
        indices.push(bot_base_idx + (k + 1) as u32);
        indices.push(bot_base_idx + k as u32);
    }

    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices));

    (mesh, solid_center, solid_size)
}

pub struct PushPullCommand {
    target_entity: Option<Entity>,
    before_geometry: Option<CadGeometryKind>,
    after_geometry: Option<CadGeometryKind>,
    before_topology: Option<EditableBlockTopology>,
    after_topology: Option<EditableBlockTopology>,
    standalone_loop_vertices: Vec<Vec3>,
    face_normal: Dir3,
    extrude_distance: f32,
    color: Color,
    group_id: Option<GroupId>,
    spawned_solid_entity: Option<Entity>,
}

impl PushPullCommand {
    pub fn for_block_subface(
        entity: Entity,
        before_geometry: CadGeometryKind,
        after_geometry: CadGeometryKind,
        before_topology: EditableBlockTopology,
        after_topology: EditableBlockTopology,
        face_normal: Dir3,
        extrude_distance: f32,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        Self {
            target_entity: Some(entity),
            before_geometry: Some(before_geometry),
            after_geometry: Some(after_geometry),
            before_topology: Some(before_topology),
            after_topology: Some(after_topology),
            standalone_loop_vertices: Vec::new(),
            face_normal,
            extrude_distance,
            color,
            group_id,
            spawned_solid_entity: None,
        }
    }

    pub fn for_closed_face_loop(
        base_vertices: &[Vec3],
        normal: Dir3,
        distance: f32,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        Self {
            target_entity: None,
            before_geometry: None,
            after_geometry: None,
            before_topology: None,
            after_topology: None,
            standalone_loop_vertices: base_vertices.to_vec(),
            face_normal: normal,
            extrude_distance: distance,
            color,
            group_id,
            spawned_solid_entity: None,
        }
    }
}

impl Command for PushPullCommand {
    fn execute(&mut self, world: &mut World) {
        if let Some(entity) = self.target_entity {
            if let Some(after_topo) = self.after_topology.clone() {
                if let Some(mut ent_mut) = world.get_entity_mut(entity) {
                    ent_mut.insert(after_topo);
                }
            }
            if let Some(after_geom) = self.after_geometry {
                apply_geometry_to_existing_entity(world, entity, after_geom);
            } else {
                rebuild_block_entity_manifold_mesh(world, entity);
            }
            return;
        }

        if self.standalone_loop_vertices.len() >= 3 {
            let (stitched_mesh, solid_center, solid_size) = build_stitched_extrusion_mesh(
                &self.standalone_loop_vertices,
                self.face_normal,
                self.extrude_distance,
            );

            let serial = world
                .get_resource_mut::<NextObjectSerial>()
                .map(|mut s| s.next_id())
                .unwrap_or(1);

            let (mesh_handle, mat_handle) = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let mh = meshes.add(stitched_mesh);
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                let mat = materials.add(StandardMaterial {
                    base_color: self.color,
                    perceptual_roughness: 0.45_f32,
                    metallic: 0.08_f32,
                    ..default()
                });
                (mh, mat)
            };

            let cad_obj = CadObject {
                label: format!("Extruded Solid #{serial}"),
                group_id: self.group_id,
                material_color: MaterialColor(self.color),
                geometry: CadGeometryKind::Block {
                    center: solid_center,
                    size: solid_size,
                },
            };

            let spawned = world
                .spawn((
                    PbrBundle {
                        mesh: mesh_handle,
                        material: mat_handle,
                        transform: Transform::from_translation(solid_center),
                        ..default()
                    },
                    cad_obj.material_color,
                    cad_obj,
                    EditableBlockTopology::default(),
                ))
                .id();

            self.spawned_solid_entity = Some(spawned);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(spawned) = self.spawned_solid_entity.take() {
            remove_cad_entity(world, spawned);
        }

        if let Some(entity) = self.target_entity {
            if let Some(before_topo) = self.before_topology.clone() {
                if let Some(mut ent_mut) = world.get_entity_mut(entity) {
                    ent_mut.insert(before_topo);
                }
            }
            if let Some(before_geom) = self.before_geometry {
                apply_geometry_to_existing_entity(world, entity, before_geom);
            } else {
                rebuild_block_entity_manifold_mesh(world, entity);
            }
        }
    }

    fn description(&self) -> &'static str {
        "Push/Pull Manifold Sub-Face"
    }
}

pub struct MoveObjectCommand {
    entity: Entity,
    before: CadGeometryKind,
    after: CadGeometryKind,
    before_topology: Option<EditableBlockTopology>,
    after_topology: Option<EditableBlockTopology>,
}

impl MoveObjectCommand {
    pub fn new(entity: Entity, before: CadGeometryKind, after: CadGeometryKind) -> Self {
        Self {
            entity,
            before,
            after,
            before_topology: None,
            after_topology: None,
        }
    }
}

impl Command for MoveObjectCommand {
    fn execute(&mut self, world: &mut World) {
        if self.before_topology.is_none() {
            if let Some(topo) = world.get::<EditableBlockTopology>(self.entity).cloned() {
                let delta = match (self.before, self.after) {
                    (
                        CadGeometryKind::Block { center: c0, .. },
                        CadGeometryKind::Block { center: c1, .. },
                    ) => c1 - c0,
                    _ => Vec3::ZERO,
                };
                self.after_topology = Some(topo.translated(delta));
                self.before_topology = Some(topo);
            }
        }
        if let Some(ref after_topo) = self.after_topology {
            if let Some(mut ent_mut) = world.get_entity_mut(self.entity) {
                ent_mut.insert(after_topo.clone());
            }
        }
        apply_geometry_to_existing_entity(world, self.entity, self.after);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(ref before_topo) = self.before_topology {
            if let Some(mut ent_mut) = world.get_entity_mut(self.entity) {
                ent_mut.insert(before_topo.clone());
            }
        }
        apply_geometry_to_existing_entity(world, self.entity, self.before);
    }

    fn description(&self) -> &'static str {
        "Move CAD Object"
    }
}

pub struct ScaleObjectCommand {
    entity: Entity,
    before: CadGeometryKind,
    after: CadGeometryKind,
    before_topology: Option<EditableBlockTopology>,
    after_topology: Option<EditableBlockTopology>,
}

impl ScaleObjectCommand {
    pub fn new(entity: Entity, before: CadGeometryKind, after: CadGeometryKind) -> Self {
        Self {
            entity,
            before,
            after,
            before_topology: None,
            after_topology: None,
        }
    }
}

impl Command for ScaleObjectCommand {
    fn execute(&mut self, world: &mut World) {
        if self.before_topology.is_none() {
            if let Some(topo) = world.get::<EditableBlockTopology>(self.entity).cloned() {
                if let (
                    CadGeometryKind::Block {
                        center: c0,
                        size: s0,
                    },
                    CadGeometryKind::Block {
                        center: c1,
                        size: s1,
                    },
                ) = (self.before, self.after)
                {
                    let factors = Vec3::new(
                        s1.x / s0.x.max(0.05_f32),
                        s1.y / s0.y.max(0.05_f32),
                        s1.z / s0.z.max(0.05_f32),
                    );
                    let shifted = topo.translated(-c0).scaled_from_anchor(Vec3::ZERO, factors).translated(c1);
                    self.after_topology = Some(shifted);
                    self.before_topology = Some(topo);
                }
            }
        }
        if let Some(ref after_topo) = self.after_topology {
            if let Some(mut ent_mut) = world.get_entity_mut(self.entity) {
                ent_mut.insert(after_topo.clone());
            }
        }
        apply_geometry_to_existing_entity(world, self.entity, self.after);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(ref before_topo) = self.before_topology {
            if let Some(mut ent_mut) = world.get_entity_mut(self.entity) {
                ent_mut.insert(before_topo.clone());
            }
        }
        apply_geometry_to_existing_entity(world, self.entity, self.before);
    }

    fn description(&self) -> &'static str {
        "Scale CAD Object"
    }
}

pub struct DeleteCommand {
    snapshots: Vec<(Option<Entity>, CadObject, EditableBlockTopology)>,
}

impl DeleteCommand {
    pub fn new(items: Vec<(Entity, CadObject)>) -> Self {
        let snapshots = items
            .into_iter()
            .map(|(ent, obj)| (Some(ent), obj, EditableBlockTopology::default()))
            .collect();
        Self { snapshots }
    }
}

impl Command for DeleteCommand {
    fn execute(&mut self, world: &mut World) {
        for (maybe_ent, _obj, saved_topo) in &mut self.snapshots {
            if let Some(ent) = maybe_ent.take() {
                if let Some(topo) = world.get::<EditableBlockTopology>(ent).cloned() {
                    *saved_topo = topo;
                }
                remove_cad_entity(world, ent);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for (maybe_ent, obj, saved_topo) in &mut self.snapshots {
            let respawned =
                instantiate_cad_entity_with_topology(world, obj.clone(), saved_topo.clone());
            *maybe_ent = Some(respawned);
        }
    }

    fn description(&self) -> &'static str {
        "Delete Selected Object"
    }
}