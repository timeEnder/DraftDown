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
                flush_history_action_queue.in_set(CadCommandExecutionSet),
            );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(pub u64);

#[derive(Debug, Clone, Copy)]
pub struct MaterialColor(pub Color);

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

    pub fn scaled_from_anchor(&self, pivot: Vec3, factors: Vec3) -> Self {
        let clamped = Vec3::new(
            factors.x.abs().max(0.05),
            factors.y.abs().max(0.05),
            factors.z.abs().max(0.05),
        );

        match *self {
            Self::Line { start, end, radius } => {
                let s = pivot + (start - pivot) * clamped;
                let mut e = pivot + (end - pivot) * clamped;
                if s.distance(e) < 0.05 {
                    let dir = (end - start).try_normalize().unwrap_or(Vec3::X);
                    e = s + dir * 0.05;
                }
                Self::Line {
                    start: s,
                    end: e,
                    radius,
                }
            }
            Self::Block { center, size } => {
                let half = size * 0.5;
                let p_min = pivot + (center - half - pivot) * clamped;
                let p_max = pivot + (center + half - pivot) * clamped;
                let low = p_min.min(p_max);
                let high = p_min.max(p_max);
                let next_size = (high - low).max(Vec3::splat(0.05));
                Self::Block {
                    center: (low + high) * 0.5,
                    size: next_size,
                }
            }
        }
    }

    pub fn centroid(&self) -> Vec3 {
        match *self {
            Self::Line { start, end, .. } => (start + end) * 0.5,
            Self::Block { center, .. } => center,
        }
    }

    pub fn bounds_min_max(&self) -> (Vec3, Vec3) {
        match *self {
            Self::Line { start, end, radius } => {
                let margin = radius.max(0.12);
                let b_min = start.min(end);
                let b_max = start.max(end);
                let span = b_max - b_min;
                let pad = Vec3::new(
                    if span.x < margin * 2.0 { margin } else { 0.0 },
                    if span.y < margin * 2.0 { margin } else { 0.0 },
                    if span.z < margin * 2.0 { margin } else { 0.0 },
                );
                (b_min - pad, b_max + pad)
            }
            Self::Block { center, size } => {
                let half = size.max(Vec3::splat(0.10)) * 0.5;
                (center - half, center + half)
            }
        }
    }

    pub fn bbox_vertices(&self) -> [Vec3; 8] {
        let (lo, hi) = self.bounds_min_max();
        [
            Vec3::new(lo.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(lo.x, hi.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(lo.x, lo.y, hi.z),
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(lo.x, hi.y, hi.z),
            Vec3::new(hi.x, hi.y, hi.z),
        ]
    }

    pub fn edge_segments(&self) -> Vec<(Vec3, Vec3)> {
        match *self {
            Self::Line { start, end, .. } => vec![(start, end)],
            Self::Block { .. } => {
                let v = self.bbox_vertices();
                vec![
                    (v[0], v[1]),
                    (v[1], v[3]),
                    (v[3], v[2]),
                    (v[2], v[0]),
                    (v[4], v[5]),
                    (v[5], v[7]),
                    (v[7], v[6]),
                    (v[6], v[4]),
                    (v[0], v[4]),
                    (v[1], v[5]),
                    (v[2], v[6]),
                    (v[3], v[7]),
                ]
            }
        }
    }

    pub fn summary_text(&self) -> String {
        match *self {
            Self::Line { start, end, .. } => {
                format!(
                    "Line L={:.2}m ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2})",
                    start.distance(end),
                    start.x,
                    start.y,
                    start.z,
                    end.x,
                    end.y,
                    end.z
                )
            }
            Self::Block { size, .. } => {
                format!("Block ({:.2}m x {:.2}m x {:.2}m)", size.x, size.y, size.z)
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

pub trait Command: Send + Sync {
    fn execute(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn description(&self) -> &'static str {
        "CAD Action"
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
        mut cmd: Box<dyn Command + Send + Sync>,
        world: &mut World,
    ) {
        cmd.execute(world);
        self.undo_stack.push(cmd);
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, world: &mut World) -> bool {
        if let Some(mut cmd) = self.undo_stack.pop() {
            cmd.undo(world);
            self.redo_stack.push(cmd);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self, world: &mut World) -> bool {
        if let Some(mut cmd) = self.redo_stack.pop() {
            cmd.execute(world);
            self.undo_stack.push(cmd);
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

pub fn remove_cad_entity(world: &mut World, target: Entity) {
    let attached: Vec<Entity> = world
        .get::<Children>(target)
        .map(|kids| kids.iter().copied().collect())
        .unwrap_or_default();

    for child in attached {
        if world.get_entity(child).is_some() {
            world.despawn(child);
        }
    }
    if world.get_entity(target).is_some() {
        world.despawn(target);
    }
}

pub fn apply_geometry_to_existing_entity(
    world: &mut World,
    entity: Entity,
    geometry: CadGeometryKind,
) {
    if world.get_entity(entity).is_none() {
        return;
    }

    match geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let span = end - start;
            let len = span.length().max(0.001);
            let mid = (start + end) * 0.5;
            let dir = (span / len).try_normalize().unwrap_or(Vec3::Y);
            let rot = Quat::from_rotation_arc(Vec3::Y, dir);

            if let Some(new_mesh) = world
                .get_resource_mut::<Assets<Mesh>>()
                .map(|mut m| m.add(Cylinder::new(radius, len)))
            {
                if let Some(mut h) = world.get_mut::<Handle<Mesh>>(entity) {
                    *h = new_mesh;
                }
            }

            if let Some(mut tr) = world.get_mut::<Transform>(entity) {
                tr.translation = mid;
                tr.rotation = rot;
                tr.scale = Vec3::ONE;
            }

            let kids: Vec<Entity> = world
                .get::<Children>(entity)
                .map(|c| c.iter().copied().collect())
                .unwrap_or_default();
            let half = len * 0.5;
            if let Some(&c0) = kids.first() {
                if let Some(mut tr) = world.get_mut::<Transform>(c0) {
                    tr.translation = Vec3::new(0.0, -half, 0.0);
                }
            }
            if let Some(&c1) = kids.get(1) {
                if let Some(mut tr) = world.get_mut::<Transform>(c1) {
                    tr.translation = Vec3::new(0.0, half, 0.0);
                }
            }
        }
        CadGeometryKind::Block { center, size } => {
            if let Some(new_mesh) = world
                .get_resource_mut::<Assets<Mesh>>()
                .map(|mut m| m.add(Cuboid::new(size.x, size.y, size.z)))
            {
                if let Some(mut h) = world.get_mut::<Handle<Mesh>>(entity) {
                    *h = new_mesh;
                }
            }

            if let Some(mut tr) = world.get_mut::<Transform>(entity) {
                tr.translation = center;
                tr.rotation = Quat::IDENTITY;
                tr.scale = Vec3::ONE;
            }
        }
    }

    if let Some(mut obj) = world.get_mut::<CadObject>(entity) {
        obj.label = geometry.summary_text();
        obj.geometry = geometry;
    }
}

pub fn instantiate_cad_entity(world: &mut World, item: CadObject) -> Option<Entity> {
    match item.geometry {
        CadGeometryKind::Line { start, end, radius } => {
            let vec = end - start;
            let len = vec.length();
            if len <= 0.0001 {
                return None;
            }

            let mid = (start + end) * 0.5;
            let rot = Quat::from_rotation_arc(Vec3::Y, vec / len);

            let (cyl_mesh, cap_mesh) = {
                let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                    return None;
                };
                (
                    meshes.add(Cylinder::new(radius, len)),
                    meshes.add(Sphere::new(radius * 1.6)),
                )
            };

            let (seg_mat, cap_mat) = {
                let Some(mut mats) = world.get_resource_mut::<Assets<StandardMaterial>>() else {
                    return None;
                };
                (
                    mats.add(StandardMaterial {
                        base_color: item.material_color.0,
                        perceptual_roughness: 0.35,
                        reflectance: 0.4,
                        ..default()
                    }),
                    mats.add(StandardMaterial {
                        base_color: Color::srgb(0.95, 0.95, 0.98),
                        unlit: true,
                        ..default()
                    }),
                )
            };

            let half = len * 0.5;
            let root = world
                .spawn((
                    PbrBundle {
                        mesh: cyl_mesh,
                        material: seg_mat,
                        transform: Transform {
                            translation: mid,
                            rotation: rot,
                            scale: Vec3::ONE,
                        },
                        ..default()
                    },
                    item,
                ))
                .with_children(|p| {
                    p.spawn(PbrBundle {
                        mesh: cap_mesh.clone(),
                        material: cap_mat.clone(),
                        transform: Transform::from_xyz(0.0, -half, 0.0),
                        ..default()
                    });
                    p.spawn(PbrBundle {
                        mesh: cap_mesh,
                        material: cap_mat,
                        transform: Transform::from_xyz(0.0, half, 0.0),
                        ..default()
                    });
                })
                .id();
            Some(root)
        }
        CadGeometryKind::Block { center, size } => {
            let box_mesh = {
                let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() else {
                    return None;
                };
                meshes.add(Cuboid::new(size.x, size.y, size.z))
            };

            let box_mat = {
                let Some(mut mats) = world.get_resource_mut::<Assets<StandardMaterial>>() else {
                    return None;
                };
                mats.add(StandardMaterial {
                    base_color: item.material_color.0,
                    perceptual_roughness: 0.42,
                    metallic: 0.06,
                    reflectance: 0.38,
                    ..default()
                })
            };

            let root = world
                .spawn((
                    PbrBundle {
                        mesh: box_mesh,
                        material: box_mat,
                        transform: Transform::from_translation(center),
                        ..default()
                    },
                    item,
                ))
                .id();
            Some(root)
        }
    }
}

#[derive(Debug)]
pub struct DrawLineCommand {
    pub start: Vec3,
    pub end: Vec3,
    pub color: Color,
    pub group_id: Option<GroupId>,
    pub spawned: Option<Entity>,
}

impl DrawLineCommand {
    pub fn new(start: Vec3, end: Vec3, color: Color, group_id: Option<GroupId>) -> Self {
        Self {
            start,
            end,
            color,
            group_id,
            spawned: None,
        }
    }
}

impl Command for DrawLineCommand {
    fn execute(&mut self, world: &mut World) {
        if let Some(old) = self.spawned.take() {
            remove_cad_entity(world, old);
        }
        let geometry = CadGeometryKind::Line {
            start: self.start,
            end: self.end,
            radius: 0.032,
        };
        let obj = CadObject {
            label: geometry.summary_text(),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry,
        };
        self.spawned = instantiate_cad_entity(world, obj);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(ent) = self.spawned.take() {
            remove_cad_entity(world, ent);
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
    pub spawned: Option<Entity>,
}

impl CreateBlockCommand {
    pub fn from_corners(
        a: Vec3,
        b: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let lo = a.min(b);
        let hi = a.max(b);
        let size = (hi - lo).max(Vec3::splat(0.05));
        let center = Vec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z + size.z * 0.5);
        Self {
            center,
            size,
            color,
            group_id,
            spawned: None,
        }
    }

    pub fn from_corner_and_dimensions(
        origin: Vec3,
        dims: Vec3,
        color: Color,
        group_id: Option<GroupId>,
    ) -> Self {
        let far = origin + dims;
        let lo = origin.min(far);
        let hi = origin.max(far);
        let size = (hi - lo).max(Vec3::splat(0.05));
        Self {
            center: (lo + hi) * 0.5,
            size,
            color,
            group_id,
            spawned: None,
        }
    }
}

impl Command for CreateBlockCommand {
    fn execute(&mut self, world: &mut World) {
        if let Some(old) = self.spawned.take() {
            remove_cad_entity(world, old);
        }
        let geometry = CadGeometryKind::Block {
            center: self.center,
            size: self.size,
        };
        let obj = CadObject {
            label: geometry.summary_text(),
            group_id: self.group_id,
            material_color: MaterialColor(self.color),
            geometry,
        };
        self.spawned = instantiate_cad_entity(world, obj);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(ent) = self.spawned.take() {
            remove_cad_entity(world, ent);
        }
    }

    fn description(&self) -> &'static str {
        "Create 3D Block"
    }
}

/// Reversible command for the Offset Tool (`[F]`): spawns a concentric set of offset line segments
/// from a 2D face or connected edge chain and despawns all generated segments on Undo.
#[derive(Debug)]
pub struct CreateOffsetCommand {
    pub segments: Vec<(Vec3, Vec3)>,
    pub color: Color,
    pub group_id: Option<GroupId>,
    pub spawned_entities: Vec<Entity>,
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
        for old in self.spawned_entities.drain(..) {
            remove_cad_entity(world, old);
        }

        for &(start, end) in &self.segments {
            if start.distance(end) <= 0.01 {
                continue;
            }
            let geometry = CadGeometryKind::Line {
                start,
                end,
                radius: 0.032,
            };
            let obj = CadObject {
                label: format!("Offset {}", geometry.summary_text()),
                group_id: self.group_id,
                material_color: MaterialColor(self.color),
                geometry,
            };
            if let Some(ent) = instantiate_cad_entity(world, obj) {
                self.spawned_entities.push(ent);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for ent in self.spawned_entities.drain(..) {
            remove_cad_entity(world, ent);
        }
    }

    fn description(&self) -> &'static str {
        "Offset Contour"
    }
}

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
        apply_geometry_to_existing_entity(world, self.entity, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        apply_geometry_to_existing_entity(world, self.entity, self.before.clone());
    }

    fn description(&self) -> &'static str {
        "Move CAD Object"
    }
}

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
        apply_geometry_to_existing_entity(world, self.entity, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        apply_geometry_to_existing_entity(world, self.entity, self.before.clone());
    }

    fn description(&self) -> &'static str {
        "Scale CAD Object"
    }
}

#[derive(Debug)]
pub struct DeleteCommand {
    pub items: Vec<(Option<Entity>, CadObject)>,
}

impl DeleteCommand {
    pub fn new(snapshots: Vec<(Entity, CadObject)>) -> Self {
        Self {
            items: snapshots.into_iter().map(|(e, o)| (Some(e), o)).collect(),
        }
    }
}

impl Command for DeleteCommand {
    fn execute(&mut self, world: &mut World) {
        for (slot, _) in &mut self.items {
            if let Some(ent) = slot.take() {
                remove_cad_entity(world, ent);
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        for (slot, obj) in &mut self.items {
            if let Some(old) = slot.take() {
                remove_cad_entity(world, old);
            }
            *slot = instantiate_cad_entity(world, obj.clone());
        }
    }

    fn description(&self) -> &'static str {
        "Delete CAD Object(s)"
    }
}

pub fn flush_history_action_queue(world: &mut World) {
    let queued: Vec<HistoryAction> = world
        .get_resource_mut::<PendingHistoryActions>()
        .map(|mut q| std::mem::take(&mut q.queue))
        .unwrap_or_default();

    for action in queued {
        let Some(mut history) = world.remove_resource::<CommandHistory>() else {
            return;
        };
        match action {
            HistoryAction::Execute(cmd) => history.execute_command(cmd, world),
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
