//! Sawmill working (Riverwood 1:1-exact milestone): the
//! sawmill's blade spins through its authored keyframed
//! performance, driven by its own NIF animation data — not by
//! a guessed speed.
//!
//! The data (sawmill_probe, `sawmill_probe3.log`): the
//! Riverwood sawmill furniture is REFR 0x00071dfc, base
//! 0x00071c47 `ResourceObjectSawmill` [FURN], NIF
//! `meshes/Furniture/Clutter/MillLogPile.nif`. The visible
//! blade is the SawGear:9 disc (574 verts, ~2 m across) under
//! node `SawGear`, which carries a DIRECT
//! `NiTransformController` — not a sequence — with Y keys 0°
//! hold 0–15.37 s, 0→1080° (3 turns) 15.37–50.67 s, snap to
//! −5° at 57.17 s, 0° hold 58.1–64.53 s, 0→892° (2.48 turns)
//! 64.53–93.43 s, wobble to 887.2° at 100.73 s. X flat, Z
//! constant 90° (rest), no translation, scale 1.0.
//! `dovah_nif::NifFile::direct_controllers` finds such nodes;
//! the scene build tags their meshes with [`SawmillBlade`].
//!
//! Flags calibration: every direct controller in the
//! grindstone NIF — whose wheel provably spins in-game when
//! the grindstone is used — carries the same flags 0x004c.
//! That is the standard furniture-controller flags, not an
//! inactive marker.
//!
//! Trigger (provisional, stated plainly): in vanilla the blade
//! plays when the mill is WORKED — the script plays the work
//! animation, and the grindstone wheel likewise spins only
//! while sharpening. This engine has no furniture-use or
//! NPC-work system yet, so the performance loops ambiently
//! from scene load: the motion is 100% authored data, the
//! trigger is a placeholder the furniture-interaction
//! milestone will gate on use. The loop snaps from 887° back
//! to 0° every 100.73 s; that is the keys as written, not
//! smoothed over.
//!
//! The sash/carriage (SawSash/SashController) is STATIC in the
//! placed design — constant keys only in every sequence — so
//! it does not move. The log cycle (LoadDust/PileDust
//! sequences, quaternion keys) needs the sequence player and
//! the script trigger; it is not part of this milestone.
//!
//! Animation: like the mill wheel, the transform is
//! recomputed from the stored rest transform every frame — no
//! accumulation error. The angle is the keyframed track
//! evaluated at the performance time (absolute elapsed time,
//! loop-wrapped), minus the track's first value (delta from
//! rest).

use bevy::prelude::*;
use dovah_nif::{FloatKey, KeyInterpolation, TransformDataBlock};

/// One keyframed rotation track: the driving axis's keys plus
/// the controller timing that maps app time onto it.
#[derive(Debug, Clone)]
pub struct BladeTrack {
    /// The driving axis's rotation keys (radians).
    pub keys: Vec<FloatKey>,
    /// How the keys interpolate (the blade's are quadratic).
    pub key_type: KeyInterpolation,
    /// First key time in seconds (the blade: 0.0).
    pub start_time: f32,
    /// Last key time in seconds (the blade: 100.7333).
    pub stop_time: f32,
    /// Playback speed multiplier (1.0 on every real controller).
    pub frequency: f32,
    /// Phase offset in seconds (0.0 on every real controller).
    pub phase: f32,
}

/// One keyframed sub-mesh's animation data in NIF-local terms,
/// as the scene build caches it per sub-mesh (mirrors
/// [`mill::SpinLocal`](crate::mill::SpinLocal)): the pivot in
/// Skyrim units, NIF-root space, the axle as a unit direction
/// in the same space, plus the keyframed track.
#[derive(Debug, Clone)]
pub struct BladeLocal {
    /// Pivot in Skyrim units, NIF-root space (the controlled
    /// node's origin = `Mesh::parent_transform.translation`).
    pub pivot: [f32; 3],
    /// Axle as a unit direction in NIF-root Skyrim space
    /// (`parent_transform.rotate(axis_unit)`).
    pub axle: [f32; 3],
    /// The keyframed rotation track for the driving axis.
    pub track: BladeTrack,
}

/// A sawmill-blade mesh entity: its rest transform plus the
/// pivot and axle (Bevy world space) it turns about, and the
/// keyframed track. Attached by the scene build to meshes
/// under a directly-controlled node.
#[derive(Component, Debug, Clone)]
pub struct SawmillBlade {
    /// The mesh's transform at rest (the ref placement).
    pub rest: Transform,
    /// Pivot in Bevy world metres (the controlled node origin).
    pub pivot: Vec3,
    /// Axle in Bevy world space (unit).
    pub axle: Vec3,
    /// The keyframed rotation track for the driving axis.
    pub track: BladeTrack,
}

/// Screenshot proof hook (`--sawmill-time <secs>`): pins the
/// blade's performance time, so a render can show the blade
/// mid-spin without running 15+ seconds of app time first
/// (the blade sits still for the first 15.37 s of its
/// performance). Absent on the windowed path; the system
/// treats a missing resource as "no override".
#[derive(Resource, Debug, Clone, Copy)]
pub struct SawmillTimeOverride(pub f32);

/// The driving axis as a unit vector in the node's local frame.
pub fn axis_unit(axis: usize) -> [f32; 3] {
    match axis {
        0 => [1.0, 0.0, 0.0],
        1 => [0.0, 1.0, 0.0],
        _ => [0.0, 0.0, 1.0],
    }
}

/// The driving axis of a direct controller's data: the axis
/// whose keys sweep the widest (max-min range), if any axis
/// moves by at least 1e-3 rad. Returns the axis index and its
/// interpolation type. Single-axis only: the blade (Y) and
/// the grindstone wheel (X) are each one axis, and no
/// furniture controller checked moves two axes at once.
/// `None` means "no motion" — the meshes stay at rest.
pub fn driving_axis(data: &TransformDataBlock) -> Option<(usize, KeyInterpolation)> {
    let groups = [
        (&data.x_keys, data.x_key_type),
        (&data.y_keys, data.y_key_type),
        (&data.z_keys, data.z_key_type),
    ];
    let mut best: Option<(usize, KeyInterpolation, f32)> = None;
    for (axis, (keys, key_type)) in groups.iter().enumerate() {
        if keys.len() < 2 {
            continue;
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for k in keys.iter() {
            lo = lo.min(k.value);
            hi = hi.max(k.value);
        }
        let range = hi - lo;
        if range < 1e-3 {
            continue;
        }
        if best.map(|(_, _, r)| range > r).unwrap_or(true) {
            best = Some((axis, *key_type, range));
        }
    }
    best.map(|(axis, key_type, _)| (axis, key_type))
}

/// Evaluate a float key track at time `t` (seconds), exactly
/// as Gamebryo does: constant tracks return their value,
/// linear tracks lerp, quadratic tracks Hermite-interpolate
/// with the keys' tangents (which are pre-scaled by the
/// segment duration — see [`FloatKey`]). Times outside the
/// key range clamp to the nearest key; an empty track
/// evaluates to 0.0. Pure — unit-tested.
pub fn eval_float_track(keys: &[FloatKey], key_type: KeyInterpolation, t: f32) -> f32 {
    let Some(first) = keys.first() else {
        return 0.0;
    };
    let Some(last) = keys.last() else {
        return 0.0;
    };
    if keys.len() == 1 || t <= first.time {
        return first.value;
    }
    if t >= last.time {
        return last.value;
    }
    // keys[i].time <= t < keys[i+1].time (keys are time-sorted).
    let i = keys.partition_point(|k| k.time <= t) - 1;
    let (k0, k1) = (&keys[i], &keys[i + 1]);
    let dt = k1.time - k0.time;
    if dt <= 0.0 {
        return k1.value;
    }
    let u = (t - k0.time) / dt;
    match key_type {
        KeyInterpolation::Constant => k0.value,
        KeyInterpolation::Linear => k0.value + (k1.value - k0.value) * u,
        KeyInterpolation::Quadratic => {
            // Cubic Hermite, tangents already m·dt.
            let u2 = u * u;
            let u3 = u2 * u;
            let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
            let h10 = u3 - 2.0 * u2 + u;
            let h01 = -2.0 * u3 + 3.0 * u2;
            let h11 = u3 - u2;
            h00 * k0.value + h10 * k0.fwd_tangent + h01 * k1.value + h11 * k1.bwd_tangent
        }
    }
}

/// Map absolute app time onto a controller's performance time:
/// phase + elapsed × frequency, loop-wrapped into
/// [start, stop]. A degenerate (zero or negative) duration
/// pins the performance at its start. Pure — unit-tested.
pub fn performance_time(elapsed: f32, start: f32, stop: f32, frequency: f32, phase: f32) -> f32 {
    let duration = stop - start;
    if duration <= 0.0 {
        return start;
    }
    let t = phase + elapsed * frequency;
    start + (t - start).rem_euclid(duration)
}

/// The blade's rotation angle (radians) at a performance time:
/// the track evaluated there, minus the track's first value
/// (the delta from rest — the rest transform already carries
/// the node's rest rotation). Pure — unit-tested.
pub fn blade_angle_at(track: &BladeTrack, performance_t: f32) -> f32 {
    let rest = track.keys.first().map(|k| k.value).unwrap_or(0.0);
    eval_float_track(&track.keys, track.key_type, performance_t) - rest
}

/// The blade's rotation angle (radians) at `elapsed` seconds
/// of app time: the performance time is loop-wrapped first.
/// Pure — unit-tested.
pub fn blade_angle(track: &BladeTrack, elapsed: f32) -> f32 {
    let t = performance_time(
        elapsed,
        track.start_time,
        track.stop_time,
        track.frequency,
        track.phase,
    );
    blade_angle_at(track, t)
}

/// Build a [`SawmillBlade`] from the ref's world transform and
/// the NIF-local blade data: the pivot is the ref transform
/// applied to the NIF-local pivot point, the axle is the ref
/// rotation applied to the NIF-local axle direction. Pure —
/// unit-tested.
pub fn sawmill_blade_spawn(ref_transform: &Transform, blade: &BladeLocal) -> SawmillBlade {
    let pivot = ref_transform.transform_point(crate::coords::to_bevy_position(blade.pivot));
    let axle =
        (ref_transform.rotation * crate::coords::to_bevy_direction(blade.axle)).normalize_or_zero();
    SawmillBlade {
        rest: *ref_transform,
        pivot,
        axle,
        track: blade.track.clone(),
    }
}

/// Turn every sawmill blade. Runs in InGame on both app paths
/// (registered in PlayerPlugin for the windowed path and
/// ScreenshotPlugin for the screenshot path, next to
/// `animate_mill_wheel`). `Time` is optional: the headless
/// unit-test app has no TimePlugin, and the system must no-op
/// there, not panic. `--sawmill-time` pins the performance
/// time when present (screenshot proof hook). The angle is
/// absolute (from elapsed time), so the blade never
/// accumulates error and never depends on frame rate.
pub fn animate_sawmill_blade(
    time: Option<Res<Time>>,
    override_time: Option<Res<SawmillTimeOverride>>,
    mut blades: Query<(&mut Transform, &SawmillBlade)>,
) {
    let Some(time) = time else {
        return;
    };
    let elapsed = time.elapsed_secs();
    // Hoisted out of the loop: `Res` is not `Copy`, and matching
    // on the `Option<Res<..>>` inside the loop would move it on
    // the first blade.
    let override_secs = override_time.as_deref().map(|o| o.0);
    for (mut transform, blade) in blades.iter_mut() {
        let angle = match override_secs {
            Some(t) => blade_angle_at(&blade.track, t),
            None => blade_angle(&blade.track, elapsed),
        };
        *transform = crate::mill::wheel_transform(&blade.rest, blade.pivot, blade.axle, angle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: f32, value: f32) -> FloatKey {
        FloatKey {
            time,
            value,
            fwd_tangent: 0.0,
            bwd_tangent: 0.0,
        }
    }

    fn key_t(time: f32, value: f32, fwd: f32, bwd: f32) -> FloatKey {
        FloatKey {
            time,
            value,
            fwd_tangent: fwd,
            bwd_tangent: bwd,
        }
    }

    /// The real blade's Y track (MillLogPile.nif block [440],
    /// sawmill_probe): 9 quadratic keys, radians, tangents as
    /// the file carries them (only the 93.43 s key has nonzero
    /// tangents — bwd 1.379, fwd 0.137 after the backward-first
    /// file order is applied).
    fn blade_track() -> BladeTrack {
        BladeTrack {
            keys: vec![
                key(0.0, 0.0),
                key(15.3667, 0.0),
                key(50.6667, 18.8496),
                key(57.1667, -0.0873),
                key(58.1, 0.0),
                key(64.5333, 0.0),
                key_t(93.4333, 15.5724, 0.137, 1.379),
                key(96.3, 15.6855),
                key(100.7333, 15.4851),
            ],
            key_type: KeyInterpolation::Quadratic,
            start_time: 0.0,
            stop_time: 100.7333,
            frequency: 1.0,
            phase: 0.0,
        }
    }

    // 1.5708 is the REAL keyframe value from MillLogPile.nif's
    // SawGear data (Z constant), not a hand-typed approximation
    // of FRAC_PI_2 — replacing it with the constant would
    // falsify the test data.
    #[allow(clippy::approx_constant)]
    fn blade_data() -> TransformDataBlock {
        TransformDataBlock {
            x_keys: vec![key(0.0, 0.0), key(100.7333, 0.0)],
            y_keys: blade_track().keys,
            z_keys: vec![key(0.0, 1.5708), key(100.7333, 1.5708)],
            x_key_type: KeyInterpolation::Quadratic,
            y_key_type: KeyInterpolation::Quadratic,
            z_key_type: KeyInterpolation::Quadratic,
            translation_keys: Vec::new(),
            translation_key_type: KeyInterpolation::Constant,
        }
    }

    #[test]
    fn driving_axis_finds_blade_y() {
        // Y sweeps 18.94 rad, X ~0, Z 0 — the blade's axis is Y.
        let (axis, key_type) = driving_axis(&blade_data()).expect("a driving axis");
        assert_eq!(axis, 1);
        assert_eq!(key_type, KeyInterpolation::Quadratic);
    }

    #[test]
    fn driving_axis_none_when_flat() {
        // All axes constant: no motion, no component.
        let mut data = blade_data();
        data.y_keys = vec![key(0.0, 0.0), key(100.7333, 0.0)];
        assert!(driving_axis(&data).is_none());
        // A sub-threshold wiggle (the grindstone root's ~0.002
        // rad noise is above this, but a 0.0005 rad jitter is
        // not motion).
        data.y_keys = vec![key(0.0, 0.0), key(100.7333, 0.0005)];
        assert!(driving_axis(&data).is_none());
    }

    #[test]
    fn eval_constant_and_linear() {
        // Constant: one key, any time.
        let keys = vec![key(0.0, 3.5)];
        assert_eq!(
            eval_float_track(&keys, KeyInterpolation::Constant, 99.0),
            3.5
        );
        // Linear: endpoints, midpoint, clamps.
        let keys = vec![key(2.0, 0.0), key(12.0, 10.0)];
        assert_eq!(eval_float_track(&keys, KeyInterpolation::Linear, 2.0), 0.0);
        assert_eq!(eval_float_track(&keys, KeyInterpolation::Linear, 7.0), 5.0);
        assert_eq!(
            eval_float_track(&keys, KeyInterpolation::Linear, 12.0),
            10.0
        );
        assert_eq!(eval_float_track(&keys, KeyInterpolation::Linear, 0.0), 0.0);
        assert_eq!(
            eval_float_track(&keys, KeyInterpolation::Linear, 99.0),
            10.0
        );
        // Empty track: 0.0, not a panic.
        assert_eq!(eval_float_track(&[], KeyInterpolation::Linear, 1.0), 0.0);
    }

    #[test]
    fn eval_quadratic_wheel_is_linear() {
        // The grindstone Wheel's real keys (X 0 -> -69.115 rad
        // over 9.1333 s, tangents ∓69.115 = the segment delta):
        // Hermite with m0 = m1 = Δ is EXACTLY linear — the
        // wheel spins at constant speed, which is what the
        // tangents-as-m·dt reading predicts.
        let keys = vec![
            key_t(2.0, 0.0, -69.115, 0.0),
            key_t(11.1333, -69.115, 0.0, -69.115),
        ];
        for (t, expect) in [
            (2.0, 0.0),
            (4.2833, -17.2788),
            (6.5667, -34.5575),
            (8.85, -51.8363),
            (11.1333, -69.115),
        ] {
            let v = eval_float_track(&keys, KeyInterpolation::Quadratic, t);
            assert!((v - expect).abs() < 1e-3, "t={t}: {v} vs {expect}");
        }
    }

    #[test]
    fn eval_quadratic_zero_tangents_is_smoothstep() {
        // Zero tangents at both ends: the S-curve — midpoint
        // is the mean, and the curve starts/ends flat.
        let keys = vec![key(0.0, 0.0), key(10.0, 10.0)];
        let mid = eval_float_track(&keys, KeyInterpolation::Quadratic, 5.0);
        assert!((mid - 5.0).abs() < 1e-6, "mid {mid}");
        // Quarter point: smoothstep(0.25) = 0.15625.
        let q = eval_float_track(&keys, KeyInterpolation::Quadratic, 2.5);
        assert!((q - 1.5625).abs() < 1e-4, "q {q}");
    }

    #[test]
    fn blade_angle_real_track() {
        let track = blade_track();
        // Still for the first 15.37 s.
        assert!((blade_angle_at(&track, 0.0)).abs() < 1e-6);
        assert!((blade_angle_at(&track, 10.0)).abs() < 1e-6);
        assert!((blade_angle_at(&track, 15.3667)).abs() < 1e-6);
        // End of the first burst: exactly 3 turns.
        let burst = blade_angle_at(&track, 50.6667);
        assert!((burst - 18.8496).abs() < 1e-4, "burst {burst}");
        // The snap-back key: -5 deg, as authored (no unwrap).
        let snap = blade_angle_at(&track, 57.1667);
        assert!((snap - (-0.0873)).abs() < 1e-4, "snap {snap}");
        // Still again at 64.53 s.
        assert!((blade_angle_at(&track, 64.5333)).abs() < 1e-6);
        // End of the second burst: 892.2 deg.
        let burst2 = blade_angle_at(&track, 93.4333);
        assert!((burst2 - 15.5724).abs() < 1e-4, "burst2 {burst2}");
        // Performance end: 887.2 deg (the wobble).
        let end = blade_angle_at(&track, 100.7333);
        assert!((end - 15.4851).abs() < 1e-4, "end {end}");
        // Mid-burst: halfway through the first spin the blade
        // is ~1.5 turns in (S-curve midpoint = the mean).
        let mid = blade_angle_at(&track, 33.0167);
        assert!((mid - 9.4248).abs() < 1e-3, "mid {mid}");
        // Past the end clamps at the last key.
        assert!((blade_angle_at(&track, 200.0) - 15.4851).abs() < 1e-4);
    }

    #[test]
    fn performance_time_loops() {
        // The blade's 100.7333 s performance loops: elapsed
        // time + one period gives the same angle.
        let track = blade_track();
        let a = blade_angle(&track, 33.0167);
        let b = blade_angle(&track, 33.0167 + 100.7333);
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        // Far future: still inside [start, stop].
        let t = performance_time(1_000_000.0, 0.0, 100.7333, 1.0, 0.0);
        assert!((0.0..=100.7333).contains(&t), "t {t}");
        // Degenerate duration pins at start.
        assert_eq!(performance_time(5.0, 2.0, 2.0, 1.0, 0.0), 2.0);
    }

    #[test]
    fn sawmill_blade_spawn_real_numbers() {
        // The real blade ref through the real code path: ref
        // 0x00071dfc at (19277.3, -44313.2, 90.9), yaw 0.790;
        // SawGear node origin (-24.3, 648.9, -11.9) NIF units,
        // local Y through Rz(90°) = NIF-root -X. Probe Part F:
        // pivot (281.73, 1.13, 626.27), axle (-0.704, 0, -0.710).
        let ref_transform = Transform {
            translation: crate::coords::to_bevy_position([19277.3, -44313.2, 90.9]),
            rotation: crate::coords::to_bevy_rotation([0.0, 0.0, 0.790]),
            scale: Vec3::ONE,
        };
        let local = BladeLocal {
            pivot: [-24.3, 648.9, -11.9],
            axle: [-1.0, 0.0, 0.0],
            track: blade_track(),
        };
        let blade = sawmill_blade_spawn(&ref_transform, &local);
        let expect_pivot = Vec3::new(281.73, 1.13, 626.27);
        assert!(
            (blade.pivot - expect_pivot).length() < 0.05,
            "pivot {:?} vs {expect_pivot:?}",
            blade.pivot
        );
        let expect_axle = Vec3::new(-0.704, 0.0, -0.710).normalize();
        assert!(
            (blade.axle - expect_axle).length() < 0.01,
            "axle {:?} vs {expect_axle:?}",
            blade.axle
        );
        assert!((blade.axle.length() - 1.0).abs() < 1e-6);
        assert_eq!(blade.track.keys.len(), 9);
    }
}
