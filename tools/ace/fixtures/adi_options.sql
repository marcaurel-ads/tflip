-- ADI index options: conditional (FOR), unique, candidate, custom (empty),
-- binary (bitmap) DELETED(), and a full-text (CONTENT) index; a second table
-- with a dynamic collation index (a table cannot mix collations in SQL), and
-- a 5000-record table whose DELETED() bitmap spans several pages.
-- types: adt
CREATE TABLE opts (id INTEGER, name CHAR(10), doc MEMO);
DECLARE i INTEGER; i = 0; WHILE i < 40 DO INSERT INTO opts VALUES (i, 'n' + TRIM(CAST(MOD(i, 13) AS SQL_CHAR)), 'word' + TRIM(CAST(MOD(i, 5) AS SQL_CHAR)) + ' shared text number ' + TRIM(CAST(i AS SQL_CHAR))); i = i + 1; END WHILE;
DELETE FROM opts WHERE MOD(id, 9) = 4;
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'cond', 'id', 'id > 30', 2, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'uniq', 'id', NULL, 3, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'cand', 'id', NULL, 2050, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'opts', 'cust', 'name', NULL, 6, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('opts', 'bitmap', 'deleted', 'DELETED()', NULL, 4098, 512, NULL);
CREATE INDEX fts ON opts (doc) CONTENT MIN WORD 3;
CREATE TABLE coll (name CHAR(10));
INSERT INTO coll VALUES ('Zebra');
INSERT INTO coll VALUES ('äpfel');
INSERT INTO coll VALUES ('Apfel');
INSERT INTO coll VALUES ('zebra');
EXECUTE PROCEDURE sp_CreateIndex90 ('coll', 'coll', 'german', 'name', NULL, 2, 512, 'GERMAN_VFP_CI_AS_1252');
CREATE TABLE bits (id INTEGER);
DECLARE i INTEGER; i = 0; WHILE i < 5000 DO INSERT INTO bits VALUES (i); i = i + 1; END WHILE;
DELETE FROM bits WHERE MOD(id, 3) = 1 OR id > 4990;
EXECUTE PROCEDURE sp_CreateIndex90 ('bits', 'bits', 'deleted', 'DELETED()', NULL, 4098, 512, NULL);
-- dump: coll -c GERMAN_VFP_CI_AS_1252
-- dump: opts -x bitmap.adi
