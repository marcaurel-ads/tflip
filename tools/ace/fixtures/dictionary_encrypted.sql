-- An RC4-encrypted data dictionary (the default encryption type) with an
-- encrypted table (table password 'tblpass') and an unencrypted one.
-- AES dictionaries cannot be made here: the engine only loads SAP's own
-- (signed) OpenSSL libeay32/ssleay32 DLLs, which none of the installers ship.
-- types: adt
CREATE DATABASE "{DIR}\rc4.add" PASSWORD 'adssyspw' ENCRYPT TRUE;
-- connect: rc4.add ADSSYS adssyspw
EXECUTE PROCEDURE sp_ModifyDatabase ('ENCRYPT_TABLE_PASSWORD', 'tblpass');
EXECUTE PROCEDURE sp_ModifyDatabase ('ENCRYPT_INDEXES', 'TRUE');
CREATE TABLE rc4_plain (id INTEGER, name CHAR(20), note MEMO);
CREATE TABLE rc4_secret (id INTEGER, name CHAR(20), note MEMO);
INSERT INTO rc4_plain VALUES (1, 'visible', 'plain memo');
INSERT INTO rc4_secret VALUES (1, 'hidden', 'secret memo');
INSERT INTO rc4_secret VALUES (2, 'also hidden', NULL);
EXECUTE PROCEDURE sp_ModifyTableProperty ('rc4_secret', 'TABLE_ENCRYPTION', 'TRUE', 'NO_VALIDATE', NULL);
CREATE INDEX by_name ON rc4_secret (name);
-- Encrypted memos: each memo value is one RC4 item numbered by its record, so
-- both of record 1's memos are item 1, including the 36-block extent that
-- replaces one of them (docs/formats/encryption.md).
CREATE TABLE rc4_memos (id INTEGER, m MEMO, n MEMO);
EXECUTE PROCEDURE sp_ModifyTableProperty ('rc4_memos', 'TABLE_ENCRYPTION', 'TRUE', 'NO_VALIDATE', NULL);
INSERT INTO rc4_memos VALUES (1, 'first memo', 'second memo');
INSERT INTO rc4_memos VALUES (2, 'third memo', REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000) + REPEAT('D', 1000));
INSERT INTO rc4_memos VALUES (3, 'fifth memo', 'sixth memo');
UPDATE rc4_memos SET n = REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) + REPEAT('G', 1000) WHERE id = 1;
