"""Measures, from the rendered frames, the colours actually used in each region and their WCAG contrast against the ground they sit on."""
import sys
from PIL import Image
from collections import Counter
def lin(c):
    c = c / 255
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
def lum(rgb):
    return 0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
def ratio(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
def hx(c): return '%02X%02X%02X' % c
def cells(col, row, wide, tall=1): return (col * 12, 1 + row * 22, (col + wide) * 12, 1 + (row + tall) * 22)
regions = [
    ('body text, sheet (row 4, cols 47..108)', cells(47, 4, 62)),
    ('hung # mark, sheet (col 44 row 2)', cells(44, 2, 1)),
    ('* marks round waste, sheet (col 81 row 12)', cells(81, 12, 1)),
    ('[[ brackets, sheet (cols 47..48 row 23)', cells(47, 23, 2)),
    ('say row, sheet (row 46)', cells(47, 46, 57)),
    ('facts, sheet (row 47 cols 89..112)', cells(89, 47, 24)),
    ('ok, sheet (row 47 cols 47..48)', cells(47, 47, 2)),
    ('contents other text, desk (row 2 cols 4..13)', cells(4, 2, 10)),
    ('contents folder, desk (row 1 cols 2..8)', cells(2, 1, 7)),
    ('contents open text, desk (row 6 cols 4..14)', cells(4, 6, 11)),
    ('leader dots, desk (row 2 cols 15..32)', cells(15, 2, 18)),
    ('count of other text, desk (row 2 cols 34..36)', cells(34, 2, 3)),
    ('label current section, desk (row 1 col 134..)', cells(134, 1, 11)),
    ('See also caption, desk (row 42)', cells(134, 42, 8)),
    ('link name, desk (row 43)', cells(136, 43, 21)),
]
for path in sys.argv[1:]:
    im = Image.open(path).convert('RGB')
    print('==', path.split('critic-look-')[-1])
    for name, box in regions:
        c = Counter(im.crop(box).getdata()).most_common()
        ground = c[0][0]
        rest = [(col, n) for col, n in c[1:]]
        if not rest:
            print('  %-48s ground %s  NOTHING DRAWN' % (name, hx(ground)))
            continue
        out = ', '.join('%s %.2f:1 (%dpx)' % (hx(col), ratio(col, ground), n) for col, n in rest[:3])
        print('  %-48s ground %s  %s' % (name, hx(ground), out))
