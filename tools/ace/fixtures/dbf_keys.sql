-- Index keys of the DBF flavours: one index per field type with positive,
-- negative, zero, blank and boundary values, plus descending and multi-field
-- keys (expressions and options: cdx_options.sql, ntx_options.sql). CDX and
-- VFP put every order in the table's compound .cdx; NTX makes one .ntx file
-- per index (named after the index).
-- types: ntx cdx vfp
CREATE TABLE dbf_keys (k_char CHAR(6), k_num NUMERIC(8, 2), k_date DATE, k_log LOGICAL, k_int INTEGER, k_dbl DOUBLE);
INSERT INTO dbf_keys VALUES ('abc', 1.5, '2017-11-08', TRUE, 1, 1.5);
INSERT INTO dbf_keys VALUES ('Zz', -1.5, '1900-01-01', FALSE, -1, -1.5);
INSERT INTO dbf_keys VALUES ('', 0, '0001-01-01', FALSE, 0, 0);
INSERT INTO dbf_keys VALUES ('Äé€', 9999.99, '9999-12-31', TRUE, 2147483647, 123456789.25);
INSERT INTO dbf_keys VALUES ('a', -999.99, '1582-10-15', TRUE, -2147483647, -0.001);
INSERT INTO dbf_keys (k_char) VALUES ('blank');
INSERT INTO dbf_keys VALUES ('gone', 5, '2000-01-01', TRUE, 5, 5);
DELETE FROM dbf_keys WHERE k_char = 'gone';
CREATE INDEX i_char ON dbf_keys (k_char);
CREATE INDEX i_num ON dbf_keys (k_num);
CREATE INDEX i_date ON dbf_keys (k_date);
CREATE INDEX i_log ON dbf_keys (k_log);
CREATE INDEX i_int ON dbf_keys (k_int);
CREATE INDEX i_dbl ON dbf_keys (k_dbl);
CREATE INDEX i_desc ON dbf_keys (k_char DESC);
CREATE INDEX i_multi ON dbf_keys (k_char, k_date);
-- dump: dbf_keys -x i_char.ntx -x i_num.ntx -x i_date.ntx -x i_log.ntx -x i_int.ntx -x i_dbl.ntx -x i_desc.ntx -x i_multi.ntx
