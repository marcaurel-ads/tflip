-- ModTime is set to the current time on every write, so its value (and the
-- record bytes holding it) differ from run to run; tests must not compare them.
-- types: adt
CREATE TABLE modtime (id INTEGER, changed MODTIME, rv ROWVERSION);
INSERT INTO modtime (id) VALUES (1);
INSERT INTO modtime (id) VALUES (2);
UPDATE modtime SET id = 3 WHERE id = 2;
