# Describes a tar archive (optionally gzipped): every member header field,
# the padding after the last member, and for a gzip file its header and
# whether zlib at levels 1-9 gives the same deflate stream:
#     tarinfo.py <archive>
import gzip, struct, sys, zlib
raw = open(sys.argv[1], 'rb').read()
data = raw
if raw[:2] == b'\x1f\x8b':
    flg = raw[3]
    print('gzip header:', raw[:10].hex(' '), f'(method {raw[2]}, flags {flg:#x}, mtime {struct.unpack("<I", raw[4:8])[0]}, xfl {raw[8]}, os {raw[9]})')
    print('gzip trailer:', raw[-8:].hex(' '), 'size', len(raw))
    data = gzip.decompress(raw)
    body = raw[10:]
    if flg & 8:
        body = body[body.index(b'\0') + 1:]
    for lvl in range(1, 10):
        for wbits, mem in ((-15, 8), (-15, 9)):
            c = zlib.compressobj(lvl, zlib.DEFLATED, wbits, mem)
            z = c.compress(data) + c.flush()
            if z == body[:-8]:
                print(f'deflate stream equals zlib level {lvl} wbits {wbits} memlevel {mem}')
print('tar size', len(data))
pos = 0
while pos + 512 <= len(data):
    h = data[pos:pos + 512]
    if h == b'\0' * 512:
        break
    f = lambda a, b: h[a:b].split(b'\0')[0]
    size = int(f(124, 136) or b'0', 8)
    chk = int(f(148, 156).strip() or b'0', 8)
    calc = sum(h[:148]) + 32 * 8 + sum(h[156:])
    print(f'@{pos}: name={f(0, 100)!r} mode={f(100, 108)!r} uid={f(108, 116)!r} gid={f(116, 124)!r} size={size} '
          f'mtime={f(136, 148)!r} chk={chk}{"" if chk == calc else " (calc " + str(calc) + ")"} type={h[156:157]!r} link={f(157, 257)!r} '
          f'magic={h[257:265]!r} uname={f(265, 297)!r} gname={f(297, 329)!r} devmaj={f(329, 337)!r} devmin={f(337, 345)!r} prefix={f(345, 500)!r} tail={h[500:512].hex()}')
    pos += 512 + (size + 511) // 512 * 512
rest = data[pos:]
print(f'after the last member: {len(rest)} bytes, all zero: {rest == b"\0" * len(rest)}')
