"""Stacks crops of frames into one picture. usage: sheet.py out zoom name:x,y,w,h ..."""
import sys
from PIL import Image
SH = r"C:/Users/jaspe/Projects/ok/shots/"
out, z = sys.argv[1], int(sys.argv[2])
ims = []
for part in sys.argv[3:]:
    name, box = part.rsplit(':', 1)
    x, y, w, h = map(int, box.split(','))
    im = Image.open(SH + name).convert('RGB').crop((x, y, x + w, y + h))
    ims.append(im.resize((w * z, h * z), Image.NEAREST))
W = max(i.width for i in ims); H = sum(i.height + 6 for i in ims)
sheet = Image.new('RGB', (W, H), (255, 0, 255))
y = 0
for i in ims:
    sheet.paste(i, (0, y)); y += i.height + 6
sheet.save(out)
