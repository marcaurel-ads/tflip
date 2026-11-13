/* Records how an Advantage Client Engine enforces a dictionary's access
 * rights, for fixtures/dictionary/rights.json.
 *
 *     rights.exe <ace32.dll> <props.add path>  > rights.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on a fresh copy of the
 * `dictionary_props` fixture (see run_rights.sh), which has
 * VERIFY_ACCESS_RIGHTS = TRUE and this access setup
 * (tools/ace/fixtures/dictionary_props.sql):
 *
 *   - ADSSYS (password adssyspw): the administrator.
 *   - bob (password bobpw), in group `readers`.
 *   - GRANT SELECT, INSERT ON parent TO readers;
 *   - GRANT UPDATE (label) ON parent TO bob;        (column-level)
 *   - no grant at all on table `child`.
 *
 * Tables: parent(id, code, label, weight) one row (1,'X1','first',1.25);
 * child(id, parent_id, amount, pic) one row.
 *
 * For ADSSYS (full rights) and bob (partial) it records:
 *   - AdsGetColumnPermissions for each field of parent and child;
 *   - opening each table read-only and read-write (does a table the user may
 *     not touch fail, and with what code?);
 *   - SELECT * / one column over each table;
 *   - as bob: UPDATE(label), UPDATE(code), UPDATE(weight), INSERT, DELETE on
 *     parent, and INSERT on child — each with a unique id so nothing collides.
 *
 * Each line is a JSON object {"user", "op", "rc", ...}: "perms" for a column
 * byte, "cols"/"row" for a query, "field"/"value" for a field read. DML lines
 * use distinct ids, so each rc reflects the right, not leftover state. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))
#define ADS_ANSI 1

typedef U32(WINAPI *Conn)(U8 *, U16, U8 *, U8 *, U32, H *);
typedef U32(WINAPI *Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
typedef U32(WINAPI *Get)(H, U8 *, U8 *, U32 *, U16);
typedef U32(WINAPI *H1)(H);
typedef U32(WINAPI *SqlD)(H, U8 *, H *);

static const char *path;
static int first = 1;

static void jstr(const char *s) {
    if (!s) { printf("null"); return; }
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static void begin(const char *user, const char *op) {
    printf("%s\n{\"user\": \"%s\", \"op\": ", first ? "[" : ",", user);
    jstr(op);
    first = 0;
}

static H connect_as(const char *user, const char *pw) {
    H c = 0;
    ((Conn)F("AdsConnect60"))((U8 *)path, 1, (U8 *)user, (U8 *)pw, 0, &c);
    return c;
}

/* Open `name`: usLockType 0, usCheckRights = check, read-only if ro. */
static U32 open_as(H c, const char *name, int ro, U16 check, H *t) {
    *t = 0;
    return ((Open)F("AdsOpenTable"))(c, (U8 *)name, NULL, 0, ADS_ANSI, 0, check, ro ? 2 : 0, t);
}

static void column_perms(H c, const char *user, const char *name, int n) {
    H t = 0;
    U32 orc = open_as(c, name, 1, 1 /*ADS_CHECKRIGHTS*/, &t);
    char op[80];
    snprintf(op, sizeof op, "AdsGetColumnPermissions %s", name);
    for (int f = 1; f <= n; f++) {
        U8 perms = 0xAA;
        U32 rc = ((U32(WINAPI *)(H, U16, U8 *))F("AdsGetColumnPermissions"))(t, (U16)f, &perms);
        begin(user, op);
        printf(", \"field\": %d, \"open_rc\": %lu, \"rc\": %lu, \"perms\": %u}", f, orc, rc, perms);
    }
    if (t) ((H1)F("AdsCloseTable"))(t);
}

/* Open `name` read-only and read-write, recording the open rc each way. */
static void open_table(H c, const char *user, const char *name) {
    for (int ro = 1; ro >= 0; ro--) {
        H t = 0;
        U32 rc = open_as(c, name, ro, 1 /*ADS_CHECKRIGHTS*/, &t);
        char op[80];
        snprintf(op, sizeof op, "open %s %s", name, ro ? "readonly" : "readwrite");
        begin(user, op);
        printf(", \"rc\": %lu}", rc);
        if (t) ((H1)F("AdsCloseTable"))(t);
    }
}

/* Open parent read-only and read each field of record 1. */
static void read_fields(H c, const char *user, const char *name, int n) {
    H t = 0;
    if (open_as(c, name, 1, 1, &t)) return;
    ((H1)F("AdsGotoTop"))(t);
    for (int f = 1; f <= n; f++) {
        U8 val[256];
        U32 len = sizeof val - 1;
        memset(val, 0, sizeof val);
        U32 rc = ((Get)F("AdsGetField"))(t, FIELD(f), val, &len, 0);
        char op[96];
        snprintf(op, sizeof op, "AdsGetField %s", name);
        begin(user, op);
        printf(", \"field\": %d, \"rc\": %lu, \"value\": ", f, rc);
        if (rc == 0) { val[len < sizeof val ? len : sizeof val - 1] = 0; jstr((char *)val); }
        else printf("null");
        printf("}");
    }
    ((H1)F("AdsCloseTable"))(t);
}

static H make_stmt(H c) {
    H s = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    return s;
}

static void sql_select(H s, const char *user, const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(s, (U8 *)sql, &cur);
    begin(user, sql);
    printf(", \"rc\": %lu", rc);
    if (rc == 0 && cur) {
        U16 cols = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &cols);
        printf(", \"cols\": %u, \"row\": [", cols);
        ((H1)F("AdsGotoTop"))(cur);
        for (U16 f = 1; f <= cols; f++) {
            U8 val[256];
            U32 len = sizeof val - 1;
            memset(val, 0, sizeof val);
            U32 frc = ((Get)F("AdsGetField"))(cur, FIELD(f), val, &len, 0);
            if (f > 1) printf(", ");
            if (frc == 0) { val[len < sizeof val ? len : sizeof val - 1] = 0; jstr((char *)val); }
            else printf("null");
        }
        printf("]");
        ((H1)F("AdsCloseTable"))(cur);
    }
    printf("}");
}

static void sql_exec(H s, const char *user, const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(s, (U8 *)sql, &cur);
    begin(user, sql);
    printf(", \"rc\": %lu}", rc);
    if (cur) ((H1)F("AdsCloseTable"))(cur);
}

/* Table-API writes on `name`: append a record (INSERT right) and delete the
 * current record (DELETE right). Records each step's rc. */
static void table_write(H c, const char *user, const char *name) {
    H t = 0;
    char op[96];
    U32 orc = open_as(c, name, 0 /*read-write*/, 1 /*ADS_CHECKRIGHTS*/, &t);
    snprintf(op, sizeof op, "AdsOpenTable rw %s", name);
    begin(user, op);
    printf(", \"rc\": %lu}", orc);
    if (orc || !t) return;
    U32 arc = ((H1)F("AdsAppendRecord"))(t);
    snprintf(op, sizeof op, "AdsAppendRecord %s", name);
    begin(user, op);
    printf(", \"rc\": %lu}", arc);
    if (!arc) ((H1)F("AdsCancelUpdate"))(t);
    ((H1)F("AdsGotoTop"))(t);
    U32 drc = ((H1)F("AdsDeleteRecord"))(t);
    snprintf(op, sizeof op, "AdsDeleteRecord %s", name);
    begin(user, op);
    printf(", \"rc\": %lu}", drc);
    ((H1)F("AdsCloseTable"))(t);
}

static void run_user(const char *user, const char *pw, int do_dml, int id0) {
    H c = connect_as(user, pw);
    if (!c) { begin(user, "connect"); printf(", \"rc\": \"FAILED\"}"); return; }
    column_perms(c, user, "parent", 4);
    column_perms(c, user, "child", 4);
    open_table(c, user, "parent");
    open_table(c, user, "child");
    read_fields(c, user, "parent", 4);
    read_fields(c, user, "child", 4);
    H s = make_stmt(c);
    sql_select(s, user, "SELECT * FROM parent");
    sql_select(s, user, "SELECT * FROM child");
    sql_select(s, user, "SELECT code FROM parent");
    if (do_dml) {
        char sql[160];
        sql_exec(s, user, "UPDATE parent SET label = 'L2' WHERE id = 1");
        sql_exec(s, user, "UPDATE parent SET code = 'CC' WHERE id = 1");
        sql_exec(s, user, "UPDATE parent SET weight = 3.5 WHERE id = 1");
        snprintf(sql, sizeof sql, "INSERT INTO parent (id, code, label, weight) VALUES (%d, 'Z', 'n', 2.0)", id0);
        sql_exec(s, user, sql);
        snprintf(sql, sizeof sql, "DELETE FROM parent WHERE id = %d", id0);
        sql_exec(s, user, sql);
        snprintf(sql, sizeof sql, "INSERT INTO child (id, parent_id, amount) VALUES (%d, 1, 5.0)", id0 + 1);
        sql_exec(s, user, sql);
        table_write(c, user, "parent");
        table_write(c, user, "child");
    }
    ((H1)F("AdsDisconnect"))(c);
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: rights.exe <ace32.dll> <props.add>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    path = argv[2];
    run_user("ADSSYS", "adssyspw", 0, 0);
    run_user("bob", "bobpw", 1, 901);
    printf("\n]\n");
    FreeLibrary(ace);
    return 0;
}
