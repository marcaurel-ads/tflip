/* Probes what AdsCacheOpenTables, AdsCacheOpenCursors and
 * AdsCloseCachedTables do: their answers, and whether a table or cursor
 * closed while caching is on stays open on the server (AdsMgGetOpenTables,
 * another connection's exclusive open, DROP TABLE from another connection).
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o cache.exe cache.c
 *     wine cache.exe <ace32.dll> <folder>                        (local server)
 *     wine cache.exe <ace32.dll> '\\127.0.0.1:6262\data\cache' 2  (remote)
 *
 * The folder must exist and be empty. Prints one line per call. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
static char dir[MAX_PATH];
static U16 server_type = 1;
#define F(name) ((void *)GetProcAddress(ace, name))

static U32 err(const char *what, U32 rc) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    text[0] = 0;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    else len = 0;
    printf("%-50s rc=%lu %.*s\n", what, rc, (int)(len > 90 ? 90 : len), text);
    return rc;
}

/* A remote connection goes over TCP (AdsConnect101), as the evaluation
 * server under Wine takes no UDP. */
static H conn(void) {
    H c = 0;
    if (server_type == 2) {
        char cs[MAX_PATH + 64];
        snprintf(cs, sizeof cs, "Data Source=%s;ServerType=REMOTE;CommType=TCP_IP", dir);
        err("AdsConnect101",
            ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &c));
    } else
        err("AdsConnect60", ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                                (U8 *)dir, server_type, NULL, NULL, 0, &c));
    return c;
}

static void sql(H c, const char *text) {
    H s = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    char what[80];
    snprintf(what, sizeof what, "  sql %.44s", text);
    err(what, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur));
    if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
}

static H open_t(H c, const char *name, U32 mode) {
    H t = 0;
    char path[MAX_PATH], what[80];
    snprintf(path, sizeof path, "%s\\%s", dir, name);
    snprintf(what, sizeof what, "  open %s %s", name, mode == 1 ? "excl" : "shared");
    err(what, ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                  c, (U8 *)path, NULL, 3, 1, 1, 2, mode, &t));
    return t;
}

static void close_t(H t, const char *label) {
    char what[80];
    snprintf(what, sizeof what, "  close %s", label);
    err(what, ((U32(WINAPI *)(H))F("AdsCloseTable"))(t));
}

static void list(H mg, const char *label) {
    U8 buf[262 * 32];
    U16 n = 32, size = 262;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, U8 *, U16 *, U16 *))F("AdsMgGetOpenTables"))(
        mg, NULL, 0, buf, &n, &size);
    printf("  [%s] MgGetOpenTables rc=%lu n=%u:", label, rc, rc ? 0 : n);
    for (U16 i = 0; !rc && i < n; i++) {
        const char *name = (const char *)buf + i * size;
        const char *base = strrchr(name, '\\');
        printf(" %s", base ? base + 1 : name);
    }
    printf("\n");
}

static U32 count(H t) {
    U32 n = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1 /*ADS_IGNOREFILTERS*/, &n);
    return n;
}

static H query(H c, const char *text, H *stmt) {
    H s = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    char what[80];
    snprintf(what, sizeof what, "  query %.42s", text);
    err(what, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur));
    if (cur) printf("    rows=%lu\n", count(cur));
    *stmt = s;
    return cur;
}

/* Runs `text` again on the statement `s`; the cursor, if any. */
static H requery(H s, const char *text) {
    H cur = 0;
    char what[80];
    snprintf(what, sizeof what, "  requery %.40s", text);
    err(what, ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur));
    if (cur) printf("    rows=%lu\n", count(cur));
    return cur;
}

static void excl(H c) {
    H t = open_t(c, "t.adt", 1);
    if (t) close_t(t, "t (excl)");
}

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) { printf("no dll\n"); return 1; }
    snprintf(dir, sizeof dir, "%s", argv[2]);
    if (argc > 3) server_type = (U16)atoi(argv[3]);
    setvbuf(stdout, NULL, _IONBF, 0);
    U32(WINAPI * cache_tables)(U16) = F("AdsCacheOpenTables");
    U32(WINAPI * cache_cursors)(U16) = F("AdsCacheOpenCursors");
    U32(WINAPI * close_cached)(H) = F("AdsCloseCachedTables");

    {
        printf("== defaults (nothing set)\n");
        H d1 = conn(), d2 = conn(), dm = conn(), ds = 0;
        sql(d1, "CREATE TABLE d (a INTEGER)");
        H d = open_t(d1, "d.adt", 4);
        close_t(d, "d");
        list(dm, "d closed");
        H dc = query(d1, "SELECT * FROM d WHERE a > 0", &ds);
        close_t(dc, "cursor");
        err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(ds));
        list(dm, "statement closed");
        d = open_t(d2, "d.adt", 1);
        if (d) close_t(d, "d (excl)");
        err("DROP via d2", 0);
        sql(d2, "DROP TABLE d");
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(d1);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(d2);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(dm);
    }

    printf("== answers before any connection\n");
    U16 values[] = {0, 1, 5, 250, 65535};
    for (int i = 0; i < 5; i++) {
        char what[60];
        snprintf(what, sizeof what, "AdsCacheOpenTables(%u)", values[i]);
        err(what, cache_tables(values[i]));
        snprintf(what, sizeof what, "AdsCacheOpenCursors(%u)", values[i]);
        err(what, cache_cursors(values[i]));
    }
    err("AdsCloseCachedTables(0)", close_cached(0));
    /* A made-up handle crashes the original (a read through it). */
    cache_tables(0);
    cache_cursors(0);

    printf("== setup\n");
    H c1 = conn(), c2 = conn();
    err("AdsCloseCachedTables(c1)", close_cached(c1));
    /* The management calls take a connection handle; a third connection
     * lists what the server has open. */
    H mg = conn();
    sql(c1, "CREATE TABLE t (a INTEGER)");
    sql(c1, "INSERT INTO t VALUES (1)");
    sql(c1, "CREATE TABLE u (a INTEGER)");
    sql(c1, "CREATE TABLE v (a INTEGER)");
    sql(c1, "CREATE TABLE w (a INTEGER)");
    list(mg, "after setup");

    printf("== tables, cache 0\n");
    H t = open_t(c1, "t.adt", 4);
    list(mg, "t open");
    close_t(t, "t");
    list(mg, "t closed");
    excl(c2);

    printf("== tables, cache 2\n");
    err("AdsCacheOpenTables(2)", cache_tables(2));
    t = open_t(c1, "t.adt", 4);
    close_t(t, "t");
    list(mg, "t closed, cache 2");
    excl(c2);
    excl(c1);
    list(mg, "after excl");
    t = open_t(c1, "t.adt", 4);
    close_t(t, "t");
    H u = open_t(c1, "u.adt", 4);
    close_t(u, "u");
    H v = open_t(c1, "v.adt", 4);
    close_t(v, "v");
    list(mg, "t,u,v closed, cache 2");
    H w = open_t(c1, "w.adt", 4);
    list(mg, "w open");
    close_t(w, "w");
    list(mg, "w closed");
    sql(c2, "INSERT INTO t VALUES (2)");
    t = open_t(c1, "t.adt", 4);
    printf("    t rows=%lu\n", count(t));
    close_t(t, "t");
    sql(c2, "DROP TABLE w");
    list(mg, "after drop w");
    err("AdsCloseCachedTables(c2)", close_cached(c2));
    list(mg, "after close cached c2");
    err("AdsCloseCachedTables(c1)", close_cached(c1));
    list(mg, "after close cached c1");
    sql(c2, "DROP TABLE w");
    t = open_t(c1, "t.adt", 4);
    close_t(t, "t");
    list(mg, "t closed again");
    err("AdsCacheOpenTables(0)", cache_tables(0));
    list(mg, "after cache 0");
    t = open_t(c1, "u.adt", 4);
    close_t(t, "u");
    list(mg, "u closed, cache 0");
    err("AdsDisconnect(c1)", ((U32(WINAPI *)(H))F("AdsDisconnect"))(c1));
    list(mg, "c1 disconnected");
    c1 = conn();
    close_cached(c1);
    list(mg, "after close cached new c1");

    printf("== table cache scope, cache 2\n");
    err("AdsCacheOpenTables(2)", cache_tables(2));
    t = open_t(c1, "t.adt", 4);
    close_t(t, "t (c1)");
    u = open_t(c2, "u.adt", 4);
    close_t(u, "u (c2)");
    list(mg, "t by c1, u by c2 closed");
    v = open_t(c2, "v.adt", 4);
    close_t(v, "v (c2)");
    list(mg, "then v by c2 closed");
    err("AdsCloseCachedTables(c2)", close_cached(c2));
    list(mg, "after close cached c2");
    t = open_t(c2, "t.adt", 4);
    close_t(t, "t (c2)");
    list(mg, "t by c2 closed");
    t = open_t(c1, "t.adt", 4);
    t = open_t(c1, "t.adt", 4);
    list(mg, "t twice open on c1");
    err("AdsCloseCachedTables(c1) while open", close_cached(c1));
    list(mg, "after close cached c1");
    err("AdsCloseAllTables", ((U32(WINAPI *)(void))F("AdsCloseAllTables"))());
    list(mg, "after close all tables");
    t = open_t(c1, "t.adt", 4);
    err("AdsLockTable", ((U32(WINAPI *)(H))F("AdsLockTable"))(t));
    close_t(t, "t (locked)");
    list(mg, "locked t closed");
    H t2 = open_t(c2, "t.adt", 4);
    err("AdsLockTable(c2)", ((U32(WINAPI *)(H))F("AdsLockTable"))(t2));
    close_t(t2, "t (c2)");
    err("AdsDisconnect(c1)", ((U32(WINAPI *)(H))F("AdsDisconnect"))(c1));
    list(mg, "c1 disconnected");
    err("AdsCacheOpenTables(0)", cache_tables(0));
    list(mg, "tables 0");

    printf("== cursors, cache 0\n");
    H s = 0, cur = 0;
    err("AdsCacheOpenCursors(0)", cache_cursors(0));
    H c3 = conn();
    cur = query(c3, "SELECT * FROM t WHERE a > 0", &s);
    list(mg, "cursor open");
    close_t(cur, "cursor");
    list(mg, "cursor closed");
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed");
    excl(c2);

    printf("== cursors, cache 5\n");
    err("AdsCacheOpenCursors(5)", cache_cursors(5));
    cur = query(c3, "SELECT * FROM t WHERE a > 0", &s);
    close_t(cur, "cursor");
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed, cache 5");
    excl(c2);
    sql(c2, "INSERT INTO t VALUES (3)");
    cur = query(c3, "SELECT * FROM t WHERE a > 0", &s);
    close_t(cur, "cursor");
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed");
    err("AdsCloseCachedTables(c3)", close_cached(c3));
    list(mg, "after close cached c3");
    err("AdsCacheOpenCursors(0)", cache_cursors(0));
    list(mg, "after cursors 0");
    excl(c2);
    err("AdsDisconnect(c3)", ((U32(WINAPI *)(H))F("AdsDisconnect"))(c3));
    list(mg, "c3 disconnected");
    excl(c2);

    /* A statement whose cursor closed, still open: does it hold the table
     * against another connection's exclusive open, a DROP TABLE, running
     * again on another table, AdsCloseCachedTables? */
    printf("== statements hold tables\n");
    err("AdsCacheOpenCursors(0)", cache_cursors(0));
    H c4 = conn();
    sql(c2, "CREATE TABLE x (a INTEGER)");
    cur = query(c4, "SELECT * FROM t WHERE a > 0", &s);
    close_t(cur, "cursor");
    list(mg, "cursor closed, statement open");
    excl(c2);
    list(mg, "after excl");
    err("AdsCloseCachedTables(c4)", close_cached(c4));
    list(mg, "after close cached c4");
    cur = requery(s, "SELECT * FROM u WHERE a > 0");
    list(mg, "ran again on u, cursor open");
    close_t(cur, "cursor");
    list(mg, "ran again on u, cursor closed");
    excl(c2);
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed");
    excl(c2);
    cur = query(c4, "SELECT * FROM x WHERE a > 0", &s);
    close_t(cur, "cursor");
    sql(c2, "DROP TABLE x");
    list(mg, "x dropped, statement open");
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed");
    cur = query(c4, "SELECT * FROM t WHERE a > 0", &s);
    list(mg, "cursor open");
    err("AdsCloseSQLStatement (cursor open)",
        ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed with its cursor");
    err("AdsCacheOpenCursors(5)", cache_cursors(5));
    cur = query(c4, "SELECT * FROM t WHERE a > 0", &s);
    close_t(cur, "cursor");
    list(mg, "cursor closed, cache 5");
    excl(c2);
    list(mg, "after excl, cache 5");
    err("AdsCloseSQLStatement", ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s));
    list(mg, "statement closed, cache 5");
    excl(c2);
    err("AdsDisconnect(c4)", ((U32(WINAPI *)(H))F("AdsDisconnect"))(c4));
    list(mg, "c4 disconnected");
    return 0;
}
