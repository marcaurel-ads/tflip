-- NTX index options and expressions: UPPER(), a concatenated expression, a
-- FOR condition, xBase UNIQUE, descending, and a 3000-record table for
-- multi-level trees. NTX files do not open with the table, so later SQL
-- statements do not maintain them: every index here is built on existing rows.
-- types: ntx
CREATE TABLE opts (id INTEGER, name CHAR(10), born DATE);
DECLARE i INTEGER; i = 0; WHILE i < 40 DO INSERT INTO opts VALUES (i, 'n' + TRIM(CAST(MOD(i, 13) AS SQL_CHAR)), CAST('2000-01-01' AS SQL_DATE) + i); i = i + 1; END WHILE;
DELETE FROM opts WHERE MOD(id, 9) = 4;
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'x_upper', NULL, 'UPPER(name)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'x_mix', NULL, 'name + DTOS(born) + STR(id, 5)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'cond', NULL, 'id', 'id > 30', 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'uniq', NULL, 'name', NULL, 1, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'desc', NULL, 'name', NULL, 8, 0, NULL);
CREATE TABLE built (id INTEGER, word CHAR(12), grp NUMERIC(2, 0));
DECLARE i INTEGER; i = 0; WHILE i < 3000 DO INSERT INTO built VALUES (MOD(i * 7919, 3001), 'w' + CHAR(97 + MOD(i, 26)) + CHAR(97 + MOD(i / 26, 26)) + TRIM(CAST(MOD(i * 31, 1000) AS SQL_CHAR)), MOD(i, 7)); i = i + 1; END WHILE;
DELETE FROM built WHERE MOD(id, 10) = 3;
CREATE INDEX b_id ON built (id);
CREATE INDEX b_word ON built (word);
-- dump: opts -x x_upper.ntx -x x_mix.ntx -x cond.ntx -x uniq.ntx -x desc.ntx
-- dump: built -x b_id.ntx -x b_word.ntx
