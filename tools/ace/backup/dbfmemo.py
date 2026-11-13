# For a DBF table of the mt_ layout of the backup probes (fields id, name, qty,
# d, flag, note MEMO) and its memo file: the header facts a backup changes,
# and per record the delete flag, the memo block number and the memo length.
#     dbfmemo.py <table.dbf> <memo file .fpt|.dbt>
import struct, sys
t = open(sys.argv[1], 'rb').read()
m = open(sys.argv[2], 'rb').read()
nrec, hlen, rlen = struct.unpack_from('<IHH', t, 4)
vfp = t[0] in (0x30, 0x31, 0x32)
print(f'{sys.argv[1]}: version {t[0]:#x} records {nrec} header {hlen} reclen {rlen} flag@0x1c={t[0x1c]:#x} size {len(t)} tail {t[-1]:#x}')
fpt = sys.argv[2].endswith('.fpt')
if fpt:
    nxt, bs = struct.unpack_from('>IxxH', m, 0)[0], struct.unpack_from('>H', m, 6)[0]
    print(f'  fpt next block {nxt} block size {bs} file size {len(m)}')
else:
    nxt, bs = struct.unpack_from('<I', m, 0)[0], 512
    print(f'  dbt next block {nxt} block size {bs} file size {len(m)} bytes 4..: {m[4:20].hex()}')
for i in range(nrec):
    r = t[hlen + i * rlen:hlen + (i + 1) * rlen]
    ptr = struct.unpack_from('<I', r, rlen - 4)[0] if vfp else int(r[-10:].strip() or b'0')
    ln = ''
    if ptr and fpt:
        ln = struct.unpack_from('>I', m, ptr * bs + 4)[0]
    elif ptr:
        blk = m[ptr * bs:]
        ln = blk.find(b'\x1a\x1a') if b'\x1a\x1a' in blk else '?'
    print(f'  rec {i + 1}: {"deleted" if r[0] == 0x2a else "active "} memo block {ptr} length {ln}')
