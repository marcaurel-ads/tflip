# The dictionary_rules fixture, built by `constraints.exe ... build` (see
# rules.sh): one SQL statement per line, run on a fresh dictionary.
#
# log: what the triggers record, in order (seq).
CREATE TABLE log (seq AUTOINC, ev CHAR(4), id INTEGER, n INTEGER)
# p, c: a RESTRICT rule; the parent has a record validation, the child a
# default, a minimum, a record validation and (set after its row) a NOT NULL
# field; BEFORE triggers on both.
CREATE TABLE p (id INTEGER, lbl CHAR(5))
CREATE UNIQUE INDEX p_pk ON p (id)
EXECUTE PROCEDURE sp_ModifyTableProperty ('p', 'TABLE_PRIMARY_KEY', 'p_pk', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('p', 'TABLE_VALIDATION_EXPR', 'lbl <> ''bad''', 'NO_VALIDATE', NULL)
INSERT INTO p (id, lbl) VALUES (1, 'one')
INSERT INTO p (id, lbl) VALUES (2, 'two')
CREATE TABLE c (cid INTEGER, pid INTEGER, n INTEGER, r CHAR(3))
CREATE INDEX c_fk ON c (pid)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('c', 'n', 'FIELD_DEFAULT_VALUE', '3', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('c', 'n', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('c', 'TABLE_VALIDATION_EXPR', 'n <> 7', 'NO_VALIDATE', NULL)
INSERT INTO c (cid, pid, r) VALUES (1, 1, 'a')
EXECUTE PROCEDURE sp_ModifyFieldProperty ('c', 'r', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_CreateReferentialIntegrity ('r_c', 'p', 'c', 'c_fk', 2, 2, NULL, 'no parent', 'cascade failed')
CREATE TRIGGER c_bi ON c BEFORE INSERT BEGIN INSERT INTO log (ev, id, n) SELECT 'cbi', cid, pid FROM __new; END
CREATE TRIGGER c_bu ON c BEFORE UPDATE BEGIN INSERT INTO log (ev, id, n) SELECT 'cbu', cid, pid FROM __new; END
CREATE TRIGGER p_bu ON p BEFORE UPDATE BEGIN INSERT INTO log (ev, id, n) SELECT 'pbu', id, 0 FROM __new; END
CREATE TRIGGER p_bd ON p BEFORE DELETE BEGIN INSERT INTO log (ev, id, n) SELECT 'pbd', id, 0 FROM __old; END
# q and its children: CASCADE into qc (whose key has a maximum and BEFORE
# triggers), SET NULL into qn (whose key cannot be NULL), SET DEFAULT into qd
# (whose key's default, a parent, is below its minimum).
CREATE TABLE q (id INTEGER)
CREATE UNIQUE INDEX q_pk ON q (id)
EXECUTE PROCEDURE sp_ModifyTableProperty ('q', 'TABLE_PRIMARY_KEY', 'q_pk', 'NO_VALIDATE', NULL)
INSERT INTO q (id) VALUES (0)
INSERT INTO q (id) VALUES (1)
INSERT INTO q (id) VALUES (2)
INSERT INTO q (id) VALUES (3)
CREATE TABLE qc (cid INTEGER, pid INTEGER)
CREATE INDEX qc_fk ON qc (pid)
INSERT INTO qc (cid, pid) VALUES (1, 1)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('qc', 'pid', 'FIELD_MAX_VALUE', '50', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_CreateReferentialIntegrity ('r_qc', 'q', 'qc', 'qc_fk', 1, 1, NULL, 'no parent', 'cascade failed')
CREATE TRIGGER qc_bi ON qc BEFORE INSERT BEGIN INSERT INTO log (ev, id, n) SELECT 'qcbi', cid, pid FROM __new; END
CREATE TRIGGER qc_bu ON qc BEFORE UPDATE BEGIN INSERT INTO log (ev, id, n) SELECT 'qcbu', cid, pid FROM __new; END
CREATE TRIGGER qc_bd ON qc BEFORE DELETE BEGIN INSERT INTO log (ev, id, n) SELECT 'qcbd', cid, pid FROM __old; END
CREATE TABLE qn (cid INTEGER, pid INTEGER)
CREATE INDEX qn_fk ON qn (pid)
INSERT INTO qn (cid, pid) VALUES (1, 2)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('qn', 'pid', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_CreateReferentialIntegrity ('r_qn', 'q', 'qn', 'qn_fk', 3, 3, NULL, 'no parent', 'cascade failed')
CREATE TABLE qd (cid INTEGER, pid INTEGER)
CREATE INDEX qd_fk ON qd (pid)
INSERT INTO qd (cid, pid) VALUES (1, 3)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('qd', 'pid', 'FIELD_DEFAULT_VALUE', '0', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('qd', 'pid', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_CreateReferentialIntegrity ('r_qd', 'q', 'qd', 'qd_fk', 4, 4, NULL, 'no parent', 'cascade failed')
# d: defaults beyond NOW() and TODAY(): expressions of each type on fields
# of each type, other fields, text that is no expression.
CREATE TABLE d (id INTEGER, a INTEGER, i1 INTEGER, i2 INTEGER, i3 INTEGER, i4 INTEGER, i5 INTEGER, i6 INTEGER, i7 INTEGER, s1 SHORT, n1 NUMERIC(8,3), n2 NUMERIC(6,2), f1 DOUBLE, f2 DOUBLE, mo MONEY, c1 CHAR(12), c2 CHAR(12), c3 CHAR(12), c4 CHAR(12), c5 CHAR(12), c6 CHAR(12), c7 CHAR(12), c8 CHAR(12), c9 CHAR(12), c10 CHAR(12), c11 CHAR(12), k1 CICHAR(12), v1 VARCHAR(12), l1 LOGICAL, l2 LOGICAL, l3 LOGICAL, l4 LOGICAL, l5 LOGICAL, d1 DATE, d2 DATE, d3 DATE, d4 DATE, d5 DATE, d6 DATE, t1 TIMESTAMP, t2 TIMESTAMP, t3 TIMESTAMP, h1 TIME, h2 TIME)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'a', 'FIELD_DEFAULT_VALUE', '42', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i1', 'FIELD_DEFAULT_VALUE', '2+3', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i2', 'FIELD_DEFAULT_VALUE', '-1.6', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i3', 'FIELD_DEFAULT_VALUE', 'abc', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i4', 'FIELD_DEFAULT_VALUE', '''7''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i5', 'FIELD_DEFAULT_VALUE', 'a+1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i6', 'FIELD_DEFAULT_VALUE', 'id', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'i7', 'FIELD_DEFAULT_VALUE', 'IIF(ISNULL(id),1,2)', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 's1', 'FIELD_DEFAULT_VALUE', 'VAL(''3.5'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'n1', 'FIELD_DEFAULT_VALUE', '1/3', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'n2', 'FIELD_DEFAULT_VALUE', '1.555', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'f1', 'FIELD_DEFAULT_VALUE', '1/4', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'f2', 'FIELD_DEFAULT_VALUE', 'NOW()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'mo', 'FIELD_DEFAULT_VALUE', '10/4', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c1', 'FIELD_DEFAULT_VALUE', '''a''+''b''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c2', 'FIELD_DEFAULT_VALUE', '1+1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c3', 'FIELD_DEFAULT_VALUE', 'NOW()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c4', 'FIELD_DEFAULT_VALUE', 'UPPER(''x'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c5', 'FIELD_DEFAULT_VALUE', 'c1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c6', 'FIELD_DEFAULT_VALUE', 'c9', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c7', 'FIELD_DEFAULT_VALUE', 'USER()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c8', 'FIELD_DEFAULT_VALUE', '.T.', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c9', 'FIELD_DEFAULT_VALUE', '''late''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c10', 'FIELD_DEFAULT_VALUE', 'STR(5)', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'c11', 'FIELD_DEFAULT_VALUE', '  x y ', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'k1', 'FIELD_DEFAULT_VALUE', 'LOWER(''AB'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'v1', 'FIELD_DEFAULT_VALUE', 'DTOS(STOD(''20200105''))', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'l1', 'FIELD_DEFAULT_VALUE', '1=1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'l2', 'FIELD_DEFAULT_VALUE', '.F.', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'l3', 'FIELD_DEFAULT_VALUE', '''T''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'l4', 'FIELD_DEFAULT_VALUE', 'yes', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'l5', 'FIELD_DEFAULT_VALUE', 'a > 40', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd1', 'FIELD_DEFAULT_VALUE', 'STOD(''20200105'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd2', 'FIELD_DEFAULT_VALUE', 'STOD(''20200105'')+1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd3', 'FIELD_DEFAULT_VALUE', 'CTOD(''01/05/2020'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd4', 'FIELD_DEFAULT_VALUE', '01/02/2020', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd5', 'FIELD_DEFAULT_VALUE', 'NOW()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'd6', 'FIELD_DEFAULT_VALUE', 'DATE()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 't1', 'FIELD_DEFAULT_VALUE', 'STOD(''20200105'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 't2', 'FIELD_DEFAULT_VALUE', '''01/05/2020 10:11:12''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 't3', 'FIELD_DEFAULT_VALUE', 'NOW()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'h1', 'FIELD_DEFAULT_VALUE', '''12:30:00''', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('d', 'h2', 'FIELD_DEFAULT_VALUE', '10:11:12', 'NO_VALIDATE', NULL)
# b: a minimum or maximum is read as an expression too.
CREATE TABLE b (id INTEGER, i INTEGER, d DATE, c CHAR(5))
EXECUTE PROCEDURE sp_ModifyFieldProperty ('b', 'i', 'FIELD_MIN_VALUE', '2+3', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('b', 'd', 'FIELD_MIN_VALUE', 'STOD(''20200101'')', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('b', 'c', 'FIELD_MIN_VALUE', '''b''', 'NO_VALIDATE', NULL)
# f: no rules; the probe sets them (AdsDDSetFieldProperty) on each type.
CREATE TABLE f (id INTEGER, m MEMO, b BLOB, l LOGICAL, n NMEMO, r RAW(4), vb VARBINARY(10), c CHAR(5), nu NUMERIC(6,2), d DATE, tm TIME, i INTEGER, ts TIMESTAMP, mo MONEY, dbl DOUBLE, ai AUTOINC, rv ROWVERSION, mt MODTIME, ci CICHAR(5), sh SHORT, g GUID)
