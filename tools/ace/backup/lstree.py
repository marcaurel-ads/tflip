# One line per folder of a scenario's tree: its files with sizes (folders of
# one level below the scenario folder only; "-" for an empty folder):
#     lstree.py <scenario> [folder prefix]
import os, sys
d = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../../fixtures/backup', sys.argv[1], 'tree')
pre = sys.argv[2] if len(sys.argv) > 2 else ''
for p, dirs, fs in sorted(os.walk(d)):
    rel = os.path.relpath(p, d)
    if rel == '.' or not rel.startswith(pre):
        continue
    print(rel + ':', ' '.join(f'{f}={os.path.getsize(os.path.join(p, f))}' for f in sorted(fs)) or '-')
