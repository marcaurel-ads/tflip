#!/bin/sh
# Generates a large (password, record) -> (i0,j0) sample with recover_ij.exe and
# runs the generator search (ij_search.py). For the table cipher's per-record
# (i0,j0) generator investigation; see FINDINGS.md.
#
#     tools/ace/crypto/search.sh <work folder> <ace32.dll as a Windows path>
#
# Example:
#     WINEPREFIX=<wine prefix> \
#       tools/ace/crypto/search.sh /tmp/s 'Z:\path\to\ace32.dll'
#
# recover_ij.exe creates an encrypted ADT per password, appends all-zero records
# (so each record's ciphertext is its keystream), reopens without the password,
# reads each keystream and brute-recovers (i0,j0) against S0 = RC4_KSA(pw[:20]).
# Passwords are DIVERSE (random length 1..30 incl. the 20-byte truncation
# boundary, full byte range 0x01..0xFF incl. non-ASCII) and passed as hex, so S0
# draws are independent and those dimensions are covered; recover_ij.c and
# ij_search.py agree on the encoding (raw bytes) and truncation (20).
# PASSWORDS and RECORDS below set the sample; records >256 expose the index wrap.
set -e
if [ $# -ne 2 ] || [ -z "$1" ]; then
    echo "usage: search.sh <work folder> <ace32.dll>" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
work=$1
dll=$2
# RECORDS must exceed 256 for the search's held-out-past-256 validation to fire.
PASSWORDS=${PASSWORDS:-300}
RECORDS=${RECORDS:-300}
BATCH=${BATCH:-1000}
mkdir -p "$work"
rm -rf "$work/data"
mkdir -p "$work/data"
data="Z:$(echo "$work/data" | tr / '\\')"
i686-w64-mingw32-gcc -O2 -o "$work/recover_ij.exe" "$here/recover_ij.c"
cd "$work"

# Generate diverse passwords as hex, in batches (keeps the command line within
# limits). Each password: random length 1..30, bytes 0x01..0xFF (no NUL, since the
# API password is a C string), so lengths straddle the 20-byte truncation and the
# byte range includes non-ASCII. Deterministic seed for reproducibility.
python3 - "$PASSWORDS" "$BATCH" > plan.txt <<'PY'
import sys, random
n=int(sys.argv[1]); b=int(sys.argv[2]); rng=random.Random(20240601)
pws=[]
for _ in range(n):
    L=rng.randint(1,30)
    pws.append(bytes(rng.randint(1,255) for _ in range(L)).hex())
for s in range(0,n,b):
    print(' '.join(pws[s:s+b]))
PY
: > parts.jsonl
while IFS= read -r pws; do
    # shellcheck disable=SC2086
    WINEDEBUG=-all wine recover_ij.exe "$dll" "$data" "$RECORDS" $pws | tr -d '\r' >> parts.jsonl
done < plan.txt
python3 - > big.json <<'PY'
import json
rows=[]
for line in open('parts.jsonl'):
    line=line.strip()
    if line: rows+=json.loads(line)
json.dump(rows,open('big.json','w'))
print('passwords',len(rows),'records',len(rows[0]['ij']))
PY

python3 "$here/ij_search.py" localize big.json
python3 "$here/ij_search.py" selftest big.json    # simple positive control
python3 "$here/ij_search.py" selftest2 big.json   # hard control (new code paths)
python3 "$here/ij_search.py" search big.json
