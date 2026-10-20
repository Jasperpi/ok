"""Sets crops of frames side by side. usage: side.py out zoom name:x,y,w,h ..."""
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
H = max(i.height for i in ims); W = sum(i.width + 6 for i in ims)
sheet = Image.new('RGB', (W, H), (255, 0, 255))
x = 0
for i in ims:
    sheet.paste(i, (x, 0)); x += i.width + 6
sheet.save(out)
