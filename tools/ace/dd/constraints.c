/* Records how an Advantage Client Engine applies a dictionary's own field
 * defaults, field constraints (required, minimum, maximum, validation message),
 * table record validation and triggers when writing, through both the table API
 * and SQL, for fixtures/dictionary/constraints.json.
 *
 *     constraints.exe <ace32.dll> <dict path> build   -- create the dictionary
 *     constraints.exe <ace32.dll> <dict path>  < ops  -- run ops > json
 *
 * The probe connects to the local server, or with SQL_SERVER_TYPE=2 in the
 * environment to a remote one over TCP/IP (the dict path then the server's,
 * such as \\127.0.0.1:6262\data\...\constraints.add).
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see constraints.sh). The
 * `build` pass runs the SQL lines it reads from standard input against a fresh
 * dictionary (no password). The probe pass reads one operation per line from
 * standard input and prints, per operation, {"op", "rc", "error"} (the
 * AdsGetLastError text when rc is not 0), plus "rows" for an operation that
 * yields rows:
 *
 *   sql <statement>          AdsExecuteSQLDirect; a cursor's rows are dumped
 *   append <table> [f=v ...] AdsAppendRecord, set each field, AdsWriteRecord
 *   peek <table> f,f,...     AdsAppendRecord, read the fields, AdsCancelUpdate
 *   update <table> <recno> f=v ...   AdsGotoRecord, set, AdsWriteRecord
 *   delete <table> <recno>   AdsGotoRecord, AdsDeleteRecord
 *
 * A value `NULL` sets the field NULL (AdsSetNull), `EMPTY` sets it empty
 * (AdsSetEmpty); anything else goes through AdsSetField. Each step's own rc is
 * recorded in "steps"; "rc" is the first non-zero one (or 0). A failed write is
 * cancelled (AdsCancelUpdate). Rows are arrays of trimmed field texts, NULL as
 * null. Lines starting with `#` and blank lines are skipped. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

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
typedef U32(WINAPI *SetS)(H, U8 *, U8 *, U32);
typedef U32(WINAPI *H1)(H);
typedef U32(WINAPI *HF)(H, U8 *);

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

static void error_text(void) {
    U32 code = 0;
    U8 text[1024];
    U16 len = sizeof text;
    text[0] = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    text[len < sizeof text ? len : sizeof text - 1] = 0;
    jstr((char *)text);
}

static void last_error(void) {
    printf(", \"error\": ");
    error_text();
}

static H conn, stmt;

static void rtrim(char *s) {
    size_t n = strlen(s);
    while (n && s[n - 1] == ' ') s[--n] = 0;
}

/* One field of the current record as trimmed text, or NULL. */
static void field_json(H t, U8 *field) {
    U16 isnull = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsNull"))(t, field, &isnull);
    if (rc) { printf("{\"rc\": %lu}", rc); return; }
    if (isnull) { printf("null"); return; }
    char val[512];
    U32 len = sizeof val - 1;
    memset(val, 0, sizeof val);
    rc = ((Get)F("AdsGetField"))(t, field, (U8 *)val, &len, 0);
    if (rc) { printf("{\"rc\": %lu}", rc); return; }
    val[len < sizeof val ? len : sizeof val - 1] = 0;
    rtrim(val);
    jstr(val);
}

static void dump_rows(H cur) {
    U16 n = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &n);
    printf(", \"rows\": [");
    ((H1)F("AdsGotoTop"))(cur);
    int rows = 0;
    for (;;) {
        U16 eof = 1;
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
        if (eof) break;
        printf("%s[", rows ? ", " : "");
        for (U16 i = 1; i <= n; i++) {
            if (i > 1) printf(", ");
            field_json(cur, FIELD(i));
        }
        printf("]");
        rows++;
        ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
    }
    printf("]");
}

static void begin(const char *op) {
    printf("%s\n{\"op\": ", first ? "[" : ",");
    jstr(op);
    first = 0;
}

static void op_sql(const char *line, const char *sql) {
    H cur = 0;
    U32 rc = ((SqlD)F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cur);
    begin(line);
    printf(", \"rc\": %lu", rc);
    if (rc) last_error();
    if (rc == 0 && cur) dump_rows(cur);
    printf("}");
    if (cur) ((H1)F("AdsCloseTable"))(cur);
}

static H open_rw(const char *name) {
    H t = 0;
    ((Open)F("AdsOpenTable"))(conn, (U8 *)name, NULL, 0, ADS_ANSI, 0, 0, 0, &t);
    return t;
}

/* Sets `f=v` assignments (space separated) on t; records each step's rc. */
static U32 assign(H t, char *rest, U32 *firstrc) {
    char *tok = strtok(rest, " ");
    while (tok) {
        char *eq = strchr(tok, '=');
        if (eq) {
            *eq = 0;
            char *v = eq + 1;
            U32 rc;
            if (strcmp(v, "NULL") == 0) rc = ((HF)F("AdsSetNull"))(t, (U8 *)tok);
            else if (strcmp(v, "EMPTY") == 0) rc = ((HF)F("AdsSetEmpty"))(t, (U8 *)tok);
            else rc = ((SetS)F("AdsSetField"))(t, (U8 *)tok, (U8 *)v, (U32)strlen(v));
            printf(", \"set %s\": %lu", tok, rc);
            if (rc) { printf(", \"set %s error\": ", tok); error_text(); }
            if (rc && !*firstrc) *firstrc = rc;
        }
        tok = strtok(NULL, " ");
    }
    return 0;
}

static void finish_write(H t, U32 firstrc) {
    U32 rc = ((H1)F("AdsWriteRecord"))(t);
    printf(", \"write\": %lu", rc);
    if (rc && !firstrc) firstrc = rc;
    printf(", \"rc\": %lu", firstrc);
    if (rc) { last_error(); ((H1)F("AdsCancelUpdate"))(t); }
    printf("}");
}

static void op_append(const char *line, char *rest) {
    char *name = strtok(rest, " ");
    char *assigns = strtok(NULL, "");
    begin(line);
    H t = open_rw(name);
    if (!t) { printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    U32 firstrc = ((H1)F("AdsAppendRecord"))(t);
    printf(", \"append\": %lu", firstrc);
    if (assigns) assign(t, assigns, &firstrc);
    finish_write(t, firstrc);
    ((H1)F("AdsCloseTable"))(t);
}

static void op_peek(const char *line, char *rest) {
    char *name = strtok(rest, " ");
    char *fields = strtok(NULL, "");
    begin(line);
    H t = open_rw(name);
    if (!t) { printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    U32 rc = ((H1)F("AdsAppendRecord"))(t);
    printf(", \"rc\": %lu, \"values\": [", rc);
    int i = 0;
    for (char *f = strtok(fields, ","); f; f = strtok(NULL, ",")) {
        printf("%s", i++ ? ", " : "");
        field_json(t, (U8 *)f);
    }
    printf("]}");
    ((H1)F("AdsCancelUpdate"))(t);
    ((H1)F("AdsCloseTable"))(t);
}

static void op_update(const char *line, char *rest) {
    char *name = strtok(rest, " ");
    char *recno = strtok(NULL, " ");
    char *assigns = strtok(NULL, "");
    begin(line);
    H t = open_rw(name);
    if (!t) { printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    U32 firstrc = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, (U32)atol(recno));
    printf(", \"goto\": %lu", firstrc);
    if (assigns) assign(t, assigns, &firstrc);
    finish_write(t, firstrc);
    ((H1)F("AdsCloseTable"))(t);
}

static void op_delete(const char *line, char *rest) {
    char *name = strtok(rest, " ");
    char *recno = strtok(NULL, " ");
    begin(line);
    H t = open_rw(name);
    if (!t) { printf(", \"rc\": \"OPEN FAILED\"}"); return; }
    U32 rc = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, (U32)atol(recno));
    if (!rc) rc = ((H1)F("AdsDeleteRecord"))(t);
    printf(", \"rc\": %lu", rc);
    if (rc) last_error();
    printf("}");
    ((H1)F("AdsCloseTable"))(t);
}

/* `fprop <table> <field> <property id> <value...>`: AdsDDSetFieldProperty
 * with ADS_NO_VALIDATE, the value the rest of the line (a string property). */
static void op_fprop(const char *line, char *rest) {
    char *table = strtok(rest, " ");
    char *field = strtok(NULL, " ");
    char *id = strtok(NULL, " ");
    char *value = strtok(NULL, "");
    if (!value) value = "";
    begin(line);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
        "AdsDDSetFieldProperty"))(conn, (U8 *)table, (U8 *)field, (U16)atoi(id), value,
                                  (U16)(strlen(value) + 1), 0, NULL);
    printf(", \"rc\": %lu", rc);
    if (rc) last_error();
    printf("}");
}

static void chomp(char *s) {
    size_t n = strlen(s);
    while (n && (s[n - 1] == '\n' || s[n - 1] == '\r')) s[--n] = 0;
}

static void build(const char *path) {
    H d = 0;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    U32 rc = ((Create)F("AdsDDCreate"))((U8 *)path, 0, (U8 *)"constraints", &d);
    if (rc) { fprintf(stderr, "AdsDDCreate rc=%lu\n", rc); return; }
    ((MkStmt)F("AdsCreateSQLStatement"))(d, &stmt);
    char line[4096];
    while (fgets(line, sizeof line, stdin)) {
        chomp(line);
        if (!line[0] || line[0] == '#') continue;
        H cur = 0;
        rc = ((SqlD)F("AdsExecuteSQLDirect"))(stmt, (U8 *)line, &cur);
        if (rc) {
            U32 code = 0; U8 text[1024]; U16 len = sizeof text; text[0] = 0;
            ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
            fprintf(stderr, "build rc=%lu: %s\n  %s\n", rc, line, text);
        }
        if (cur) ((H1)F("AdsCloseTable"))(cur);
    }
    ((H1)F("AdsCloseSQLStatement"))(stmt);
    ((H1)F("AdsDisconnect"))(d);
}

static void probe(const char *path) {
    U32 rc;
    if (getenv("SQL_SERVER_TYPE") && atoi(getenv("SQL_SERVER_TYPE")) == 2) {
        // Over TCP/IP, as AdsConnect60's default (UDP) does not reach the
        // server under Wine.
        char cs[1024];
        snprintf(cs, sizeof cs,
                 "Data Source=%s;ServerType=REMOTE;CommType=TCP_IP;User ID=ADSSYS;Password=", path);
        rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &conn);
    } else {
        rc = ((Conn)F("AdsConnect60"))((U8 *)path, ADS_LOCAL, (U8 *)"ADSSYS", (U8 *)"", 0, &conn);
    }
    if (rc || !conn) { begin("connect"); printf(", \"rc\": %lu}\n]\n", rc); return; }
    ((MkStmt)F("AdsCreateSQLStatement"))(conn, &stmt);
    char line[4096], work[4096];
    while (fgets(line, sizeof line, stdin)) {
        chomp(line);
        if (!line[0] || line[0] == '#') continue;
        strcpy(work, line);
        if (strncmp(work, "sql ", 4) == 0) op_sql(line, work + 4);
        else if (strncmp(work, "append ", 7) == 0) op_append(line, work + 7);
        else if (strncmp(work, "peek ", 5) == 0) op_peek(line, work + 5);
        else if (strncmp(work, "update ", 7) == 0) op_update(line, work + 7);
        else if (strncmp(work, "delete ", 7) == 0) op_delete(line, work + 7);
        else if (strncmp(work, "fprop ", 6) == 0) op_fprop(line, work + 6);
        else { begin(line); printf(", \"rc\": \"UNKNOWN OP\"}"); }
        fflush(stdout);
    }
    ((H1)F("AdsCloseSQLStatement"))(stmt);
    ((H1)F("AdsDisconnect"))(conn);
    printf("\n]\n");
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: constraints.exe <ace32.dll> <dict> [build]\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    if (argc >= 4 && strcmp(argv[3], "build") == 0) build(argv[2]);
    else probe(argv[2]);
    FreeLibrary(ace);
    return 0;
}
