-- Character types and static collations with the default Local Server settings
-- (ANSI_CHAR_SET empty: the workstation's ANSI collation; OEM_CHAR_SET=USA).
-- Table ca is ANSI, co is OEM (ADS_OEM); each holds the characters 32-255
-- (CHAR(n)) and some words, with an index on each column.
-- types: adt ntx cdx
CREATE TABLE ca (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO ca VALUES (i, CHAR(i), NULL); i = i + 1; END WHILE;
INSERT INTO ca VALUES (1000, NULL, 'resume');
INSERT INTO ca VALUES (1001, NULL, 'resumé');
INSERT INTO ca VALUES (1002, NULL, 'resumes');
INSERT INTO ca VALUES (1003, NULL, 'Äpfel');
INSERT INTO ca VALUES (1004, NULL, 'apfel');
INSERT INTO ca VALUES (1005, NULL, 'Apfel');
INSERT INTO ca VALUES (1006, NULL, 'Zebra');
INSERT INTO ca VALUES (1007, NULL, 'zebra');
INSERT INTO ca VALUES (1008, NULL, 'ß');
INSERT INTO ca VALUES (1009, NULL, 'ss');
INSERT INTO ca VALUES (1010, NULL, 'öre');
INSERT INTO ca VALUES (1011, NULL, 'Ångström');
INSERT INTO ca VALUES (1012, NULL, 'naïve');
INSERT INTO ca VALUES (1013, NULL, 'ñandú');
CREATE INDEX ca_c ON ca (c);
CREATE INDEX ca_w ON ca (w);
-- chartype: oem
CREATE TABLE co (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO co VALUES (i, CHAR(i), NULL); i = i + 1; END WHILE;
INSERT INTO co VALUES (1000, NULL, 'resume');
INSERT INTO co VALUES (1001, NULL, 'resumé');
INSERT INTO co VALUES (1002, NULL, 'resumes');
INSERT INTO co VALUES (1003, NULL, 'Äpfel');
INSERT INTO co VALUES (1004, NULL, 'apfel');
INSERT INTO co VALUES (1005, NULL, 'Apfel');
INSERT INTO co VALUES (1006, NULL, 'Zebra');
INSERT INTO co VALUES (1007, NULL, 'zebra');
INSERT INTO co VALUES (1008, NULL, 'ß');
INSERT INTO co VALUES (1009, NULL, 'ss');
INSERT INTO co VALUES (1010, NULL, 'öre');
INSERT INTO co VALUES (1011, NULL, 'Ångström');
INSERT INTO co VALUES (1012, NULL, 'naïve');
INSERT INTO co VALUES (1013, NULL, 'ñandú');
CREATE INDEX co_c ON co (c);
CREATE INDEX co_w ON co (w);
-- dump: ca -x ca_c.ntx -x ca_w.ntx
-- dump: co -t oem -x co_c.ntx -x co_w.ntx
