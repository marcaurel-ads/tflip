# Hex dump of a range of files, 16 bytes a line, one block a file:
#     hexs.py <offset> <length> <file>...
import sys
off, n = int(sys.argv[1], 0), int(sys.argv[2], 0)
for f in sys.argv[3:]:
    d = open(f, 'rb').read()
    print(f'-- {f} ({len(d)} bytes) at {off:#x}')
    for i in range(off, min(off + n, len(d)), 16):
        c = d[i:i + 16]
        print(f'{i:06x}: {c.hex(" ")}  {"".join(chr(b) if 32 <= b < 127 else "." for b in c)}')
