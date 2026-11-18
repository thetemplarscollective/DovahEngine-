import sys
from PIL import Image
import numpy as np

def diff(a_path, b_path, thresh=3):
    a = np.asarray(Image.open(a_path).convert("RGB")).astype(np.int16)
    b = np.asarray(Image.open(b_path).convert("RGB")).astype(np.int16)
    d = np.abs(a - b).max(axis=2)
    changed = d > thresh
    n = int(changed.sum())
    total = changed.size
    ys, xs = np.where(changed)
    bbox = (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())) if n else None
    print(f"{a_path} vs {b_path}: {n} px >{thresh} LSB ({100.0*n/total:.2f}%), mean abs {float(d.mean()):.3f}, max {int(d.max())}")
    print(f"  bbox x {bbox[0]}..{bbox[2]}, y {bbox[1]}..{bbox[3]}" if bbox else "  no diff")
    return changed, bbox

if __name__ == "__main__":
    diff(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 3)
