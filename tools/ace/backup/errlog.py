# Prints rows of an adsdump JSON of the server's error log (ads_err.adt) as
# "code source:line file | more info", without their times, for what a backup
# or restore logged beside its result set:
#     errlog.py <dump.json>            the number of the last row
#     errlog.py <dump.json> <recno>    the rows after row <recno>
import json, sys
t = open(sys.argv[1], encoding='utf-8', errors='replace').read()
d = json.loads(t[t.index('{'):])
rows = [r for r in d['records'] if not r['deleted']]
if len(sys.argv) < 3:
    print(rows[-1]['recno'] if rows else 0)
    sys.exit()
for r in rows:
    if r['recno'] <= int(sys.argv[2]):
        continue
    v = r['values']
    g = lambda k: (v[k].get('text') or '').strip() if k in v else ''
    print(g('Error_Code'), g('ADS_Source') + ':' + g('Src_Line'), g('FileName'), '|', g('More_Info'))
