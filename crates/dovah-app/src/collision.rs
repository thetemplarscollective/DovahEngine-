//! Per-object collision (Riverwood-complete milestone 1).
//!
//! The game's own Havok collision — the `bhk` trees in the NIFs,
//! parsed by dovah-nif — drives movement. Two grids share the
//! FloorGrid's layout (1 m cells over the room bounds):
//!
//! - The FloorGrid (scene.rs) takes collision triangles INSTEAD
//!   OF visual ones when a NIF has collision meshes. The
//!   collision is what the game's physics walks on, and the
//!   visual mesh skips treads: the porch's "0.52 m" rises were a
//!   visual-rasterization artifact (the 1 m cells merged pairs
//!   of treads); the collision steps rise 0.26 m each, inside
//!   STEP_HEIGHT 0.3 (collision_design probe, Part E).
//! - The ObstacleGrid (here) takes steep collision triangles +
//!   convex-primitive footprints from STATIC-like layers only
//!   (clutter, weapons, props and animated statics are movable
//!   or handled elsewhere — doors by `door_blocks_move`). A move
//!   is blocked iff an obstacle interval overlaps
//!   `[feet + STEP_HEIGHT, feet + PLAYER_HEIGHT]`.
//!
//! No stair identification is needed, and none exists in the
//! data: Riverwood has NO STAIRHELPER (layer 31) bodies, and the
//! porch steps share their layer (1) and material (0x1dd9c611,
//! wood) with the bench seat. What separates them is HEIGHT:
//! the tallest stair rise is 0.26 m (porch treads, base plate
//! to top landing), the lowest furniture is 0.42 m (bench seat),
//! and STEP_HEIGHT 0.3 sits between them with room on both
//! sides. Every stair riser tops out below the obstacle band
//! (feet + 0.3) and never blocks; every bench side and wall
//! pokes into it and does. The step-up probe and the obstacle
//! band use the SAME threshold, so the two rules always agree:
//! a surface you can step onto never blocks you, and a surface
//! that blocks you is never steppable.

use crate::coords;
use crate::scene::RoomBounds;
use bevy::ecs::prelude::Resource;
use bevy::math::Vec3;
use dovah_nif::CollisionShape;
use std::cmp::Ordering;
use std::sync::Arc;

/// Havok layers whose collision BLOCKS movement. STATIC (1) is
/// the world; TREES (9) is trunk capsules; TERRAIN (13) is
/// terrain collision; INVISIBLE_WALL (27) exists to block.
/// Excluded: UNIDENTIFIED (0), ANIMSTATIC (2 — doors, handled
/// by `door_blocks_move`; their hulls would block open
/// doorways), TRANSPARENT (3), CLUTTER (4 — movable, picked up
/// or pushed), WEAPON (5), PROPS (10). TRIGGER (12) and
/// NONCOLLIDABLE (15) never reach us (dovah-nif filters them).
pub const OBSTACLE_LAYERS: [u8; 4] = [1, 9, 13, 27];

/// Whether a Havok layer's collision blocks the player's
/// horizontal movement.
pub fn layer_blocks_movement(layer: u8) -> bool {
    OBSTACLE_LAYERS.contains(&layer)
}

/// A convex primitive's footprint: its axis-aligned bounds in
/// Bevy coords (metres, NIF-local — the ref transform is applied
/// at rasterize time by [`transform_footprint`]). The AABB is
/// conservative (a capsule blocks its bounding square, not its
/// circle) — fine for trunks and fences, whose AABBs are close
/// to their true shapes.
#[derive(Debug, Clone)]
pub struct Footprint {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
    pub min_y: f32,
    pub max_y: f32,
}

/// One NIF's collision geometry, converted once at parse time
/// and cached per NIF path (like `CachedSubmesh`): positions are
/// Bevy coords with the NIF-local transform already baked in,
/// so `world_triangles` applies the ref transform directly.
pub struct CachedCollision {
    /// Bevy-coords vertex positions, all mesh shapes
    /// concatenated (NIF-local transform baked in).
    pub positions: Arc<Vec<[f32; 3]>>,
    /// ALL mesh triangles (every layer) — the floor geometry.
    /// The FloorGrid's own up-facing filter picks the walkable
    /// ones; a clutter chest's top is a floor you can jump onto.
    pub floor_indices: Arc<Vec<u32>>,
    /// Obstacle-layer mesh triangles only — the wall geometry.
    pub obstacle_indices: Arc<Vec<u32>>,
    /// Obstacle-layer capsule/hull footprints.
    pub footprints: Arc<Vec<Footprint>>,
}

impl CachedCollision {
    /// Whether this NIF has collision meshes — the trigger for
    /// the "collision wins" floor rule (visual triangles are
    /// skipped for the FloorGrid when this is true).
    pub fn has_mesh(&self) -> bool {
        !self.floor_indices.is_empty()
    }

    /// Whether this NIF contributes anything to the obstacle
    /// grid.
    pub fn has_obstacles(&self) -> bool {
        !self.obstacle_indices.is_empty() || !self.footprints.is_empty()
    }
}

/// Convert parsed collision shapes (NIF-local Skyrim units) to
/// cached Bevy-coords geometry. Returns `None` when the shapes
/// carry nothing usable (no mesh triangles, no footprints) — the
/// caller caches the `None` so the NIF is not re-parsed per ref.
pub fn collision_from_shapes(shapes: &[CollisionShape]) -> Option<CachedCollision> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut floor_indices: Vec<u32> = Vec::new();
    let mut obstacle_indices: Vec<u32> = Vec::new();
    let mut footprints: Vec<Footprint> = Vec::new();
    for shape in shapes {
        match shape {
            CollisionShape::Mesh {
                vertices,
                triangles,
                layer,
                ..
            } => {
                let base = positions.len() as u32;
                for v in vertices {
                    positions.push(coords::to_bevy_position(*v).to_array());
                }
                let obstacle = layer_blocks_movement(*layer);
                for t in triangles {
                    let tri = [t[0] + base, t[1] + base, t[2] + base];
                    floor_indices.extend_from_slice(&tri);
                    if obstacle {
                        obstacle_indices.extend_from_slice(&tri);
                    }
                }
            }
            CollisionShape::Capsule {
                a,
                b,
                radius,
                layer,
            } => {
                if !layer_blocks_movement(*layer) {
                    continue;
                }
                let ab = coords::to_bevy_position(*a);
                let bb = coords::to_bevy_position(*b);
                let r = radius / coords::SKYRIM_UNITS_PER_METER;
                footprints.push(Footprint {
                    min_x: ab.x.min(bb.x) - r,
                    max_x: ab.x.max(bb.x) + r,
                    min_z: ab.z.min(bb.z) - r,
                    max_z: ab.z.max(bb.z) + r,
                    min_y: ab.y.min(bb.y) - r,
                    max_y: ab.y.max(bb.y) + r,
                });
            }
            CollisionShape::Hull { points, layer } => {
                if !layer_blocks_movement(*layer) {
                    continue;
                }
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for p in points {
                    let v = coords::to_bevy_position(*p);
                    min = min.min(v);
                    max = max.max(v);
                }
                footprints.push(Footprint {
                    min_x: min.x,
                    max_x: max.x,
                    min_z: min.z,
                    max_z: max.z,
                    min_y: min.y,
                    max_y: max.y,
                });
            }
        }
    }
    if positions.is_empty() && footprints.is_empty() {
        return None;
    }
    Some(CachedCollision {
        positions: Arc::new(positions),
        floor_indices: Arc::new(floor_indices),
        obstacle_indices: Arc::new(obstacle_indices),
        footprints: Arc::new(footprints),
    })
}

/// Transform a NIF-local footprint by a ref transform: all 8
/// AABB corners through the matrix, then the new AABB
/// (conservative under rotation). Returns
/// `(min_x, max_x, min_z, max_z, min_y, max_y)` in world space.
pub fn transform_footprint(
    footprint: &Footprint,
    transform: &bevy::transform::components::Transform,
) -> (f32, f32, f32, f32, f32, f32) {
    let matrix = transform.to_matrix();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for x in [footprint.min_x, footprint.max_x] {
        for y in [footprint.min_y, footprint.max_y] {
            for z in [footprint.min_z, footprint.max_z] {
                let w = matrix.transform_point3(Vec3::new(x, y, z));
                min = min.min(w);
                max = max.max(w);
            }
        }
    }
    (min.x, max.x, min.z, max.z, min.y, max.y)
}

/// 2D point-in-triangle test (x/z plane), barycentric sign method.
pub(crate) fn point_in_triangle_2d(px: f32, pz: f32, a: Vec3, b: Vec3, c: Vec3) -> bool {
    let d1 = (px - b.x) * (a.z - b.z) - (pz - b.z) * (a.x - b.x);
    let d2 = (px - c.x) * (b.z - c.z) - (pz - c.z) * (b.x - c.x);
    let d3 = (px - a.x) * (c.z - a.z) - (pz - a.z) * (c.x - a.x);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// 2D triangle-vs-cell-square overlap (x/z plane), separating
/// axis theorem. The box is the grid cell [x0,x1]x[z0,z1].
/// Only the triangle's three edge normals can separate: the
/// caller only visits cells in the triangle's bounding box, so
/// the box axes overlap by construction. Touching counts as
/// overlap (a wall edge exactly on a cell boundary still blocks
/// the cell). Degenerate (zero-length) edges contribute no axis.
pub(crate) fn triangle_overlaps_cell_2d(
    a: Vec3,
    b: Vec3,
    c: Vec3,
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
) -> bool {
    for (p, q) in [(a, b), (b, c), (c, a)] {
        // Edge normal (2D): (-dz, dx).
        let ex = q.x - p.x;
        let ez = q.z - p.z;
        let nx = -ez;
        let nz = ex;
        if nx == 0.0 && nz == 0.0 {
            continue;
        }
        let t0 = a.x * nx + a.z * nz;
        let t1 = b.x * nx + b.z * nz;
        let t2 = c.x * nx + c.z * nz;
        let tmin = t0.min(t1).min(t2);
        let tmax = t0.max(t1).max(t2);
        let c0 = x0 * nx + z0 * nz;
        let c1 = x1 * nx + z0 * nz;
        let c2 = x0 * nx + z1 * nz;
        let c3 = x1 * nx + z1 * nz;
        let bmin = c0.min(c1).min(c2).min(c3);
        let bmax = c0.max(c1).max(c2).max(c3);
        if tmax < bmin || bmax < tmin {
            return false;
        }
    }
    true
}

/// The blocking predicate: do any obstacle intervals overlap the
/// band `[band_lo, band_hi]`? Pure — the unit-tested core of
/// horizontal collision (the `swinging_door_fires` pattern).
/// The overlap is STRICT at the boundaries: an interval that
/// merely touches the band (a riser topping out exactly at
/// `feet + STEP_HEIGHT`) does NOT block, matching the step-up
/// probe, which steps onto a surface exactly at the probe
/// height. A 0.30 m rise is steppable AND passable; a 0.31 m
/// rise is neither.
pub fn move_blocked(intervals: &[(f32, f32)], band_lo: f32, band_hi: f32) -> bool {
    intervals
        .iter()
        .any(|&(lo, hi)| hi > band_lo && lo < band_hi)
}

/// Horizontal collision: per-cell vertical extents of the
/// obstacles in it. Same bounds as [`FloorGrid`] but 0.25 m
/// cells (see `CELL_SIZE` — 1 m cells conflate stair risers).
#[derive(Resource)]
pub struct ObstacleGrid {
    /// World x/z of cell (0, 0)'s corner.
    origin_x: f32,
    origin_z: f32,
    /// Cell size in metres.
    cell: f32,
    cols: usize,
    rows: usize,
    /// Sorted, merged vertical extents per cell: (min_y, max_y).
    intervals: Vec<Vec<(f32, f32)>>,
}

impl ObstacleGrid {
    /// Cell size in metres — 0.25 m, NOT the FloorGrid's 1 m.
    /// Stair risers are ~0.3–0.43 m apart horizontally; 1 m
    /// cells conflate the riser two steps ahead with the
    /// player's cell, and it blocks the climb (the first walk
    /// sim failed on the porch for exactly this reason — a
    /// riser spanning -1.46..-1.10 m shared the player's cell
    /// while their band was [-1.59, -0.19]). A 0.25 m cell
    /// cannot hold two risers 0.3 m apart, and a riser one step
    /// ahead never blocks anyway (its top is the tread above,
    /// below feet + STEP_HEIGHT from the new feet). The
    /// FloorGrid keeps 1 m cells: multiple heights per cell is
    /// its design (the probe picks the highest below it).
    pub const CELL_SIZE: f32 = 0.25;
    /// Hard cap on grid cells (the FloorGrid's rule: a
    /// degenerate bounds box must not turn the grid into a
    /// multi-gigabyte allocation). 1M cells covers the 175 m
    /// exterior at 0.25 m (493k cells, ~12 MB of empty Vecs).
    pub const MAX_CELLS: usize = 1_000_000;
    /// Per-cell interval cap.
    pub const MAX_INTERVALS_PER_CELL: usize = 4_096;

    pub fn new(bounds: &RoomBounds) -> Self {
        let mut cell = Self::CELL_SIZE;
        let (mut cols, mut rows) = Self::dims(bounds, cell);
        while cols.saturating_mul(rows) > Self::MAX_CELLS {
            cell *= 2.0;
            (cols, rows) = Self::dims(bounds, cell);
        }
        ObstacleGrid {
            origin_x: bounds.min.x,
            origin_z: bounds.min.z,
            cell,
            cols,
            rows,
            intervals: vec![Vec::new(); cols * rows],
            // (no fallback: a cell with no intervals is open)
        }
    }

    fn dims(bounds: &RoomBounds, cell: f32) -> (usize, usize) {
        (
            ((bounds.max.x - bounds.min.x) / cell).ceil().max(1.0) as usize,
            ((bounds.max.z - bounds.min.z) / cell).ceil().max(1.0) as usize,
        )
    }

    /// Add one world-space triangle: if it is STEEP (normal
    /// within ~60° of horizontal — walls, risers, table legs,
    /// bridge undersides), its vertical span becomes an obstacle
    /// interval in every cell the triangle overlaps. Up-facing
    /// triangles are floors (the FloorGrid's job) and are
    /// skipped here; a 0.26 m stair riser IS recorded, but it
    /// tops out below the obstacle band and never blocks.
    pub fn add_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let normal = (b - a).cross(c - a);
        if normal.length_squared() < 1e-12 {
            return;
        }
        let n = normal.normalize();
        // Steep only: the complement of the FloorGrid's up-facing
        // filter (n.y >= 0.5). Downward-facing triangles
        // (ceilings, undersides) are steep by this test and ARE
        // recorded — a 1.5 m walkway underside blocks a 1.7 m
        // player, a 3 m bridge underside does not (its interval
        // sits above the band).
        if n.y >= 0.5 {
            return;
        }
        let min_y = a.y.min(b.y).min(c.y);
        let max_y = a.y.max(b.y).max(c.y);
        let min_x = a.x.min(b.x).min(c.x);
        let max_x = a.x.max(b.x).max(c.x);
        let min_z = a.z.min(b.z).min(c.z);
        let max_z = a.z.max(b.z).max(c.z);
        let c0 = self.col(min_x);
        let c1 = self.col(max_x);
        let r0 = self.row(min_z);
        let r1 = self.row(max_z);
        for r in r0..=r1 {
            for col in c0..=c1 {
                let cx = self.origin_x + (col as f32 + 0.5) * self.cell;
                let cz = self.origin_z + (r as f32 + 0.5) * self.cell;
                let x0 = self.origin_x + col as f32 * self.cell;
                let z0 = self.origin_z + r as f32 * self.cell;
                let overlaps = point_in_triangle_2d(cx, cz, a, b, c)
                    || triangle_overlaps_cell_2d(a, b, c, x0, x0 + self.cell, z0, z0 + self.cell);
                if overlaps {
                    self.add_interval(r * self.cols + col, min_y, max_y);
                }
            }
        }
    }

    /// Add a primitive footprint (world-space AABB + vertical
    /// span): the interval goes in every cell the footprint
    /// overlaps.
    pub fn add_footprint(
        &mut self,
        min_x: f32,
        max_x: f32,
        min_z: f32,
        max_z: f32,
        min_y: f32,
        max_y: f32,
    ) {
        let c0 = self.col(min_x);
        let c1 = self.col(max_x);
        let r0 = self.row(min_z);
        let r1 = self.row(max_z);
        for r in r0..=r1 {
            for col in c0..=c1 {
                self.add_interval(r * self.cols + col, min_y, max_y);
            }
        }
    }

    fn add_interval(&mut self, cell: usize, min_y: f32, max_y: f32) {
        let intervals = &mut self.intervals[cell];
        if intervals.len() < Self::MAX_INTERVALS_PER_CELL {
            intervals.push((min_y, max_y));
        }
    }

    /// Sort each cell's intervals, drop the ones that are steps
    /// rather than walls, and merge the survivors. Call once
    /// after all triangles/footprints — and AFTER the floor
    /// grid's own `finish` (the post-process binary-searches
    /// its sorted heights).
    ///
    /// The step filter is the point: an obstacle interval whose
    /// top is within `step_height` of a walkable floor in the
    /// same 1 m floor cell is a STEP, not a wall — the player
    /// can stand on that floor and step onto the obstacle's top
    /// (a stair riser tops out at the tread above it; a curb
    /// tops out at its own top). Walls, bench sides and table
    /// legs top out far above every floor in their cell and
    /// survive. Without this, a stair riser two steps ahead
    /// blocks the player at the bottom: its line clips the
    /// player's cell diagonally, and from the bottom its top is
    /// 0.53 m up — but from the tread it stands on, it is a
    /// 0.26 m step (the porch climb failed three walk sims on
    /// exactly this).
    pub fn finish(&mut self, floor: &crate::scene::FloorGrid, step_height: f32) {
        for r in 0..self.rows {
            for col in 0..self.cols {
                let cell = &mut self.intervals[r * self.cols + col];
                if cell.is_empty() {
                    continue;
                }
                // The floors in the 1 m floor cell containing
                // this obstacle cell's centre.
                let cx = self.origin_x + (col as f32 + 0.5) * self.cell;
                let cz = self.origin_z + (r as f32 + 0.5) * self.cell;
                let floors = floor.heights_at(cx, cz);
                cell.retain(|&(_lo, hi)| {
                    // Highest floor at or below the interval's
                    // top: the surface the obstacle stands on
                    // (or the tread below its top).
                    let below = match floors
                        .binary_search_by(|h| h.partial_cmp(&hi).unwrap_or(Ordering::Equal))
                    {
                        Ok(i) => Some(floors[i]),
                        Err(0) => None,
                        Err(i) => Some(floors[i - 1]),
                    };
                    // Keep the interval unless a floor within
                    // step height of its top exists (then it is
                    // a step). No floor at all: keep (fail
                    // closed — a wall on bare terrain still
                    // blocks).
                    match below {
                        Some(f) => hi - f > step_height,
                        None => true,
                    }
                });
                if cell.is_empty() {
                    continue;
                }
                cell.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
                let mut merged: Vec<(f32, f32)> = Vec::with_capacity(cell.len());
                for (lo, hi) in cell.drain(..) {
                    if let Some(last) = merged.last_mut() {
                        // Touching (within 1 mm) merges: a riser
                        // ending at 0.260 and the next starting
                        // at 0.260 is one wall, not two with a gap.
                        if lo <= last.1 + 0.001 {
                            last.1 = last.1.max(hi);
                            continue;
                        }
                    }
                    merged.push((lo, hi));
                }
                *cell = merged;
            }
        }
    }

    /// Whether the point (x, z) is blocked for a player whose
    /// obstacle band is `[band_lo, band_hi]` (the caller owns
    /// the band rule: grounded, the bottom is feet + STEP_HEIGHT
    /// so stairs pass under it; airborne, the bottom is the feet
    /// themselves; the top is always feet + PLAYER_HEIGHT so
    /// high beams pass over the head).
    pub fn blocked(&self, x: f32, z: f32, band_lo: f32, band_hi: f32) -> bool {
        let col = self.col(x);
        let row = self.row(z);
        let cell = &self.intervals[row * self.cols + col];
        move_blocked(cell, band_lo, band_hi)
    }

    /// The obstacle intervals in the cell containing (x, z) —
    /// a diagnostic hook (the collision_walk probe prints them
    /// at a stop point). Empty when the cell has no obstacles.
    pub fn intervals_at(&self, x: f32, z: f32) -> &[(f32, f32)] {
        let col = self.col(x);
        let row = self.row(z);
        &self.intervals[row * self.cols + col]
    }

    fn col(&self, x: f32) -> usize {
        (((x - self.origin_x) / self.cell).floor() as i32).clamp(0, self.cols as i32 - 1) as usize
    }

    fn row(&self, z: f32) -> usize {
        (((z - self.origin_z) / self.cell).floor() as i32).clamp(0, self.rows as i32 - 1) as usize
    }

    /// (cells with data, total intervals) for the summary.
    pub fn stats(&self) -> (usize, usize) {
        let mut cells = 0;
        let mut intervals = 0;
        for cell in &self.intervals {
            if cell.is_empty() {
                continue;
            }
            cells += 1;
            intervals += cell.len();
        }
        (cells, intervals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> RoomBounds {
        RoomBounds {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 5.0, 10.0),
            floor_y: 0.0,
        }
    }

    #[test]
    fn layer_filter() {
        for l in [1u8, 9, 13, 27] {
            assert!(layer_blocks_movement(l), "layer {l} blocks");
        }
        for l in [0u8, 2, 3, 4, 5, 10, 12, 15] {
            assert!(!layer_blocks_movement(l), "layer {l} does not block");
        }
    }

    #[test]
    fn move_blocked_bands() {
        // A stair riser (0.26 m) tops out below the band: pass.
        assert!(!move_blocked(&[(0.0, 0.26)], 0.3, 1.7));
        // A riser exactly at the step height: pass (strict
        // boundary — the step-up probe steps onto it, so the
        // obstacle check must let it through too).
        assert!(!move_blocked(&[(0.0, 0.30)], 0.3, 1.7));
        // A bench side (0.43 m) pokes into the band: blocked.
        assert!(move_blocked(&[(0.0, 0.43)], 0.3, 1.7));
        // A wall: blocked.
        assert!(move_blocked(&[(0.0, 3.0)], 0.3, 1.7));
        // A high beam (2.5 m): above the head, pass.
        assert!(!move_blocked(&[(2.5, 3.0)], 0.3, 1.7));
        // A low beam (1.5 m): hits the head, blocked.
        assert!(move_blocked(&[(1.5, 1.6)], 0.3, 1.7));
        // A beam exactly at head height: pass (strict boundary).
        assert!(!move_blocked(&[(1.7, 1.8)], 0.3, 1.7));
        // Empty cell: pass.
        assert!(!move_blocked(&[], 0.3, 1.7));
        // Jumping over a bench (feet 0.5, band 0.8..2.2): pass.
        assert!(!move_blocked(&[(0.0, 0.43)], 0.8, 2.2));
        // Jumping into a wall (feet 0.5): still blocked.
        assert!(move_blocked(&[(0.0, 3.0)], 0.8, 2.2));
    }

    /// A FloorGrid with a flat floor at the given heights
    /// (one triangle each, covering (5, 5)). Wound
    /// counter-clockwise so the normal points up (a clockwise
    /// triangle is downward-facing and `add_triangle` rejects
    /// it — the first version of this helper wound it the wrong
    /// way and the grid came out empty).
    fn floor_with_heights(heights: &[f32]) -> crate::scene::FloorGrid {
        let mut floor = crate::scene::FloorGrid::new(&bounds());
        for h in heights {
            floor.add_triangle(
                Vec3::new(4.0, *h, 4.0),
                Vec3::new(4.0, *h, 6.0),
                Vec3::new(6.0, *h, 4.0),
            );
        }
        floor.finish();
        floor
    }

    #[test]
    fn obstacle_grid_steep_filter_and_merge() {
        let mut grid = ObstacleGrid::new(&bounds());
        // A flat floor triangle: not an obstacle.
        grid.add_triangle(
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(2.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 2.0),
        );
        // A vertical wall triangle, 0..3 m.
        grid.add_triangle(
            Vec3::new(5.0, 0.0, 5.0),
            Vec3::new(5.0, 3.0, 5.0),
            Vec3::new(6.0, 0.0, 5.0),
        );
        // A second wall triangle stacked on the first (2..4 m):
        // merges into one interval after finish().
        grid.add_triangle(
            Vec3::new(5.0, 2.0, 5.0),
            Vec3::new(5.0, 4.0, 5.0),
            Vec3::new(6.0, 2.0, 5.0),
        );
        let floor = floor_with_heights(&[0.0]);
        grid.finish(&floor, 0.3);
        let (cells, intervals) = grid.stats();
        assert!(cells >= 1, "wall cells recorded");
        // The floor triangle added nothing; the wall merged.
        assert!(intervals >= 1);
        assert!(grid.blocked(5.5, 5.0, 0.3, 1.7));
        assert!(!grid.blocked(1.5, 1.5, 0.3, 1.7));
        // Merged: the interval spans 0..4 m, so a player at
        // feet 2.5 (band 2.8..4.2) is still blocked by the wall.
        assert!(grid.blocked(5.5, 5.0, 2.8, 4.2));
    }

    #[test]
    fn obstacle_grid_footprint() {
        let mut grid = ObstacleGrid::new(&bounds());
        // A tree trunk: 0.5 m square, 0..8 m tall.
        grid.add_footprint(4.75, 5.25, 4.75, 5.25, 0.0, 8.0);
        let floor = floor_with_heights(&[0.0]);
        grid.finish(&floor, 0.3);
        assert!(grid.blocked(5.0, 5.0, 0.3, 1.7));
        assert!(!grid.blocked(3.0, 3.0, 0.3, 1.7));
    }

    #[test]
    fn obstacle_grid_finish_drops_steps_keeps_walls() {
        // The porch riser (WalkwayStairs8): [-1.72, -1.36] with
        // treads at -1.89, -1.62, -1.36 in the cell. Its top is
        // 0.26 m above the tread below it — a step, dropped.
        let mut grid = ObstacleGrid::new(&bounds());
        grid.add_triangle(
            Vec3::new(5.0, -1.72, 5.0),
            Vec3::new(5.0, -1.36, 5.0),
            Vec3::new(6.0, -1.72, 5.0),
        );
        let floor = floor_with_heights(&[-1.89, -1.62, -1.36]);
        grid.finish(&floor, 0.3);
        // A player at the base plate (feet -1.89, band
        // [-1.59, -0.19]) is NOT blocked by the riser.
        assert!(!grid.blocked(5.5, 5.0, -1.59, -0.19));

        // A bench side [0.25, 0.68] on a 0.25 floor: its top is
        // 0.43 above the floor — a wall, kept.
        let mut grid = ObstacleGrid::new(&bounds());
        grid.add_triangle(
            Vec3::new(5.0, 0.25, 5.0),
            Vec3::new(5.0, 0.68, 5.0),
            Vec3::new(6.0, 0.25, 5.0),
        );
        let floor = floor_with_heights(&[0.25]);
        grid.finish(&floor, 0.3);
        assert!(grid.blocked(5.5, 5.0, 0.55, 1.95));

        // A curb [0.0, 0.15] whose own top (0.15) is in the
        // floor grid: a step, dropped.
        let mut grid = ObstacleGrid::new(&bounds());
        grid.add_triangle(
            Vec3::new(5.0, 0.0, 5.0),
            Vec3::new(5.0, 0.15, 5.0),
            Vec3::new(6.0, 0.0, 5.0),
        );
        let floor = floor_with_heights(&[0.0, 0.15]);
        grid.finish(&floor, 0.3);
        assert!(!grid.blocked(5.5, 5.0, 0.3, 1.7));
    }

    #[test]
    fn collision_from_shapes_converts_and_filters() {
        // One wood mesh triangle (layer 1) + one clutter capsule
        // (layer 4) + one tree capsule (layer 9).
        let shapes = vec![
            CollisionShape::Mesh {
                vertices: vec![[0.0, 0.0, 0.0], [70.0, 0.0, 0.0], [0.0, 70.0, 0.0]],
                triangles: vec![[0, 1, 2]],
                material: 0x1dd9c611,
                layer: 1,
            },
            CollisionShape::Capsule {
                a: [0.0, 0.0, 0.0],
                b: [0.0, 0.0, 70.0],
                radius: 7.0,
                layer: 4,
            },
            CollisionShape::Capsule {
                a: [700.0, 0.0, 0.0],
                b: [700.0, 0.0, 700.0],
                radius: 70.0,
                layer: 9,
            },
        ];
        let cached = collision_from_shapes(&shapes).expect("converts");
        assert!(cached.has_mesh());
        assert!(cached.has_obstacles());
        // Vertices: Skyrim units -> Bevy metres, axis map
        // (x, y, z) -> (x, z, -y). (70,0,0) -> (1,0,0);
        // (0,70,0) -> (0,0,-1).
        assert_eq!(
            *cached.positions,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]
        );
        assert_eq!(*cached.floor_indices, vec![0, 1, 2]);
        assert_eq!(*cached.obstacle_indices, vec![0, 1, 2]);
        // Only the tree capsule survives the layer filter.
        assert_eq!(cached.footprints.len(), 1);
        let fp = &cached.footprints[0];
        // Capsule (10,0,0)..(10,0,10) m, radius 1 m.
        assert_eq!(fp.min_x, 9.0);
        assert_eq!(fp.max_x, 11.0);
        assert_eq!(fp.min_y, -1.0);
        assert_eq!(fp.max_y, 11.0);

        // Nothing usable -> None.
        assert!(collision_from_shapes(&[]).is_none());
        let clutter_only = vec![CollisionShape::Capsule {
            a: [0.0, 0.0, 0.0],
            b: [0.0, 0.0, 70.0],
            radius: 7.0,
            layer: 4,
        }];
        assert!(collision_from_shapes(&clutter_only).is_none());
    }
}
