#!/usr/bin/env python3
"""Writes the data tflip's catalog procedures ship, from the original engine's
answers (the fixtures that spcat.txt and spcoll.txt make):

    gen_catalog_data.py <spcat.json> <spcoll.json> <engine src folder>

- procedure/system_procedures.tsv: system.systemprocedures, one row a line,
  its five columns split by tabs (a NULL is \\N);
- procedure/type_info.tsv: the rows of sp_GetTypeInfo, a column to a tab;
- procedure/icu_collations.tsv: the ICU locales sp_GetCollations lists after
  the repository's, as name, short name and description;
- the pairs sp_SupportsConvert says yes to are printed as Rust for
  procedure/catalog.rs.
"""
import json
import re
import sys

cat = json.load(open(sys.argv[1]))
coll = json.load(open(sys.argv[2]))
out = sys.argv[3]


def cell(value, pad=False):
    if value is None:
        return "\\N"
    if pad:
        value = value.rstrip(" ")
    assert "\t" not in value and "\n" not in value and "\\" not in value, value
    return value


# system.systemprocedures: the statement of spcat.txt that lists all its rows.
table = None
for statement in cat:
    cursor = statement.get("cursor")
    if cursor and cursor["fields"][0][0] == "Name" and cursor["count"] == 166:
        table = cursor
assert table is not None, "no fixture statement lists all of system.systemprocedures"
with open(out + "/procedure/system_procedures.tsv", "w") as f:
    for _, row in table["rows"]:
        f.write("\t".join(cell(v, i == 0) for i, v in enumerate(row)) + "\n")

for statement in cat:
    if statement["cmd"] == "query EXECUTE PROCEDURE sp_GetTypeInfo()":
        with open(out + "/procedure/type_info.tsv", "w") as f:
            for _, row in statement["cursor"]["rows"]:
                f.write("\t".join(cell(v, True) for v in row) + "\n")
        break

seen = {}
for statement in coll:
    m = re.match(r"query EXECUTE PROCEDURE sp_GetCollations\('([a-z])%'\)", statement["cmd"])
    if not m:
        continue
    for _, row in statement["cursor"]["rows"]:
        if row[3] is None:
            seen[row[0].rstrip()] = (row[1].rstrip(), row[2].rstrip())
names = sorted(seen)
with open(out + "/procedure/icu_collations.tsv", "w") as f:
    for name in names:
        f.write("%s\t%s\t%s\n" % (name, seen[name][0], seen[name][1]))
print(len(names), "ICU collations")

# sp_SupportsConvert
yes = {}
types = []
for statement in cat:
    m = re.match(r"query EXECUTE PROCEDURE sp_SupportsConvert\((-?\d+), (-?\d+)\)$", statement["cmd"])
    if m and statement["rc"] == 0:
        a, b = int(m.group(1)), int(m.group(2))
        yes[(a, b)] = statement["cursor"]["rows"][0][1][0] == "T"
        if a not in types:
            types.append(a)
types = [t for t in types if t != 0]
print("const CONVERT_TYPES: [i32;", len(types), "] =", types)
for a in types:
    bits = "".join("1" if yes.get((a, b)) else "0" for b in types)
    print("    %r, // %d" % (bits, a))
