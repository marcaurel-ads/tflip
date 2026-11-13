import os, random, sys
# usage: gen8.py <out folder> <variant: adt|cdx|vfp>
out = sys.argv[1]
variant = sys.argv[2]
os.makedirs(out, exist_ok=True)
rng = random.Random(11)
base = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "The", "Quick", "Brown", "Fox", "jumps", "the", "and", "for", "O'Malley", "3.14"]


def text(n):
    return " ".join(rng.choice(base) for _ in range(n))


def q(v):
    return "NULL" if v is None else "'" + v.replace("'", "''") + "'"


rows = [(1, "alpha beta gamma", "The Quick Brown Fox", "quick memo words"),
        (2, "beta delta epsilon", "jumps over lazy dog", None),
        (3, "gamma gamma zeta", "the and for", "noise gamma"),
        (4, "eta theta", "Fox and Hound", "hound fox")]
big = [(i, text(rng.randint(1, 4)), text(3), text(rng.randint(0, 12))) for i in range(1, 301)]
s = ["ddcreate dd.add"]
if variant != "adt":
    s.insert(0, "tabletype " + variant)
    s[0], s[1] = s[1], s[0]
    s = ["tabletype " + variant, "ddcreate dd.add"]


def table(n, rs=rows):
    s.append("sql CREATE TABLE %s (id INTEGER, body CHAR(40), title CHAR(30), m MEMO)" % n)
    for i in range(0, len(rs), 50):
        s.append("sql INSERT INTO %s VALUES %s" % (n, ", ".join("(%d, %s, %s, %s)" % (a, q(b), q(c), q(d)) for a, b, c, d in rs[i:i + 50])))


def fts(n, f, extra=""):
    s.append("sql CREATE INDEX %s_%s ON %s (%s) CONTENT %s" % (n, f, n, f, extra))


for n in "abcdefg":
    table(n)
fts("a", "body")
fts("b", "body", "KEEPSCORE")
fts("c", "body")
fts("d", "body")
fts("e", "m")
fts("f", "body", "IN FILE words")
s.append("sql CREATE INDEX f_id ON f (id)")
fts("g", "body")
table("p", big)
fts("p", "m")
fts("p", "body")
s.append("sql CREATE INDEX p_id ON p (id)")
s.append("sql CREATE TABLE zz (x INTEGER)")
C = "Char" if variant == "adt" else "Character"
N = "Integer,4" if variant in ("adt", "vfp") else "Date,8"
o = ["ddconnect dd.add ADSSYS -"]
o.append("tabletype database")
ext = "adt" if variant == "adt" else "dbf"
o += [
    "restructure :a 0 extra,%s,4||" % C,
    "sql ALTER TABLE b ADD COLUMN extra CHAR(4)",
    "sql ALTER TABLE c ALTER COLUMN body text CHAR(40)",
    "sql ALTER TABLE d DROP COLUMN body",
    "sql ALTER TABLE e ALTER COLUMN m m MEMO",
    "restructure :f 0 extra,%s,4||" % C,
    "restructure :g 0 ||body,body,%s" % N,
    "sql ALTER TABLE p DROP COLUMN title",
    "restructure :p 0 ||m,m2,Memo,10",
]
for n in "abcdefp":
    o += ["open :%s" % n, "top", "record", "close"]
# g lost its index file to the failed restructure: it cannot be opened.
o += ["open :g"]
open(os.path.join(out, "setup.txt"), "w").write("\n".join(s) + "\n")
open(os.path.join(out, "ops.txt"), "w").write("\n".join(o) + "\n")
