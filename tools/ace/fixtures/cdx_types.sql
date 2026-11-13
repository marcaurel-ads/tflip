-- Extended field types of the FoxPro-compatible CDX flavour, with CDX indexes.
-- types: cdx
CREATE TABLE cdx_types (c_char CHAR(10), c_integer INTEGER, c_double DOUBLE, c_blob BLOB, c_numeric NUMERIC(8, 3));
INSERT INTO cdx_types VALUES ('abc', 123456789, 3.5, X'00FF10', 1234.567);
INSERT INTO cdx_types VALUES ('bcd', -1, -2.25, X'', -1.5);
CREATE INDEX by_char ON cdx_types (c_char);
CREATE INDEX by_int ON cdx_types (c_integer);
