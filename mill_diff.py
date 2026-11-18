#!/usr/bin/env python3
"""Pixel-diff two renders (mill wheel proof): reports how much
changed, where, and whether the change is confined to a region.
Same method as the water f10-vs-f60 proof.

Usage: mill_diff.py <a.png> <b.png> [out-diff.png]
"""
import sys
import numpy as np
from PIL import Image

a = np.asarray(Image.open(sys.argv[1]).convert("RGB")).astype(np.int16)
b = np.asarray(Image.open(sys.argv[2]).convert("RGB")).astype(np.int16)
assert a.shape == b.shape, f"shape mismatch {a.shape} vs {b.shape}"
h, w, _ = a.shape
diff = np.abs(a - b).max(axis=2)  # worst channel per pixel
changed = diff > 3  # > 3 LSB counts as a real change, not dither
n_changed = int(changed.sum())
print(f"size {w}x{h} ({w*h} px)")
print(f"mean abs diff (max-channel): {diff.mean():.3f} LSB")
print(f"pixels changed (>3 LSB): {n_changed} ({100.0*n_changed/(w*h):.2f}%)")
if n_changed:
    ys, xs = np.where(changed)
    print(f"changed bbox: x {xs.min()}..{xs.max()}, y {ys.min()}..{ys.max()}")
    print(f"bbox size: {xs.max()-xs.min()+1}x{ys.max()-ys.min()+1}")
    # Row profile: which horizontal bands changed (sky should be 0).
    bands = 8
    for i in range(bands):
        y0, y1 = i * h // bands, (i + 1) * h // bands
        n = int(changed[y0:y1].sum())
        print(f"  rows {y0:4d}..{y1:4d}: {n:7d} changed px")
if len(sys.argv) > 3:
    vis = np.asarray(Image.open(sys.argv[1]).convert("RGB")).copy()
    vis[changed] = [255, 0, 0]
    Image.fromarray(vis).save(sys.argv[3])
    print(f"diff overlay saved to {sys.argv[3]}")
