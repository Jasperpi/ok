"""Adds a column and row ruler round a frame. usage: ruler.py in out cellw cellh left top [guidecols comma] [guiderows comma]"""
import sys
from PIL import Image, ImageDraw, ImageFont
src, dst, cw, ch, left, top = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]), int(sys.argv[6])
gcols = [int(x) for x in sys.argv[7].split(',')] if len(sys.argv) > 7 and sys.argv[7] else []
grows = [int(x) for x in sys.argv[8].split(',')] if len(sys.argv) > 8 and sys.argv[8] else []
im = Image.open(src).convert('RGB')
W, H = im.size
bx, by = 34, 30
out = Image.new('RGB', (W + bx, H + by), (255, 255, 255))
out.paste(im, (bx, by))
d = ImageDraw.Draw(out)
f = ImageFont.load_default()
cols = (W - left) // cw
rows = (H - top) // ch
for c in range(cols + 1):
    x = bx + left + c * cw
    h = 10 if c % 10 == 0 else (6 if c % 5 == 0 else 3)
    d.line([(x, by - h), (x, by - 1)], fill=(0, 0, 0))
    if c % 10 == 0 and c < cols:
        d.text((x + 2, 2), str(c), fill=(0, 0, 0), font=f)
for r in range(rows + 1):
    y = by + top + r * ch
    d.line([(bx - 6, y), (bx - 1, y)], fill=(0, 0, 0))
    if r < rows:
        d.text((2, y + 5), str(r), fill=(0, 0, 0), font=f)
for c in gcols:
    x = bx + left + c * cw
    for y in range(by, by + H, 4):
        out.putpixel((x, y), (255, 0, 0)); out.putpixel((x, y + 1 if y + 1 < by + H else y), (255, 0, 0))
for r in grows:
    y = by + top + r * ch
    for x in range(bx, bx + W, 4):
        out.putpixel((x, y), (0, 120, 255)); out.putpixel((x + 1 if x + 1 < bx + W else x, y), (0, 120, 255))
out.save(dst)
