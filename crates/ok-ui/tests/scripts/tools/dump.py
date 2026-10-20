"""Prints a screen dump with a column ruler and row numbers, and for each row the first and last used column of each run of text separated by 2+ spaces."""
import sys, re
lines = open(sys.argv[1], encoding='utf-8').read().split('\n')
width = int(sys.argv[2]) if len(sys.argv) > 2 else 160
print('   ' + ''.join(str(c // 100) if c % 10 == 0 else ' ' for c in range(width)))
print('   ' + ''.join(str(c // 10 % 10) if c % 10 == 0 else ' ' for c in range(width)))
print('   ' + ''.join(str(c % 10) for c in range(width)))
for r, l in enumerate(lines):
    if r >= 60: break
    print('%2d|%s' % (r, l))
if len(sys.argv) > 3:
    print()
    for r, l in enumerate(lines):
        runs = [(m.start(), m.end() - 1, m.group()) for m in re.finditer(r'\S+(?: \S+)*', l)]
        if runs:
            print('%2d: ' % r + '  '.join('[%d..%d]' % (a, b) for a, b, _ in runs))
