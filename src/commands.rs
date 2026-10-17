use std::collections::VecDeque;

use bevy::math::primitives::{Cuboid, Cylinder, Sphere};
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
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
pub struct ManifoldPolygonFace {
    pub subface_id: usize,
    pub region_kind: SubFaceRegionKind,
    pub normal: Dir3,
    pub outer_loop: [Vec3; 4],
    pub inner_holes: Vec<[Vec3; 4]>,
}

impl ManifoldPolygonFace {
    pub fn plane_origin(&self) -> Vec3 {
        (self.outer_loop[0] + self.outer_loop[1] + self.outer_loop[2] + self.outer_loop[3])
            * 0.25_f32
    }

    pub fn translated(&self, delta: Vec3) -> Self {
        let mut moved = self.clone();
        for v in &mut moved.outer_loop {
            *v += delta;
        }
        for hole in &mut moved.inner_holes {
            for v in hole {
                *v += delta;
            }
        }
        moved
    }

    pub fn scaled_from_anchor(&self, anchor: Vec3, factors: Vec3) -> Self {
        let mut scaled = self.clone();
        for v in &mut scaled.outer_loop {
            *v = anchor + (*v - anchor) * factors;
        }
        for hole in &mut scaled.inner_holes {
            for v in hole {
                *v = anchor + (*v - anchor) * factors;
            }
        }
        scaled
    }
}

pub fn strip_subface_label_suffix(label: &str) -> String {
    if let Some(bracket_pos) = label.find(" [") {
        label[..bracket_pos].to_string()
    } else {
        label.to_string()
    }
}

fn project_loop_2d_with_basis(
    loop_3d: &[Vec3; 4],
    origin: Vec3,
    u_axis: Vec3,
    v_axis: Vec3,
) -> [Vec2; 4] {
    [
        Vec2::new(
            (loop_3d[0] - origin).dot(u_axis),
            (loop_3d[0] - origin).dot(v_axis),
        ),
        Vec2::new(
            (loop_3d[1] - origin).dot(u_axis),
            (loop_3d[1] - origin).dot(v_axis),
        ),
        Vec2::new(
            (loop_3d[2] - origin).dot(u_axis),
            (loop_3d[2] - origin).dot(v_axis),
        ),
        Vec2::new(
            (loop_3d[3] - origin).dot(u_axis),
            (loop_3d[3] - origin).dot(v_axis),
        ),
    ]
}

fn point_in_quad_2d(pt: Vec2, quad: &[Vec2; 4]) -> bool {
    let mut inside = false;
    let mut j = 3_usize;
    for i in 0..4 {
        let pi = quad[i];
        let pj = quad[j];
        let intersects = ((pi.y > pt.y) != (pj.y > pt.y))
            && (pt.x < (pj.x - pi.x) * (pt.y - pi.y) / ((pj.y - pi.y) + 1e-9_f32) + pi.x);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn build_plane_uv_axes(normal: Vec3) -> (Vec3, Vec3) {
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

#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct EditableBlockTopology {
    pub faces: Vec<ManifoldPolygonFace>,
    pub next_subface_id: usize,
    pub selected_subface_id: Option<usize>,
    pub base_color_rgba: [f32; 4],
    pub base_label: String,
}

impl EditableBlockTopology {
    pub fn from_box(center: Vec3, size: Vec3) -> Self {
        let clamped = size.max(Vec3::splat(0.05_f32));
        let half = clamped * 0.5_f32;
        let min = center - half;
        let max = center + half;

        let faces = vec![
            ManifoldPolygonFace {
                subface_id: 0,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::Z,
                outer_loop: [
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                inner_holes: Vec::new(),
            },
            ManifoldPolygonFace {
                subface_id: 1,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::NEG_Z,
                outer_loop: [
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(max.x, min.y, min.z),
                ],
                inner_holes: Vec::new(),
            },
            ManifoldPolygonFace {
                subface_id: 2,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::X,
                outer_loop: [
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                ],
                inner_holes: Vec::new(),
            },
            ManifoldPolygonFace {
                subface_id: 3,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::NEG_X,
                outer_loop: [
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ],
                inner_holes: Vec::new(),
            },
            ManifoldPolygonFace {
                subface_id: 4,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::Y,
                outer_loop: [
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(min.x, max.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                ],
                inner_holes: Vec::new(),
            },
            ManifoldPolygonFace {
                subface_id: 5,
                region_kind: SubFaceRegionKind::WholeFace,
                normal: Dir3::NEG_Y,
                outer_loop: [
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(min.x, min.y, max.z),
                ],
                inner_holes: Vec::new(),
            },
        ];

        Self {
            faces,
            next_subface_id: 6,
            selected_subface_id: None,
            base_color_rgba: [0.76, 0.80, 0.86, 1.0],
            base_label: String::new(),
        }
    }

    pub fn ensure_initialized(&mut self, center: Vec3, size: Vec3) {
        if self.faces.is_empty() {
            let prev_color = self.base_color_rgba;
            let prev_label = self.base_label.clone();
            *self = Self::from_box(center, size);
            if prev_color[3] > 0.0_f32 {
                self.base_color_rgba = prev_color;
            }
            if !prev_label.is_empty() {
                self.base_label = prev_label;
            }
        }
        if self.base_color_rgba[3] <= 0.0_f32 {
            self.base_color_rgba = [0.76, 0.80, 0.86, 1.0];
        }
        let min_next = self
            .faces
            .iter()
            .map(|f| f.subface_id.saturating_add(1))
            .max()
            .unwrap_or(6);
        if self.next_subface_id < min_next {
            self.next_subface_id = min_next;
        }
    }

    pub fn allocate_subface_id(&mut self) -> usize {
        let min_next = self
            .faces
            .iter()
            .map(|f| f.subface_id.saturating_add(1))
            .max()
            .unwrap_or(0);
        if self.next_subface_id < min_next {
            self.next_subface_id = min_next;
        }
        let id = self.next_subface_id;
        self.next_subface_id = self.next_subface_id.saturating_add(1);
        id
    }

    pub fn find_subface_by_id(&self, subface_id: usize) -> Option<&ManifoldPolygonFace> {
        self.faces.iter().find(|f| f.subface_id == subface_id)
    }

    pub fn delete_subface_by_id(&mut self, subface_id: usize) -> bool {
        let Some(pos) = self.faces.iter().position(|f| f.subface_id == subface_id) else {
            return false;
        };
        if self.faces.len() <= 1 {
            return false;
        }
        self.faces.remove(pos);
        if self.selected_subface_id == Some(subface_id) {
            self.selected_subface_id = None;
        }
        true
    }

    pub fn formatted_selection_label(&self, fallback_label: &str) -> String {
        let base = if self.base_label.is_empty() {
            strip_subface_label_suffix(fallback_label)
        } else {
            self.base_label.clone()
        };
        if let Some(sel_id) = self.selected_subface_id {
            if let Some(subface) = self.find_subface_by_id(sel_id) {
                return format!("{} [{} #{}]", base, subface.region_kind.label(), sel_id);
            }
        }
        base
    }

    pub fn compute_bounds(&self) -> Option<(Vec3, Vec3)> {
        if self.faces.is_empty() {
            return None;
        }
        let mut min_pt = Vec3::splat(f32::MAX);
        let mut max_pt = Vec3::splat(f32::MIN);
        for face in &self.faces {
            for v in &face.outer_loop {
                min_pt = min_pt.min(*v);
                max_pt = max_pt.max(*v);
            }
            for hole in &face.inner_holes {
                for v in hole {
                    min_pt = min_pt.min(*v);
                    max_pt = max_pt.max(*v);
                }
            }
        }
        let size = (max_pt - min_pt).max(Vec3::splat(0.05_f32));
        let center = (min_pt + max_pt) * 0.5_f32;
        Some((center, size))
    }

    pub fn translated(&self, delta: Vec3) -> Self {
        Self {
            faces: self.faces.iter().map(|f| f.translated(delta)).collect(),
            next_subface_id: self.next_subface_id,
            selected_subface_id: self.selected_subface_id,
            base_color_rgba: self.base_color_rgba,
            base_label: self.base_label.clone(),
        }
    }

    pub fn scaled_from_anchor(&self, anchor: Vec3, factors: Vec3) -> Self {
        Self {
            faces: self
                .faces
                .iter()
                .map(|f| f.scaled_from_anchor(anchor, factors))
                .collect(),
            next_subface_id: self.next_subface_id,
            selected_subface_id: self.selected_subface_id,
            base_color_rgba: self.base_color_rgba,
            base_label: self.base_label.clone(),
        }
    }

    pub fn find_matching_face_index(
        &self,
        face_normal: Dir3,
        target_loop: &[Vec3; 4],
    ) -> Option<usize> {
        let target_centroid =
            (target_loop[0] + target_loop[1] + target_loop[2] + target_loop[3]) * 0.25_f32;
        let n_vec = face_normal.as_vec3();

        let mut best_idx: Option<usize> = None;
        let mut best_score = f32::MAX;

        for (idx, face) in self.faces.iter().enumerate() {
            if face.normal.as_vec3().dot(n_vec) < 0.98_f32 {
                continue;
            }
            let plane_dist = (face.plane_origin() - target_centroid).dot(n_vec).abs();
            if plane_dist > 0.03_f32 {
                continue;
            }
            let aligned = align_inner_loop_to_outer(&face.outer_loop, target_loop, n_vec);
            let mut corner_err = 0.0_f32;
            for k in 0..4 {
                corner_err += face.outer_loop[k].distance(aligned[k]);
            }
            if corner_err < best_score {
                best_score = corner_err;
                best_idx = Some(idx);
            }
        }

        if best_score <= 0.40_f32 {
            best_idx
        } else {
            None
        }
    }

    fn find_opposing_back_face(
        &self,
        face_idx: usize,
        front_loop_ccw: &[Vec3; 4],
        n_vec: Vec3,
    ) -> Option<(usize, f32)> {
        let front_centroid =
            (front_loop_ccw[0] + front_loop_ccw[1] + front_loop_ccw[2] + front_loop_ccw[3])
                * 0.25_f32;
        let mut best_back: Option<(usize, f32)> = None;

        for (idx, candidate) in self.faces.iter().enumerate() {
            if idx == face_idx {
                continue;
            }
            let cand_n = candidate.normal.as_vec3();
            if cand_n.dot(n_vec) > -0.98_f32 {
                continue;
            }
            let cand_origin = candidate.plane_origin();
            let thickness = (front_centroid - cand_origin).dot(n_vec);
            if thickness <= 0.03_f32 {
                continue;
            }

            let projected_center = front_centroid - n_vec * thickness;
            let (u_axis, v_axis) = build_plane_uv_axes(cand_n);
            let quad_uv =
                project_loop_2d_with_basis(&candidate.outer_loop, cand_origin, u_axis, v_axis);
            let pt_uv = Vec2::new(
                (projected_center - cand_origin).dot(u_axis),
                (projected_center - cand_origin).dot(v_axis),
            );
            if !point_in_quad_2d(pt_uv, &quad_uv) {
                continue;
            }

            match best_back {
                Some((_, prev_t)) if thickness >= prev_t => {}
                _ => {
                    best_back = Some((idx, thickness));
                }
            }
        }

        best_back
    }

    pub fn apply_offset_split(
        &mut self,
        face_normal: Dir3,
        base_face_loop: [Vec3; 4],
        offset_loop: [Vec3; 4],
        signed_distance: f32,
    ) -> usize {
        let n_vec = face_normal.as_vec3();
        let base_ccw = orient_loop_ccw(&base_face_loop, n_vec);
        let offset_ccw = align_inner_loop_to_outer(&base_ccw, &offset_loop, n_vec);

        if signed_distance <= 0.0_f32 {
            let inherited_holes =
                if let Some(idx) = self.find_matching_face_index(face_normal, &base_ccw) {
                    self.faces[idx].region_kind = SubFaceRegionKind::OuterFrame;
                    std::mem::replace(&mut self.faces[idx].inner_holes, vec![offset_ccw])
                } else {
                    let frame_id = self.allocate_subface_id();
                    self.faces.push(ManifoldPolygonFace {
                        subface_id: frame_id,
                        region_kind: SubFaceRegionKind::OuterFrame,
                        normal: face_normal,
                        outer_loop: base_ccw,
                        inner_holes: vec![offset_ccw],
                    });
                    Vec::new()
                };

            let inner_id = self.allocate_subface_id();
            self.faces.push(ManifoldPolygonFace {
                subface_id: inner_id,
                region_kind: SubFaceRegionKind::InnerLoop,
                normal: face_normal,
                outer_loop: offset_ccw,
                inner_holes: inherited_holes,
            });
            self.selected_subface_id = Some(inner_id);
            inner_id
        } else {
            if let Some(idx) = self.find_matching_face_index(face_normal, &base_ccw) {
                if self.faces[idx].inner_holes.is_empty() {
                    self.faces[idx].region_kind = SubFaceRegionKind::InnerLoop;
                }
            }
            let outer_id = self.allocate_subface_id();
            self.faces.push(ManifoldPolygonFace {
                subface_id: outer_id,
                region_kind: SubFaceRegionKind::OuterFrame,
                normal: face_normal,
                outer_loop: offset_ccw,
                inner_holes: vec![base_ccw],
            });
            self.selected_subface_id = Some(outer_id);
            outer_id
        }
    }

    pub fn extrude_face_region(
        &mut self,
        face_normal: Dir3,
        target_outer_loop: [Vec3; 4],
        region_kind: SubFaceRegionKind,
        distance: f32,
    ) {
        if distance.abs() < 0.002_f32 {
            return;
        }
        let n_vec = face_normal.as_vec3();
        let target_ccw = orient_loop_ccw(&target_outer_loop, n_vec);

        let Some(face_idx) = self.find_matching_face_index(face_normal, &target_ccw) else {
            return;
        };

        let target_subface_id = self.faces[face_idx].subface_id;
        let old_outer = orient_loop_ccw(&self.faces[face_idx].outer_loop, n_vec);
        let old_holes = self.faces[face_idx].inner_holes.clone();
        let effective_kind = if region_kind != SubFaceRegionKind::WholeFace {
            region_kind
        } else {
            self.faces[face_idx].region_kind
        };

        // 1. Check if old_outer is an inner hole of a coplanar surrounding frame face
        let mut is_hole_of_coplanar_frame = false;
        for (other_idx, other_face) in self.faces.iter().enumerate() {
            if other_idx == face_idx {
                continue;
            }
            if other_face.normal.as_vec3().dot(n_vec) > 0.98_f32 {
                for hole in &other_face.inner_holes {
                    let aligned_h = align_inner_loop_to_outer(&old_outer, hole, n_vec);
                    let err: f32 = (0..4).map(|k| old_outer[k].distance(aligned_h[k])).sum();
                    if err < 0.15_f32 {
                        is_hole_of_coplanar_frame = true;
                        break;
                    }
                }
            }
        }

        let opposing_back = self.find_opposing_back_face(face_idx, &old_outer, n_vec);

        // 2. BOOLEAN THROUGH-HOLE CUTTING:
        // If an Inner Sub-Face (either flush with a frame or already partially recessed) is pushed
        // backwards (distance < 0.0) and its depth reaches or exceeds the thickness to the opposing back face:
        let is_inner_subface =
            effective_kind == SubFaceRegionKind::InnerLoop || is_hole_of_coplanar_frame;
        if is_inner_subface && old_holes.is_empty() && distance < 0.0_f32 {
            if let Some((back_idx, thickness)) = opposing_back {
                let push_depth = -distance;
                if push_depth >= thickness - 0.04_f32 {
                    let back_origin = self.faces[back_idx].plane_origin();
                    let back_n = self.faces[back_idx].normal.as_vec3();
                    let cut_shift = -n_vec * thickness;

                    let mut back_opening = [Vec3::ZERO; 4];
                    for k in 0..4 {
                        let raw_pt = old_outer[k] + cut_shift;
                        let residual = (raw_pt - back_origin).dot(back_n);
                        back_opening[k] = raw_pt - back_n * residual;
                    }

                    // Cut the matching hole in the opposing BACK face of the block
                    let back_outer_ccw =
                        orient_loop_ccw(&self.faces[back_idx].outer_loop, back_n);
                    let back_hole_ccw =
                        align_inner_loop_to_outer(&back_outer_ccw, &back_opening, back_n);

                    let mut hole_already_present = false;
                    for existing_hole in &self.faces[back_idx].inner_holes {
                        let aligned_existing =
                            align_inner_loop_to_outer(&back_outer_ccw, existing_hole, back_n);
                        let err: f32 = (0..4)
                            .map(|k| back_hole_ccw[k].distance(aligned_existing[k]))
                            .sum();
                        if err < 0.15_f32 {
                            hole_already_present = true;
                            break;
                        }
                    }
                    if !hole_already_present {
                        self.faces[back_idx].inner_holes.push(back_hole_ccw);
                        self.faces[back_idx].region_kind = SubFaceRegionKind::OuterFrame;
                    }

                    if is_hole_of_coplanar_frame {
                        // Stitch 4 Counter-Clockwise interior tunnel side-walls connecting front opening to back opening
                        for k in 0..4 {
                            let next_k = (k + 1) % 4;
                            let b0 = old_outer[k];
                            let b1 = old_outer[next_k];
                            let t1 = back_opening[next_k];
                            let t0 = back_opening[k];

                            let edge_dir = (b1 - b0).normalize_or_zero();
                            let outward_ring_normal = edge_dir.cross(n_vec).normalize_or_zero();
                            let wall_normal_vec = -outward_ring_normal;

                            if let Ok(wall_dir) = Dir3::new(wall_normal_vec) {
                                let wall_loop =
                                    orient_loop_ccw(&[b0, b1, t1, t0], wall_normal_vec);
                                let wall_id = self.allocate_subface_id();
                                self.faces.push(ManifoldPolygonFace {
                                    subface_id: wall_id,
                                    region_kind: SubFaceRegionKind::WholeFace,
                                    normal: wall_dir,
                                    outer_loop: wall_loop,
                                    inner_holes: Vec::new(),
                                });
                            }
                        }
                    } else {
                        // Slide existing pocket side-wall vertices to stitch flush onto the back-face opening
                        for (other_idx, other_face) in self.faces.iter_mut().enumerate() {
                            if other_idx == face_idx || other_idx == back_idx {
                                continue;
                            }
                            if other_face.normal.as_vec3().dot(n_vec).abs() < 0.95_f32 {
                                for v in &mut other_face.outer_loop {
                                    for k in 0..4 {
                                        if v.distance(old_outer[k]) < 0.03_f32 {
                                            *v = back_opening[k];
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Delete the overlapping pushed cap face so the tunnel is completely open at both ends
                    self.faces.remove(face_idx);
                    if self.selected_subface_id == Some(target_subface_id) {
                        self.selected_subface_id = None;
                    }
                    return;
                }
            }
        }

        // Clamp inward push distance for WholeFace or OuterFrame so the block never inverts through its back wall
        let clamped_distance = if distance < 0.0_f32 {
            if let Some((_back_idx, thickness)) = opposing_back {
                let min_remaining = if is_inner_subface { 0.02_f32 } else { 0.10_f32 };
                distance.max(-(thickness - min_remaining).max(0.02_f32))
            } else {
                distance
            }
        } else {
            distance
        };

        if clamped_distance.abs() < 0.002_f32 {
            return;
        }
        let shift = n_vec * clamped_distance;

        // 3. If this face is NOT a flush hole inside a coplanar frame and has NO inner holes
        // (i.e. either a main WholeFace of the block or an already-extruded/recessed InnerLoop cap),
        // slide its outer_loop AND the shared vertices on its attached perpendicular side walls!
        if !is_hole_of_coplanar_frame && old_holes.is_empty() {
            for v in &mut self.faces[face_idx].outer_loop {
                *v += shift;
            }
            for (other_idx, other_face) in self.faces.iter_mut().enumerate() {
                if other_idx == face_idx {
                    continue;
                }
                if other_face.normal.as_vec3().dot(n_vec).abs() < 0.95_f32 {
                    for v in &mut other_face.outer_loop {
                        if old_outer.iter().any(|orig| orig.distance(*v) < 0.025_f32) {
                            *v += shift;
                        }
                    }
                }
            }
            return;
        }

        // 4. If this is an OuterFrame face (!old_holes.is_empty()) and its outer_loop is the block's
        // exterior perimeter (!is_hole_of_coplanar_frame), slide the shared vertices on the block's
        // perpendicular exterior walls instead of leaving hollow fins behind!
        if !is_hole_of_coplanar_frame && !old_holes.is_empty() {
            for v in &mut self.faces[face_idx].outer_loop {
                *v += shift;
            }
            for hole in &mut self.faces[face_idx].inner_holes {
                for v in hole {
                    *v += shift;
                }
            }
            for (other_idx, other_face) in self.faces.iter_mut().enumerate() {
                if other_idx == face_idx {
                    continue;
                }
                if other_face.normal.as_vec3().dot(n_vec).abs() < 0.95_f32 {
                    for v in &mut other_face.outer_loop {
                        if old_outer.iter().any(|orig| orig.distance(*v) < 0.025_f32) {
                            *v += shift;
                        }
                    }
                }
            }

            // Stitch 4 inner step side-wall faces along each hole -> hole + shift
            for hole in old_holes {
                let hole_ccw = orient_loop_ccw(&hole, n_vec);
                for k in 0..4 {
                    let next_k = (k + 1) % 4;
                    let b0 = hole_ccw[k];
                    let b1 = hole_ccw[next_k];
                    let t1 = b1 + shift;
                    let t0 = b0 + shift;

                    let edge_dir = (b1 - b0).normalize_or_zero();
                    let outward_ring_normal = edge_dir.cross(n_vec).normalize_or_zero();
                    let wall_normal_vec = if clamped_distance >= 0.0_f32 {
                        -outward_ring_normal
                    } else {
                        outward_ring_normal
                    };

                    if let Ok(wall_dir) = Dir3::new(wall_normal_vec) {
                        let wall_loop = orient_loop_ccw(&[b0, b1, t1, t0], wall_normal_vec);
                        let wall_id = self.allocate_subface_id();
                        self.faces.push(ManifoldPolygonFace {
                            subface_id: wall_id,
                            region_kind: SubFaceRegionKind::WholeFace,
                            normal: wall_dir,
                            outer_loop: wall_loop,
                            inner_holes: Vec::new(),
                        });
                    }
                }
            }
            return;
        }

        // 5. Flush sub-face inside a coplanar frame (is_hole_of_coplanar_frame == true):
        // Move the target sub-face's outer loop and inner holes by +shift and stitch 4 side walls
        for v in &mut self.faces[face_idx].outer_loop {
            *v += shift;
        }
        for hole in &mut self.faces[face_idx].inner_holes {
            for v in hole {
                *v += shift;
            }
        }

        for k in 0..4 {
            let next_k = (k + 1) % 4;
            let b0 = old_outer[k];
            let b1 = old_outer[next_k];
            let t1 = b1 + shift;
            let t0 = b0 + shift;

            let edge_dir = (b1 - b0).normalize_or_zero();
            let outward_ring_normal = edge_dir.cross(n_vec).normalize_or_zero();
            let wall_normal_vec = if clamped_distance >= 0.0_f32 {
                outward_ring_normal
            } else {
                -outward_ring_normal
            };

            if let Ok(wall_dir) = Dir3::new(wall_normal_vec) {
                let wall_loop = orient_loop_ccw(&[b0, b1, t1, t0], wall_normal_vec);
                let wall_id = self.allocate_subface_id();
                self.faces.push(ManifoldPolygonFace {
                    subface_id: wall_id,
                    region_kind: SubFaceRegionKind::WholeFace,
                    normal: wall_dir,
                    outer_loop: wall_loop,
                    inner_holes: Vec::new(),
                });
            }
        }

        for hole in old_holes {
            let hole_ccw = orient_loop_ccw(&hole, n_vec);
            for k in 0..4 {
                let next_k = (k + 1) % 4;
                let b0 = hole_ccw[k];
                let b1 = hole_ccw[next_k];
                let t1 = b1 + shift;
                let t0 = b0 + shift;

                let edge_dir = (b1 - b0).normalize_or_zero();
                let outward_ring_normal = edge_dir.cross(n_vec).normalize_or_zero();
                let wall_normal_vec = if clamped_distance >= 0.0_f32 {
                    -outward_ring_normal
                } else {
                    outward_ring_normal
                };

                if let Ok(wall_dir) = Dir3::new(wall_normal_vec) {
                    let wall_loop = orient_loop_ccw(&[b0, b1, t1, t0], wall_normal_vec);
                    let wall_id = self.allocate_subface_id();
                    self.faces.push(ManifoldPolygonFace {
                        subface_id: wall_id,
                        region_kind: SubFaceRegionKind::WholeFace,
                        normal: wall_dir,
                        outer_loop: wall_loop,
                        inner_holes: Vec::new(),
                    });
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

fn push_quad_ccw(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
    v3: Vec3,
    expected_normal: Vec3,
    vertex_color: [f32; 4],
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
    colors.extend_from_slice(&[vertex_color, vertex_color, vertex_color, vertex_color]);
    indices.extend_from_slice(&[
        base_idx,
        base_idx + 1,
        base_idx + 2,
        base_idx,
        base_idx + 2,
        base_idx + 3,
    ]);
}

pub fn orient_loop_ccw(loop_pts: &[Vec3; 4], normal: Vec3) -> [Vec3; 4] {
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

pub fn align_inner_loop_to_outer(
    outer_ccw: &[Vec3; 4],
    inner_raw: &[Vec3; 4],
    normal: Vec3,
) -> [Vec3; 4] {
    let inner_ccw = orient_loop_ccw(inner_raw, normal);
    let mut best_shift = 0_usize;
    let mut best_score = f32::NEG_INFINITY;
    for shift in 0..4 {
        let mut dir_alignment = 0.0_f32;
        let mut corner_dist = 0.0_f32;
        for k in 0..4 {
            let next_k = (k + 1) % 4;
            let s_k = (k + shift) % 4;
            let s_next = (k + shift + 1) % 4;
            let outer_edge = (outer_ccw[next_k] - outer_ccw[k]).normalize_or_zero();
            let inner_edge = (inner_ccw[s_next] - inner_ccw[s_k]).normalize_or_zero();
            dir_alignment += outer_edge.dot(inner_edge);
            corner_dist += outer_ccw[k].distance(inner_ccw[s_k]);
        }
        let score = dir_alignment * 100.0_f32 - corner_dist;
        if score > best_score {
            best_score = score;
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
    let fallback_topo;
    let active_faces = if topology.faces.is_empty() {
        fallback_topo = EditableBlockTopology::from_box(center, size);
        &fallback_topo.faces
    } else {
        &topology.faces
    };

    let base_rgba = if topology.base_color_rgba[3] > 0.0_f32 {
        topology.base_color_rgba
    } else {
        [0.76, 0.80, 0.86, 1.0]
    };
    let default_linear_color = LinearRgba::from(Color::srgba(
        base_rgba[0],
        base_rgba[1],
        base_rgba[2],
        base_rgba[3],
    ))
    .to_f32_array();
    let selected_subface_linear_color =
        LinearRgba::from(Color::srgb(0.08, 0.88, 1.00)).to_f32_array();

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for face in active_faces {
        let n_vec = face.normal.as_vec3();
        let face_vertex_color = if topology.selected_subface_id == Some(face.subface_id) {
            selected_subface_linear_color
        } else {
            default_linear_color
        };

        let outer_ccw = orient_loop_ccw(&face.outer_loop, n_vec);
        let outer_local: [Vec3; 4] = [
            outer_ccw[0] - center,
            outer_ccw[1] - center,
            outer_ccw[2] - center,
            outer_ccw[3] - center,
        ];

        if let Some(first_hole) = face.inner_holes.last() {
            let hole_ccw = align_inner_loop_to_outer(&outer_ccw, first_hole, n_vec);
            let hole_local: [Vec3; 4] = [
                hole_ccw[0] - center,
                hole_ccw[1] - center,
                hole_ccw[2] - center,
                hole_ccw[3] - center,
            ];

            for k in 0..4 {
                let next_k = (k + 1) % 4;
                push_quad_ccw(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut colors,
                    &mut indices,
                    outer_local[k],
                    outer_local[next_k],
                    hole_local[next_k],
                    hole_local[k],
                    n_vec,
                    face_vertex_color,
                );
            }
        } else {
            push_quad_ccw(
                &mut positions,
                &mut normals,
                &mut uvs,
                &mut colors,
                &mut indices,
                outer_local[0],
                outer_local[1],
                outer_local[2],
                outer_local[3],
                n_vec,
                face_vertex_color,
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
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

pub fn extract_world_triangles_from_bevy_mesh(
    mesh: &Mesh,
    transform: &Transform,
) -> Vec<(Vec3, Vec3, Vec3, Vec3)> {
    let Some(VertexAttributeValues::Float32x3(raw_positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return Vec::new();
    };

    let world_verts: Vec<Vec3> = raw_positions
        .iter()
        .map(|p| transform.transform_point(Vec3::from_array(*p)))
        .collect();

    let mut tris = Vec::new();
    match mesh.indices() {
        Some(Indices::U32(idx_list)) => {
            for chunk in idx_list.chunks_exact(3) {
                let i0 = chunk[0] as usize;
                let i1 = chunk[1] as usize;
                let i2 = chunk[2] as usize;
                if i0 < world_verts.len() && i1 < world_verts.len() && i2 < world_verts.len() {
                    let a = world_verts[i0];
                    let b = world_verts[i1];
                    let c = world_verts[i2];
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    if n.length_squared() > 0.5_f32 {
                        tris.push((a, b, c, n));
                    }
                }
            }
        }
        Some(Indices::U16(idx_list)) => {
            for chunk in idx_list.chunks_exact(3) {
                let i0 = chunk[0] as usize;
                let i1 = chunk[1] as usize;
                let i2 = chunk[2] as usize;
                if i0 < world_verts.len() && i1 < world_verts.len() && i2 < world_verts.len() {
                    let a = world_verts[i0];
                    let b = world_verts[i1];
                    let c = world_verts[i2];
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    if n.length_squared() > 0.5_f32 {
                        tris.push((a, b, c, n));
                    }
                }
            }
        }
        None => {
            for chunk in world_verts.chunks_exact(3) {
                let a = chunk[0];
                let b = chunk[1];
                let c = chunk[2];
                let n = (b - a).cross(c - a).normalize_or_zero();
                if n.length_squared() > 0.5_f32 {
                    tris.push((a, b, c, n));
                }
            }
        }
    }

    tris
}

pub fn rebuild_block_entity_manifold_mesh(world: &mut World, target: Entity) {
    let Some((mut center, mut size, base_color, current_label, mut topo)) =
        world.get_entity(target).and_then(|ent| {
            let obj = ent.get::<CadObject>()?;
            let CadGeometryKind::Block { center, size } = obj.geometry else {
                return None;
            };
            let topo = ent
                .get::<EditableBlockTopology>()
                .cloned()
                .unwrap_or_default();
            Some((
                center,
                size,
                obj.material_color.0,
                obj.label.clone(),
                topo,
            ))
        })
    else {
        return;
    };

    topo.base_color_rgba = base_color.to_srgba().to_f32_array();
    if topo.base_label.is_empty() {
        topo.base_label = strip_subface_label_suffix(&current_label);
    }
    topo.ensure_initialized(center, size);
    if let Some((bounds_center, bounds_size)) = topo.compute_bounds() {
        center = bounds_center;
        size = bounds_size;
    }

    let formatted_label = topo.formatted_selection_label(&current_label);
    let new_mesh = build_manifold_block_mesh_with_topology(center, size, &topo);
    let new_mesh_handle = {
        let mut meshes = world.resource_mut::<Assets<Mesh>>();
        meshes.add(new_mesh)
    };

    if let Some(mut ent_mut) = world.get_entity_mut(target) {
        if let Some(mut obj) = ent_mut.get_mut::<CadObject>() {
            obj.geometry = CadGeometryKind::Block { center, size };
            obj.label = formatted_label;
        }
        ent_mut.insert(topo);
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
    let topo = match cad_obj.geometry {
        CadGeometryKind::Block { center, size } => EditableBlockTopology::from_box(center, size),
        CadGeometryKind::Line { .. } => EditableBlockTopology::default(),
    };
    instantiate_cad_entity_with_topology(world, cad_obj, topo)
}

pub fn instantiate_cad_entity_with_topology(
    world: &mut World,
    cad_obj: CadObject,
    mut topology: EditableBlockTopology,
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
            topology.base_color_rgba = cad_obj.material_color.0.to_srgba().to_f32_array();
            if topology.base_label.is_empty() {
                topology.base_label = strip_subface_label_suffix(&cad_obj.label);
            }
            topology.ensure_initialized(center, clamped);
            let built_mesh = build_manifold_block_mesh_with_topology(center, clamped, &topology);
            let (box_mesh, box_mat) = {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let m = meshes.add(built_mesh);
                let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
                let mat = materials.add(StandardMaterial {
                    base_color: Color::WHITE,
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
    face_normal: Option<Dir3>,
    base_face_vertices: Option<[Vec3; 4]>,
    offset_vertices: Option<[Vec3; 4]>,
    signed_distance: f32,
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
        Self {
            target_block_entity: Some(parent_entity),
            face_normal: Some(face_normal),
            base_face_vertices: Some(base_face_vertices),
            offset_vertices: Some(offset_vertices),
            signed_distance,
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
            face_normal: None,
            base_face_vertices: None,
            offset_vertices: None,
            signed_distance: 0.0_f32,
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
        if let (Some(parent_entity), Some(normal), Some(base_v4), Some(offset_v4)) = (
            self.target_block_entity,
            self.face_normal,
            self.base_face_vertices,
            self.offset_vertices,
        ) {
            let (center, size) = world
                .get::<CadObject>(parent_entity)
                .and_then(|o| match o.geometry {
                    CadGeometryKind::Block { center, size } => Some((center, size)),
                    CadGeometryKind::Line { .. } => None,
                })
                .unwrap_or((Vec3::ZERO, Vec3::ONE));

            let mut existing_topo = world
                .get::<EditableBlockTopology>(parent_entity)
                .cloned()
                .unwrap_or_default();
            existing_topo.ensure_initialized(center, size);
            self.previous_topology = Some(existing_topo.clone());

            let mut updated_topo = existing_topo;
            updated_topo.apply_offset_split(normal, base_v4, offset_v4, self.signed_distance);

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
        let mut dummy_colors: Vec<[f32; 4]> = Vec::new();
        push_quad_ccw(
            &mut positions,
            &mut normals,
            &mut uvs,
            &mut dummy_colors,
            &mut indices,
            b0,
            b1,
            t1,
            t0,
            outward_normal,
            [1.0, 1.0, 1.0, 1.0],
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
                    EditableBlockTopology::from_box(solid_center, solid_size),
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
                    let shifted = topo
                        .translated(-c0)
                        .scaled_from_anchor(Vec3::ZERO, factors)
                        .translated(c1);
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

#[derive(Debug, Clone)]
struct DeleteSnapshotEntry {
    entity: Option<Entity>,
    object: CadObject,
    saved_topology: EditableBlockTopology,
    isolated_subface_id: Option<usize>,
    erased_subface_only: bool,
}

pub struct DeleteCommand {
    snapshots: Vec<DeleteSnapshotEntry>,
}

impl DeleteCommand {
    pub fn new(items: Vec<(Entity, CadObject)>) -> Self {
        let snapshots = items
            .into_iter()
            .map(|(ent, obj)| DeleteSnapshotEntry {
                entity: Some(ent),
                object: obj,
                saved_topology: EditableBlockTopology::default(),
                isolated_subface_id: None,
                erased_subface_only: false,
            })
            .collect();
        Self { snapshots }
    }

    pub fn for_isolated_subface(
        entity: Entity,
        object: CadObject,
        subface_id: usize,
    ) -> Self {
        Self {
            snapshots: vec![DeleteSnapshotEntry {
                entity: Some(entity),
                object,
                saved_topology: EditableBlockTopology::default(),
                isolated_subface_id: Some(subface_id),
                erased_subface_only: false,
            }],
        }
    }
}

impl Command for DeleteCommand {
    fn execute(&mut self, world: &mut World) {
        for entry in &mut self.snapshots {
            let Some(ent) = entry.entity else {
                continue;
            };
            if let Some(topo) = world.get::<EditableBlockTopology>(ent).cloned() {
                entry.saved_topology = topo.clone();
                let target_subface = entry.isolated_subface_id.or(topo.selected_subface_id);
                if let Some(sub_id) = target_subface {
                    let mut mutated_topo = topo;
                    if mutated_topo.delete_subface_by_id(sub_id) {
                        entry.isolated_subface_id = Some(sub_id);
                        entry.erased_subface_only = true;
                        if let Some(mut ent_mut) = world.get_entity_mut(ent) {
                            ent_mut.insert(mutated_topo);
                        }
                        rebuild_block_entity_manifold_mesh(world, ent);
                        continue;
                    }
                }
            }

            entry.erased_subface_only = false;
            entry.entity = None;
            remove_cad_entity(world, ent);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for entry in &mut self.snapshots {
            if entry.erased_subface_only {
                if let Some(ent) = entry.entity {
                    if let Some(mut ent_mut) = world.get_entity_mut(ent) {
                        ent_mut.insert(entry.saved_topology.clone());
                    }
                    rebuild_block_entity_manifold_mesh(world, ent);
                }
            } else {
                let respawned = instantiate_cad_entity_with_topology(
                    world,
                    entry.object.clone(),
                    entry.saved_topology.clone(),
                );
                entry.entity = Some(respawned);
            }
        }
    }

    fn description(&self) -> &'static str {
        "Delete Selected Sub-Face / Object"
    }
}