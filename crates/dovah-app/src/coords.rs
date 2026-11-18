//! Coordinate conversion: Skyrim space -> Bevy space.
//!
//! Skyrim (and NIF files) use a right-handed system with Z up: X east,
//! Y north, Z up. Positions, NIF vertices, and bounds are in Skyrim
//! units: 64 units = 1 yard, so 1 unit = 1.42875 cm and 1 metre = 70
//! units. Rotations on REFR records are Euler angles in radians.
//!
//! Bevy uses a right-handed system with Y up. The axis map is a pure
//! rotation (determinant +1, so no mirroring and triangle winding is
//! preserved):
//!
//! ```text
//! Bevy.x = Skyrim.x
//! Bevy.y = Skyrim.z
//! Bevy.z = -Skyrim.y
//! ```
//!
//! Rotation convention: a REFR's Euler angles are composed as
//! `Rz * Ry * Rx` (yaw, then pitch, then roll — the standard
//! yaw-pitch-roll order) in Skyrim space, then conjugated by the axis
//! map. The YAW angle negates: Skyrim yaw is CLOCKWISE-positive seen
//! from above (yaw 0 faces north/+Y, positive yaw turns toward
//! east/+X — proven by the M9 door spawn and by the M12 Orgnar
//! render, where the un-negated sign put the barkeep facing the back
//! wall instead of the room), while `Mat3::from_rotation_z` is the
//! right-hand-rule counterclockwise rotation. So the Skyrim-space
//! yaw matrix is `Rz(-r[2])`. Pitch and roll signs are UNVERIFIED —
//! no render has yet distinguished them — so they stay un-negated
//! until proven otherwise; do not "fix" them on symmetry grounds.

use bevy::math::{Mat3, Quat, Vec3};

/// Skyrim units per metre (64 units = 1 yard = 0.9144 m).
pub const SKYRIM_UNITS_PER_METER: f32 = 70.0;

/// The Skyrim -> Bevy axis map as a rotation matrix. Columns are the
/// images of the Skyrim basis vectors: X -> X, Y -> -Z, Z -> Y.
fn axis_map() -> Mat3 {
    Mat3::from_cols(Vec3::X, Vec3::new(0.0, 0.0, -1.0), Vec3::Y)
}

/// Convert a Skyrim-space position (units) to Bevy space (metres).
pub fn to_bevy_position(p: [f32; 3]) -> Vec3 {
    let v = Vec3::new(p[0], p[1], p[2]) / SKYRIM_UNITS_PER_METER;
    axis_map() * v
}

/// Convert a Bevy-space position (metres) back to Skyrim space
/// (units) — the exact inverse of `to_bevy_position`, for the
/// save system (a saved eye position becomes a spawn point).
/// Bevy.x = Sky.x/70, Bevy.y = Sky.z/70, Bevy.z = -Sky.y/70, so
/// Sky.x = Bevy.x*70, Sky.y = -Bevy.z*70, Sky.z = Bevy.y*70.
pub fn to_skyrim_position(p: Vec3) -> [f32; 3] {
    [
        p.x * SKYRIM_UNITS_PER_METER,
        -p.z * SKYRIM_UNITS_PER_METER,
        p.y * SKYRIM_UNITS_PER_METER,
    ]
}

/// Convert a Skyrim-space direction (no unit scaling: normals,
/// tangents, velocities) to Bevy space.
pub fn to_bevy_direction(d: [f32; 3]) -> Vec3 {
    axis_map() * Vec3::new(d[0], d[1], d[2])
}

/// Convert a Skyrim-space Euler rotation (radians, `Rx, Ry, Rz`) to a
/// Bevy quaternion.
pub fn to_bevy_rotation(r: [f32; 3]) -> Quat {
    let m = axis_map();
    // NOTE: the yaw (Z) angle is NEGATED — Skyrim yaw is
    // clockwise-positive, `from_rotation_z` is counterclockwise.
    // Pitch/roll stay un-negated (signs unverified, see module doc).
    let sky =
        Mat3::from_rotation_z(-r[2]) * Mat3::from_rotation_y(r[1]) * Mat3::from_rotation_x(r[0]);
    Quat::from_mat3(&(m * sky * m.transpose()))
}

/// Convert a Skyrim-space axis-aligned bounding box (units) to a Bevy
/// AABB in metres: returns `(min, max)`. The Y axis flips sign under
/// the map, so min/max swap on that axis.
pub fn to_bevy_bounds(min: [f32; 3], max: [f32; 3]) -> (Vec3, Vec3) {
    let lo = Vec3::new(
        min[0] / SKYRIM_UNITS_PER_METER,
        min[2] / SKYRIM_UNITS_PER_METER,
        -max[1] / SKYRIM_UNITS_PER_METER,
    );
    let hi = Vec3::new(
        max[0] / SKYRIM_UNITS_PER_METER,
        max[2] / SKYRIM_UNITS_PER_METER,
        -min[1] / SKYRIM_UNITS_PER_METER,
    );
    (lo, hi)
}

/// Convert a NIF row-major 3x3 rotation matrix (as stored in the
/// file) to a glam `Mat3` (column-major storage).
pub fn nif_mat3_to_glam(m: [[f32; 3]; 3]) -> Mat3 {
    Mat3::from_cols(
        Vec3::new(m[0][0], m[1][0], m[2][0]),
        Vec3::new(m[0][1], m[1][1], m[2][1]),
        Vec3::new(m[0][2], m[1][2], m[2][2]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn axis_map_is_a_rotation() {
        let m = axis_map();
        // Determinant +1: no mirroring, winding preserved.
        assert!((m.determinant() - 1.0).abs() < 1e-6);
        // Orthonormal: M * M^T = I.
        let id = m * m.transpose();
        assert!((id.x_axis - Vec3::X).length() < 1e-6);
        assert!((id.y_axis - Vec3::Y).length() < 1e-6);
        assert!((id.z_axis - Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn position_converts_axes_and_units() {
        // 70 units east, 140 north, 210 up -> 1m east, 3m up, 2m back.
        let p = to_bevy_position([70.0, 140.0, 210.0]);
        assert!(approx(p, Vec3::new(1.0, 3.0, -2.0)), "{p:?}");
        assert!(approx(to_bevy_position([0.0, 0.0, 0.0]), Vec3::ZERO));
    }

    #[test]
    fn skyrim_position_is_the_exact_inverse() {
        // The save system's round trip: Bevy metres -> Skyrim
        // units -> Bevy metres must come back bit-identical
        // (up to float error), and the Skyrim side must match
        // the axis map by hand.
        for sky in [
            [0.0, 0.0, 0.0],
            [70.0, 140.0, 210.0],
            [-350.5, 1234.0, -77.25],
            [287_000.0, -450_000.0, 512.0],
        ] {
            let bevy = to_bevy_position(sky);
            let back = to_skyrim_position(bevy);
            for i in 0..3 {
                assert!(
                    (back[i] - sky[i]).abs() < 0.01,
                    "{sky:?} -> {bevy:?} -> {back:?}"
                );
            }
        }
        // By hand: Bevy (1, 3, -2) m is Skyrim (70, 140, 210) u.
        assert_eq!(
            to_skyrim_position(Vec3::new(1.0, 3.0, -2.0)),
            [70.0, 140.0, 210.0]
        );
    }

    #[test]
    fn yaw_only_rotation_is_exact() {
        // Skyrim yaw is CLOCKWISE-positive seen from above: +90
        // degrees takes a point on Skyrim +X (east) to Skyrim -Y
        // (south); in Bevy that is +X -> +Z. (The old test asserted
        // +X -> -Z, the counterclockwise reading — wrong, and the
        // M12 Orgnar render proved it: the barkeep faced the back
        // wall instead of the room.)
        let q = to_bevy_rotation([0.0, 0.0, FRAC_PI_2]);
        let rotated = q * Vec3::X;
        assert!(approx(rotated, Vec3::new(0.0, 0.0, 1.0)), "{rotated:?}");
        // Identity stays identity.
        let id = to_bevy_rotation([0.0, 0.0, 0.0]);
        assert!(approx(id * Vec3::X, Vec3::X));
        assert!(approx(id * Vec3::Y, Vec3::Y));
    }

    #[test]
    fn yaw_turns_facing_from_north_toward_east() {
        // A character model faces Skyrim +Y (Bevy -Z) at yaw 0.
        // Positive yaw turns it toward east (+X): the rotated
        // forward must equal the spawn-facing formula
        // (sin yaw, cos yaw) in Skyrim space, mapped to Bevy —
        // i.e. `to_bevy_rotation` and `spawn_viewpoint` must agree.
        // Orgnar's yaw 1.783 faces mostly east with a slight
        // southern (+Z) component — into the inn's common room,
        // not at the back wall (M12 regression).
        for yaw in [FRAC_PI_2, 1.783, 3.125] {
            let q = to_bevy_rotation([0.0, 0.0, yaw]);
            let forward = q * Vec3::new(0.0, 0.0, -1.0);
            let expected = Vec3::new(yaw.sin(), 0.0, -yaw.cos());
            assert!(approx(forward, expected), "yaw {yaw}: {forward:?}");
        }
        // And the east case exactly: yaw pi/2 faces Bevy +X.
        let east = to_bevy_rotation([0.0, 0.0, FRAC_PI_2]) * Vec3::new(0.0, 0.0, -1.0);
        assert!(approx(east, Vec3::X), "{east:?}");
    }

    #[test]
    fn pitch_only_rotation_maps_x_axis() {
        // 90-degree pitch about Skyrim X: +Z (up) goes to -Y... check
        // the converted frame instead: the rotation must stay a pure
        // rotation (unit quaternion) and keep +X fixed.
        let q = to_bevy_rotation([FRAC_PI_2, 0.0, 0.0]);
        assert!((q.length() - 1.0).abs() < 1e-6);
        assert!(approx(q * Vec3::X, Vec3::X));
        // +90 degrees about Skyrim X sends +Z (up) to -Y (south);
        // Skyrim -Y is Bevy +Z, so Bevy up (Y) goes to +Z.
        let up = q * Vec3::Y;
        assert!(approx(up, Vec3::new(0.0, 0.0, 1.0)), "{up:?}");
    }

    #[test]
    fn bounds_flip_y_min_max() {
        let (lo, hi) = to_bevy_bounds([-70.0, -140.0, 0.0], [70.0, 140.0, 70.0]);
        assert!(approx(lo, Vec3::new(-1.0, 0.0, -2.0)), "{lo:?}");
        assert!(approx(hi, Vec3::new(1.0, 1.0, 2.0)), "{hi:?}");
    }

    #[test]
    fn nif_mat3_transposes_rows_to_columns() {
        let m = nif_mat3_to_glam([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
        // First column is the first row's transpose.
        assert_eq!(m.x_axis, Vec3::new(1.0, 4.0, 7.0));
        assert_eq!(m.y_axis, Vec3::new(2.0, 5.0, 8.0));
        assert_eq!(m.z_axis, Vec3::new(3.0, 6.0, 9.0));
    }
}
