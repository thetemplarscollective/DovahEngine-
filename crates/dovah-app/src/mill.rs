//! Mill wheel turning (Riverwood 1:1-exact milestone): the
//! lumber mill waterwheel rotates continuously, driven by its
//! own NIF animation data — not by a guessed speed.
//!
//! The data (mill_probe, `mill_probe.log`): the Riverwood wheel
//! is REFR 0x000a1ff8, base 0x000f3e42
//! `FarmLumbermill01WaterWheel01` [MSTT], NIF
//! `meshes/Architecture/Farmhouse/Lumbermill01WaterWheel01.nif`.
//! No scripts (VMAD empty on ref and base). The NIF carries one
//! `NiControllerSequence`, named `Idle`, driving node
//! `SawWaterWheel03`: Y keys 0 -> -2π over 16.6667 s, X/Z
//! constant — one full turn every 16⅔ s, 0.377 rad/s, about the
//! node's local Y axis. `dovah_nif::NifFile::spinning_nodes`
//! finds such nodes; the scene build tags their meshes with
//! [`MillWheel`].
//!
//! Axle proof (mill_probe Part C): the node's rest rotation is
//! Rz(90°), so its local Y maps to NIF-root -X — and both wheel
//! meshes' AABB centres lie EXACTLY on the -X line through the
//! node origin (|d x axle| = 0.0; the other candidate, parent Y,
//! misses by 106.2 / 9.2 units). The wheel disc spans Y/Z
//! (512x512 units, 7.3 m) and is thin in X, as a disc on an X
//! axle must be. The pivot is the node origin
//! (`Mesh::parent_transform.translation`), the axle is
//! `parent_transform.rotate(local Y)` — the same hinge pattern
//! as the door panels, generalized to an arbitrary axis.
//!
//! Direction check: the authored turn (negative about local Y)
//! puts the wheel's bottom moving at Bevy (0.711, 0, -0.703)
//! through the ref yaw (0.790 rad) — within ~17° of the river
//! flow (RiverWaterFlowNE NAM0 [2.54, 1.35] -> Bevy
//! (0.883, 0, -0.469), dot 0.958). The water pushes the wheel
//! downstream, as Bethesda authored it. The
//! `bottom_moves_with_the_flow` test pins this.
//!
//! Animation: like [`door_swing`](crate::doors::door_swing), the
//! transform is recomputed from the stored rest transform every
//! frame — no accumulation error, and a mid-game load can never
//! leave the wheel at a wrong angle. The angle comes from
//! `Time::elapsed_secs` (absolute, wrapped), not from summed
//! deltas.

use bevy::prelude::*;

/// One spinning sub-mesh's animation data in NIF-local terms,
/// as the scene build caches it per sub-mesh (mirrors the door
/// `hinge_local` in `CachedSubmesh`): the pivot in Skyrim units,
/// NIF-root space, and the axle as a unit direction in the same
/// space, plus the signed angular velocity from the NIF's keys.
#[derive(Debug, Clone, Copy)]
pub struct SpinLocal {
    /// Pivot in Skyrim units, NIF-root space (the spinning
    /// node's origin = `Mesh::parent_transform.translation`).
    pub pivot: [f32; 3],
    /// Axle as a unit direction in NIF-root Skyrim space
    /// (`parent_transform.rotate(axis_unit)`).
    pub axle: [f32; 3],
    /// Signed angular velocity in rad/s, from the NIF's keys
    /// (the wheel: -0.377 about local Y).
    pub angular_velocity: f32,
}

/// A mill-wheel mesh entity: its rest transform plus the pivot
/// and axle (Bevy world space) it turns about. Attached by the
/// scene build to meshes under a spinning node.
#[derive(Component, Debug, Clone)]
pub struct MillWheel {
    /// The mesh's transform at rest (the ref placement).
    pub rest: Transform,
    /// Pivot in Bevy world metres (the spinning node origin).
    pub pivot: Vec3,
    /// Axle in Bevy world space (unit).
    pub axle: Vec3,
    /// Signed angular velocity in rad/s (from the NIF's keys).
    pub angular_velocity: f32,
}

/// Build a [`MillWheel`] from the ref's world transform and the
/// NIF-local spin data: the pivot is the ref transform applied
/// to the NIF-local pivot point, the axle is the ref rotation
/// applied to the NIF-local axle direction. Pure — unit-tested.
pub fn mill_wheel_spawn(ref_transform: &Transform, spin: SpinLocal) -> MillWheel {
    let pivot = ref_transform.transform_point(crate::coords::to_bevy_position(spin.pivot));
    let axle =
        (ref_transform.rotation * crate::coords::to_bevy_direction(spin.axle)).normalize_or_zero();
    MillWheel {
        rest: *ref_transform,
        pivot,
        axle,
        angular_velocity: spin.angular_velocity,
    }
}

/// The wheel's rotation angle (radians) at `elapsed` seconds of
/// app time: angular velocity times elapsed, wrapped to (-π, π]
/// so the floats stay small forever. Pure — unit-tested.
pub fn wheel_angle(angular_velocity: f32, elapsed: f32) -> f32 {
    let two_pi = 2.0 * std::f32::consts::PI;
    let wrapped = (angular_velocity * elapsed).rem_euclid(two_pi);
    if wrapped > std::f32::consts::PI {
        wrapped - two_pi
    } else {
        wrapped
    }
}

/// Rotate `rest` about `pivot`/`axle` by `angle` radians: the
/// `swing_transform` pattern generalized from world-Y to an
/// arbitrary axle. The pivot stays fixed; points on the axle
/// stay put; everything else orbits it. Pure — unit-tested.
pub fn wheel_transform(rest: &Transform, pivot: Vec3, axle: Vec3, angle: f32) -> Transform {
    let rot = Quat::from_axis_angle(axle, angle);
    Transform {
        translation: pivot + rot * (rest.translation - pivot),
        rotation: rot * rest.rotation,
        scale: rest.scale,
    }
}

/// Turn every mill wheel. Runs in InGame on both app paths
/// (registered in PlayerPlugin for the windowed path and
/// ScreenshotPlugin for the screenshot path, next to
/// `animate_water_flow`). `Time` is optional: the headless
/// unit-test app has no TimePlugin, and the system must no-op
/// there, not panic. The angle is absolute (from elapsed time),
/// so the wheel never accumulates error and never depends on
/// frame rate.
pub fn animate_mill_wheel(
    time: Option<Res<Time>>,
    mut wheels: Query<(&mut Transform, &MillWheel)>,
) {
    let Some(time) = time else {
        return;
    };
    let elapsed = time.elapsed_secs();
    for (mut transform, wheel) in wheels.iter_mut() {
        let angle = wheel_angle(wheel.angular_velocity, elapsed);
        *transform = wheel_transform(&wheel.rest, wheel.pivot, wheel.axle, angle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    /// The real wheel's numbers (mill_probe): pivot = the
    /// SawWaterWheel03 node origin in NIF-root Skyrim units,
    /// axle = NIF-root -X, ω = -2π/16.6667 rad/s about local Y.
    fn wheel_spin() -> SpinLocal {
        SpinLocal {
            pivot: [459.2, 0.0, -160.2],
            axle: [-1.0, 0.0, 0.0],
            angular_velocity: -2.0 * PI / 16.6667,
        }
    }

    /// The real wheel ref's placement: pos
    /// (19650.4, -43941.3, 90.9), yaw 0.790 rad.
    fn wheel_ref_transform() -> Transform {
        Transform {
            translation: crate::coords::to_bevy_position([19650.4, -43941.3, 90.9]),
            rotation: crate::coords::to_bevy_rotation([0.0, 0.0, 0.790]),
            scale: Vec3::ONE,
        }
    }

    #[test]
    fn wheel_angle_wraps() {
        let omega = -2.0 * PI / 16.6667;
        // At t=0 the wheel is at rest.
        assert!((wheel_angle(omega, 0.0)).abs() < 1e-6);
        // Half a period: half a turn — exactly on the branch
        // cut, so either sign is the same rotation; check the
        // magnitude, not the sign.
        assert!((wheel_angle(omega, 16.6667 / 2.0).abs() - PI).abs() < 1e-3);
        // A full period: back at rest.
        assert!((wheel_angle(omega, 16.6667)).abs() < 1e-3);
        // Far future: still wrapped to (-π, π].
        let far = wheel_angle(omega, 100_000.0);
        assert!(far > -PI && far <= PI, "far {far}");
        // And the wrap is consistent: t and t+period agree.
        let a = wheel_angle(omega, 1234.5);
        let b = wheel_angle(omega, 1234.5 + 16.6667);
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn wheel_transform_keeps_pivot_fixed() {
        let rest = Transform::IDENTITY;
        let pivot = Vec3::new(1.0, 2.0, 3.0);
        let axle = Vec3::X;
        for angle in [-0.377, 1.0, -3.0, 0.5] {
            let turned = wheel_transform(&rest, pivot, axle, angle);
            let moved = turned.transform_point(pivot);
            assert!((moved - pivot).length() < 1e-5, "angle {angle}: {moved:?}");
        }
    }

    #[test]
    fn wheel_transform_orbits_around_axle() {
        // Axle = world X through the origin; a point 1 m below
        // the axle (0,-1,0) rotated +90° about +X lands at
        // (0,0,-1): +90° about X sends -Y to -Z.
        let rest = Transform::IDENTITY;
        let turned = wheel_transform(&rest, Vec3::ZERO, Vec3::X, FRAC_PI_2);
        let moved = turned.transform_point(Vec3::new(0.0, -1.0, 0.0));
        assert!(
            (moved - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "{moved:?}"
        );
        // Radius preserved for an off-axle point.
        let p = Vec3::new(0.5, -2.0, 1.0);
        let pivot = Vec3::new(1.0, 1.0, 1.0);
        let turned = wheel_transform(&rest, pivot, Vec3::Y, 0.7);
        let r0 = (p - pivot).length();
        let r1 = (turned.transform_point(p) - pivot).length();
        assert!((r0 - r1).abs() < 1e-5);
    }

    #[test]
    fn mill_wheel_spawn_real_numbers() {
        // The real wheel ref through the real code path: pivot
        // and axle in Bevy world space.
        let wheel = mill_wheel_spawn(&wheel_ref_transform(), wheel_spin());
        // Pivot: ref pos + ref-rotated NIF pivot, /70.
        // NIF pivot (459.2, 0, -160.2) under yaw 0.79:
        // Rz(-0.79) * (459.2, 0) = (323.0, -326.5); z -160.2.
        // World Skyrim (19973.4, -44267.8, -69.3) -> Bevy.
        let expect_pivot = Vec3::new(19973.4 / 70.0, -69.3 / 70.0, 44267.8 / 70.0);
        assert!(
            (wheel.pivot - expect_pivot).length() < 0.05,
            "pivot {:?} vs {expect_pivot:?}",
            wheel.pivot
        );
        // Axle: NIF -X through yaw 0.79 -> Skyrim
        // (-0.703, 0.711, 0) -> Bevy (-0.703, 0, -0.711).
        let expect_axle = Vec3::new(-0.703, 0.0, -0.711).normalize();
        assert!(
            (wheel.axle - expect_axle).length() < 0.01,
            "axle {:?} vs {expect_axle:?}",
            wheel.axle
        );
        assert!((wheel.axle.length() - 1.0).abs() < 1e-6);
        assert!((wheel.angular_velocity - (-0.37699112)).abs() < 1e-4);
    }

    #[test]
    fn bottom_moves_with_the_flow() {
        // The wheel's bottom point must move downstream: the
        // river flows NE (RiverWaterFlowNE NAM0 [2.54, 1.35]
        // Skyrim units -> Bevy), and the authored turn drives
        // the bottom the same way (dot 0.958 by hand). The
        // velocity is ω_vec x r with r straight down.
        let wheel = mill_wheel_spawn(&wheel_ref_transform(), wheel_spin());
        let omega_vec = wheel.axle * wheel.angular_velocity;
        let bottom_v = omega_vec.cross(Vec3::new(0.0, -1.0, 0.0));
        let flow = crate::coords::to_bevy_direction([2.54, 1.35, 0.0]).normalize();
        let dot = bottom_v.normalize().dot(flow);
        assert!(dot > 0.9, "bottom velocity dot flow = {dot}");
    }
}
