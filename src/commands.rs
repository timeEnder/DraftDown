use std::collections::VecDeque;

use bevy::math::primitives::{Cuboid, Cylinder, Sphere};
use bevy::prelude::*;

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

pub fn instantiate_cad_entity(world: &mut World, cad_obj: CadObject) -> Entity {
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
            let (box_mesh, box_mat) = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let m = meshes.add(Cuboid::from_size(clamped));
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
            let clamped = size.max(Vec3::splat(0.05_f32));
            let new_box_mesh = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                meshes.add(Cuboid::from_size(clamped))
            };

            if let Some(mut ent_mut) = world.get_entity_mut(target) {
                if let Some(mut obj) = ent_mut.get_mut::<CadObject>() {
                    obj.geometry = new_geometry;
                }
                if let Some(mut tf) = ent_mut.get_mut::<Transform>() {
                    tf.translation = center;
                    tf.scale = Vec3::ONE;
                }
                if let Some(mut mesh_handle) = ent_mut.get_mut::<Handle<Mesh>>() {
                    *mesh_handle = new_box_mesh;
                }
            }
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
    segments: Vec<(Vec3, Vec3)>,
    color: Color,
    group_id: Option<GroupId>,
    spawned_entities: Vec<Entity>,
}

impl CreateOffsetCommand {
    pub fn new(
        segments: Vec<(Vec3, Vec3)>,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        Self {
            segments,
            color,
            group_id,
            spawned_entities: Vec::new(),
        }
    }
}

impl Command for CreateOffsetCommand {
    fn execute(&mut self, world: &mut World) {
        self.spawned_entities.clear();
        let resolved_group = self.group_id.unwrap_or(GroupId(1));

        for (start, end) in self.segments.clone() {
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
        for ent in self.spawned_entities.drain(..) {
            remove_cad_entity(world, ent);
        }
    }

    fn description(&self) -> &'static str {
        "Offset Coplanar Contour"
    }
}

pub struct PushPullCommand {
    target_entity: Option<Entity>,
    before_geometry: Option<CadGeometryKind>,
    after_geometry: Option<CadGeometryKind>,
    extruded_solid_block: Option<(Vec3, Vec3)>,
    boundary_wall_segments: Vec<(Vec3, Vec3)>,
    color: Color,
    group_id: Option<GroupId>,
    spawned_entities: Vec<Entity>,
}

impl PushPullCommand {
    pub fn for_block_face(
        entity: Entity,
        before: CadGeometryKind,
        after: CadGeometryKind,
        boundary_wall_segments: Vec<(Vec3, Vec3)>,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        Self {
            target_entity: Some(entity),
            before_geometry: Some(before),
            after_geometry: Some(after),
            extruded_solid_block: None,
            boundary_wall_segments,
            color,
            group_id,
            spawned_entities: Vec::new(),
        }
    }

    pub fn for_closed_face_loop(
        base_vertices: &[Vec3],
        normal: Dir3,
        distance: f32,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let shift = normal.as_vec3() * distance;
        let count = base_vertices.len();
        let mut wall_segments = Vec::with_capacity(count * 2);

        let mut min_pt = Vec3::splat(f32::MAX);
        let mut max_pt = Vec3::splat(f32::MIN);

        for i in 0..count {
            let b0 = base_vertices[i];
            let b1 = base_vertices[(i + 1) % count];
            let t0 = b0 + shift;
            let t1 = b1 + shift;

            wall_segments.push((b0, t0));
            wall_segments.push((t0, t1));

            min_pt = min_pt.min(b0).min(t0);
            max_pt = max_pt.max(b0).max(t0);
        }

        let span = (max_pt - min_pt).max(Vec3::splat(0.1_f32));
        let center = (min_pt + max_pt) * 0.5_f32;

        Self {
            target_entity: None,
            before_geometry: None,
            after_geometry: None,
            extruded_solid_block: Some((center, span)),
            boundary_wall_segments: wall_segments,
            color,
            group_id,
            spawned_entities: Vec::new(),
        }
    }
}

impl Command for PushPullCommand {
    fn execute(&mut self, world: &mut World) {
        self.spawned_entities.clear();

        if let (Some(entity), Some(after)) = (self.target_entity, self.after_geometry) {
            apply_geometry_to_existing_entity(world, entity, after);
        }

        if let Some((center, size)) = self.extruded_solid_block {
            let serial = world
                .get_resource_mut::<NextObjectSerial>()
                .map(|mut s| s.next_id())
                .unwrap_or(1);

            let block_obj = CadObject {
                label: format!("Extruded Block #{serial}"),
                group_id: self.group_id,
                material_color: MaterialColor(self.color),
                geometry: CadGeometryKind::Block { center, size },
            };
            let ent = instantiate_cad_entity(world, block_obj);
            self.spawned_entities.push(ent);
        }

        let wall_color = Color::srgb(0.86, 0.90, 0.96);
        for (start, end) in self.boundary_wall_segments.clone() {
            if start.distance(end) < 0.01_f32 {
                continue;
            }
            let serial = world
                .get_resource_mut::<NextObjectSerial>()
                .map(|mut s| s.next_id())
                .unwrap_or(1);

            let wall_edge = CadObject {
                label: format!("Extrude Wall Edge #{serial}"),
                group_id: self.group_id,
                material_color: MaterialColor(wall_color),
                geometry: CadGeometryKind::Line {
                    start,
                    end,
                    radius: 0.036_f32,
                },
            };
            let ent = instantiate_cad_entity(world, wall_edge);
            self.spawned_entities.push(ent);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for ent in self.spawned_entities.drain(..) {
            remove_cad_entity(world, ent);
        }

        if let (Some(entity), Some(before)) = (self.target_entity, self.before_geometry) {
            apply_geometry_to_existing_entity(world, entity, before);
        }
    }

    fn description(&self) -> &'static str {
        "Push/Pull Extrude Face"
    }
}

pub struct MoveObjectCommand {
    entity: Entity,
    before: CadGeometryKind,
    after: CadGeometryKind,
}

impl MoveObjectCommand {
    pub fn new(entity: Entity, before: CadGeometryKind, after: CadGeometryKind) -> Self {
        Self {
            entity,
            before,
            after,
        }
    }
}

impl Command for MoveObjectCommand {
    fn execute(&mut self, world: &mut World) {
        apply_geometry_to_existing_entity(world, self.entity, self.after);
    }

    fn undo(&mut self, world: &mut World) {
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
}

impl ScaleObjectCommand {
    pub fn new(entity: Entity, before: CadGeometryKind, after: CadGeometryKind) -> Self {
        Self {
            entity,
            before,
            after,
        }
    }
}

impl Command for ScaleObjectCommand {
    fn execute(&mut self, world: &mut World) {
        apply_geometry_to_existing_entity(world, self.entity, self.after);
    }

    fn undo(&mut self, world: &mut World) {
        apply_geometry_to_existing_entity(world, self.entity, self.before);
    }

    fn description(&self) -> &'static str {
        "Scale CAD Object"
    }
}

pub struct DeleteCommand {
    snapshots: Vec<(Option<Entity>, CadObject)>,
}

impl DeleteCommand {
    pub fn new(items: Vec<(Entity, CadObject)>) -> Self {
        let snapshots = items
            .into_iter()
            .map(|(ent, obj)| (Some(ent), obj))
            .collect();
        Self { snapshots }
    }
}

impl Command for DeleteCommand {
    fn execute(&mut self, world: &mut World) {
        for (maybe_ent, _obj) in &mut self.snapshots {
            if let Some(ent) = maybe_ent.take() {
                remove_cad_entity(world, ent);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for (maybe_ent, obj) in &mut self.snapshots {
            let respawned = instantiate_cad_entity(world, obj.clone());
            *maybe_ent = Some(respawned);
        }
    }

    fn description(&self) -> &'static str {
        "Delete Selected Object"
    }
}