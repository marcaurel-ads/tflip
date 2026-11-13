-- Visual FoxPro field types, including NULL-able columns (the _NullFlags field).
-- DBF field names are limited to 10 characters.
-- types: vfp
CREATE TABLE vfp_types (
  c_char       CHAR(10) NULL,
  c_varchar    VARCHAR(20) NULL,
  c_varbin  VARBINARY(8) NULL,
  c_numeric    NUMERIC(10, 2) NULL,
  c_integer    INTEGER NULL,
  c_double     DOUBLE NULL,
  c_money      MONEY NULL,
  c_date       DATE NULL,
  c_tstamp  TIMESTAMP NULL,
  c_logical    LOGICAL NULL,
  c_memo       MEMO NULL,
  c_blob       BLOB NULL,
  c_autoinc    AUTOINC,
  c_nochar     CHAR(10) NOCPTRANS
);
INSERT INTO vfp_types (c_char, c_varchar, c_varbin, c_numeric, c_integer, c_double, c_money, c_date, c_tstamp, c_logical, c_memo, c_blob, c_nochar)
VALUES ('abc', 'var', X'0102', 12345.67, 123456789, 3.5, 12.3456, '2017-11-08', '2017-11-08 13:45:30', TRUE, 'memo text', X'00FF10', 'raw€');
INSERT INTO vfp_types (c_char) VALUES (NULL);
INSERT INTO vfp_types (c_char, c_varchar) VALUES ('', '');
