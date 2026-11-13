# Lists the byte ranges in which two files differ (offset: old -> new, hex),
# for characterising what a backup or restore changed:
#     diffrange.py <a> <b> [max ranges]
import sys
a = open(sys.argv[1], 'rb').read()
b = open(sys.argv[2], 'rb').read()
limit = int(sys.argv[3]) if len(sys.argv) > 3 else 40
print(f'sizes {len(a)} {len(b)}')
n = min(len(a), len(b))
i = shown = 0
while i < n and shown < limit:
    if a[i] != b[i]:
        j = i
        gap = 0
        while j < n and gap < 8:
            if a[j] != b[j]:
                gap = 0
            else:
                gap += 1
            j += 1
        j -= gap
        print(f'{i:#x}-{j:#x} ({j - i}): {a[i:j][:24].hex()} -> {b[i:j][:24].hex()}')
        i = j
        shown += 1
    else:
        i += 1
if i < n:
    print('more differences')
