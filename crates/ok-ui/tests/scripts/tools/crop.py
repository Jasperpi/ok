"""usage: crop.py in x y w h zoom out"""
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert('RGB')
x, y, w, h, z = map(int, sys.argv[2:7])
c = im.crop((x, y, x + w, y + h)).resize((w * z, h * z), Image.NEAREST)
c.save(sys.argv[7])
