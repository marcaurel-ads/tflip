import os, sys
out = sys.argv[1]
variant = sys.argv[2]
os.makedirs(out, exist_ok=True)
ext = "adt" if variant == "adt" else "dbf"
C = "Char" if variant == "adt" else "Character"
rows = "(1, 'alpha beta gamma', 'The Quick Brown Fox', 'quick memo words'), (2, 'beta delta epsilon', 'jumps over lazy dog', NULL), (3, 'gamma gamma zeta', 'the and for', 'noise gamma')"
s = []
if variant != "adt":
    s.append("tabletype " + variant)
names = "abcde"
for n in names:
    s.append("sql CREATE TABLE %s (id INTEGER, body CHAR(40), title CHAR(30), m MEMO)" % n)
    s.append("sql INSERT INTO %s VALUES %s" % (n, rows))
s.append("sql CREATE TABLE zz (x INTEGER)")
for n in "abcde":
    s.append("open %s.%s" % (n, ext))
    s.append("createfts %s_w.%s w_%s body 0 3 30 0 1 - 1 - 1 - 1 -" % (n, "adi" if variant == "adt" else "idx", n) if variant == "adt" else "createfts - w_%s body 0 3 30 0 1 - 1 - 1 - 1 -" % n)
    s.append("close")
s.append("sql CREATE INDEX b_id ON b (id)")
s.append("sql CREATE INDEX c_f ON c (m) CONTENT")
o = []
if variant != "adt":
    o.append("tabletype " + variant)
o += [
    "restructure a.%s 0 front,%s,4,(1)||" % (ext, C),
    "restructure b.%s 0 extra,%s,4||" % (ext, C),
    "restructure c.%s 0 extra,%s,4|title|" % (ext, C),
    "restructure d.%s 0 |body|" % ext,
    "restructure e.%s 0 ||body,text,%s,40" % (ext, C),
]
for n in names:
    o += ["open %s.%s" % (n, ext), "top", "record", "close"]
open(os.path.join(out, "setup.txt"), "w").write("\n".join(s) + "\n")
open(os.path.join(out, "ops.txt"), "w").write("\n".join(o) + "\n")
