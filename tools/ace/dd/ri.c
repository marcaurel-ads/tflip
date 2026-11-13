/* Records how an Advantage Client Engine enforces a dictionary's referential
 * integrity rules, for fixtures/dictionary/ri.json.
 *
 *     ri.exe <ace32.dll> <ri.add path> build   -- create the dictionary
 *     ri.exe <ace32.dll> <ri.add path>         -- probe enforcement > ri.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_ri.sh). The
 * `build` pass creates a dictionary (no password, so access rights are not
 * verified and only RI is in play) with a parent table `p (id, lbl)` whose
 * `id` is a unique primary key `p_pk`, and four child tables, one per
 * update/delete rule, each with a foreign-key index on `pid`:
 *
 *   - cr / r_restrict   : UPDATE RESTRICT (2),  DELETE RESTRICT (2)
 *   - cc / r_cascade    : UPDATE CASCADE  (1),  DELETE CASCADE  (1)
 *   - cn / r_setnull    : UPDATE SET NULL (3),  DELETE SET NULL (3)
 *   - cd / r_setdefault : UPDATE SET DFLT (4),  DELETE SET DFLT (4)  (pid default 0)
 *
 * Parent rows id 1..20. One child per rule points at a parent reserved for the
 * delete test and one at a parent reserved for the update test:
 *   cr(100->1, 105->5)  cc(200->2, 206->6)  cn(300->3, 307->7)  cd(400->4, 408->8)
 *
 * The probe pass runs, in order, the operations that carry an RI outcome —
 * parent deletes and key updates (one per rule, plus one with no children),
 * child inserts/updates with a present, absent and NULL foreign key, each
 * through SQL and some through the table API — and records each return code.
 * Interleaved "check" lines re-read a child so the cascade/set-null/set-default
 * effect is visible. Each line is {"op", "rc", ...}: "rows"/"value" on a check.
 *
 * Because the operations mutate the dictionary, the probe always runs on a
 * fresh copy (run_ri.sh copies the committed fixture first). */
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
#define ADS_LOCAL 1

typedef U32(WINAPI *Create)(U8 *, U16, U8 *, H *);
typedef U32(WINAPI *Conn)(U8 *, U16, U8 *, U8 *, U32, H *);
typedef U32(WINAPI *SqlD)(H, U8 *, H *);
typedef U32(WINAPI *MkStmt)(H, H *);
typedef U32(WINAPI *Open)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
typedef U32(WINAPI *Get)(H, U8 *, U8 *, U32 *, U16);
typedef U32(WINAPI *SetD)(H, U8 *, double);
typedef U32(WINAPI *Seek)(H, U8 *, U16, U16, U16, U16 *);
typedef U32(WINAPI *RIname)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U16, U8 *, U8 *);
typedef U32(WINAPI *H1)(H);
#define ADS_STRINGKEY 2
#define ADS_HARDSEEK 0x0002

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

static void begin(const char *op) {
    printf("%s\n{\"op\": ", first ? "[" : ",");
    jstr(op);
    first = 0;
}

/* ---- build pass ---- */

static H bstmt;

static void ddl(const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(bstmt, (U8 *)sql, &cur);
    if (rc) fprintf(stderr, "DDL rc=%lu: %s\n", rc, sql);
    if (cur) ((H1)F("AdsCloseTable"))(cur);
}

static void ri(H d, const char *name, const char *parent, const char *ptag,
               const char *child, const char *ctag, U16 upd, U16 del) {
    U32 rc = ((RIname)F("AdsDDCreateRefIntegrity62"))(
        d, (U8 *)name, NULL, (U8 *)parent, (U8 *)ptag, (U8 *)child, (U8 *)ctag,
        upd, del, (U8 *)"no parent key", (U8 *)"cascade failed");
    if (rc) fprintf(stderr, "RI %s rc=%lu\n", name, rc);
}

static void build(const char *path) {
    H d = 0;
    /* The local server, which AdsDDCreate needs to find. */
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    U32 rc = ((Create)F("AdsDDCreate"))((U8 *)path, 0, (U8 *)"RI test dictionary", &d);
    if (rc) { fprintf(stderr, "AdsDDCreate rc=%lu\n", rc); return; }
    ((MkStmt)F("AdsCreateSQLStatement"))(d, &bstmt);
    ddl("CREATE TABLE p (id INTEGER, lbl CHAR(5))");
    ddl("CREATE TABLE cr (cid INTEGER, pid INTEGER)");
    ddl("CREATE TABLE cc (cid INTEGER, pid INTEGER)");
    ddl("CREATE TABLE cn (cid INTEGER, pid INTEGER)");
    ddl("CREATE TABLE cd (cid INTEGER, pid INTEGER)");
    ddl("CREATE UNIQUE INDEX p_pk ON p (id)");
    ddl("EXECUTE PROCEDURE sp_ModifyTableProperty ('p', 'TABLE_PRIMARY_KEY', 'p_pk', 'NO_VALIDATE', NULL)");
    ddl("CREATE INDEX cr_fk ON cr (pid)");
    ddl("CREATE INDEX cc_fk ON cc (pid)");
    ddl("CREATE INDEX cn_fk ON cn (pid)");
    ddl("CREATE INDEX cd_fk ON cd (pid)");
    ddl("EXECUTE PROCEDURE sp_ModifyFieldProperty ('cd', 'pid', 'FIELD_DEFAULT_VALUE', '0', 'NO_VALIDATE', NULL)");
    ri(d, "r_restrict", "p", "p_pk", "cr", "cr_fk", 2, 2);
    ri(d, "r_cascade", "p", "p_pk", "cc", "cc_fk", 1, 1);
    ri(d, "r_setnull", "p", "p_pk", "cn", "cn_fk", 3, 3);
    ri(d, "r_setdefault", "p", "p_pk", "cd", "cd_fk", 4, 4);
    char sql[128];
    for (int i = 1; i <= 20; i++) {
        snprintf(sql, sizeof sql, "INSERT INTO p (id, lbl) VALUES (%d, 'r%d')", i, i);
        ddl(sql);
    }
    ddl("INSERT INTO cr (cid, pid) VALUES (100, 1)");
    ddl("INSERT INTO cr (cid, pid) VALUES (105, 5)");
    ddl("INSERT INTO cc (cid, pid) VALUES (200, 2)");
    ddl("INSERT INTO cc (cid, pid) VALUES (206, 6)");
    ddl("INSERT INTO cn (cid, pid) VALUES (300, 3)");
    ddl("INSERT INTO cn (cid, pid) VALUES (307, 7)");
    ddl("INSERT INTO cd (cid, pid) VALUES (400, 4)");
    ddl("INSERT INTO cd (cid, pid) VALUES (408, 8)");
    ((H1)F("AdsCloseSQLStatement"))(bstmt);
    ((H1)F("AdsDisconnect"))(d);
}

/* ---- probe pass ---- */

static H conn, stmt;

/* Execute a DML/DDL statement, record its rc. */
static void exec(const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur);
    begin(sql);
    printf(", \"rc\": %lu}", rc);
    if (cur) ((H1)F("AdsCloseTable"))(cur);
}

/* A check: run a SELECT and walk the cursor, recording how many rows it holds
 * and the first row's `pid` (so a NULL from SET NULL, a changed value from
 * CASCADE, or an empty result from a cascaded delete is visible). */
static void check(const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur);
    begin(sql);
    printf(", \"rc\": %lu", rc);
    if (rc == 0 && cur) {
        U32 rows = 0;
        char first[16] = "null";
        ((H1)F("AdsGotoTop"))(cur);
        for (;;) {
            U16 eof = 1;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            if (eof) break;
            if (rows == 0) {
                U8 val[64];
                U32 len = sizeof val - 1;
                U16 isnull = 0;
                memset(val, 0, sizeof val);
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(cur, (U8 *)"pid", &isnull);
                if (isnull) snprintf(first, sizeof first, "\"NULL\"");
                else {
                    U32 frc = ((Get)F("AdsGetField"))(cur, (U8 *)"pid", val, &len, 0);
                    if (frc == 0) { val[len < sizeof val ? len : sizeof val - 1] = 0; snprintf(first, sizeof first, "\"%s\"", val); }
                }
            }
            rows++;
            ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
        }
        printf(", \"rows\": %lu, \"value\": %s", rows, first);
    }
    printf("}");
    if (cur) ((H1)F("AdsCloseTable"))(cur);
}

static H open_rw(const char *name) {
    H t = 0;
    ((Open)F("AdsOpenTable"))(conn, (U8 *)name, NULL, 0, ADS_ANSI, 0, 0, 0, &t);
    return t;
}

/* Table-API: seek a parent row by its primary key on the index order and
 * delete it (AdsDeleteRecord on the index handle, so the positioned record is
 * the one deleted), recording the rc. */
static void api_delete_parent(const char *tag, double key) {
    H t = open_rw("p");
    char op[64];
    snprintf(op, sizeof op, "API delete p id=%g", key);
    if (!t) { begin(op); printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    H ord = 0;
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(t, (U8 *)tag, &ord);
    U16 found = 0;
    char keybuf[32];
    snprintf(keybuf, sizeof keybuf, "%g", key);
    ((Seek)F("AdsSeek"))(ord, (U8 *)keybuf, (U16)strlen(keybuf), ADS_STRINGKEY, ADS_HARDSEEK, &found);
    U32 rc = ((H1)F("AdsDeleteRecord"))(t);
    begin(op);
    printf(", \"found\": %u, \"rc\": %lu}", found, rc);
    ((H1)F("AdsCloseTable"))(t);
}

/* Table-API: append a child row with foreign key `fk`, record the write rc. */
static void api_insert_child(const char *name, double cid, double fk) {
    H t = open_rw(name);
    char op[64];
    snprintf(op, sizeof op, "API insert %s cid=%g pid=%g", name, cid, fk);
    if (!t) { begin(op); printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    ((H1)F("AdsAppendRecord"))(t);
    ((SetD)F("AdsSetDouble"))(t, (U8 *)"cid", cid);
    ((SetD)F("AdsSetDouble"))(t, (U8 *)"pid", fk);
    U32 rc = ((H1)F("AdsWriteRecord"))(t);
    begin(op);
    printf(", \"rc\": %lu}", rc);
    if (rc) ((H1)F("AdsCancelUpdate"))(t);
    ((H1)F("AdsCloseTable"))(t);
}

static void probe(const char *path) {
    U32 rc = ((Conn)F("AdsConnect60"))((U8 *)path, ADS_LOCAL, (U8 *)"ADSSYS", (U8 *)"", 0, &conn);
    if (rc || !conn) { begin("connect"); printf(", \"rc\": %lu}\n]\n", rc); return; }
    ((MkStmt)F("AdsCreateSQLStatement"))(conn, &stmt);

    /* --- parent DELETE, one per rule --- */
    exec("DELETE FROM p WHERE id = 1");          /* restrict: has cr child -> fail */
    check("SELECT pid FROM cr WHERE cid = 100"); /* unchanged */
    exec("DELETE FROM p WHERE id = 2");          /* cascade: cc child removed */
    check("SELECT pid FROM cc WHERE cid = 200"); /* gone */
    exec("DELETE FROM p WHERE id = 3");          /* set null: cn child pid -> NULL */
    check("SELECT pid FROM cn WHERE cid = 300");
    exec("DELETE FROM p WHERE id = 4");          /* set default: cd child pid -> 0 */
    check("SELECT pid FROM cd WHERE cid = 400");
    exec("DELETE FROM p WHERE id = 15");         /* no children -> ok */

    /* --- parent key UPDATE, one per rule --- */
    exec("UPDATE p SET id = 105 WHERE id = 5");  /* restrict -> fail */
    check("SELECT pid FROM cr WHERE cid = 105");
    exec("UPDATE p SET id = 106 WHERE id = 6");  /* cascade: cc child pid -> 106 */
    check("SELECT pid FROM cc WHERE cid = 206");
    exec("UPDATE p SET id = 107 WHERE id = 7");  /* set null -> NULL */
    check("SELECT pid FROM cn WHERE cid = 307");
    exec("UPDATE p SET id = 108 WHERE id = 8");  /* set default -> 0 */
    check("SELECT pid FROM cd WHERE cid = 408");
    exec("UPDATE p SET id = 116 WHERE id = 16"); /* no children -> ok */

    /* --- child INSERT, foreign key present / absent / NULL --- */
    exec("INSERT INTO cr (cid, pid) VALUES (500, 10)");  /* parent exists -> ok */
    exec("INSERT INTO cr (cid, pid) VALUES (501, 999)"); /* no parent -> fail */
    exec("INSERT INTO cr (cid, pid) VALUES (502, NULL)");/* NULL fk -> ok */

    /* --- child UPDATE of the foreign key --- */
    exec("UPDATE cr SET pid = 11 WHERE cid = 100");   /* parent exists -> ok */
    exec("UPDATE cr SET pid = 998 WHERE cid = 100");  /* no parent -> fail */
    exec("UPDATE cr SET pid = NULL WHERE cid = 100"); /* NULL fk -> ok */

    /* --- table API: RI enforced outside SQL too --- */
    api_insert_child("cc", 601, 997); /* no parent -> fail (child-side check) */
    api_insert_child("cc", 602, 12);  /* parent exists -> ok */
    api_delete_parent("p_pk", 10);    /* cr(500) is a child of 10 -> restrict? */
    check("SELECT id FROM p WHERE id = 10");        /* still there if restricted */
    check("SELECT pid FROM cr WHERE cid = 500");    /* child untouched if restricted */
    api_delete_parent("p_pk", 12);    /* cc(602) is a child of 12 -> cascade? */
    check("SELECT id FROM p WHERE id = 12");        /* gone if deleted */
    check("SELECT pid FROM cc WHERE cid = 602");    /* gone if cascaded */
    api_delete_parent("p_pk", 13);    /* cn has no child of 13 -> plain delete */
    check("SELECT id FROM p WHERE id = 13");

    ((H1)F("AdsCloseSQLStatement"))(stmt);
    ((H1)F("AdsDisconnect"))(conn);
    printf("\n]\n");
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: ri.exe <ace32.dll> <ri.add> [build]\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    if (argc >= 4 && strcmp(argv[3], "build") == 0) build(argv[2]);
    else probe(argv[2]);
    FreeLibrary(ace);
    return 0;
}
