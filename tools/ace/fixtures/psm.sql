-- A data dictionary with SQL/PSM code objects: script stored procedures
-- (fixed and CURSOR VARYING output), an AEP reference, table triggers of
-- every type with options and priorities, a user-defined function and a
-- package. The DML at the end fires the triggers, so items.json and
-- audit.json show their effect (the INSTEAD OF DELETE trigger's UPDATE fires
-- the AFTER UPDATE trigger). See docs/sql/psm.md.
-- types: adt
CREATE DATABASE "{DIR}\psm.add" PASSWORD 'adssyspw' DESCRIPTION 'PSM fixture';
-- connect: psm.add ADSSYS adssyspw
CREATE TABLE items (id INTEGER, name CHAR(10), qty INTEGER, note MEMO);
CREATE TABLE audit (seq AUTOINC, msg CHAR(80));
CREATE PROCEDURE add_item (id INTEGER, name CHAR(10), qty INTEGER, new_count INTEGER OUTPUT) BEGIN INSERT INTO items (id, name, qty) VALUES (_id, _name, _qty); INSERT INTO __output SELECT COUNT(*) FROM items; END;
CREATE PROCEDURE numbers (n INTEGER, v INTEGER OUTPUT) BEGIN DECLARE i INTEGER; i = 0; WHILE i < _n DO i = i + 1; INSERT INTO __output VALUES (i); END WHILE; END;
CREATE PROCEDURE pick (which INTEGER, CURSOR VARYING OUTPUT) BEGIN IF _which = 1 THEN SELECT * FROM items; ELSE SELECT COUNT(*) AS n FROM audit; END IF; END;
CREATE PROCEDURE external_one (a INTEGER, b INTEGER OUTPUT) FUNCTION MyFunc IN LIBRARY "example.aep";
CREATE FUNCTION sq (i INTEGER) RETURNS INTEGER DESCRIPTION 'square' BEGIN RETURN i * i; END;
CREATE PACKAGE util DESCRIPTION 'helpers';
CREATE FUNCTION util.pad (i INTEGER, len INTEGER) RETURNS STRING BEGIN DECLARE s STRING; s = TRIM(CAST(i AS SQL_CHAR)); IF LENGTH(s) < len THEN s = REPEAT('0', len - LENGTH(s)) + s; END IF; RETURN s; END;
CREATE TRIGGER items_bi ON items BEFORE INSERT BEGIN INSERT INTO audit (msg) SELECT 'BI ' + TRIM(name) + ' rec ' + TRIM(CAST(::stmt.TrigRecNo AS SQL_CHAR)) FROM __new; END PRIORITY 2;
CREATE TRIGGER items_ai ON items AFTER INSERT BEGIN INSERT INTO audit (msg) SELECT 'AI ' + TRIM(name) + ' ' + util.pad(id, 4) FROM __new; END NO MEMOS PRIORITY 1;
CREATE TRIGGER items_au ON items AFTER UPDATE BEGIN INSERT INTO audit (msg) VALUES ('AU ' + TRIM(CAST(__old.qty AS SQL_CHAR)) + '->' + TRIM(CAST(__new.qty AS SQL_CHAR))); END;
CREATE TRIGGER items_iod ON items INSTEAD OF DELETE BEGIN UPDATE items SET qty = 0 WHERE id = (SELECT id FROM __old); END NO TRANSACTION;
CREATE TRIGGER items_conflict ON items CONFLICT UPDATE BEGIN INSERT INTO audit (msg) VALUES ('conflict'); END;
EXECUTE PROCEDURE add_item (1, 'apple', 5);
EXECUTE PROCEDURE add_item (2, 'pear', sq(3));
UPDATE items SET qty = qty + 1 WHERE id = 1;
DELETE FROM items WHERE id = 2;
