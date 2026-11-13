-- Memo and binary values of assorted sizes (empty, one byte, exactly one block,
-- spanning several blocks) with the smallest, default and a large block size.
-- types: adt
CREATE TABLE memo8 (id INTEGER, m MEMO, b BLOB, n NMEMO) MEMOBLOCKSIZE 8;
CREATE TABLE memo256 (id INTEGER, m MEMO, b BLOB, n NMEMO);
CREATE TABLE memo1024 (id INTEGER, m MEMO, b BLOB, n NMEMO) MEMOBLOCKSIZE 1024;
INSERT INTO memo8 (id, m, b, n) VALUES (1, '', X'', '');
INSERT INTO memo8 (id, m, b, n) VALUES (2, 'x', X'01', 'ü');
INSERT INTO memo8 (id, m, b, n) VALUES (3, 'exactly8', X'0102030405060708', 'four');
INSERT INTO memo8 (id, m, b, n) VALUES (4, REPEAT('0123456789', 30), X'00112233445566778899AABBCCDDEEFF00112233445566778899AABBCCDDEEFF', REPEAT('Ωmega', 20));
INSERT INTO memo8 (id) VALUES (5);
INSERT INTO memo256 SELECT * FROM memo8;
INSERT INTO memo1024 SELECT * FROM memo8;
UPDATE memo256 SET m = REPEAT('grown ', 100) WHERE id = 2;
UPDATE memo256 SET m = 'shrunk' WHERE id = 4;
