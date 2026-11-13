# The dictionary_constraints fixture, built by `constraints.exe ... build`
# (see constraints.sh): one SQL statement per line, run on a fresh dictionary.
#
# log: what the triggers record, in order (seq).
CREATE TABLE log (seq AUTOINC, ev CHAR(4), id INTEGER, n INTEGER)
# t: field defaults, a required field, min/max with and without a field
# message, a date minimum, and a record validation with its own message.
CREATE TABLE t (id INTEGER, name CHAR(10), qty INTEGER, price NUMERIC(8,2), d DATE, flag LOGICAL, note MEMO)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'name', 'FIELD_DEFAULT_VALUE', 'abc', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'name', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'qty', 'FIELD_DEFAULT_VALUE', '5', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'qty', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'qty', 'FIELD_MAX_VALUE', '100', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'qty', 'FIELD_VALIDATION_MSG', 'qty out of range', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'price', 'FIELD_MIN_VALUE', '0', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'd', 'FIELD_DEFAULT_VALUE', '01/02/2020', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'd', 'FIELD_MIN_VALUE', '01/01/2000', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('t', 'flag', 'FIELD_DEFAULT_VALUE', 'TRUE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_VALIDATION_EXPR', 'qty <> 13', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('t', 'TABLE_VALIDATION_MSG', 'thirteen is unlucky', 'NO_VALIDATE', NULL)
# u: required fields without defaults (one with a field message, one a memo),
# character min/max, and a record validation without a message.
CREATE TABLE u (id INTEGER, req CHAR(5), s CHAR(5), n INTEGER, m MEMO)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 'req', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 'req', 'FIELD_VALIDATION_MSG', 'req is required', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 's', 'FIELD_MIN_VALUE', 'b', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 's', 'FIELD_MAX_VALUE', 'x', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 'n', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('u', 'm', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('u', 'TABLE_VALIDATION_EXPR', 'n <> 7', 'NO_VALIDATE', NULL)
# w, w2: which field a write reports first (field order; NULL and min/max
# checked together per field), with defaults that break the constraints.
CREATE TABLE w (a INTEGER, b CHAR(5), c INTEGER)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w', 'a', 'FIELD_DEFAULT_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w', 'a', 'FIELD_MIN_VALUE', '5', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w', 'b', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w', 'c', 'FIELD_DEFAULT_VALUE', '9', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w', 'c', 'FIELD_MAX_VALUE', '3', 'NO_VALIDATE', NULL)
CREATE TABLE w2 (b CHAR(5), a INTEGER)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w2', 'a', 'FIELD_DEFAULT_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w2', 'a', 'FIELD_MIN_VALUE', '5', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('w2', 'b', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
# v: rows written before the constraints, which then break them.
CREATE TABLE v (id INTEGER, a INTEGER, m MEMO)
INSERT INTO v (id, a) VALUES (1, NULL)
INSERT INTO v (id, a) VALUES (2, 0)
INSERT INTO v (id, a, m) VALUES (3, 4, 'x')
EXECUTE PROCEDURE sp_ModifyFieldProperty ('v', 'a', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('v', 'a', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('v', 'm', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('v', 'TABLE_VALIDATION_EXPR', 'id < 50', 'NO_VALIDATE', NULL)
# x: defaults of other types, NOW() and TODAY() among them.
CREATE TABLE x (id INTEGER, ts TIMESTAMP, dd DATE, c CHAR(20), n NUMERIC(6,2), l LOGICAL, tm TIME)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'ts', 'FIELD_DEFAULT_VALUE', 'NOW()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'dd', 'FIELD_DEFAULT_VALUE', 'TODAY()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'c', 'FIELD_DEFAULT_VALUE', 'USER()', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'n', 'FIELD_DEFAULT_VALUE', '1.5', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'l', 'FIELD_DEFAULT_VALUE', 'F', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('x', 'tm', 'FIELD_DEFAULT_VALUE', '10:11:12', 'NO_VALIDATE', NULL)
# g: triggers around constraints (a BEFORE INSERT that refuses id 99).
CREATE TABLE g (id INTEGER, n INTEGER)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('g', 'n', 'FIELD_MIN_VALUE', '1', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyFieldProperty ('g', 'n', 'FIELD_DEFAULT_VALUE', '3', 'NO_VALIDATE', NULL)
EXECUTE PROCEDURE sp_ModifyTableProperty ('g', 'TABLE_VALIDATION_EXPR', 'n <> 7', 'NO_VALIDATE', NULL)
CREATE TRIGGER g_bi ON g BEFORE INSERT BEGIN INSERT INTO log (ev, id, n) SELECT 'bi', id, n FROM __new; IF __new.id = 99 THEN RAISE nope(1, 'refused'); END IF; END
CREATE TRIGGER g_ai ON g AFTER INSERT BEGIN INSERT INTO log (ev, id, n) SELECT 'ai', id, n FROM __new; END
CREATE TRIGGER g_bu ON g BEFORE UPDATE BEGIN INSERT INTO log (ev, id, n) SELECT 'bu', id, n FROM __new; END
CREATE TRIGGER g_ad ON g AFTER DELETE BEGIN INSERT INTO log (ev, id, n) SELECT 'ad', id, n FROM __old; END
# h: an INSTEAD OF INSERT trigger.
CREATE TABLE h (id INTEGER)
CREATE TRIGGER h_ii ON h INSTEAD OF INSERT BEGIN INSERT INTO log (ev, id) SELECT 'ii', id FROM __new; END
# k: a BEFORE DELETE trigger that refuses id 2, and an AFTER UPDATE.
CREATE TABLE k (id INTEGER)
INSERT INTO k (id) VALUES (1)
INSERT INTO k (id) VALUES (2)
CREATE TRIGGER k_bd ON k BEFORE DELETE BEGIN INSERT INTO log (ev, id) SELECT 'bd', id FROM __old; IF __old.id = 2 THEN RAISE nope(2, 'keep'); END IF; END
CREATE TRIGGER k_au ON k AFTER UPDATE BEGIN INSERT INTO log (ev, id, n) SELECT 'au', o.id, n.id FROM __old o, __new n; END
