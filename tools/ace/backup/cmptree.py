# Compares the files of two folders by name: same, differing (with the count
# of differing bytes and the sizes), or only on one side:
#     cmptree.py <a> <b>
import os, sys
a, b = sys.argv[1:3]
names = sorted(set(os.listdir(a)) | set(os.listdir(b)))
for n in names:
    pa, pb = os.path.join(a, n), os.path.join(b, n)
    if not os.path.exists(pa):
        print(f'{n}: only in b ({os.path.getsize(pb)})')
    elif not os.path.exists(pb):
        print(f'{n}: only in a ({os.path.getsize(pa)})')
    elif os.path.isdir(pa):
        print(f'{n}: folder')
    else:
        x, y = open(pa, 'rb').read(), open(pb, 'rb').read()
        if x == y:
            print(f'{n}: same ({len(x)})')
        else:
            d = sum(1 for i in range(min(len(x), len(y))) if x[i] != y[i])
            print(f'{n}: differs, {d} bytes, sizes {len(x)} {len(y)}')
