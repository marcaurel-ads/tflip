# Prints, for ADT tables, the status byte of every record and the block and
# length its memo field points at, and the header of the memo file:
#     memoref.py <table.adt> [<field offset in the record, default 19>]...
# (the default is the MEMO of "id INTEGER, name CHAR(10), note MEMO, bin BLOB").
import struct, sys
args = sys.argv[1:]
off_in_rec = 19
for f in args:
    if f.isdigit():
        off_in_rec = int(f)
        continue
    d = open(f, 'rb').read()
    u32 = lambda o: struct.unpack_from('<I', d, o)[0]
    slots, off, rl = u32(0x18), u32(0x20), u32(0x24)
    refs = []
    for i in range(slots):
        r = off + i * rl
        refs.append(f'{i + 1}:{d[r]:02x}:{u32(r + off_in_rec)}/{u32(r + off_in_rec + 4)}')
    print(f, 'stamp', hex(u32(0x174)))
    print('  ', ' '.join(refs))
    m = open(f[:-4] + '.adm', 'rb').read()
    print('   adm size', len(m), 'header', m[:0x20].hex(' '))
