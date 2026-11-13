-- Clipper-compatible table with NTX index files.
-- types: ntx
CREATE TABLE ntx_index (name CHAR(10), qty NUMERIC(5, 0));
INSERT INTO ntx_index VALUES ('pear', 3);
INSERT INTO ntx_index VALUES ('apple', 10);
INSERT INTO ntx_index VALUES ('fig', 7);
CREATE INDEX name ON ntx_index (name);
-- dump: ntx_index -x name.ntx
