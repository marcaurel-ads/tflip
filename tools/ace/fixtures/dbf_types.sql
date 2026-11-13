-- The standard DBF field types, available in every DBF flavour.
-- types: ntx cdx vfp
CREATE TABLE dbf_types (c_char CHAR(10), c_numeric NUMERIC(10, 2), c_date DATE, c_logical LOGICAL, c_memo MEMO);
INSERT INTO dbf_types VALUES ('abc', 12345.67, '2017-11-08', TRUE, 'memo text');
INSERT INTO dbf_types VALUES ('äöü€', -99999.99, '0001-01-01', FALSE, REPEAT('0123456789', 60));
INSERT INTO dbf_types (c_char) VALUES ('empty');
INSERT INTO dbf_types VALUES ('deleted', 1, '2000-01-01', TRUE, 'gone');
DELETE FROM dbf_types WHERE c_char = 'deleted';
