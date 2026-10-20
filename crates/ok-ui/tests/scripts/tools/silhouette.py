"""Gray-boxes a manila frame: every run of inked cells becomes one flat block, words and letters gone."""
import sys
import numpy as np
from PIL import Image, ImageDraw
src, dst = sys.argv[1], sys.argv[2]
a = np.array(Image.open(src).convert('RGB')).astype(int)
H, W, _ = a.shape
PAPER = np.array([0xEE, 0xE3, 0xC8]); DESK = np.array([0x35, 0x25, 0x1A])
out = Image.new('RGB', (W, H), (70, 70, 70))
d = ImageDraw.Draw(out)
d.rectangle([492, 0, 492 + 936 - 1, H], fill=(225, 225, 225))
cols, rows = W // 12, (H - 1) // 22
for r in range(rows):
    run = None
    for c in range(cols + 1):
        inked = False
        if c < cols:
            cell = a[1 + r * 22:1 + (r + 1) * 22, c * 12:(c + 1) * 12]
            sheet = 41 <= c <= 118
            ground = PAPER if sheet else DESK
            # the strip of the map counts as one block of its own
            diff = np.any(cell != ground, axis=2)
            inked = diff.sum() > 0
        if inked and run is None:
            run = c
        gap_next = False
        if not inked and run is not None:
            # bridge a gap of one empty cell (a space between words)
            if c + 1 < cols:
                nxt = a[1 + r * 22:1 + (r + 1) * 22, (c + 1) * 12:(c + 2) * 12]
                g2 = PAPER if 41 <= c + 1 <= 118 else DESK
                gap_next = np.any(nxt != g2) and (41 <= c <= 118) == (41 <= c + 1 <= 118)
            if not gap_next:
                sheet = 41 <= run <= 118
                fill = (120, 120, 120) if sheet else (150, 150, 150)
                d.rectangle([run * 12, 1 + r * 22 + 5, c * 12 - 1, 1 + (r + 1) * 22 - 5], fill=fill)
                run = None
out.save(dst)
