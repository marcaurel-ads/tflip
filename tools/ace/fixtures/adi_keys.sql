-- ADI key encodings: one index order per indexable ADT field type, with
-- negative, zero, positive, boundary, empty and NULL values, plus expression
-- and multi-field keys. adsdump records every key as AdsExtractKey returns it.
-- types: adt
CREATE TABLE keys (
  k_char       CHAR(6),
  k_cichar     CICHAR(6),
  k_varchar    VARCHAR(6),
  k_nchar      NCHAR(3),
  k_nvarchar   NVARCHAR(3),
  k_raw        RAW(3),
  k_guid       GUID,
  k_logical    LOGICAL,
  k_numeric    NUMERIC(8, 2),
  k_short      SHORT,
  k_integer    INTEGER,
  k_longint    LONGINT,
  k_double     DOUBLE,
  k_curdouble  CURDOUBLE,
  k_money      MONEY,
  k_date       DATE,
  k_time       TIME,
  k_timestamp  TIMESTAMP,
  k_autoinc    AUTOINC,
  k_rowversion ROWVERSION
);
INSERT INTO keys (k_char, k_cichar, k_varchar, k_nchar, k_nvarchar, k_raw, k_guid, k_logical, k_numeric, k_short, k_integer, k_longint, k_double, k_curdouble, k_money, k_date, k_time, k_timestamp) VALUES ('abc', 'ABC', 'abc', 'abc', 'abc', X'010203', '{00112233-4455-6677-8899-AABBCCDDEEFF}', TRUE, 1.5, 1, 1, 1, 1.5, 1.5, 1.5, '2017-11-08', '12:34:56', '2017-11-08 12:34:56.789');
INSERT INTO keys (k_char, k_cichar, k_varchar, k_nchar, k_nvarchar, k_raw, k_guid, k_logical, k_numeric, k_short, k_integer, k_longint, k_double, k_curdouble, k_money, k_date, k_time, k_timestamp) VALUES ('Zz', 'zZ', 'Zz', 'Zz', 'Zz', X'FF0000', '{FFFFFFFF-0000-0000-0000-000000000001}', FALSE, -1.5, -1, -1, -1, -1.5, -1.5, -1.5, '1900-01-01', '00:00:00', '1900-01-01 00:00:00');
INSERT INTO keys (k_char, k_cichar, k_varchar, k_nchar, k_nvarchar, k_raw, k_guid, k_logical, k_numeric, k_short, k_integer, k_longint, k_double, k_curdouble, k_money, k_date, k_time, k_timestamp) VALUES ('', '', '', '', '', X'000000', '{00000000-0000-0000-0000-000000000001}', FALSE, 0, 0, 0, 0, 0, 0, 0, '0001-01-01', '23:59:59.999', '9999-12-31 23:59:59.999');
INSERT INTO keys (k_char, k_cichar, k_varchar, k_nchar, k_nvarchar, k_raw, k_guid, k_logical, k_numeric, k_short, k_integer, k_longint, k_double, k_curdouble, k_money, k_date, k_time, k_timestamp) VALUES ('Äé€', 'äÉ€', 'Äé€', 'Ωж€', 'Ωж€', X'7F8081', '{7FFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF}', TRUE, 9999.99, 32767, 2147483647, 9223372036854775807, 1e300, 1e-300, 922337203685477.5807, '9999-12-31', '00:00:00.001', '0001-01-01 00:00:00');
INSERT INTO keys (k_char, k_cichar, k_varchar, k_nchar, k_nvarchar, k_raw, k_guid, k_logical, k_numeric, k_short, k_integer, k_longint, k_double, k_curdouble, k_money, k_date, k_time, k_timestamp) VALUES ('a', 'a', 'a', 'a', 'a', X'01', '{00000000-0000-0000-0000-000000000002}', TRUE, -999.99, -32767, -2147483647, -9223372036854775807, -1e300, -1e-300, -922337203685477.5807, '1582-10-15', '12:00:00', '1970-01-01 00:00:00');
INSERT INTO keys (k_char) VALUES (NULL);
CREATE INDEX i_char ON keys (k_char);
CREATE INDEX i_cichar ON keys (k_cichar);
CREATE INDEX i_varchar ON keys (k_varchar);
CREATE INDEX i_nchar ON keys (k_nchar);
CREATE INDEX i_nvarchar ON keys (k_nvarchar);
CREATE INDEX i_raw ON keys (k_raw);
CREATE INDEX i_guid ON keys (k_guid);
CREATE INDEX i_logical ON keys (k_logical);
CREATE INDEX i_numeric ON keys (k_numeric);
CREATE INDEX i_short ON keys (k_short);
CREATE INDEX i_integer ON keys (k_integer);
CREATE INDEX i_longint ON keys (k_longint);
CREATE INDEX i_double ON keys (k_double);
CREATE INDEX i_curdouble ON keys (k_curdouble);
CREATE INDEX i_money ON keys (k_money);
CREATE INDEX i_date ON keys (k_date);
CREATE INDEX i_time ON keys (k_time);
CREATE INDEX i_timestamp ON keys (k_timestamp);
CREATE INDEX i_autoinc ON keys (k_autoinc);
CREATE INDEX i_rowversion ON keys (k_rowversion);
CREATE INDEX i_desc ON keys (k_char DESC);
CREATE INDEX i_multi ON keys (k_integer, k_char, k_date);
EXECUTE PROCEDURE sp_CreateIndex90 ('keys', 'keys', 'x_upper', 'UPPER(k_char)', NULL, 2, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('keys', 'keys', 'x_concat', 'k_char + k_varchar', NULL, 2, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('keys', 'keys', 'x_str', 'STR(k_integer, 11)', NULL, 2, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('keys', 'keys', 'x_dtos', 'DTOS(k_date)', NULL, 2, 512, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('keys', 'keys', 'x_logical', 'k_logical', NULL, 2, 512, NULL);
