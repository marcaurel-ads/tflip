-- The ADT deleted-record free list: a LIFO singly-linked stack threaded
-- through the 4 bytes after each record's status byte, with its head at header
-- 0x28 and its length at 0x30 (see docs/formats/adt.md). Verified raw, not
-- through adsdump, by tools/formats/freelist.py, because the engine never shows
-- deleted ADT records.
-- types: adt

-- fl_chain: eight records, then delete 3, 6, 1 in that order. Each delete
-- pushes its slot onto the head, so the chain is 1 -> 6 -> 3 -> 0 (reverse of
-- the deletion order) with head = 1 and count = 3. The free list is left
-- populated (no reinsert), so the committed .adt keeps the whole chain.
CREATE TABLE fl_chain (c_id INTEGER, c_name CHAR(12));
INSERT INTO fl_chain VALUES (1, 'one');
INSERT INTO fl_chain VALUES (2, 'two');
INSERT INTO fl_chain VALUES (3, 'three');
INSERT INTO fl_chain VALUES (4, 'four');
INSERT INTO fl_chain VALUES (5, 'five');
INSERT INTO fl_chain VALUES (6, 'six');
INSERT INTO fl_chain VALUES (7, 'seven');
INSERT INTO fl_chain VALUES (8, 'eight');
DELETE FROM fl_chain WHERE c_id = 3;
DELETE FROM fl_chain WHERE c_id = 6;
DELETE FROM fl_chain WHERE c_id = 1;

-- fl_reuse: the same, then insert two rows. The engine pops the free-list head
-- each time (LIFO), so slot 1 then slot 6 are reused in place before any slot
-- is appended: the dump shows record 1 = 101 and record 6 = 102, and slot 3 is
-- still on the free list (head = 3, count = 1). A third and fourth insert would
-- reuse slot 3 and then append record 9.
CREATE TABLE fl_reuse (c_id INTEGER, c_name CHAR(12));
INSERT INTO fl_reuse VALUES (1, 'one');
INSERT INTO fl_reuse VALUES (2, 'two');
INSERT INTO fl_reuse VALUES (3, 'three');
INSERT INTO fl_reuse VALUES (4, 'four');
INSERT INTO fl_reuse VALUES (5, 'five');
INSERT INTO fl_reuse VALUES (6, 'six');
INSERT INTO fl_reuse VALUES (7, 'seven');
INSERT INTO fl_reuse VALUES (8, 'eight');
DELETE FROM fl_reuse WHERE c_id = 3;
DELETE FROM fl_reuse WHERE c_id = 6;
DELETE FROM fl_reuse WHERE c_id = 1;
INSERT INTO fl_reuse VALUES (101, 'reuse-head');
INSERT INTO fl_reuse VALUES (102, 'reuse-next');
