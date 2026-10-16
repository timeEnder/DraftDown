use bevy::prelude::*;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CadCommandExecutionSet;

pub struct CadCommandsPlugin;

impl Plugin for CadCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandHistory>()
            .init_resource::<PendingHistoryActions>()
            .add_systems(
                Update,
                process_pending_history_actions.in_set(CadCommandExecutionSet),
            );
    }
}

// ============================================================================
// CAD OBJECT METADATA FOR GROUPS, MATERIALS & GEOMETRY INSPECTION
// ============================================================================

/// Identifier preparing `CadObject` entities for hierarchical group operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub u64);

/// Stores the intrinsic surface color of a `CadObject` so selection highlighting
/// can temporarily override the material and cleanly restore it on deselection.
#[derive(Debug, Clone, Copy)]
pub struct MaterialColor(pub Color);

/// Geometric descriptor stored on each `CadObject` to power raycast selection,
/// bounding-box handles, neighbor parallel-edge/flush-face inference, and Undo/Redo.
#[derive(Debug, Clone)]
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
    /// Returns a new `CadGeometryKind` translated by `offset`.
    pub fn translated(&self, offset: Vec3) -> Self {
        match *self {
            Self::Line { start, end, radius } => Self::Line {
                start: start + offset,
                end: end + offset,
                radius,
            },
            Self::Block { center, size } => Self::Block {
                center: center + offset,
                size,
            },
        }
    }

    /// Returns a new `CadGeometryKind` scaled relative to `anchor` by `scale_factors`.
    pub fn scaled_from_anchor(&self, anchor: Vec3, scale_factors: Vec3) -> Self {
        let clamped_factors = Vec3::new(
            scale_factors.x.abs().max(0.05),
            scale_factors.y.abs().max(0.05),
            scale_factors.z.abs().max(0.05),
        );

        match *self {
            Self::Line { start, end, radius } => {
                let new_start = anchor + (start - anchor) * clamped_factors;
                let mut new_end = anchor + (end - anchor) * clamped_factors;
                if new_start.distance(new_end) < 0.05 {
                    let dir = (end - start).try_normalize().unwrap_or(Vec3::X);
                    new_end = new_start + dir * 0.05;
                }
                Self::Line {
                    start: new_start,
                    end: new_end,
                    radius,
                }
            }
            Self::Block { center, size } => {
                let half = size * 0.5;
                let min_pt = center - half;
                let max_pt = center + half;
                let scaled_min = anchor + (min_pt - anchor) * clamped_factors;
                let scaled_max = anchor + (max_pt - anchor) * clamped_factors;
                let final_min = scaled_min.min(scaled_max);
                let final_max = scaled_min.max(scaled_max);
                let new_size = Vec3::new(
                    (final_max.x - final_min.x).max(0.05),
                    (final_max.y - final_min.y).max(0.05),
                    (final_max.z - final_min.z).max(0.05),
                );
                let new_center = (final_min + final_max) * 0.5;
                Self::Block {
                    center: new_center,
                    size: new_size,
                }
            }
        }
    }

    /// Returns the world-space center of this geometry.
    pub fn center(&self) -> Vec3 {
        match *self {
            Self::Line { start, end, .. } => (start + end) * 0.5,
            Self::Block { center, .. } => center,
        }
    }

    /// Computes the world-space axis-aligned bounding box `(min_corner, max_corner)`,
    /// ensuring non-zero thickness so 8 scale handles are always distinct and clickable.
    pub fn aabb_min_max(&self) -> (Vec3, Vec3) {
        match *self {
            Self::Line { start, end, radius } => {
                let pad = radius.max(0.12);
                let raw_min = start.min(end);
                let raw_max = start.max(end);
                let span = raw_max - raw_min;
                let extra = Vec3::new(
                    if span.x < pad * 2.0 { pad } else { 0.0 },
                    if span.y < pad * 2.0 { pad } else { 0.0 },
                    if span.z < pad * 2.0 { pad } else { 0.0 },
                );
                (raw_min - extra, raw_max + extra)
            }
            Self::Block { center, size } => {
                let half = size.max(Vec3::splat(0.10)) * 0.5;
                (center - half, center + half)
            }
        }
    }

    /// Returns the 8 corner vertices of the bounding box.
    /// Opposite corner of index `i` is always `7 - i`.
    pub fn corners_8(&self) -> [Vec3; 8] {
        let (min_p, max_p) = self.aabb_min_max();
        [
            Vec3::new(min_p.x, min_p.y, min_p.z), // 0: ---
            Vec3::new(max_p.x, min_p.y, min_p.z), // 1: +--
            Vec3::new(min_p.x, max_p.y, min_p.z), // 2: -+-
            Vec3::new(max_p.x, max_p.y, min_p.z), // 3: ++-
            Vec3::new(min_p.x, min_p.y, max_p.z), // 4: --+
            Vec3::new(max_p.x, min_p.y, max_p.z), // 5: +-+
            Vec3::new(min_p.x, max_p.y, max_p.z), // 6: -++
            Vec3::new(max_p.x, max_p.y, max_p.z), // 7: +++
        ]
    }

    /// Returns all characteristic edge segments `(start, end)` of this geometry
    /// for the Neighbor Parallel Inference Engine.
    pub fn edges(&self) -> Vec<(Vec3, Vec3)> {
        match *self {
            Self::Line { start, end, .. } => vec![(start, end)],
            Self::Block { .. } => {
                let c = self.corners_8();
                vec![
                    // Bottom face edges
                    (c[0], c[1]),
                    (c[1], c[3]),
                    (c[3], c[2]),
                    (c[2], c[0]),
                    // Top face edges
                    (c[4], c[5]),
                    (c[5], c[7]),
                    (c[7], c[6]),
                    (c[6], c[4]),
                    // Vertical pillar edges
                    (c[0], c[4]),
                    (c[1], c[5]),
                    (c[2], c[6]),
                    (c[3], c[7]),
                ]
            }
        }
    }

    pub fn formatted_label(&self) -> String {
        match *self {
            Self::Line { start, end, .. } => {
                let len = start.distance(end);
                format!(
                    "Line L={:.2}m ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2})",
                    len, start.x, start.y, start.z, end.x, end.y, end.z
                )
            }
            Self::Block { size, .. } => {
                format!("Block ({:.2}m x {:.2}m x {:.2}m)", size.x, size.y, size.z)
            }
        }
    }
}

/// Core component attached to all user-created 3D CAD entities in the scene.
#[derive(Component, Debug, Clone)]
pub struct CadObject {
    pub label: String,
    pub group_id: Option<GroupId>,
    pub material_color: MaterialColor,
    pub geometry: CadGeometryKind,
}

// ============================================================================
// COMMAND TRAIT & UNDO / REDO HISTORY STACKS
// ============================================================================

pub trait Command: Send + Sync {
    fn execute(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn description(&self) -> &'static str {
        "CAD Operation"
    }
}

#[derive(Resource, Default)]
pub struct CommandHistory {
    pub undo_stack: Vec<Box<dyn Command + Send + Sync>>,
    pub redo_stack: Vec<Box<dyn Command + Send + Sync>>,
}

impl CommandHistory {
    pub fn execute_command(
        &mut self,
        mut command: Box<dyn Command + Send + Sync>,
        world: &mut World,
    ) {
        command.execute(world);
        self.undo_stack.push(command);
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, world: &mut World) -> bool {
        if let Some(mut command) = self.undo_stack.pop() {
            command.undo(world);
            self.redo_stack.push(command);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self, world: &mut World) -> bool {
        if let Some(mut command) = self.redo_stack.pop() {
            command.execute(world);
            self.undo_stack.push(command);
            true
        } else {
            false
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

pub enum HistoryAction {
    Execute(Box<dyn Command + Send + Sync>),
    Undo,
    Redo,
}

#[derive(Resource, Default)]
pub struct PendingHistoryActions {
    pub queue: Vec<HistoryAction>,
}

/// Safely despawns a `CadObject` entity and any attached child entities (such as line end-caps)
/// using `World::despawn` directly.
pub fn despawn_cad_entity(world: &mut World, entity: Entity) {
    let child_entities: Vec<Entity> = world
        .get::<Children>(entity)
        .map(|children| children.iter().copied().collect())
        .unwrap_or_default();

    for child in child_entities {
        if world.get_entity(child).is_some() {
            world.despawn(child);
        }
    }

    if world.get_entity(entity).is_some() {
        world.despawn(entity);
    }
}

/// Updates an existing `CadObject` entity's mesh, transform, children, and metadata in-place
/// so its `Entity` ID stays stable across Move/Scale Undo/Redo chains.
pub fn update_cad_object_geometry_in_place(
    world: &mut World,
    entity: Entity,
    new_geometry: CadGeometryKind,
) {
    if world.get_entity(entity).is_none() {
        return;
    }

    match new_geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let delta = end - start;
            let length = delta.length().max(0.001);
            let midpoint = (start + end) * 0.5;
            let direction = (delta / length).try_normalize().unwrap_or(Vec3::Y);
            let rotation = Quat::from_rotation_arc(Vec3::Y, direction);

            let new_mesh_handle = world
                .get_resource_mut::<Assets<Mesh>>()
                .map(|mut meshes| meshes.add(Cylinder::new(radius, length)));

            if let Some(mesh_handle) = new_mesh_handle {
                if let Some(mut mesh_comp) = world.get_mut::<Handle<Mesh>>(entity) {
                    *mesh_comp = mesh_handle;
                }
            }

            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                transform.translation = midpoint;
                transform.rotation = rotation;
                transform.scale = Vec3::ONE;
            }

            // Update child endpoint spheres
            let children_vec: Vec<Entity> = world
                .get::<Children>(entity)
                .map(|c| c.iter().copied().collect())
                .unwrap_or_default();
            let half_len = length * 0.5;
            if let Some(&first_child) = children_vec.first() {
                if let Some(mut t) = world.get_mut::<Transform>(first_child) {
                    t.translation = Vec3::new(0.0, -half_len, 0.0);
                }
            }
            if let Some(&second_child) = children_vec.get(1) {
                if let Some(mut t) = world.get_mut::<Transform>(second_child) {
                    t.translation = Vec3::new(0.0, half_len, 0.0);
                }
            }
        }
        CadGeometryKind::Block { center, size } => {
            let new_mesh_handle = world
                .get_resource_mut::<Assets<Mesh>>()
                .map(|mut meshes| meshes.add(Cuboid::new(size.x, size.y, size.z)));

            if let Some(mesh_handle) = new_mesh_handle {
                if let Some(mut mesh_comp) = world.get_mut::<Handle<Mesh>>(entity) {
                    *mesh_comp = mesh_handle;
                }
            }

            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                transform.translation = center;
                transform.rotation = Quat::IDENTITY;
                transform.scale = Vec3::ONE;
            }
        }
    }

    if let Some(mut cad_obj) = world.get_mut::<CadObject>(entity) {
        cad_obj.label = new_geometry.formatted_label();
        cad_obj.geometry = new_geometry;
    }
}

/// Helper that spawns any `CadObject` (Line or Block) into `World` and returns its root `Entity`.
pub fn spawn_cad_object_entity(world: &mut World, cad_object: CadObject) -> Option<Entity> {
    match cad_object.geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let delta = end - start;
            let length = delta.length();
            if length <= 0.0001 {
                warn!("Skipped spawning zero-length CadObject line.");
                return None;
            }

            let midpoint = (start + end) * 0.5;
            let direction = delta / length;
            let rotation = Quat::from_rotation_arc(Vec3::Y, direction);

            let (cylinder_mesh, endpoint_mesh) = {
                let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                    warn!("Assets<Mesh> unavailable while spawning CAD line.");
                    return None;
                };
                (
                    meshes.add(Cylinder::new(radius, length)),
                    meshes.add(Sphere::new(radius * 1.65)),
                )
            };

            let (line_material, endpoint_material) = {
                let Some(mut materials) = world.get_resource_mut::<Assets<StandardMaterial>>()
                else {
                    warn!("Assets<StandardMaterial> unavailable while spawning CAD line.");
                    return None;
                };
                let line_mat = materials.add(StandardMaterial {
                    base_color: cad_object.material_color.0,
                    perceptual_roughness: 0.35,
                    reflectance: 0.4,
                    ..default()
                });
                let cap_mat = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.95, 0.95, 0.98),
                    unlit: true,
                    ..default()
                });
                (line_mat, cap_mat)
            };

            let half_len = length * 0.5;
            let entity = world
                .spawn((
                    PbrBundle {
                        mesh: cylinder_mesh,
                        material: line_material,
                        transform: Transform {
                            translation: midpoint,
                            rotation,
                            scale: Vec3::ONE,
                        },
                        ..default()
                    },
                    cad_object,
                ))
                .with_children(|parent| {
                    parent.spawn(PbrBundle {
                        mesh: endpoint_mesh.clone(),
                        material: endpoint_material.clone(),
                        transform: Transform::from_xyz(0.0, -half_len, 0.0),
                        ..default()
                    });
                    parent.spawn(PbrBundle {
                        mesh: endpoint_mesh,
                        material: endpoint_material,
                        transform: Transform::from_xyz(0.0, half_len, 0.0),
                        ..default()
                    });
                })
                .id();

            Some(entity)
        }
        CadGeometryKind::Block { center, size } => {
            let mesh_handle = {
                let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                    warn!("Assets<Mesh> unavailable while spawning CAD block.");
                    return None;
                };
                meshes.add(Cuboid::new(size.x, size.y, size.z))
            };

            let material_handle = {
                let Some(mut materials) = world.get_resource_mut::<Assets<StandardMaterial>>()
                else {
                    warn!("Assets<StandardMaterial> unavailable while spawning CAD block.");
                    return None;
                };
                materials.add(StandardMaterial {
                    base_color: cad_object.material_color.0,
                    perceptual_roughness: 0.42,
                    metallic: 0.06,
                    reflectance: 0.38,
                    ..default()
                })
            };

            let entity = world
                .spawn((
                    PbrBundle {
                        mesh: mesh_handle,
                        material: material_handle,
                        transform: Transform::from_translation(center),
                        ..default()
                    },
                    cad_object,
                ))
                .id();

            Some(entity)
        }
    }
}

// ============================================================================
// CONCRETE COMMAND IMPLEMENTATIONS (DRAW, BLOCK, MOVE, SCALE, DELETE)
// ============================================================================

#[derive(Debug)]
pub struct DrawLineCommand {
    pub start: Vec3,
    pub end: Vec3,
    pub color: Color,
    pub group_id: Option<GroupId>,
    pub spawned_entity: Option<Entity>,
}

impl DrawLineCommand {
    pub fn new(start: Vec3, end: Vec3, color: Color, group_id: Option<GroupId>) -> Self {
        Self {
            start,
            end,
            color,
            group_id,
            spawned_entity: None,
        }
    }
}

impl Command for DrawLineCommand {
    fn execute(&mut self, world: &mut World) {
        if let Some(existing) = self.spawned_entity.take() {
            despawn_cad_entity(world, existing);
        }

        let geometry = CadGeometryKind::Line {
            start: self.start,
            end: self.end,
            radius: 0.032,
        };
        let cad_obj = CadObject {
            label: geometry.formatted_label(),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry,
        };

        self.spawned_entity = spawn_cad_object_entity(world, cad_obj);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned_entity.take() {
            despawn_cad_entity(world, entity);
        }
    }

    fn description(&self) -> &'static str {
        "Draw 3D Line"
    }
}

#[derive(Debug)]
pub struct CreateBlockCommand {
    pub center: Vec3,
    pub size: Vec3,
    pub color: Color,
    pub group_id: Option<GroupId>,
    pub spawned_entity: Option<Entity>,
}

impl CreateBlockCommand {
    pub fn from_corners(
        corner_a: Vec3,
        corner_b: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let min_pt = corner_a.min(corner_b);
        let max_pt = corner_a.max(corner_b);
        let size = Vec3::new(
            (max_pt.x - min_pt.x).max(0.05),
            (max_pt.y - min_pt.y).max(0.05),
            (max_pt.z - min_pt.z).max(0.05),
        );
        let center = Vec3::new(
            (min_pt.x + max_pt.x) * 0.5,
            (min_pt.y + max_pt.y) * 0.5,
            min_pt.z + size.z * 0.5,
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
        start_corner: Vec3,
        dimensions: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let end_corner = start_corner + dimensions;
        let min_pt = start_corner.min(end_corner);
        let max_pt = start_corner.max(end_corner);
        let size = Vec3::new(
            (max_pt.x - min_pt.x).max(0.05),
            (max_pt.y - min_pt.y).max(0.05),
            (max_pt.z - min_pt.z).max(0.05),
        );
        let center = (min_pt + max_pt) * 0.5;
        Self {
            center,
            size,
            color,
            group_id,
            spawned_entity: None,
        }
    }
}

impl Command for CreateBlockCommand {
    fn execute(&mut self, world: &mut World) {
        if let Some(existing) = self.spawned_entity.take() {
            despawn_cad_entity(world, existing);
        }

        let geometry = CadGeometryKind::Block {
            center: self.center,
            size: self.size,
        };
        let cad_obj = CadObject {
            label: geometry.formatted_label(),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry,
        };

        self.spawned_entity = spawn_cad_object_entity(world, cad_obj);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = self.spawned_entity.take() {
            despawn_cad_entity(world, entity);
        }
    }

    fn description(&self) -> &'static str {
        "Create 3D Block"
    }
}

/// Reversible command that translates a `CadObject` entity from `before` to `after`.
#[derive(Debug)]
pub struct MoveObjectCommand {
    pub entity: Entity,
    pub before: CadGeometryKind,
    pub after: CadGeometryKind,
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
        update_cad_object_geometry_in_place(world, self.entity, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        update_cad_object_geometry_in_place(world, self.entity, self.before.clone());
    }

    fn description(&self) -> &'static str {
        "Move CAD Object"
    }
}

/// Reversible command that scales/resizes a `CadObject` entity's mesh dimensions
/// from `before` to `after` via bounding-box handle manipulation or VCB scale input.
#[derive(Debug)]
pub struct ScaleObjectCommand {
    pub entity: Entity,
    pub before: CadGeometryKind,
    pub after: CadGeometryKind,
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
        update_cad_object_geometry_in_place(world, self.entity, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        update_cad_object_geometry_in_place(world, self.entity, self.before.clone());
    }

    fn description(&self) -> &'static str {
        "Scale CAD Object"
    }
}

/// Reversible command that deletes one or more selected `CadObject` entities on `execute`
/// and restores their exact geometry, `GroupId`, and `MaterialColor` on `undo`.
#[derive(Debug)]
pub struct DeleteCommand {
    pub deleted_items: Vec<(Option<Entity>, CadObject)>,
}

impl DeleteCommand {
    pub fn new(snapshots: Vec<(Entity, CadObject)>) -> Self {
        let deleted_items = snapshots
            .into_iter()
            .map(|(entity, obj)| (Some(entity), obj))
            .collect();
        Self { deleted_items }
    }
}

impl Command for DeleteCommand {
    fn execute(&mut self, world: &mut World) {
        for (entity_slot, _) in &mut self.deleted_items {
            if let Some(entity) = entity_slot.take() {
                despawn_cad_entity(world, entity);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for (entity_slot, cad_obj) in &mut self.deleted_items {
            if let Some(existing) = entity_slot.take() {
                despawn_cad_entity(world, existing);
            }
            *entity_slot = spawn_cad_object_entity(world, cad_obj.clone());
        }
    }

    fn description(&self) -> &'static str {
        "Delete CAD Object(s)"
    }
}

/// Exclusive system that drains queued `HistoryAction` items (`Execute`, `Undo`, `Redo`).
pub fn process_pending_history_actions(world: &mut World) {
    let actions: Vec<HistoryAction> = world
        .get_resource_mut::<PendingHistoryActions>()
        .map(|mut pending| std::mem::take(&mut pending.queue))
        .unwrap_or_default();

    if actions.is_empty() {
        return;
    }

    for action in actions {
        let Some(mut history) = world.remove_resource::<CommandHistory>() else {
            warn!("CommandHistory resource missing while processing history actions.");
            return;
        };

        match action {
            HistoryAction::Execute(cmd) => {
                history.execute_command(cmd, world);
            }
            HistoryAction::Undo => {
                history.undo(world);
            }
            HistoryAction::Redo => {
                history.redo(world);
            }
        }

        world.insert_resource(history);
    }
}
