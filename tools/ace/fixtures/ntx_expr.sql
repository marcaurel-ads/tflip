-- Key widths of numeric, date and logical expression results. The engine fixes
-- an NTX key's width and decimals when the order is created, from the value on
-- the current record, here record 1. Every order is built on existing rows; two
-- identical tables because a work area opens at most 15 index files (5045).
-- e_mul (a*b), e_ai (a+i) and e_valw (VAL(w)) are kept as files but not dumped:
-- a later row's value has a different width (above 1410065407, or other
-- decimals), and navigating to it fails with 4004. IIF(l,a,b) is left out for
-- the same error. The same orders on CDX and ADT: expr_widths.sql.
-- types: ntx
CREATE TABLE expr (a NUMERIC(6, 0), b NUMERIC(8, 2), i INTEGER, d DATE, l LOGICAL, c CHAR(8), w CHAR(14));
INSERT INTO expr VALUES (12, 3.25, 7, '2017-11-08', TRUE, '42', '123456.5');
INSERT INTO expr VALUES (-5, -1.5, -3, '1900-01-01', FALSE, '-7.5', '-1');
INSERT INTO expr VALUES (0, 0, 0, '2000-02-29', FALSE, '', '');
INSERT INTO expr VALUES (999999, 9999.99, 2147483647, '9999-12-31', TRUE, '1e3', '99999999999');
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_add', NULL, 'a+b', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_add0', NULL, 'a+a', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_sub1', NULL, 'a-1', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_addlit', NULL, 'a+1.5', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_mul', NULL, 'a*b', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_mulbb', NULL, 'b*b', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_div', NULL, 'a/4', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_int', NULL, 'i+1', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_imul', NULL, 'i*2', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'e_ai', NULL, 'a+i', NULL, 0, 0, NULL);
CREATE TABLE expr2 (a NUMERIC(6, 0), b NUMERIC(8, 2), i INTEGER, d DATE, l LOGICAL, c CHAR(8), w CHAR(14));
INSERT INTO expr2 VALUES (12, 3.25, 7, '2017-11-08', TRUE, '42', '123456.5');
INSERT INTO expr2 VALUES (-5, -1.5, -3, '1900-01-01', FALSE, '-7.5', '-1');
INSERT INTO expr2 VALUES (0, 0, 0, '2000-02-29', FALSE, '', '');
INSERT INTO expr2 VALUES (999999, 9999.99, 2147483647, '9999-12-31', TRUE, '1e3', '99999999999');
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_recno', NULL, 'RECNO()', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_year', NULL, 'YEAR(d)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_len', NULL, 'LEN(c)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_val', NULL, 'VAL(c)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_valw', NULL, 'VAL(w)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_round', NULL, 'ROUND(b,1)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_roundi', NULL, 'ROUND(a,0)', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_dplus', NULL, 'd+1', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_cmp', NULL, 'a=b', NULL, 0, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'e_notl', NULL, '!l', NULL, 0, 0, NULL);
-- dump: expr -x e_add.ntx -x e_add0.ntx -x e_sub1.ntx -x e_addlit.ntx -x e_mulbb.ntx -x e_div.ntx -x e_int.ntx -x e_imul.ntx
-- dump: expr2 -x e_recno.ntx -x e_year.ntx -x e_len.ntx -x e_val.ntx -x e_round.ntx -x e_roundi.ntx -x e_dplus.ntx -x e_cmp.ntx -x e_notl.ntx
