# AdsFindServers detail columns — the connect-for-info protocol

tflip's `AdsFindServers` fills the address, port, multicast-address and
server-name columns from the multicast reply (see the README). The other ten
columns — Serial Number, Version, Owner, Install Date, User Count, Server Type,
Up Time, Operations — are **not** in the multicast reply. The original engine
gathers them, as its docs say, "through a normal server connection": a full ADS
client connection to each discovered server. tflip leaves them blank (it has no
real-ADS-protocol client), matching the original's `ADS_FS_MULTICAST_ONLY`.

This note records that connection protocol, reversed black-box from a capture,
in case the detail columns are ever wanted. It was **not** implemented — doing
so means tflip speaking the real ADS wire protocol, which it otherwise never
does (it talks its own protocol to `tflipd`).

## How it was captured

The real 12.0 evaluation server was run under Wine
(setup in `LOCAL.md`), the original `ace32.dll`'s
`AdsFindServers` (default option) was run under Wine on the same host, and the
traffic was captured with `dumpcap` (which has `cap_net_raw`, so no root):

```sh
dumpcap -i any -q -f "udp port 2989 or port 6262" -w findservers.pcap
```

## The exchange (one server, UDP port 6262)

The detail fetch is the ADS **connectionless (UDP) protocol** on port 6262 — not
TCP. For one discovered address:

| dir | payload | contents |
| --- | ------- | -------- |
| C→S | 797 B | login/connect request: a mostly-fixed template carrying a connection id and a nonce, then the server path (`\\192.168.0.10:6262\`), the date format (`MM/DD/CCYY`), the client app name (`findservers.exe`) and the OS user |
| S→C | 39 B  | connect response / ack |
| both | 34 B | reliable-UDP control frames (sequence/ack headers) exchanged around the data |
| S→C | 178 B | **text info**: Owner, Version, Install Date, Serial Number, language (`USA`/`ENU`), evaluation expiry — all as readable, space-padded strings |
| S→C | 206 B | **numeric info**: User Count, Server Type, Operations, Up Time |

The 797-byte login begins `00 14 01 00 …`, with a 4-byte connection id
(`6c 07 02 00`) and a 4-byte nonce (`51 9f eb 7b`) near offset 0x10, then the
NUL-terminated strings from offset 0x22 on. The 34-byte frames are the
protocol's control/request headers (sequence numbers + an opcode), the same
transport a normal remote `AdsConnect` uses.

## What implementing it would take

A partial real-ADS UDP client: the 797-byte login layout, the reliable-UDP
transport (sequencing and acks — the 34-byte frames), and parsing the two info
blocks. That is a sizable subsystem for ten optional columns, and it would make
tflip speak the real ADS protocol, so it was deliberately left out. See
`docs/KNOWN_ISSUES.md` (server discovery) and `docs/DIFFERENCES.md`.
