-- A data dictionary with distinctive values for many object properties, to
-- map the property slots of each object type: database, table and field
-- properties, comments, a referential integrity rule, a conditional index, a
-- package, a link, user properties, trigger variants, a view long enough to
-- overflow into the "More Property" memo, and a dropped table.
-- types: adt
CREATE DATABASE "{DIR}\props.add" PASSWORD 'adssyspw' DESCRIPTION 'Props dictionary';
-- connect: props.add ADSSYS adssyspw
EXECUTE PROCEDURE sp_ModifyDatabase ('VERSION_MAJOR', '7');
EXECUTE PROCEDURE sp_ModifyDatabase ('VERSION_MINOR', '42');
EXECUTE PROCEDURE sp_ModifyDatabase ('LOG_IN_REQUIRED', 'TRUE');
EXECUTE PROCEDURE sp_ModifyDatabase ('VERIFY_ACCESS_RIGHTS', 'TRUE');
EXECUTE PROCEDURE sp_ModifyDatabase ('ENABLE_INTERNET', 'TRUE');
EXECUTE PROCEDURE sp_ModifyDatabase ('INTERNET_SECURITY_LEVEL', '2');
EXECUTE PROCEDURE sp_ModifyDatabase ('MAX_FAILED_ATTEMPTS', '9');
EXECUTE PROCEDURE sp_ModifyDatabase ('LOGINS_DISABLED_MSG', 'come back later');
EXECUTE PROCEDURE sp_ModifyDatabase ('FTS_DELIMITERS', ';:');
EXECUTE PROCEDURE sp_ModifyDatabase ('FTS_NOISE_WORDS', 'foo bar');
EXECUTE PROCEDURE sp_ModifyDatabase ('FTS_DROP_CHARS', '#');
EXECUTE PROCEDURE sp_ModifyDatabase ('FTS_CONDITIONAL_CHARS', '-');
EXECUTE PROCEDURE sp_ModifyDatabase ('ENCRYPT_COMMUNICATION', 'TRUE');
EXECUTE PROCEDURE sp_ModifyDatabase ('DISABLE_DLL_CACHING', 'TRUE');
EXECUTE PROCEDURE sp_ModifyDatabase ('QUERY_VIA_ROOT', '1');
EXECUTE PROCEDURE sp_ModifyDatabase ('ENFORCE_MAX_FAILED_LOGINS', 'TRUE');
CREATE TABLE parent (id INTEGER, code CHAR(10), label VARCHAR(40), weight DOUBLE(3));
CREATE TABLE child (id INTEGER, parent_id INTEGER, amount NUMERIC(10, 2), pic BLOB);
CREATE UNIQUE INDEX parent_id ON parent (id);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_PRIMARY_KEY', 'parent_id', 'NO_VALIDATE', NULL);
CREATE INDEX child_parent ON child (parent_id);
EXECUTE PROCEDURE sp_CreateIndex90 ('child', 'child_big', 'big_amount', 'amount', 'amount > 100', 8, 1024, NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'COMMENT', 'Parent table comment', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_VALIDATION_EXPR', 'id > 0', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_VALIDATION_MSG', 'id must be positive', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_DEFAULT_INDEX', 'parent_id', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_PERMISSION_LEVEL', '2', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_AUTO_CREATE', 'TRUE', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('parent', 'TABLE_MEMO_BLOCK_SIZE', '64', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('child', 'TABLE_TRANS_FREE', 'TRUE', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('child', 'TABLE_CACHING', '1', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'code', 'COMMENT', 'Code field comment', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'code', 'FIELD_VALIDATION_MSG', 'bad code', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'code', 'FIELD_DEFAULT_VALUE', 'ABC', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'code', 'FIELD_CAN_BE_NULL', 'FALSE', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'weight', 'FIELD_MIN_VALUE', '-1.5', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyFieldProperty ('parent', 'weight', 'FIELD_MAX_VALUE', '99.25', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_CreateReferentialIntegrity ('child_to_parent', 'parent', 'child', 'child_parent', 1, 2, NULL, 'no parent', 'cascade failed');
CREATE PACKAGE util DESCRIPTION 'Utility package';
CREATE FUNCTION util.add3 (a INTEGER, b INTEGER, c INTEGER) RETURNS INTEGER DESCRIPTION 'Adds three' BEGIN RETURN a + b + c; END;
EXECUTE PROCEDURE sp_CreateLink ('selflink', 'props.add', TRUE, FALSE, FALSE, 'ADSSYS', 'adssyspw');
EXECUTE PROCEDURE sp_CreateUser ('bob', 'bobpw', 'User bob');
EXECUTE PROCEDURE sp_ModifyUserProperty ('bob', 'ENABLE_INTERNET', 'TRUE');
EXECUTE PROCEDURE sp_CreateGroup ('readers', 'Readers group');
EXECUTE PROCEDURE sp_AddUserToGroup ('bob', 'readers');
GRANT SELECT, INSERT ON parent TO readers;
GRANT UPDATE (label) ON parent TO bob;
INSERT INTO parent VALUES (1, 'X1', 'first', 1.25);
INSERT INTO child VALUES (1, 1, 150.5, NULL);
CREATE PROCEDURE lookup (pid INTEGER, label CHAR(40) OUTPUT) BEGIN INSERT INTO __output SELECT label FROM parent WHERE id = (SELECT pid FROM __input); END;
CREATE VIEW wide_parent AS SELECT id AS parent_identifier_column, code AS parent_code_column, label AS parent_label_column, weight AS parent_weight_column, id + 1 AS next_identifier_column, id * 2 AS double_identifier_column, weight * 2 AS double_weight_column FROM parent WHERE id > 0 AND code IS NOT NULL;
CREATE TRIGGER child_before_update ON child BEFORE UPDATE BEGIN UPDATE child SET amount = amount WHERE 1 = 0; END NO MEMOS PRIORITY 5;
CREATE TRIGGER child_instead_delete ON child INSTEAD OF DELETE FUNCTION trigfn IN LIBRARY mytrig;
CREATE TRIGGER on_connect ON DATABASE AFTER CONNECT BEGIN DELETE FROM child WHERE 1 = 0; END;
CREATE TRIGGER parent_after_insert ON parent AFTER INSERT BEGIN DELETE FROM child WHERE 1 = 0; END;
EXECUTE PROCEDURE sp_DisableTriggers ('parent', NULL, FALSE, 0);
CREATE TABLE scratch (id INTEGER);
DROP TABLE scratch;
CREATE TABLE extra (id AUTOINC, rv ROWVERSION, m MEMO COMPRESSED);
EXECUTE PROCEDURE sp_ModifyTableProperty ('extra', 'TABLE_PERMISSION_LEVEL', '1', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('extra', 'TABLE_CACHING', '2', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('extra', 'TABLE_CONCURRENCY_ENABLED', 'TRUE', 'NO_VALIDATE', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('extra', 'TABLE_WEB_DELTA', 'TRUE', 'NO_VALIDATE', NULL);
