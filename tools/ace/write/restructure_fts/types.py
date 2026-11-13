import os, random, sys
# usage: gen2.py <out folder> <variant: adt|cdx|vfp>
out = sys.argv[1]
variant = sys.argv[2]
os.makedirs(out, exist_ok=True)
rng = random.Random(7)
base = ["alpha","beta","gamma","delta","epsilon","zeta","eta","theta","The","Quick","Brown","Fox","jumps","the","and","for","O'Malley","e-mail","3.14","Ärger"]
def text(n):
    return " ".join(rng.choice(base) for _ in range(n))
rows = [(1, "alpha beta gamma", "The Quick Brown Fox", "quick memo words"),
        (2, "beta delta epsilon", "jumps over lazy dog", None),
        (3, "gamma gamma zeta", "the and for", "noise gamma"),
        (4, "eta theta", "Fox and Hound", "hound fox"),
        (5, "", "", ""),
        (6, "O'Malley e-mail 3.14 Ärger", "x", "y z")]
def q(v):
    return "NULL" if v is None else "'" + v.replace("'", "''") + "'"
def insert(n, rs):
    return "sql INSERT INTO %s VALUES %s" % (n, ", ".join("(%d, %s, %s, %s)" % (a, q(b), q(c), q(d)) for a, b, c, d in rs))
big = [(i, text(rng.randint(1, 4)), text(3), text(rng.randint(0, 12))) for i in range(1, 401)]
s = ["# Tables with full-text orders to restructure."]
if variant != "adt":
    s.insert(0, "tabletype " + variant)
def table(n, rs=rows, m="MEMO", body="CHAR(40)"):
    s.append("sql CREATE TABLE %s (id INTEGER, body %s, title CHAR(30), m %s)" % (n, body, m))
    s.append("sql DELETE FROM %s WHERE 1 = 0" % n)
    for i in range(0, len(rs), 50):
        s.append(insert(n, rs[i:i + 50]))
fts = lambda n, f, extra="": s.append("sql CREATE INDEX %s_%s ON %s (%s) CONTENT %s" % (n, f, n, f, extra))
o = ["# Types of a full-text field changed, fields moved, options kept."]
tabs = {}
for n, f, extra in [("a", "body", ""), ("b", "m", ""), ("c", "body", "MIN WORD 1 MAX WORD 6 CASESENSITIVE KEEPSCORE"), ("d", "body", "MIN WORD 2 MAX WORD 12 NEW NOISE 'b a c B'"),
                    ("e", "body", ""), ("f", "m", ""), ("g", "body", ""), ("h", "body", ""), ("i", "body", ""), ("j", "body", ""), ("k", "body", "")]:
    table(n)
    fts(n, f, extra)
# big, empty
table("p", big)
fts("p", "m")
fts("p", "body")
s.append("sql CREATE INDEX p_id ON p (id)")
table("r", [(1, "12", "a", "b"), (2, "7", "c", None)])
fts("r", "body")
table("q", [])
fts("q", "body")
# ops: each table restructured differently
ext = "adt" if variant == "adt" else "dbf"
C = "Char" if variant == "adt" else "Character"
N = "Integer,4" if variant in ("adt", "vfp") else "Numeric,8"
D = "Double,8" if variant in ("adt", "vfp") else "Date,8"
G = "CICharacter,40" if variant == "adt" else "Character,60"
o = ["# Restructures of tables with full-text orders."]
if variant != "adt":
    o.append("tabletype " + variant)
o += [
    "restructure a.%s 0 front,%s,4,(1)||" % (ext, C),
    "restructure b.%s 0 ||m,m,%s,40" % (ext, C),
    "restructure c.%s 0 ||body,words,%s,50" % (ext, C),
    "restructure d.%s 0 |title|body,body,Memo,10" % ext,
    "restructure e.%s 0 ||body,body,%s" % (ext, N),
    "restructure f.%s 0 ||m,m,%s" % (ext, N),
    "restructure g.%s 0 ||body,body,%s" % (ext, G),
    "restructure h.%s 0 ||body,body,%s" % (ext, D),
    "restructure i.%s 0 ||body,body,%s,10" % (ext, C),
    "restructure j.%s 0 |m|" % ext,
    "restructure k.%s 0 |id|body,text,%s,40" % (ext, C),
    "restructure p.%s 0 extra,Memo,10|title|m,m2,Memo,10" % ext,
    "restructure r.%s 0 ||body,body,%s" % (ext, N),
    "restructure q.%s 0 extra,%s,4||" % (ext, C),
]
for n in "abcdefghijkpqr":
    o += ["open %s.%s" % (n, ext), "top", "record", "close"]
open(os.path.join(out, "setup.txt"), "w").write("\n".join(s) + "\n")
open(os.path.join(out, "ops.txt"), "w").write("\n".join(o) + "\n")
