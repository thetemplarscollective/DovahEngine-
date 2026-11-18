#!/usr/bin/env python3
"""Pixel-brightness comparison for the light-falloff milestone.

Usage:
  analyze_falloff.py BEFORE AFTER [--region x0,y0,x1,y1] [--label NAME]

Prints whole-image and (optional) region luminance stats for both
images plus the pixel-diff summary. Luminance is Rec.709 on the
sRGB bytes (0..1). Clipped = > 0.98, black = < 0.02.
"""
import sys
import numpy as np
from PIL import Image


def lum(path):
    img = np.asarray(Image.open(path).convert("RGB"), dtype=np.float64) / 255.0
    return 0.2126 * img[..., 0] + 0.7152 * img[..., 1] + 0.0722 * img[..., 2]


def stats(a, name):
    flat = a.ravel()
    print(
        f"{name:>28}: mean={flat.mean():.3f} p5={np.percentile(flat, 5):.3f} "
        f"p50={np.percentile(flat, 50):.3f} p95={np.percentile(flat, 95):.3f} "
        f"max={flat.max():.3f} clipped={100 * (flat > 0.98).mean():.2f}% "
        f"black={100 * (flat < 0.02).mean():.2f}%"
    )


def main():
    args = sys.argv[1:]
    before, after = args[0], args[1]
    region = None
    label = "region"
    i = 2
    while i < len(args):
        if args[i] == "--region":
            region = tuple(int(v) for v in args[i + 1].split(","))
            i += 2
        elif args[i] == "--label":
            label = args[i + 1]
            i += 2
        else:
            i += 1
    b, a = lum(before), lum(after)
    print(f"before: {before}  {b.shape[1]}x{b.shape[0]}")
    print(f"after:  {after}  {a.shape[1]}x{a.shape[0]}")
    stats(b, "BEFORE whole")
    stats(a, "AFTER whole")
    if region:
        x0, y0, x1, y1 = region
        stats(b[y0:y1, x0:x1], f"BEFORE {label}")
        stats(a[y0:y1, x0:x1], f"AFTER {label}")
    d = np.abs(a - b)
    print(
        f"diff: mean|d|={d.mean():.3f} changed(>0.02)={100 * (d > 0.02).mean():.1f}% "
        f"of pixels"
    )


if __name__ == "__main__":
    main()
