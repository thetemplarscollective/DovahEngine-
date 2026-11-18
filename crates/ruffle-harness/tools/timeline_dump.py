#!/usr/bin/env python3
"""Dump one DefineSprite's timeline frame by frame from a SWF:
frame labels, DoAction stop/play/goto, and per-frame placements with
CxForm alpha (mult/add). Used to answer "what is actually visible at
frame N of this clip" without rendering.

Usage: timeline_dump.py <file.swf> <sprite-id> [<sprite-id>...]
"""
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "interface-extract"))
sys.path.insert(0, "/home/hatch/workspace/dovahengine/interface-extract")
from swf_parse import BitReader  # noqa: E402


def read_cxform(r, with_alpha=True):
    """Parse a CXFORMWITHALPHA; return (mult_rgba, add_rgba) as 0-256 / int."""
    has_add = r.ub(1)
    has_mult = r.ub(1)
    nbits = r.ub(4)
    mult = [256, 256, 256, 256]
    add = [0, 0, 0, 0]
    n = 4 if with_alpha else 3
    if has_mult:
        for i in range(n):
            mult[i] = r.sb(nbits)
    if has_add:
        for i in range(n):
            add[i] = r.sb(nbits)
    return mult, add


def parse_actions(data):
    """Parse an AS1/2 action stream; return list of (op, arg) for the
    control-flow ops we care about (stop/play/goto)."""
    ops = []
    i = 0
    while i < len(data):
        op = data[i]
        i += 1
        if op >= 0x80:
            if i + 2 > len(data):
                break
            ln = struct.unpack_from("<H", data, i)[0]
            i += 2
            payload = data[i : i + ln]
            i += ln
            if op == 0x81 and len(payload) >= 2:  # GotoFrame
                ops.append(("gotoFrame", struct.unpack_from("<H", payload, 0)[0]))
            elif op == 0x8C:  # GotoLabel
                ops.append(("gotoLabel", payload.split(b"\x00")[0].decode("utf-8", "replace")))
            elif op == 0x87:  # StoreRegister etc — skip
                pass
            else:
                ops.append((f"op{op:#x}", len(payload)))
        else:
            if op == 0x06:
                ops.append(("play", None))
            elif op == 0x07:
                ops.append(("stop", None))
            elif op == 0x00:
                ops.append(("end", None))
                break
            else:
                ops.append((f"op{op:#x}", None))
    return ops


def iter_tags(r, end):
    """Yield (code, data_start, length) for tags in [r.pos, end)."""
    while r.pos + 2 <= end:
        start = r.pos
        h = r.u16()
        code = h >> 6
        ln = h & 0x3F
        if ln == 0x3F:
            ln = r.u32()
        yield code, r.pos, ln
        r.pos = r.pos + ln
        if code == 0:
            break


def dump_sprite(path, wanted_ids):
    raw = Path(path).read_bytes()
    magic = raw[:3]
    if magic in (b"CWS", b"CFX"):  # CFX = Scaleform's zlib-compressed .gfx
        body = zlib.decompress(raw[8:])
        data = raw[:8] + body
    elif magic in (b"FWS", b"GFX"):
        data = raw
    else:
        print(f"unsupported magic {magic}")
        return
    r = BitReader(data, 0)
    r.pos = 3  # skip magic
    r.u8()  # version
    r.u32()  # file length
    r.rect()  # frame size
    r.u16()  # frame rate
    r.u16()  # frame count
    for code, dstart, ln in iter_tags(r, len(data)):
        if code != 39:  # DefineSprite
            continue
        sr = BitReader(data, dstart)
        cid = sr.u16()
        if cid not in wanted_ids:
            continue
        nframes = sr.u16()
        print(f"=== sprite id {cid} ({nframes} frames) ===")
        frame = 1
        dend = dstart + ln
        for scode, sstart, sln in iter_tags(sr, dend):
            if scode == 1:  # ShowFrame
                frame += 1
            elif scode == 43:  # FrameLabel
                lr = BitReader(data, sstart)
                label = lr.string()
                print(f"  frame {frame}: LABEL '{label}'")
            elif scode == 12:  # DoAction
                ops = parse_actions(data[sstart : sstart + sln])
                interesting = [o for o in ops if o[0] in ("stop", "play", "gotoFrame", "gotoLabel")]
                if interesting:
                    print(f"  frame {frame}: ACTIONS {interesting}")
                elif ops:
                    print(f"  frame {frame}: actions ({len(ops)} ops, no stop/play/goto)")
            elif scode in (26, 70):  # PlaceObject2/3
                pr = BitReader(data, sstart)
                flags = pr.u8()
                has_clip_actions = flags & 0x80
                has_clip_depth = flags & 0x40
                has_name = flags & 0x20
                has_ratio = flags & 0x10
                has_cxform = flags & 0x08
                has_matrix = flags & 0x04
                has_char = flags & 0x02
                move = flags & 0x01
                depth = pr.u16()
                char = pr.u16() if has_char else None
                if has_matrix:
                    pr.matrix()
                alpha = None
                if has_cxform:
                    mult, add = read_cxform(pr, with_alpha=True)
                    alpha = (mult[3], add[3])
                if has_ratio:
                    pr.u16()
                name = pr.string() if has_name else ""
                clip_depth = None
                if has_clip_depth:
                    clip_depth = pr.u16()
                # (clip actions skipped: tag end known)
                tag = "move" if move else "place"
                a = f" alphaMult={alpha[0]}/256 alphaAdd={alpha[1]}" if alpha else ""
                cd = f" CLIPDEPTH={clip_depth}" if clip_depth is not None else ""
                print(f"  frame {frame}: {tag} depth={depth} char={char} name='{name}'{a}{cd}")
        print()


if __name__ == "__main__":
    swf = sys.argv[1]
    ids = {int(x) for x in sys.argv[2:]}
    dump_sprite(swf, ids)
