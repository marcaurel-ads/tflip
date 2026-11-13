-- CDX/VFP index options and expressions: UPPER(), concatenated expressions,
-- a FOR condition, xBase UNIQUE (first record of each key only), binary
-- (bitmap) indexes on a logical field and on DELETED(), a second compound .cdx, a single-order .idx, and a 3000-record table for
-- multi-level trees (grown by inserts and then modified).
-- types: cdx vfp
CREATE TABLE opts (id INTEGER, name CHAR(10), flag LOGICAL, born DATE);
DECLARE i INTEGER; i = 0; WHILE i < 40 DO INSERT INTO opts VALUES (i, 'n' + TRIM(CAST(MOD(i, 13) AS SQL_CHAR)), MOD(i, 3) = 0, CAST('2000-01-01' AS SQL_DATE) + i); i = i + 1; END WHILE;
DELETE FROM opts WHERE MOD(id, 9) = 4;
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'x_upper', 'UPPER(name)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'x_mix', 'name + DTOS(born) + STR(id, 5)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'cond', 'id', 'id > 30', 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'uniq', 'name', NULL, 3, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'bin', 'flag', NULL, 4098, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'deleted', 'DELETED()', NULL, 4098, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'second', 'by_born', 'born', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'single', NULL, 'name', NULL, 0, 0, NULL);
CREATE TABLE grown (id INTEGER, word CHAR(12), grp NUMERIC(2, 0));
CREATE INDEX by_id ON grown (id);
CREATE INDEX by_word ON grown (word);
CREATE INDEX by_grp ON grown (grp);
DECLARE i INTEGER; i = 0; WHILE i < 3000 DO INSERT INTO grown VALUES (MOD(i * 7919, 3001), 'w' + CHAR(97 + MOD(i, 26)) + CHAR(97 + MOD(i / 26, 26)) + TRIM(CAST(MOD(i * 31, 1000) AS SQL_CHAR)), MOD(i, 7)); i = i + 1; END WHILE;
DELETE FROM grown WHERE MOD(id, 10) = 3;
UPDATE grown SET word = 'zz' + TRIM(word) WHERE MOD(id, 10) = 5;
UPDATE grown SET id = id + 5000 WHERE MOD(id, 10) = 7;
CREATE TABLE built (id INTEGER, word CHAR(12), grp NUMERIC(2, 0));
INSERT INTO built SELECT * FROM grown;
CREATE INDEX by_id ON built (id);
CREATE INDEX by_word ON built (word);
-- dump: opts -x second.cdx -x single.idx
