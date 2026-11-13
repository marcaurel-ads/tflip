# Prints what a scenario's result.json answered that is not a plain success
# (a failed statement, a result set), cut at width characters; with -t the
# files of its tree with their sizes, with -v every command, with -e the
# rows of the server's error log it caused:
#     show.py <scenario> [width] [-t] [-v] [-e]
import json, os, sys
d = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../../fixtures/backup', sys.argv[1])
w = int(next((a for a in sys.argv[2:] if a.isdigit()), 400))
v = '-v' in sys.argv
for r in json.load(open(d + '/result.json')):
    r.pop('ms', None)
    if v or r.get('rc', 0) != 0 or 'fields' in r or 'value' in r:
        if 'fields' in r:
            r['fields'] = len(r['fields'])
        print(str(r)[:w])
if '-t' in sys.argv:
    for p, _, fs in sorted(os.walk(d + '/tree')):
        for f in sorted(fs):
            q = os.path.join(p, f)
            print(os.path.getsize(q), os.path.relpath(q, d + '/tree'))
if '-e' in sys.argv:
    print(open(d + '/errlog.txt').read())
