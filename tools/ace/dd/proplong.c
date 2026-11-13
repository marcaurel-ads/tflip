/* Probes property setters that make an object's properties too long for the
 * dictionary record's `Property` field (275 bytes), or short again: whether the
 * original continues them in the `More Property` memo, what shrinking does to
 * the continuation, and how a comment set afterwards sits in the memo file.
 * Prints, after each step, the memo references of the dictionary's records
 * (More Property at record offset 498, Comment at 515) and the .am's length;
 * leaves the dictionaries as goldens (fixtures/dictionary/proplong). Black-box,
 * against the real ACE.
 *
 *     proplong.exe <ace64.dll> <out folder>
 *
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see proplong.sh). */
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

#define ADS_COMMENT 1
#define ADS_USER_DEFINED 3
#define ADS_VIEW_STMT 700
#define ADS_PROC_SCRIPT 805

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
        static U8 buf[524 * 64];
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
        printf(" am=%lu", GetFileSize(f, NULL));
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

/* A statement `length` bytes long (its NUL aside). */
static char *sql(int length) {
    static char s[8000];
    const char *head = "SELECT id FROM t WHERE s <> '";
    int fill = length - (int)strlen(head) - 1;
    if (fill < 0) { snprintf(s, sizeof s, "SELECT id FROM t"); return s; }
    snprintf(s, sizeof s, "%s", head);
    memset(s + strlen(head), 'x', fill);
    s[strlen(head) + fill] = '\'';
    s[strlen(head) + fill + 1] = 0;
    return s;
}

static void add_view(H c, const char *name, int length) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(c, (U8 *)name, NULL,
                                                                    (U8 *)sql(length));
    if (rc) oops(name, rc);
}
static void view_set(H c, const char *name, U16 id, const char *what, void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetViewProperty"))(
        c, (U8 *)name, id, value, (U16)len);
    printf("  %s %s -> rc %lu\n", name, what, rc);
    if (rc) oops(name, rc);
}
static void view_stmt(H c, const char *name, int length) {
    char what[32], *s = sql(length);
    snprintf(what, sizeof what, "stmt %d", length);
    view_set(c, name, ADS_VIEW_STMT, what, s, (int)strlen(s) + 1);
}
static void view_comment(H c, const char *name, const char *text) {
    view_set(c, name, ADS_COMMENT, text, (void *)text, (int)strlen(text) + 1);
}

/* A string of `n` filler characters in quotes. */
static char *quoted(int n) {
    static char s[8000];
    s[0] = '\'';
    memset(s + 1, 'x', n - 2);
    s[n - 1] = '\'';
    s[n] = 0;
    return s;
}

static void proc_set(H c, const char *name, U16 id, const char *what, void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetProcedureProperty"))(
        c, (U8 *)name, id, value, (U16)len);
    printf("  %s %s -> rc %lu\n", name, what, rc);
    if (rc) oops(name, rc);
}
static void table_set(H c, const char *name, U16 id, const char *what, void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetTableProperty"))(
        c, (U8 *)name, id, value, (U16)len);
    printf("  %s %s -> rc %lu\n", name, what, rc);
    if (rc) oops(name, rc);
}
static void field_set(H c, const char *name, const char *field, U16 id, const char *what,
                      void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
        "AdsDDSetFieldProperty"))(c, (U8 *)name, (U8 *)field, id, value, (U16)len, 0, NULL);
    printf("  %s.%s %s -> rc %lu\n", name, field, what, rc);
    if (rc) oops(name, rc);
}
static void user_set(H c, const char *name, U16 id, const char *what, void *value, int len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetUserProperty"))(
        c, (U8 *)name, id, value, (U16)len);
    printf("  %s %s -> rc %lu\n", name, what, rc);
    if (rc) oops(name, rc);
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: proplong.exe <ace64.dll> <out>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    char dir[MAX_PATH];
    H c;

    snprintf(dir, sizeof dir, "%s\\viewlater", argv[2]);
    c = dictionary_with_table(dir);
    dump("table");
    add_view(c, "v1", 100); view_stmt(c, "v1", 300); dump("v1 100 -> 300");
    add_view(c, "v2", 100); view_stmt(c, "v2", 1000); dump("v2 100 -> 1000");
    view_comment(c, "v2", "note"); dump("v2 comment");
    add_view(c, "v3", 300); view_comment(c, "v3", "c3"); dump("v3 300, comment");
    view_stmt(c, "v3", 1000); dump("v3 -> 1000");
    view_stmt(c, "v3", 300); dump("v3 -> 300");
    view_comment(c, "v3", "c3 longer comment"); dump("v3 comment replaced");
    add_view(c, "v4", 1000); dump("v4 1000");
    view_stmt(c, "v4", 300); dump("v4 -> 300");
    view_stmt(c, "v4", 100); dump("v4 -> 100");
    view_stmt(c, "v4", 300); dump("v4 -> 300 again");
    add_view(c, "v5", 3000); view_comment(c, "v5", "five"); dump("v5 3000, comment");
    view_stmt(c, "v5", 50); dump("v5 -> 50");
    add_view(c, "v6", 100); view_stmt(c, "v6", 268); dump("v6 100 -> 268");
    view_stmt(c, "v6", 269); dump("v6 -> 269");
    view_stmt(c, "v6", 268); dump("v6 -> 268");
    add_view(c, "v7", 300); view_stmt(c, "v7", 301); dump("v7 300 -> 301");
    view_stmt(c, "v7", 5000); dump("v7 -> 5000");
    view_stmt(c, "v7", 3000); dump("v7 -> 3000");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    snprintf(dir, sizeof dir, "%s\\proplater", argv[2]);
    c = dictionary_with_table(dir);
    {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))(
            c, (U8 *)"p1", (U8 *)"SELECT 1 FROM t", (U8 *)"", 4, (U8 *)"a,I;", (U8 *)"b,I;",
            (U8 *)"");
        if (rc) oops("AddProcedure", rc);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
            c, (U8 *)"", (U8 *)"alice", (U8 *)"secret", (U8 *)"");
        if (rc) oops("CreateUser", rc);
    }
    dump("setup");
    char *s = sql(400);
    proc_set(c, "p1", ADS_PROC_SCRIPT, "script 400", s, (int)strlen(s) + 1);
    dump("p1 script 400");
    proc_set(c, "p1", ADS_COMMENT, "comment", "pc", 3);
    dump("p1 comment");
    s = sql(60);
    proc_set(c, "p1", ADS_PROC_SCRIPT, "script 60", s, (int)strlen(s) + 1);
    dump("p1 script 60");

    s = quoted(500);
    table_set(c, "t", ADS_USER_DEFINED, "user-defined 500", s, (int)strlen(s) + 1);
    dump("t user-defined 500");
    table_set(c, "t", ADS_USER_DEFINED, "user-defined 2", "ud", 3);
    dump("t user-defined small");

    s = quoted(400);
    field_set(c, "t", "s", ADS_USER_DEFINED, "user-defined 400", s, (int)strlen(s) + 1);
    dump("t.s user-defined 400");

    s = quoted(400);
    user_set(c, "alice", ADS_USER_DEFINED, "user-defined 400", s, (int)strlen(s) + 1);
    dump("alice user-defined 400");
    user_set(c, "alice", ADS_COMMENT, "comment", "hello", 6);
    dump("alice comment");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    return 0;
}
