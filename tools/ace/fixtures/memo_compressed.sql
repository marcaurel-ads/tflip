-- Compressed memo, binary and Unicode memo fields (16-byte record entries).
-- REPEAT() is limited in length, so large values are built by doubling.
-- types: adt
CREATE TABLE compressed (id INTEGER, m MEMO COMPRESSED, b BLOB COMPRESSED, n NMEMO COMPRESSED);
INSERT INTO compressed (id, m, b, n) VALUES (1, 'short', X'0102', 'kurz');
INSERT INTO compressed (id, m, b, n) VALUES (2, REPEAT('compressible text ', 50), X'0000000000000000000000000000000000000000000000000000000000000000', REPEAT('äöü', 50));
UPDATE compressed SET m = m + m, n = n + n WHERE id = 2;
UPDATE compressed SET m = m + m, n = n + n WHERE id = 2;
UPDATE compressed SET m = m + m, n = n + n WHERE id = 2;
INSERT INTO compressed (id) VALUES (3);
