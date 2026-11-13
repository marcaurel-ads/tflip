-- A data dictionary (dict.add, .am, .ai) with bound tables, column
-- constraints, an index in a separate file, a view, a stored procedure, a
-- function, a trigger, a user and a group with permissions.
-- types: adt
CREATE DATABASE "{DIR}\dict.add" PASSWORD 'adssyspw' DESCRIPTION 'Fixture dictionary';
-- connect: dict.add ADSSYS adssyspw
CREATE TABLE customers (id AUTOINC CONSTRAINT pk_customers PRIMARY KEY, name CHAR(30) CONSTRAINT NOT NULL DEFAULT '''unknown''', credit MONEY CONSTRAINT MINIMUM '0' CONSTRAINT MAXIMUM '10000', note MEMO);
CREATE TABLE orders (id AUTOINC PRIMARY KEY, customer INTEGER, total MONEY, placed DATE);
CREATE INDEX by_customer ON orders (customer) IN FILE orders_extra;
INSERT INTO customers (name, credit, note) VALUES ('Alice', 100, 'first customer');
INSERT INTO customers (credit) VALUES (250.5);
INSERT INTO orders (customer, total, placed) VALUES (1, 19.99, '2017-11-08');
CREATE VIEW big_orders AS SELECT * FROM orders WHERE total > 10;
CREATE FUNCTION double_it (x INTEGER) RETURNS INTEGER BEGIN RETURN x * 2; END;
CREATE PROCEDURE add_order (cust INTEGER, amount MONEY) BEGIN INSERT INTO orders (customer, total, placed) SELECT cust, amount, CURDATE() FROM __input; END;
CREATE TRIGGER orders_credit ON orders AFTER INSERT BEGIN UPDATE customers SET credit = credit - (SELECT total FROM __new) WHERE id = (SELECT customer FROM __new); END;
EXECUTE PROCEDURE add_order (2, 12.5);
EXECUTE PROCEDURE sp_CreateGroup ('staff', 'Fixture group');
EXECUTE PROCEDURE sp_CreateUser ('alice', 'secret', 'Fixture user');
EXECUTE PROCEDURE sp_AddUserToGroup ('alice', 'staff');
GRANT SELECT ON customers TO staff;
