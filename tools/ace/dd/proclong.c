/* Probes stored procedures whose parameter lists do not fit the dictionary
 * record's `Property` field (275 bytes): AdsDDAddProcedure, SQL CREATE
 * PROCEDURE and AdsDDSetProcedureProperty with long input and output lists,
 * with a script, a comment and an external library besides. Prints, after each
 * step, the memo references of the dictionary's records and the .am's length;
 * leaves the dictionaries as goldens (fixtures/dictionary/proclong). Black-box,
 * against the real ACE.
 *
 *     proclong.exe <ace32.dll> <out folder>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see proclong.sh). */
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

static char dict_dir[MAX_PATH];

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s: rc=%lu %lu %s\n", what, rc, code, text);
}

static void dump(const char *step) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\db.add", dict_dir);
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL,
                           OPEN_EXISTING, 0, NULL);
    printf("%s:", step);
    if (f != INVALID_HANDLE_VALUE) {
        static U8 buf[524 * 512];
        DWORD got = 0;
        U8 head[0x40];
        ReadFile(f, head, sizeof head, &got, NULL);
        U32 count = *(U32 *)(head + 0x18), data = *(U32 *)(head + 0x20);
        SetFilePointer(f, data, NULL, FILE_BEGIN);
        ReadFile(f, buf, sizeof buf, &got, NULL);
        for (U32 r = 0; r < count && (r + 1) * 524 <= got; r++) {
            U8 *rec = buf + r * 524;
            U32 mb = *(U32 *)(rec + 498), ml = *(U32 *)(rec + 502);
            U32 cb = *(U32 *)(rec + 515), cl = *(U32 *)(rec + 519);
            if (mb || ml || cb || cl)
                printf(" r%lu[more %lu/%lu comment %lu/%lu]", r + 1, mb, ml, cb, cl);
        }
        CloseHandle(f);
    }
    snprintf(path, sizeof path, "%s\\db.am", dict_dir);
    f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING,
                    0, NULL);
    if (f != INVALID_HANDLE_VALUE) {
        static U8 am[4096];
        DWORD got = 0;
        ReadFile(f, am, sizeof am, &got, NULL);
        printf(" am=%lu count=%d end=%lu", GetFileSize(f, NULL), am[0xd],
               (am[0] << 24) | (am[1] << 16) | (am[2] << 8) | am[3]);
        for (int i = 0; i < *(U16 *)(am + 8); i++)
            printf(" frag%d=%lu@%lu", i, *(U32 *)(am + 0x10 + 8 * i), *(U32 *)(am + 0x14 + 8 * i));
        for (int i = 0; i < 128; i++)
            if (*(U32 *)(am + 0x400 + 8 * i + 4))
                printf(" cls%d=%lu", i, *(U32 *)(am + 0x400 + 8 * i + 4));
        for (int i = 0; i < 32; i++)
            if (*(U32 *)(am + 0x800 + 8 * i + 4))
                printf(" slab%d=%lu", i + 1, *(U32 *)(am + 0x800 + 8 * i + 4));
        CloseHandle(f);
    }
    printf("\n");
}

/* The dictionary <dir>\db.add with an ADT table t (id, s) added. */
static H dictionary_with_table(const char *dir) {
    char path[MAX_PATH];
    H c = 0, t = 0;
    CreateDirectoryA(dir, NULL);
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)dir, 1, NULL, NULL, 0, &c);
    if (rc) oops("connect", rc);
    snprintf(path, sizeof path, "%s\\t.adt", dir);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(c, (U8 *)path, NULL, 3, 1, 0, 1, 0, (U8 *)"id,Integer;s,Character,20",
                           &t);
    if (rc) oops("create table", rc);
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    snprintf(path, sizeof path, "%s\\db.add", dir);
    snprintf(dict_dir, sizeof dict_dir, "%s", dir);
    c = 0;
    rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) oops("AdsDDCreate", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))(
        c, (U8 *)"t", (U8 *)"t.adt", 3, 1, (U8 *)"", (U8 *)"");
    if (rc) oops("AddTable", rc);
    return c;
}

/* A parameter list of exactly `n` bytes (n >= 7): "p000,I;" tokens, the last
 * one's name padded. */
static char *plist(int n) {
    char *s = malloc(8000); /* one per call: two lists are arguments of one call */
    int at = 0, k = 0;
    while (n - at >= 14) at += snprintf(s + at, 16, "p%03d,I;", k++);
    snprintf(s + at, 8000 - at, "p%0*d,I;", 3 + (n - at - 7), k);
    return s;
}

static void add_proc(H c, const char *name, const char *script, int is_script, const char *in,
                     const char *out, const char *comment) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))(
        c, (U8 *)name, (U8 *)script, (U8 *)(is_script ? "" : "Func"), is_script ? 4 : 1,
        (U8 *)in, (U8 *)out, (U8 *)comment);
    printf("  %s in %d out %d -> rc %lu\n", name, (int)strlen(in), (int)strlen(out), rc);
    if (rc) oops(name, rc);
}

static void proc_set(H c, const char *name, U16 id, const char *what, void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetProcedureProperty"))(
        c, (U8 *)name, id, value, (U16)len);
    printf("  %s %s -> rc %lu\n", name, what, rc);
    if (rc) oops(name, rc);
}

static void run_sql(H c, const char *text) {
    H s = 0, cur = 0;
    U32 rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)text, &cur);
    printf("  sql (%d bytes) -> rc %lu\n", (int)strlen(text), rc);
    if (rc) oops("sql", rc);
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
}

/* CREATE (ALTER if `cursor` is -1) PROCEDURE `name` with n INTEGER parameters,
 * m INTEGER OUTPUT ones and, if `cursor` is 1, a cursor. */
static void sql_proc(H c, const char *name, int n, int m, int cursor) {
    static char text[16000];
    int at = snprintf(text, sizeof text, "%s %s(",
                      cursor < 0 ? "ALTER PROCEDURE" : "CREATE PROCEDURE", name);
    int first = 1;
    for (int i = 0; i < n; i++) {
        at += snprintf(text + at, 64, "%sa%03d INTEGER", first ? "" : ", ", i);
        first = 0;
    }
    for (int i = 0; i < m; i++) {
        at += snprintf(text + at, 64, "%so%03d INTEGER OUTPUT", first ? "" : ", ", i);
        first = 0;
    }
    if (cursor > 0) at += snprintf(text + at, 64, "%sCURSOR VARYING OUTPUT", first ? "" : ", ");
    snprintf(text + at, 128, ") BEGIN SELECT 1 FROM system.iota; END");
    run_sql(c, text);
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: proclong.exe <ace32.dll> <out>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    char dir[MAX_PATH];
    H c;

    snprintf(dir, sizeof dir, "%s\\proclongapi", argv[2]);
    c = dictionary_with_table(dir);
    dump("table");
    static const int lens[] = {100, 240, 250, 255, 258, 259, 260, 261, 262, 265, 270, 275, 276,
                               300};
    for (unsigned i = 0; i < sizeof lens / sizeof *lens; i++) {
        char name[16];
        snprintf(name, sizeof name, "pi%d", lens[i]);
        add_proc(c, name, "SELECT 1 FROM t", 1, plist(lens[i]), "", "");
        dump(name);
    }
    for (unsigned i = 0; i < sizeof lens / sizeof *lens; i++) {
        char name[16];
        snprintf(name, sizeof name, "po%d", lens[i]);
        add_proc(c, name, "SELECT 1 FROM t", 1, "a,I;", plist(lens[i]), "");
        dump(name);
    }
    add_proc(c, "pio", "SELECT 1 FROM t", 1, plist(150), plist(150), "");
    dump("pio 150/150");
    add_proc(c, "pic", "SELECT 1 FROM t", 1, plist(250), plist(20), "a comment");
    dump("pic 250/20 comment");
    add_proc(c, "pis",
             "SELECT id FROM t WHERE s <> 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'",
             1, plist(150), "b,I;", "");
    dump("pis script+150");
    add_proc(c, "pex", ".\\x.aep", 0, plist(150), plist(150), "ext comment");
    dump("pex external 150/150");
    add_proc(c, "pex2", ".\\x.aep", 0, plist(100), "", "");
    dump("pex2 external 100");
    {
        char *s = plist(400);
        proc_set(c, "pi100", 800, "input 400", s, (int)strlen(s) + 1);
        dump("pi100 input 400");
        s = plist(20);
        proc_set(c, "pi100", 800, "input 20", s, (int)strlen(s) + 1);
        dump("pi100 input 20");
        s = plist(300);
        proc_set(c, "po100", 801, "output 300", s, (int)strlen(s) + 1);
        dump("po100 output 300");
        s = plist(100);
        proc_set(c, "po240", 800, "input 100", s, (int)strlen(s) + 1);
        dump("po240 input 100");
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    /* Lists whose continuation is more than 32 blocks go in extents, not slabs. */
    snprintf(dir, sizeof dir, "%s\\proclongext", argv[2]);
    c = dictionary_with_table(dir);
    dump("table");
    add_proc(c, "pe1", "SELECT 1 FROM t", 1, plist(600), "", "");
    dump("pe1 600");
    add_proc(c, "pe2", "SELECT 1 FROM t", 1, "a,I;", plist(1500), "");
    dump("pe2 out 1500");
    add_proc(c, "pe3", "SELECT 1 FROM t", 1, plist(300), "", "");
    dump("pe3 300");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    snprintf(dir, sizeof dir, "%s\\proclongsql", argv[2]);
    c = dictionary_with_table(dir);
    dump("table");
    {
        static const int counts[] = {5, 20, 25, 30, 35, 60, 120};
        for (unsigned i = 0; i < sizeof counts / sizeof *counts; i++) {
            char name[16];
            snprintf(name, sizeof name, "si%d", counts[i]);
            sql_proc(c, name, counts[i], 0, 0);
            dump(name);
            snprintf(name, sizeof name, "so%d", counts[i]);
            sql_proc(c, name, 0, counts[i], 0);
            dump(name);
        }
        sql_proc(c, "sio", 20, 20, 0);
        dump("sio 20/20");
        sql_proc(c, "sic", 40, 0, 1);
        dump("sic 40 + cursor");
        run_sql(c, "CREATE PROCEDURE sx(a INTEGER, b CHAR(10) OUTPUT) FUNCTION f IN LIBRARY "
                   "\"x.aep\"");
        dump("sx");
        sql_proc(c, "si5", 40, 0, -1);
        dump("alter si5 long");
        run_sql(c, "ALTER PROCEDURE si5(a INTEGER) BEGIN SELECT 2 FROM system.iota; END");
        dump("alter si5 short");
        run_sql(c, "DROP PROCEDURE si35");
        dump("drop si35");
        sql_proc(c, "sn", 40, 0, 0);
        dump("sn 40 in the freed slots");
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    return 0;
}
