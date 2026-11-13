import os, random, sys
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
rng = random.Random(9)
base = ["alpha","beta","gamma","delta","epsilon","zeta","eta","theta","The","Quick","Brown","Fox","jumps","the","and","for","O'Malley","e-mail","3.14","Ärger"]
def text(n):
    return " ".join(rng.choice(base) for _ in range(n))
def q(v):
    return "'" + v.replace("'", "''") + "'"
big = [(i, text(rng.randint(1, 4)), text(3), text(rng.randint(0, 12))) for i in range(1, 401)]
s = []
s.append("sql CREATE TABLE pp (id INTEGER, body CHAR(40), title CHAR(30), m MEMO)")
for i in range(0, 400, 50):
    s.append("sql INSERT INTO pp VALUES " + ", ".join("(%d, %s, %s, %s)" % (a, q(b), q(c), q(d)) for a, b, c, d in big[i:i + 50]))
s.append("sql CREATE INDEX pp_body ON pp (body)")
s.append("sql CREATE INDEX pp_id ON pp (id)")
s.append("sql CREATE INDEX pp_title ON pp (title)")
# an FTS in the middle
s.append("sql CREATE TABLE pq (id INTEGER, body CHAR(40), title CHAR(30), m MEMO)")
for i in range(0, 400, 50):
    s.append("sql INSERT INTO pq VALUES " + ", ".join("(%d, %s, %s, %s)" % (a, q(b), q(c), q(d)) for a, b, c, d in big[i:i + 50]))
s.append("sql CREATE INDEX pq_body ON pq (body)")
s.append("sql CREATE INDEX pq_fts ON pq (body) CONTENT")
s.append("sql CREATE INDEX pq_title ON pq (title)")
s.append("sql CREATE INDEX pq_fts2 ON pq (m) CONTENT")
o = ["restructure pp.adt 0 extra,Char,4||", "restructure pq.adt 0 extra,Char,4||"]
open(os.path.join(out, "setup.txt"), "w").write("\n".join(s) + "\n")
open(os.path.join(out, "ops.txt"), "w").write("\n".join(o) + "\n")
