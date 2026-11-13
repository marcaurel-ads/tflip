-- Dynamic (VFP-compatible) collations from adscollate.adt: one table per
-- collation, created with it, holding the characters 32-255 (CHAR(n)) and
-- some words, with an index on each column. CZECH has contractions (ch).
-- types: adt vfp
-- chartype: GENERAL_VFP_CI_AS_1252
CREATE TABLE gen (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO gen VALUES (i, CHAR(i), ''); i = i + 1; END WHILE;
INSERT INTO gen VALUES (1000, '', 'resume');
INSERT INTO gen VALUES (1001, '', 'resumé');
INSERT INTO gen VALUES (1002, '', 'résumé');
INSERT INTO gen VALUES (1003, '', 'resumes');
INSERT INTO gen VALUES (1004, '', 'Äpfel');
INSERT INTO gen VALUES (1005, '', 'apfel');
INSERT INTO gen VALUES (1006, '', 'Apfel');
INSERT INTO gen VALUES (1007, '', 'Zebra');
INSERT INTO gen VALUES (1008, '', 'zebra');
INSERT INTO gen VALUES (1009, '', 'ß');
INSERT INTO gen VALUES (1010, '', 'ss');
INSERT INTO gen VALUES (1011, '', 'c');
INSERT INTO gen VALUES (1012, '', 'ch');
INSERT INTO gen VALUES (1013, '', 'cha');
INSERT INTO gen VALUES (1014, '', 'cz');
INSERT INTO gen VALUES (1015, '', 'h');
INSERT INTO gen VALUES (1016, '', 'hz');
INSERT INTO gen VALUES (1017, '', 'i');
INSERT INTO gen VALUES (1018, '', 'ae');
INSERT INTO gen VALUES (1019, '', 'æ');
EXECUTE PROCEDURE sp_CreateIndex90 ('gen', 'gen', 'gen_c', 'c', NULL, 2, 512, 'GENERAL_VFP_CI_AS_1252');
EXECUTE PROCEDURE sp_CreateIndex90 ('gen', 'gen', 'gen_w', 'w', NULL, 2, 512, 'GENERAL_VFP_CI_AS_1252');
-- chartype: GERMAN_VFP_CI_AS_1252
CREATE TABLE ger (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO ger VALUES (i, CHAR(i), ''); i = i + 1; END WHILE;
INSERT INTO ger VALUES (1000, '', 'resume');
INSERT INTO ger VALUES (1001, '', 'resumé');
INSERT INTO ger VALUES (1002, '', 'résumé');
INSERT INTO ger VALUES (1003, '', 'resumes');
INSERT INTO ger VALUES (1004, '', 'Äpfel');
INSERT INTO ger VALUES (1005, '', 'apfel');
INSERT INTO ger VALUES (1006, '', 'Apfel');
INSERT INTO ger VALUES (1007, '', 'Zebra');
INSERT INTO ger VALUES (1008, '', 'zebra');
INSERT INTO ger VALUES (1009, '', 'ß');
INSERT INTO ger VALUES (1010, '', 'ss');
INSERT INTO ger VALUES (1011, '', 'c');
INSERT INTO ger VALUES (1012, '', 'ch');
INSERT INTO ger VALUES (1013, '', 'cha');
INSERT INTO ger VALUES (1014, '', 'cz');
INSERT INTO ger VALUES (1015, '', 'h');
INSERT INTO ger VALUES (1016, '', 'hz');
INSERT INTO ger VALUES (1017, '', 'i');
INSERT INTO ger VALUES (1018, '', 'ae');
INSERT INTO ger VALUES (1019, '', 'æ');
EXECUTE PROCEDURE sp_CreateIndex90 ('ger', 'ger', 'ger_c', 'c', NULL, 2, 512, 'GERMAN_VFP_CI_AS_1252');
EXECUTE PROCEDURE sp_CreateIndex90 ('ger', 'ger', 'ger_w', 'w', NULL, 2, 512, 'GERMAN_VFP_CI_AS_1252');
-- chartype: MACHINE_VFP_BIN_1252
CREATE TABLE mac (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO mac VALUES (i, CHAR(i), ''); i = i + 1; END WHILE;
INSERT INTO mac VALUES (1000, '', 'resume');
INSERT INTO mac VALUES (1001, '', 'resumé');
INSERT INTO mac VALUES (1002, '', 'résumé');
INSERT INTO mac VALUES (1003, '', 'resumes');
INSERT INTO mac VALUES (1004, '', 'Äpfel');
INSERT INTO mac VALUES (1005, '', 'apfel');
INSERT INTO mac VALUES (1006, '', 'Apfel');
INSERT INTO mac VALUES (1007, '', 'Zebra');
INSERT INTO mac VALUES (1008, '', 'zebra');
INSERT INTO mac VALUES (1009, '', 'ß');
INSERT INTO mac VALUES (1010, '', 'ss');
INSERT INTO mac VALUES (1011, '', 'c');
INSERT INTO mac VALUES (1012, '', 'ch');
INSERT INTO mac VALUES (1013, '', 'cha');
INSERT INTO mac VALUES (1014, '', 'cz');
INSERT INTO mac VALUES (1015, '', 'h');
INSERT INTO mac VALUES (1016, '', 'hz');
INSERT INTO mac VALUES (1017, '', 'i');
INSERT INTO mac VALUES (1018, '', 'ae');
INSERT INTO mac VALUES (1019, '', 'æ');
EXECUTE PROCEDURE sp_CreateIndex90 ('mac', 'mac', 'mac_c', 'c', NULL, 2, 512, 'MACHINE_VFP_BIN_1252');
EXECUTE PROCEDURE sp_CreateIndex90 ('mac', 'mac', 'mac_w', 'w', NULL, 2, 512, 'MACHINE_VFP_BIN_1252');
-- chartype: UNIQWT_VFP_CS_AS_1252
CREATE TABLE uni (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO uni VALUES (i, CHAR(i), ''); i = i + 1; END WHILE;
INSERT INTO uni VALUES (1000, '', 'resume');
INSERT INTO uni VALUES (1001, '', 'resumé');
INSERT INTO uni VALUES (1002, '', 'résumé');
INSERT INTO uni VALUES (1003, '', 'resumes');
INSERT INTO uni VALUES (1004, '', 'Äpfel');
INSERT INTO uni VALUES (1005, '', 'apfel');
INSERT INTO uni VALUES (1006, '', 'Apfel');
INSERT INTO uni VALUES (1007, '', 'Zebra');
INSERT INTO uni VALUES (1008, '', 'zebra');
INSERT INTO uni VALUES (1009, '', 'ß');
INSERT INTO uni VALUES (1010, '', 'ss');
INSERT INTO uni VALUES (1011, '', 'c');
INSERT INTO uni VALUES (1012, '', 'ch');
INSERT INTO uni VALUES (1013, '', 'cha');
INSERT INTO uni VALUES (1014, '', 'cz');
INSERT INTO uni VALUES (1015, '', 'h');
INSERT INTO uni VALUES (1016, '', 'hz');
INSERT INTO uni VALUES (1017, '', 'i');
INSERT INTO uni VALUES (1018, '', 'ae');
INSERT INTO uni VALUES (1019, '', 'æ');
EXECUTE PROCEDURE sp_CreateIndex90 ('uni', 'uni', 'uni_c', 'c', NULL, 2, 512, 'UNIQWT_VFP_CS_AS_1252');
EXECUTE PROCEDURE sp_CreateIndex90 ('uni', 'uni', 'uni_w', 'w', NULL, 2, 512, 'UNIQWT_VFP_CS_AS_1252');
-- chartype: CZECH_VFP_CI_AS_1250
CREATE TABLE cze (code INTEGER, c CHAR(1), w CHAR(10));
DECLARE i INTEGER; i = 32; WHILE i < 256 DO INSERT INTO cze VALUES (i, CHAR(i), ''); i = i + 1; END WHILE;
INSERT INTO cze VALUES (1000, '', 'resume');
INSERT INTO cze VALUES (1003, '', 'resumes');
INSERT INTO cze VALUES (1005, '', 'apfel');
INSERT INTO cze VALUES (1006, '', 'Apfel');
INSERT INTO cze VALUES (1007, '', 'Zebra');
INSERT INTO cze VALUES (1008, '', 'zebra');
INSERT INTO cze VALUES (1010, '', 'ss');
INSERT INTO cze VALUES (1011, '', 'c');
INSERT INTO cze VALUES (1012, '', 'ch');
INSERT INTO cze VALUES (1013, '', 'cha');
INSERT INTO cze VALUES (1014, '', 'cz');
INSERT INTO cze VALUES (1015, '', 'h');
INSERT INTO cze VALUES (1016, '', 'hz');
INSERT INTO cze VALUES (1017, '', 'i');
INSERT INTO cze VALUES (1018, '', 'ae');
EXECUTE PROCEDURE sp_CreateIndex90 ('cze', 'cze', 'cze_c', 'c', NULL, 2, 512, 'CZECH_VFP_CI_AS_1250');
EXECUTE PROCEDURE sp_CreateIndex90 ('cze', 'cze', 'cze_w', 'w', NULL, 2, 512, 'CZECH_VFP_CI_AS_1250');
-- dump: gen -c GENERAL_VFP_CI_AS_1252
-- dump: ger -c GERMAN_VFP_CI_AS_1252
-- dump: mac -c MACHINE_VFP_BIN_1252
-- dump: uni -c UNIQWT_VFP_CS_AS_1252
-- dump: cze -c CZECH_VFP_CI_AS_1250
