"""usage: px.py in x y w h  -> the colours in a region and how many pixels of each"""
import sys
from PIL import Image
from collections import Counter
im = Image.open(sys.argv[1]).convert('RGB')
x, y, w, h = map(int, sys.argv[2:6])
c = Counter(im.crop((x, y, x + w, y + h)).getdata())
for col, n in c.most_common(12):
    print('%02X%02X%02X' % col, n)
