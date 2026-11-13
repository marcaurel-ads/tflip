-- ADI indexes: primary key, single-field, descending, compound and unique
-- (case-insensitive) orders in the auto-open index file, and a second table
-- whose index uses 8 KB pages. Conditional indexes need AdsCreateIndex61, and
-- separate index files (IN FILE) need a data dictionary; SQL on free tables
-- cannot make them.
-- types: adt
CREATE TABLE indexed (id INTEGER PRIMARY KEY, name CHAR(20), ciname CICHAR(20), amount MONEY, born DATE, active LOGICAL);
INSERT INTO indexed VALUES (3, 'Charlie', 'charlie', 30.5, '1990-03-03', TRUE);
INSERT INTO indexed VALUES (1, 'alpha', 'ALPHA', 10.25, '1970-01-01', FALSE);
INSERT INTO indexed VALUES (2, 'Bravo', 'Bravo', -5, '1985-12-24', TRUE);
INSERT INTO indexed VALUES (5, 'echo', 'Echo', 0, NULL, NULL);
INSERT INTO indexed VALUES (4, 'Delta', 'delta', 99999.9999, '2000-02-29', TRUE);
CREATE INDEX by_name ON indexed (name);
CREATE INDEX by_amount_desc ON indexed (amount DESC);
CREATE INDEX by_born_name ON indexed (born, name);
CREATE UNIQUE INDEX by_ciname ON indexed (ciname);
CREATE TABLE pages8k (name CHAR(20));
INSERT INTO pages8k SELECT name FROM indexed;
CREATE INDEX by_name ON pages8k (name) PAGESIZE 8192;
DELETE FROM indexed WHERE id = 5;
