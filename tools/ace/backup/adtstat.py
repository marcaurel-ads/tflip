# Prints the header fields a backup touches and the status byte of every
# record of ADT tables:
#     adtstat.py <table.adt>...
import struct, sys
for f in sys.argv[1:]:
    d = open(f, 'rb').read()
    u32 = lambda o: struct.unpack_from('<I', d, o)[0]
    slots, off, rl = u32(0x18), u32(0x20), u32(0x24)
    st = ' '.join(f'{d[off + i * rl]:02x}' for i in range(slots))
    print(f'{f}: 0x14={u32(0x14)} slots={slots} dataoff={off:#x} reclen={rl} free_head={u32(0x28)} free_count={u32(0x30)} '
          f'enc={d[0x4c]} flags0x58={u32(0x58):#x} 0x160={u32(0x160)} 0x174={u32(0x174):#x} size={len(d)}')
    print('   status:', st)
