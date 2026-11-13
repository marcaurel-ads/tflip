/* Probes what removing objects whose properties continue in the `More
 * Property` memo does to the dictionary's memo file: a view, a procedure, a
 * user, a group and a table (with a field). All but the table and the group
 * have a comment or description too, so that the order of the two frees shows
 * in the free lists. Prints,
 * after each step, the memo references of the dictionary's records; leaves the
 * dictionary as a golden (fixtures/dictionary/removelong). Black-box, against
 * the real ACE.
 *
 *     removelong.exe <ace32.dll> <out folder>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see removelong.sh). */
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

/* A string of `n` filler characters in quotes. */
static char *quoted(int n) {
    static char s[8000];
    s[0] = '\'';
    memset(s + 1, 'x', n - 2);
    s[n - 1] = '\'';
    s[n] = 0;
    return s;
}

/* A parameter list of exactly `n` bytes (n >= 7): "p000,I;" tokens, the last
 * one's name padded. */
static char *plist(int n) {
    static char s[8000];
    int at = 0, k = 0;
    while (n - at >= 14) at += snprintf(s + at, 16, "p%03d,I;", k++);
    snprintf(s + at, 8000 - at, "p%0*d,I;", 3 + (n - at - 7), k);
    return s;
}

static void check(const char *what, U32 rc) {
    printf("  %s -> rc %lu\n", what, rc);
    if (rc) oops(what, rc);
}
static void set(H c, const char *api, const char *name, U16 id, void *value, int len) {
    char what[64];
    snprintf(what, sizeof what, "%s %s %d", api, name, id);
    check(what, ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F(api))(c, (U8 *)name, id, value, (U16)len));
}
static void remove_object(H c, const char *api, const char *name) {
    char what[64];
    snprintf(what, sizeof what, "%s %s", api, name);
    check(what, ((U32(WINAPI *)(H, U8 *))F(api))(c, (U8 *)name));
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: removelong.exe <ace32.dll> <out>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    char dir[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\removed", argv[2]);
    H c = dictionary_with_table(dir);
    char *s;

    /* Objects with a continuation in `More Property` and a comment, so that
     * the order of the two frees shows in the memo file's free lists. */
    check("view", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(
        c, (U8 *)"v1", NULL,
        (U8 *)sql(400)));
    set(c, "AdsDDSetViewProperty", "v1", 1, "vc", 3);
    dump("v1 long statement, comment");
    check("procedure", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))(
        c, (U8 *)"p1", (U8 *)"SELECT 1 FROM t", (U8 *)"", 4, (U8 *)plist(259), (U8 *)"",
        (U8 *)"pc"));
    dump("p1 long input, comment");
    check("user", ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
        c, (U8 *)"", (U8 *)"alice", (U8 *)"secret", (U8 *)""));
    s = quoted(400);
    set(c, "AdsDDSetUserProperty", "alice", 3, s, (int)strlen(s) + 1);
    set(c, "AdsDDSetUserProperty", "alice", 1, "uc", 3);
    dump("alice user-defined 400, comment");
    check("group", ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)"grp",
                                                                                (U8 *)"gd"));
    s = quoted(300);
    set(c, "AdsDDSetUserGroupProperty", "grp", 3, s, (int)strlen(s) + 1);
    dump("grp user-defined 300, description");
    s = quoted(400);
    set(c, "AdsDDSetTableProperty", "t", 3, s, (int)strlen(s) + 1);
    s = quoted(300);
    check("field", ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16, U16, U8 *))F(
        "AdsDDSetFieldProperty"))(c, (U8 *)"t", (U8 *)"s", 3, s, (U16)strlen(s) + 1, 0, NULL));
    dump("t user-defined 400, t.s user-defined 300");

    remove_object(c, "AdsDDRemoveView", "v1");
    dump("v1 removed");
    remove_object(c, "AdsDDDeleteUser", "alice");
    dump("alice removed");
    remove_object(c, "AdsDDRemoveProcedure", "p1");
    dump("p1 removed");
    remove_object(c, "AdsDDDeleteUserGroup", "grp");
    dump("grp removed");
    check("table", ((U32(WINAPI *)(H, U8 *, U16))F("AdsDDRemoveTable"))(c, (U8 *)"t", 0));
    dump("t removed");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    /* Extents (more than 32 blocks) taken from the free runs that removed
     * views leave. */
    snprintf(dir, sizeof dir, "%s\\extents", argv[2]);
    c = dictionary_with_table(dir);
    static const struct { const char *name; int length; } views[] = {
        {"v1", 3000}, {"v2", 2000}, {"v3", 4000}, {"v4", 500}, {"v5", 1000}, {"v6", 1500},
        {"v7", 6000}, {"v8", 300}, {"v9", 2500}, {"v10", 700}, {"v11", 900}};
    #define ADD(i) check(views[i].name, ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))( \
        c, (U8 *)views[i].name, NULL, (U8 *)sql(views[i].length)))
    ADD(0); ADD(1); ADD(2); ADD(3);
    dump("v1 3000, v2 2000, v3 4000, v4 500");
    remove_object(c, "AdsDDRemoveView", "v2");
    dump("v2 removed");
    ADD(4);
    dump("v5 1000");
    ADD(5);
    dump("v6 1500");
    remove_object(c, "AdsDDRemoveView", "v1");
    remove_object(c, "AdsDDRemoveView", "v3");
    dump("v1, v3 removed");
    ADD(6); ADD(7);
    dump("v7 6000, v8 300");
    remove_object(c, "AdsDDRemoveView", "v4");
    remove_object(c, "AdsDDRemoveView", "v5");
    dump("v4, v5 removed");
    ADD(8);
    dump("v9 2500");
    ADD(9); ADD(10);
    dump("v10 700, v11 900");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    return 0;
}
