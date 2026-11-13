-- The orders of ntx_expr.sql on CDX and ADT tables: numeric and date
-- expression results are 8-byte keys whatever the expression, so the NTX
-- orders that cannot be navigated work here.
-- types: cdx adt
CREATE TABLE expr (a NUMERIC(6, 0), b NUMERIC(8, 2), i INTEGER, d DATE, l LOGICAL, c CHAR(8), w CHAR(14));
INSERT INTO expr VALUES (12, 3.25, 7, '2017-11-08', TRUE, '42', '123456.5');
INSERT INTO expr VALUES (-5, -1.5, -3, '1900-01-01', FALSE, '-7.5', '-1');
INSERT INTO expr VALUES (0, 0, 0, '2000-02-29', FALSE, '', '');
INSERT INTO expr VALUES (99999, 9999.99, 2147483647, '9999-12-31', TRUE, '1e3', '99999999999');
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_add', 'a+b', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_add0', 'a+a', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_sub1', 'a-1', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_addlit', 'a+1.5', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_mul', 'a*b', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_mulbb', 'b*b', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_div', 'a/4', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_int', 'i+1', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_imul', 'i*2', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr', 'expr', 'e_ai', 'a+i', NULL, 2, 0, NULL);
CREATE TABLE expr2 (a NUMERIC(6, 0), b NUMERIC(8, 2), i INTEGER, d DATE, l LOGICAL, c CHAR(8), w CHAR(14));
INSERT INTO expr2 VALUES (12, 3.25, 7, '2017-11-08', TRUE, '42', '123456.5');
INSERT INTO expr2 VALUES (-5, -1.5, -3, '1900-01-01', FALSE, '-7.5', '-1');
INSERT INTO expr2 VALUES (0, 0, 0, '2000-02-29', FALSE, '', '');
INSERT INTO expr2 VALUES (99999, 9999.99, 2147483647, '9999-12-31', TRUE, '1e3', '99999999999');
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_recno', 'RECNO()', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_year', 'YEAR(d)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_len', 'LEN(c)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_val', 'VAL(c)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_valw', 'VAL(w)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_round', 'ROUND(b,1)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_roundi', 'ROUND(a,0)', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_dplus', 'd+1', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_cmp', 'a=b', NULL, 2, 0, NULL);
EXECUTE PROCEDURE sp_CreateIndex90 ('expr2', 'expr2', 'e_notl', '!l', NULL, 2, 0, NULL);
