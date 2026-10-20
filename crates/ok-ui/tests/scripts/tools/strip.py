"""Measures the map's strip of paper, its marks, the notch and the flags in a manila frame."""
import sys
import numpy as np
from PIL import Image
PAPER = (0xEE, 0xE3, 0xC8); DESK = (0x35, 0x25, 0x1A); DESKINK = (0xE6, 0xD9, 0xBA)
for path in sys.argv[1:]:
    a = np.array(Image.open(path).convert('RGB'))
    right = a[:, 1428:1606]
    paper = np.all(right == PAPER, axis=2)
    ys, xs = np.where(paper)
    x0, x1, y0, y1 = xs.min() + 1428, xs.max() + 1428, ys.min(), ys.max()
    inner = a[y0:y1 + 1, x0:x1 + 1]
    mark = ~np.all(inner == PAPER, axis=2)
    mys, mxs = np.where(mark)
    print(path.split('/')[-1])
    print('  strip x %d..%d (w %d)  y %d..%d (h %d)' % (x0, x1, x1 - x0 + 1, y0, y1, y1 - y0 + 1))
    print('  marks x %d..%d  y %d..%d  -> margins left %d right %d top %d bottom %d' % (mxs.min() + x0, mxs.max() + x0, mys.min() + y0, mys.max() + y0, mxs.min(), x1 - x0 - mxs.max(), mys.min(), y1 - y0 - mys.max()))
    # desk-ink pixels outside the strip between x 1428 and 1606: notch and flags
    di = np.all(a[:, 1428:1606] == DESKINK, axis=2)
    ys, xs = np.where(di)
    seen = set()
    for y, x in zip(ys, xs):
        seen.add((int(y), 'left' if x + 1428 < x0 else 'right'))
    rows = {}
    for y, side in sorted(seen):
        xsr = [int(x) + 1428 for yy, x in zip(ys, xs) if yy == y and ((x + 1428 < x0) == (side == 'left'))]
        rows.setdefault(side, []).append((y, min(xsr), max(xsr)))
    for side, lst in rows.items():
        print('  %s of strip:' % side, lst[:12])
