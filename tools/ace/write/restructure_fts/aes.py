import sys

out = sys.argv[1]
lines = ["# Restructuring tables with full-text orders in dictionaries that encrypt: x (AES-128, records and indexes enciphered), y (AES-256, records enciphered, indexes in the clear) and s (RC4, records and indexes, table password tblpass) each get a table with a full-text order on a memo and on a character field, a keep-score one and an ordinary one, and ALTER TABLE adds a column, widens the indexed column, drops another, and a record is added after each."]


def table(where, ct):
    for sql in [
        "CREATE TABLE tr (id INTEGER, body MEMO, name CHAR(20), note CHAR(30))",
        "INSERT INTO tr VALUES (1, 'red green blue', 'one two', 'n1')",
        "INSERT INTO tr VALUES (2, 'green yellow the', 'two three', 'n2')",
        "INSERT INTO tr VALUES (3, 'blue black red', 'three red', 'n3')",
        "CREATE INDEX tr_body ON tr (body) CONTENT",
        "CREATE INDEX tr_name ON tr (name) CONTENT KEEPSCORE MIN WORD 2 MAX WORD 9",
        "CREATE INDEX tr_id ON tr (id)",
    ]:
        lines.append("sql " + sql)
    if ct == "rc4":
        lines.append("sql EXECUTE PROCEDURE sp_ModifyTableProperty('tr', 'TABLE_ENCRYPTION', 'TRUE', 'NO_VALIDATE', NULL)")
    elif ct:
        lines.append("sql EXECUTE PROCEDURE sp_EncryptTable('tr')")
    for sql in [
        "INSERT INTO tr VALUES (4, 'white red', 'four red', 'n4')",
        "ALTER TABLE tr ADD COLUMN extra CHAR(5)",
        "INSERT INTO tr VALUES (5, 'red red red', 'five', 'n5', 'ex')",
        "ALTER TABLE tr ALTER COLUMN name name CHAR(30)",
        "ALTER TABLE tr DROP COLUMN note",
        "INSERT INTO tr VALUES (6, 'six six', 'six red', 'e6')",
        "INSERT INTO tr VALUES (7, 'seven', 'seven red', 'e7')",
        "SELECT id FROM tr WHERE CONTAINS(name, 'red')",
    ]:
        lines.append("sql " + sql)


lines += [
    "connect Data Source=%D\\;ServerType=LOCAL;EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;",
    "sql CREATE DATABASE \"x\\x.add\" ENCRYPT TRUE ENCRYPTIONTYPE AES128 DDPASSWORD 'ddpw'",
    "sql CREATE DATABASE \"y\\y.add\" ENCRYPT TRUE ENCRYPTIONTYPE AES256 DDPASSWORD 'ddpw'",
    "disconnect",
    "connect Data Source=%D\\x\\x.add;ServerType=LOCAL;User ID=ADSSYS;EncryptionType=AES128;FIPS=FALSE;DDPassword=ddpw;",
    "sql EXECUTE PROCEDURE sp_ModifyDatabase('ENCRYPT_INDEXES', 'TRUE')",
]
table("x", True)
lines += [
    "disconnect",
    "connect Data Source=%D\\y\\y.add;ServerType=LOCAL;User ID=ADSSYS;EncryptionType=AES256;FIPS=FALSE;DDPassword=ddpw;",
]
table("y", True)
lines += [
    "disconnect",
    "connect Data Source=%D\\;ServerType=LOCAL;",
    "sql CREATE DATABASE \"s\\s.add\" ENCRYPT TRUE",
    "disconnect",
    "connect Data Source=%D\\s\\s.add;ServerType=LOCAL;User ID=ADSSYS;",
    "sql EXECUTE PROCEDURE sp_ModifyDatabase('ENCRYPT_TABLE_PASSWORD', 'tblpass')",
    "sql EXECUTE PROCEDURE sp_ModifyDatabase('ENCRYPT_INDEXES', 'TRUE')",
]
table("s", "rc4")
lines += [
    "disconnect",
    "connect Data Source=%D\\;ServerType=LOCAL;",
    "sql CREATE DATABASE \"t\\t.add\"",
    "disconnect",
    "connect Data Source=%D\\t\\t.add;ServerType=LOCAL;User ID=ADSSYS;",
]
table("t", False)
lines.append("disconnect")
open(out, "w").write("\n".join(lines) + "\n")
