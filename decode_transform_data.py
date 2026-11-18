#!/usr/bin/env python3
"""Raw NiTransformData decoder for MillLogPile.nif (sawmill probe).

The Rust parser only handles rotation type 4 (XYZ Euler). The sawmill
furniture NIF also carries type-1 (quaternion?) rotation data and
num_rot_keys=0 blocks. This script tries candidate layouts and validates
each by EXACT byte consumption, then reports the motion each block
encodes (rotation sweep, translation range).

Usage: python3 decode_transform_data.py /tmp/milllogpile.nif
"""
import struct
import sys
import math

KEY_CONSTANT = 0
KEY_LINEAR = 1
KEY_QUADRATIC = 2


class R:
    def __init__(self, buf, pos=0):
        self.b = buf
        self.p = pos

    def u32(self):
        v, = struct.unpack_from('<I', self.b, self.p)
        self.p += 4
        return v

    def f32(self):
        v, = struct.unpack_from('<f', self.b, self.p)
        self.p += 4
        return v

    def vec3(self):
        return (self.f32(), self.f32(), self.f32())

    def quat(self):
        # Gamebryo quaternion keys: (w, x, y, z)? or (x, y, z, w)?
        # Try (w, x, y, z) first; validation is by consumption, and
        # the identity check (first key ~ (1,0,0,0)) confirms order.
        return (self.f32(), self.f32(), self.f32(), self.f32())

    def remaining(self, end):
        return end - self.p


def parse_float_group(r, end):
    num = r.u32()
    if num == 0:
        return []
    kt = r.u32()
    keys = []
    for _ in range(num):
        if kt == KEY_CONSTANT:
            keys.append((0.0, r.f32(), 0.0, 0.0))
        elif kt == KEY_LINEAR:
            keys.append((r.f32(), r.f32(), 0.0, 0.0))
        elif kt == KEY_QUADRATIC:
            keys.append((r.f32(), r.f32(), r.f32(), r.f32()))
        else:
            raise ValueError(f"bad float key type {kt}")
    return keys


def parse_vec3_group(r, end):
    num = r.u32()
    if num == 0:
        return []
    kt = r.u32()
    keys = []
    for _ in range(num):
        if kt == KEY_CONSTANT:
            keys.append((0.0, r.vec3(), (0, 0, 0), (0, 0, 0)))
        elif kt == KEY_LINEAR:
            keys.append((r.f32(), r.vec3(), (0, 0, 0), (0, 0, 0)))
        elif kt == KEY_QUADRATIC:
            keys.append((r.f32(), r.vec3(), r.vec3(), r.vec3()))
        else:
            raise ValueError(f"bad vec3 key type {kt}")
    return keys


def parse_quat_group(r, num, with_key_type):
    """Quaternion keys. with_key_type: a u32 key-type precedes the keys."""
    if with_key_type:
        kt = r.u32()
    else:
        kt = KEY_LINEAR
    keys = []
    for _ in range(num):
        if kt == KEY_CONSTANT:
            keys.append((0.0, r.quat(), None, None))
        elif kt == KEY_LINEAR:
            keys.append((r.f32(), r.quat(), None, None))
        elif kt == KEY_QUADRATIC:
            keys.append((r.f32(), r.quat(), r.quat(), r.quat()))
        else:
            raise ValueError(f"bad quat key type {kt}")
    return keys, kt


def try_decode(block, size):
    """Try candidate layouts; return (layout_name, result) for the one
    that consumes exactly `size` bytes, else None."""
    end = size
    # Layout A: XYZ (type 4) — the proven Rust layout.
    try:
        r = R(block)
        num_rot = r.u32()
        rot_type = r.u32()
        if rot_type == 4 and num_rot > 0:
            x = parse_float_group(r, end)
            y = parse_float_group(r, end)
            z = parse_float_group(r, end)
            t = parse_vec3_group(r, end)
            s = parse_float_group(r, end)
            if r.p == end:
                return ("XYZ", dict(num_rot=num_rot, x=x, y=y, z=z, t=t, s=s))
    except (ValueError, struct.error, IndexError):
        pass
    # Layout B: num_rot == 0, NO rotation-type field, straight to
    # translation + scale.
    try:
        r = R(block)
        num_rot = r.u32()
        if num_rot == 0:
            t = parse_vec3_group(r, end)
            s = parse_float_group(r, end)
            if r.p == end:
                return ("NOTRANS" if not t else "TRANS_ONLY",
                        dict(num_rot=0, t=t, s=s))
    except (ValueError, struct.error, IndexError):
        pass
    # Layout C: quaternion rotation, key-type field present.
    for with_kt in (True, False):
        try:
            r = R(block)
            num_rot = r.u32()
            rot_type = r.u32()
            if num_rot == 0:
                continue
            q, kt = parse_quat_group(r, num_rot, with_kt)
            t = parse_vec3_group(r, end)
            s = parse_float_group(r, end)
            if r.p == end:
                return (f"QUAT(kt={kt},with_type={with_kt},rot_type={rot_type})",
                        dict(num_rot=num_rot, rot_type=rot_type, q=q, t=t, s=s))
        except (ValueError, struct.error, IndexError):
            pass
    return None


def quat_angle_axis(q):
    """(angle_rad, axis) for a unit quat stored (w,x,y,z)."""
    w, x, y, z = q
    n = math.sqrt(w * w + x * x + y * y + z * z)
    if n == 0:
        return 0.0, (0.0, 0.0, 0.0)
    w, x, y, z = w / n, x / n, y / n, z / n
    w = max(-1.0, min(1.0, w))
    angle = 2.0 * math.acos(w)
    s = math.sqrt(max(0.0, 1.0 - w * w))
    if s < 1e-6:
        return angle, (1.0, 0.0, 0.0)
    return angle, (x / s, y / s, z / s)


def summarize(name, idx, layout, res):
    print(f"[{idx}] {name}: layout={layout}")
    if 'q' in res:
        qs = res['q']
        print(f"    quat keys: {len(qs)}  t={qs[0][0]:.3f}..{qs[-1][0]:.3f}")
        # Unwrap the total rotation: sum of successive relative angles.
        total = 0.0
        prev = None
        axes = []
        for (t, q, _, _) in qs:
            a, ax = quat_angle_axis(q)
            axes.append(ax)
            if prev is not None:
                # relative rotation angle between prev and q
                w1, x1, y1, z1 = prev
                w2, x2, y2, z2 = q
                dot = w1 * w2 + x1 * x2 + y1 * y2 + z1 * z2
                dot = max(-1.0, min(1.0, dot))
                total += 2.0 * math.acos(abs(dot))
            prev = q
        print(f"    total swept rotation: {total:.3f} rad = {math.degrees(total):.1f} deg "
              f"over {qs[-1][0] - qs[0][0]:.3f} s")
        # Dominant axis: average of per-key axes (sign-aligned).
        if axes:
            ax0 = axes[0]
            sx = sy = sz = 0.0
            for ax in axes:
                d = ax[0] * ax0[0] + ax[1] * ax0[1] + ax[2] * ax0[2]
                s = 1.0 if d >= 0 else -1.0
                sx += s * ax[0]
                sy += s * ax[1]
                sz += s * ax[2]
            n = math.sqrt(sx * sx + sy * sy + sz * sz) or 1.0
            print(f"    mean axis: ({sx / n:.3f},{sy / n:.3f},{sz / n:.3f})")
        # First/last quats.
        print(f"    first quat: {qs[0][1]}  last quat: {qs[-1][1]}")
    for k in ('x', 'y', 'z'):
        if k in res and res[k]:
            ks = res[k]
            vals = [v for (_, v, _, _) in ks]
            print(f"    {k}: {len(ks)} keys t={ks[0][0]:.3f}..{ks[-1][0]:.3f} "
                  f"range {min(vals):.4}..{max(vals):.4} rad "
                  f"({math.degrees(min(vals)):.1}..{math.degrees(max(vals)):.1} deg)")
    if 't' in res and res['t']:
        ts = res['t']
        xs = [v[0] for (_, v, _, _) in ts]
        ys = [v[1] for (_, v, _, _) in ts]
        zs = [v[2] for (_, v, _, _) in ts]
        print(f"    T: {len(ts)} keys t={ts[0][0]:.3f}..{ts[-1][0]:.3f} "
              f"x {min(xs):.1}..{max(xs):.1}  y {min(ys):.1}..{max(ys):.1}  "
              f"z {min(zs):.1}..{max(zs):.1}")
    if 's' in res and res['s']:
        print(f"    S: {len(res['s'])} keys (scale — not used)")


def main():
    path = sys.argv[1]
    data = open(path, 'rb').read()
    pos = 0
    nl = data.index(b'\n', pos)
    pos = nl + 1
    version, = struct.unpack_from('<I', data, pos)
    pos += 4
    pos += 1  # endian
    user_version, num_blocks = struct.unpack_from('<II', data, pos)
    pos += 8
    bs_version, = struct.unpack_from('<I', data, pos)
    pos += 4

    def export_string(p):
        n = data[p]
        p += 1
        return data[p:p + n], p + n

    _, pos = export_string(pos)
    if bs_version < 131:
        _, pos = export_string(pos)
    _, pos = export_string(pos)
    num_block_types, = struct.unpack_from('<H', data, pos)
    pos += 2

    def sized_string(p):
        n, = struct.unpack_from('<I', data, p)
        p += 4
        return data[p:p + n], p + n

    block_types = []
    for _ in range(num_block_types):
        s, pos = sized_string(pos)
        block_types.append(s.decode())
    type_idx = []
    for _ in range(num_blocks):
        i, = struct.unpack_from('<H', data, pos)
        pos += 2
        type_idx.append(i)
    sizes = []
    for _ in range(num_blocks):
        s, = struct.unpack_from('<I', data, pos)
        pos += 4
        sizes.append(s)
    num_strings, _ = struct.unpack_from('<II', data, pos)
    pos += 8
    for _ in range(num_strings):
        _, pos = sized_string(pos)
    num_groups, = struct.unpack_from('<I', data, pos)
    pos += 4 + 4 * num_groups

    # Block index -> (sequence, target) for the sawmill rig, from the
    # probe log (LoadDust interp/data pairs).
    targets = {
        8: "LoadDust/MillLog3", 10: "LoadDust/MillLog4",
        12: "LoadDust/MillLog2", 14: "LoadDust/MillLog1",
        16: "LoadDust/MillLogRoot", 18: "LoadDust/MillLogLeftBone",
        20: "LoadDust/MillLogLeft", 22: "LoadDust/MillLogRightBone",
        25: "LoadDust/SawTeeth", 52: "LoadDust/SawChute",
        62: "LoadDust/LogDropEmitGeo", 98: "LoadDust/NoiseController",
        100: "LoadDust/SashController", 102: "LoadDust/SawSash",
        104: "LoadDust/SawPivot", 106: "LoadDust/SawBlade",
        166: "PileDust/SawSash", 168: "PileDust/SawPivot",
        170: "PileDust/SawBlade",
    }
    off = pos
    for i in range(num_blocks):
        size = sizes[i]
        t = block_types[type_idx[i]]
        if t == "NiTransformData" and i in targets:
            block = data[off:off + size]
            got = try_decode(block, size)
            if got is None:
                print(f"[{i}] {targets[i]}: NO LAYOUT FITS (size={size}, "
                      f"first u32s={struct.unpack_from('<8I', block)})")
            else:
                layout, res = got
                summarize(targets[i], i, layout, res)
        off += size


if __name__ == "__main__":
    main()
