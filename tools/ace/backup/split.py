# Splits a file of scenarios, each started by a line "=== <name>", into
# scripts/<name>.txt:
#     split.py <file>
import os, sys
d = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'scripts')
name = None
out = []
def flush():
    if name:
        open(os.path.join(d, name + '.txt'), 'w').write(''.join(out))
for line in open(sys.argv[1]):
    if line.startswith('=== '):
        flush()
        name, out = line[4:].strip(), []
    else:
        out.append(line)
flush()
