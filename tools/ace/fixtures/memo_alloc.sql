-- The .adm allocator (docs/formats/adt.md, verified by tools/formats/memo.py):
-- slabs for memos of up to 32 blocks, size-class extents for larger ones, the
-- header fragment list, free-list reuse and the tail trim of a large extent.
-- REPEAT is limited to 1000 characters, so longer values are concatenated.
-- types: adt
CREATE TABLE alloc (id INTEGER, m MEMO);
-- three 32-block memos fill the 3-slot chunk-32 slab, which leaves its list
INSERT INTO alloc VALUES (1, REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000) + REPEAT('a', 1000));
INSERT INTO alloc VALUES (2, REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000) + REPEAT('b', 1000));
INSERT INTO alloc VALUES (3, REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000) + REPEAT('c', 1000));
-- 1-block memos in a chunk-1 slab; deleting both frees the slab as a 129-block extent
INSERT INTO alloc VALUES (4, 'd');
INSERT INTO alloc VALUES (5, 'e');
-- 40 blocks round up to 43 (class 38..43), split from the 98-block run at 16:
-- the remaining 55 become a 49-block extent and a 6-block fragment
INSERT INTO alloc VALUES (6, REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000) + REPEAT('f', 1000));
-- 36 blocks round up to 37; taken from the 43 freed by deleting id 6, leaving
-- a 6-block fragment; the next one splits the 49-block run and merges its
-- remainder with the adjacent fragment into an 18-block fragment
DELETE FROM alloc WHERE id = 6;
INSERT INTO alloc VALUES (7, REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000) + REPEAT('g', 1000));
INSERT INTO alloc VALUES (8, REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000) + REPEAT('h', 1000));
DELETE FROM alloc WHERE id = 4;
DELETE FROM alloc WHERE id = 5;
-- 157 blocks round up to 170 and are appended at the end of the file; freeing
-- the inner one puts it on the 149..170 class list, freeing the last one
-- rewinds header 0x00 instead
INSERT INTO alloc VALUES (9, REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000) + REPEAT('i', 1000));
INSERT INTO alloc VALUES (10, REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000) + REPEAT('j', 1000));
INSERT INTO alloc VALUES (11, REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000) + REPEAT('k', 1000));
DELETE FROM alloc WHERE id = 9;
DELETE FROM alloc WHERE id = 11;
