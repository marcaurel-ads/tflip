-- ADI B-tree structure: tables large enough for multi-level trees. "grown" has
-- its indexes before the rows are inserted (the tree grows by page splits),
-- "built" gets them after (bulk build); both hold the same 3000 rows. Then
-- "grown" is modified: rows deleted, keys updated. Keys include many
-- duplicates, shared prefixes (for key compression) and pages of 512, 1024
-- and 8192 bytes.
-- types: adt
CREATE TABLE grown (id INTEGER, word CHAR(12), grp SHORT);
CREATE INDEX by_id ON grown (id);
CREATE INDEX by_word ON grown (word);
CREATE INDEX by_grp ON grown (grp);
DECLARE i INTEGER; i = 0; WHILE i < 3000 DO INSERT INTO grown VALUES (MOD(i * 7919, 3001), 'w' + CHAR(97 + MOD(i, 26)) + CHAR(97 + MOD(i / 26, 26)) + TRIM(CAST(MOD(i * 31, 1000) AS SQL_CHAR)), MOD(i, 7)); i = i + 1; END WHILE;
CREATE TABLE built (id INTEGER, word CHAR(12), grp SHORT);
INSERT INTO built SELECT * FROM grown;
CREATE INDEX by_id ON built (id);
CREATE INDEX by_word ON built (word);
CREATE INDEX by_grp ON built (grp);
EXECUTE PROCEDURE sp_CreateIndex90 ('built', 'built1k', 'word1k', 'word', NULL, 2, 1024, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('built', 'built8k', 'word8k', 'word', NULL, 2, 8192, NULL);
DELETE FROM grown WHERE MOD(id, 10) = 3;
UPDATE grown SET word = 'zz' + TRIM(word) WHERE MOD(id, 10) = 5;
UPDATE grown SET id = id + 5000 WHERE MOD(id, 10) = 7;
-- dump: built -x built1k.adi -x built8k.adi
