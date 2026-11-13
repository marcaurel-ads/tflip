-- More Visual FoxPro field types: the Unicode types, LongInt, GUID, NOCPTRANS
-- memos, and VarChar/VarBinary values shorter than, equal to and longer than
-- their field length's worth of padding, NULL and not NULL, to show how the
-- _NullFlags field records NULLs and variable lengths.
-- types: vfp
CREATE TABLE vfp_more (v_vchar VARCHAR(4) NULL, v_vbin VARBINARY(4) NULL, v_nchar NCHAR(3) NULL, v_nvchar NVARCHAR(3) NULL, v_nmemo NMEMO NULL, v_long LONGINT NULL, v_guid GUID NULL, v_memo MEMO NOCPTRANS NULL, v_int INTEGER, v_vc2 VARCHAR(3));
INSERT INTO vfp_more VALUES ('a', X'01', 'a', 'a', 'n', 1, '{00112233-4455-6677-8899-AABBCCDDEEFF}', 'raw memo €', 1, 'x');
INSERT INTO vfp_more VALUES ('abcd', X'01020304', 'abc', 'abc', REPEAT('Ω', 100), -1, NULL, NULL, 2, 'xyz');
INSERT INTO vfp_more VALUES ('', X'', '', '', '', 0, '{00000000-0000-0000-0000-000000000000}', '', 3, '');
INSERT INTO vfp_more (v_int) VALUES (4);
