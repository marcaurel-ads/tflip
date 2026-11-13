-- Every ADT field type except ModTime (see modtime.sql), with typical,
-- boundary and NULL values, a deleted record and a reused record slot.
-- types: adt
CREATE TABLE all_types (
  c_char       CHAR(10),
  c_cichar     CICHAR(10),
  c_varchar    VARCHAR(20),
  c_nchar      NCHAR(5),
  c_nvarchar   NVARCHAR(10),
  c_memo       MEMO,
  c_nmemo      NMEMO,
  c_blob       BLOB,
  c_raw        RAW(4),
  c_varbinary  VARBINARY(8),
  c_guid       GUID,
  c_logical    LOGICAL,
  c_numeric    NUMERIC(10, 2),
  c_short      SHORT,
  c_integer    INTEGER,
  c_longint    LONGINT,
  c_double     DOUBLE,
  c_curdouble  CURDOUBLE,
  c_money      MONEY,
  c_date       DATE,
  c_time       TIME,
  c_timestamp  TIMESTAMP,
  c_autoinc    AUTOINC,
  c_rowversion ROWVERSION
);
INSERT INTO all_types (c_char, c_cichar, c_varchar, c_nchar, c_nvarchar, c_memo, c_nmemo,
  c_blob, c_raw, c_varbinary, c_guid, c_logical, c_numeric, c_short, c_integer, c_longint,
  c_double, c_curdouble, c_money, c_date, c_time, c_timestamp)
VALUES ('abc', 'MiXeD', 'var', 'ä€', 'Ω✓', 'memo text', 'nmemo ünïcødé ✓',
  X'00FF10', X'DEADBEEF', X'0102', '{01234567-89AB-CDEF-0123-456789ABCDEF}', TRUE, 12345.67,
  -2, 123456789, 1234567890123, 3.5, 2.25, 12.3456, '2017-11-08', '13:45:30.250',
  '2017-11-08 13:45:30.250');
INSERT INTO all_types (c_char, c_logical, c_numeric, c_short, c_integer, c_longint, c_double,
  c_money, c_date, c_time, c_timestamp)
VALUES ('zzzzzzzzzz', FALSE, -99999.99, 32767, -2147483647, -9223372036854775807, -1.0E308,
  -922337203685477.5807, '0001-01-01', '00:00:00', '9999-12-31 23:59:59.999');
INSERT INTO all_types (c_char) VALUES (NULL);
INSERT INTO all_types (c_char, c_varchar) VALUES ('äöü€', '');
INSERT INTO all_types (c_char, c_integer) VALUES ('deleted', 42);
DELETE FROM all_types WHERE c_char = 'deleted';
INSERT INTO all_types (c_char, c_memo) VALUES ('reuse', 'placed in the deleted record''s slot?');
