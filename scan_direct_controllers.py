#!/usr/bin/env python3
"""Scan NIFs for directly-attached NiTransformControllers (not via
sequences): report the controller flags/frequency/phase/start/stop,
the driven node, and the rotation sweep of the data behind it.

Usage: python3 scan_direct_controllers.py <nif> [<nif> ...]
"""
import struct
import sys
import math

sys.path.insert(0, '/home/hatch/workspace/dovahengine')
from decode_transform_data import try_decode


def load_nif(path):
    data = open(path, 'rb').read()
    pos = 0
    nl = data.index(b'\n', pos)
    pos = nl + 1
    pos += 4
    pos += 1
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
    strings = []
    for _ in range(num_strings):
        s, pos = sized_string(pos)
        strings.append(s.decode(errors='replace'))
    num_groups, = struct.unpack_from('<I', data, pos)
    pos += 4 + 4 * num_groups
    blks = []
    off = pos
    for i in range(num_blocks):
        blks.append((block_types[type_idx[i]], data[off:off + sizes[i]]))
        off += sizes[i]
    return blks, strings


def node_name(blks, strings, i):
    t, b = blks[i]
    if len(b) < 8:
        return '?'
    name_idx, = struct.unpack_from('<I', b, 0)
    return strings[name_idx] if name_idx < len(strings) else '?'


def scan(path):
    blks, strings = load_nif(path)
    print(f"== {path} ({len(blks)} blocks) ==")
    found = False
    for i, (t, b) in enumerate(blks):
        if t not in ('NiNode', 'BSTriShape', 'NiTriShape', 'BSDynamicTriShape'):
            continue
        if len(b) < 12:
            continue
        name_idx, num_extra = struct.unpack_from('<II', b, 0)
        coff = 8 + 4 * num_extra
        if coff + 4 > len(b):
            continue
        ctrl, = struct.unpack_from('<i', b, coff)
        if ctrl == -1:
            continue
        found = True
        name = strings[name_idx] if name_idx < len(strings) else '?'
        ct, cb = blks[ctrl]
        print(f"  [{i}] {t} '{name}' controller -> [{ctrl}] {ct} size={len(cb)}")
        if ct == 'NiTransformController' and len(cb) >= 26:
            nxt, = struct.unpack_from('<i', cb, 0)
            flags, = struct.unpack_from('<H', cb, 4)
            freq, phase, start, stop = struct.unpack_from('<ffff', cb, 6)
            rest = cb[22:]
            refs = struct.unpack_from('<' + 'i' * (len(rest) // 4), rest) if len(rest) >= 4 else ()
            print(f"      next={nxt} flags=0x{flags:04x} freq={freq} phase={phase} "
                  f"start={start} stop={stop} tail_refs={refs}")
            # The interpolator ref is the LAST i32 (target first, if present).
            interp = refs[-1] if refs else -1
            if 0 <= interp < len(blks):
                it, ib = blks[interp]
                print(f"      interp [{interp}] {it} size={len(ib)}")
                if it == 'NiTransformInterpolator' and len(ib) >= 4:
                    data_ref, = struct.unpack_from('<i', ib, len(ib) - 4)
                    print(f"      data ref [{data_ref}]", end='')
                    if 0 <= data_ref < len(blks):
                        dt, db = blks[data_ref]
                        print(f" {dt} size={len(db)}")
                        got = try_decode(db, len(db))
                        if got is None:
                            print("        (TransformData: no layout fits)")
                        else:
                            layout, res = got
                            print(f"        layout={layout}")
                            for k in ('x', 'y', 'z'):
                                if k in res and res[k]:
                                    ks = res[k]
                                    vals = [v for (_, v, _, _) in ks]
                                    print(f"        {k}: {len(ks)} keys "
                                          f"t={ks[0][0]:.2f}..{ks[-1][0]:.2f} "
                                          f"range {min(vals):.3f}..{max(vals):.3f} rad "
                                          f"({math.degrees(min(vals)):.0f}.."
                                          f"{math.degrees(max(vals)):.0f} deg)")
                            if 't' in res and res['t']:
                                ts = res['t']
                                print(f"        T: {len(ts)} keys "
                                      f"t={ts[0][0]:.2f}..{ts[-1][0]:.2f}")
                    else:
                        print()
    if not found:
        print("  (no direct controllers)")


if __name__ == '__main__':
    for p in sys.argv[1:]:
        scan(p)
