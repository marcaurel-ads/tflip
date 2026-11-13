-- Visual FoxPro index keys: the VFP field types with NULL values (NULL-able
-- columns), a CANDIDATE (true unique) order, and a second table with a
-- dynamic collation order.
-- types: vfp
CREATE TABLE vfp_keys (k_char CHAR(6) NULL, k_vchar VARCHAR(6) NULL, k_num NUMERIC(8, 2) NULL, k_int INTEGER NULL, k_dbl DOUBLE NULL, k_money MONEY NULL, k_date DATE NULL, k_ts TIMESTAMP NULL, k_log LOGICAL NULL, k_auto AUTOINC);
INSERT INTO vfp_keys (k_char, k_vchar, k_num, k_int, k_dbl, k_money, k_date, k_ts, k_log) VALUES ('abc', 'abc', 1.5, 1, 1.5, 1.5, '2017-11-08', '2017-11-08 12:34:56', TRUE);
INSERT INTO vfp_keys (k_char, k_vchar, k_num, k_int, k_dbl, k_money, k_date, k_ts, k_log) VALUES ('Zz', 'Zz', -1.5, -1, -1.5, -1.5, '1900-01-01', '1900-01-01 00:00:00', FALSE);
INSERT INTO vfp_keys (k_char, k_vchar, k_num, k_int, k_dbl, k_money, k_date, k_ts, k_log) VALUES ('', '', 0, 0, 0, 0, '0001-01-01', '9999-12-31 23:59:59', FALSE);
INSERT INTO vfp_keys (k_char, k_vchar, k_num, k_int, k_dbl, k_money, k_date, k_ts, k_log) VALUES ('Äé€', 'Äé€', 9999.99, 2147483647, 123456789.25, 922337203685477.5807, '9999-12-31', '1970-01-01 00:00:00.5', TRUE);
INSERT INTO vfp_keys (k_char) VALUES (NULL);
CREATE INDEX i_char ON vfp_keys (k_char);
CREATE INDEX i_vchar ON vfp_keys (k_vchar);
CREATE INDEX i_num ON vfp_keys (k_num);
CREATE INDEX i_int ON vfp_keys (k_int);
CREATE INDEX i_dbl ON vfp_keys (k_dbl);
CREATE INDEX i_money ON vfp_keys (k_money);
CREATE INDEX i_date ON vfp_keys (k_date);
CREATE INDEX i_ts ON vfp_keys (k_ts);
CREATE INDEX i_log ON vfp_keys (k_log);
CREATE INDEX i_auto ON vfp_keys (k_auto);
EXECUTE PROCEDURE sp_CreateIndex90 ('vfp_keys', 'vfp_keys', 'cand', 'k_auto', NULL, 2050, 0, NULL);
CREATE TABLE coll (name CHAR(10));
INSERT INTO coll VALUES ('Zebra');
INSERT INTO coll VALUES ('äpfel');
INSERT INTO coll VALUES ('Apfel');
INSERT INTO coll VALUES ('zebra');
EXECUTE PROCEDURE sp_CreateIndex90 ('coll', 'coll', 'german', 'name', NULL, 2, 0, 'GERMAN_VFP_CI_AS_1252');
-- dump: coll -c GERMAN_VFP_CI_AS_1252
