/* Builds golden data dictionaries for tflip's dictionary-writing tests with
 * the real Advantage Client Engine (black-box: tflip's own output is compared
 * to these byte for byte, the login secrets of the Database record aside).
 *
 *     ddwrite.exe <ace32.dll> <out folder>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see ddwrite.sh). Each
 * scenario makes <out>\<name>\, where the tables it adds are first made by the
 * engine on a free connection and copied to <out>\<name>\free\ before they are
 * added, then a dictionary db.add (no password, not encrypted):
 *
 * - viewlong: an ADT table t and views whose statements fit the record's
 *   property field exactly (268 bytes) or go on into the `More Property` memo
 *   (269, 300, 1000 and 3000 bytes, the last one with a comment).
 * - dbftables: DBF tables added to the dictionary, each with its index file:
 *   a FoxPro (CDX) table with a memo and a compound index, a Clipper (NTX)
 *   table with an .ntx index, a Visual FoxPro table with a compound index,
 *   and a CDX table without an index.
 *
 * - dbfcorners: DBF tables of the corners the first ones leave: OEM tables
 *   (made and added with either character type), a Visual FoxPro table with
 *   NULL-able fields (and an index on one), and ADS_DEFAULT as the table type
 *   to add (an ADT table, a DBF table, a missing file).
 *
 * What each table then reports opened by name on the dictionary connection is
 * printed, one line each. */
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

#define ADS_NTX 1
#define ADS_CDX 2
#define ADS_ADT 3
#define ADS_VFP 4
#define ADS_DEFAULT 0
#define ADS_ANSI 1
#define ADS_OEM 2
#define ADS_COMPOUND 2

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s: rc=%lu %s\n", what, rc, text);
}

/* The character type the tables are made with, and whether the first record's
 * name is "\xe9t\xe9" (an accent, to show how an OEM table keeps it). */
static U16 make_chars = ADS_ANSI;
static int accent = 0;

/* Makes the table <dir>\<file> on the free connection c, with two records
 * (id 1 and 2, name "one" and "two") and an index if `index` is given. */
static void make_table(H c, const char *dir, const char *file, U16 type, const char *fields,
                       const char *index, const char *tag) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\%s", dir, file);
    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(c, (U8 *)path, NULL, type, make_chars, 0, 1, 0, (U8 *)fields, &t);
    if (rc) { oops(file, rc); return; }
    const char *names[] = {accent ? "\xe9t\xe9" : "one", "two"};
    for (int i = 0; i < 2; i++) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
        char id[8];
        snprintf(id, sizeof id, "%d", i + 1);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"id", (U8 *)id,
                                                              strlen(id));
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"name", (U8 *)names[i],
                                                              strlen(names[i]));
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    }
    if (index) {
        char ipath[MAX_PATH];
        snprintf(ipath, sizeof ipath, "%s\\%s", dir, index);
        H i = 0;
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
            "AdsCreateIndex61"))(t, (U8 *)ipath, (U8 *)tag, (U8 *)"id", (U8 *)"", (U8 *)"",
                                 tag ? ADS_COMPOUND : 0, 0, &i);
        if (rc) oops(index, rc);
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
}

/* Copies every file of `dir` to `dir`\free. */
static void keep_free(const char *dir) {
    char pattern[MAX_PATH], free_dir[MAX_PATH], from[MAX_PATH], to[MAX_PATH];
    snprintf(free_dir, sizeof free_dir, "%s\\free", dir);
    CreateDirectoryA(free_dir, NULL);
    snprintf(pattern, sizeof pattern, "%s\\*", dir);
    WIN32_FIND_DATAA found;
    HANDLE h = FindFirstFileA(pattern, &found);
    if (h == INVALID_HANDLE_VALUE) return;
    do {
        if (found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
        snprintf(from, sizeof from, "%s\\%s", dir, found.cFileName);
        snprintf(to, sizeof to, "%s\\%s", free_dir, found.cFileName);
        CopyFileA(from, to, FALSE);
    } while (FindNextFileA(h, &found));
    FindClose(h);
}

static H free_connection(const char *dir) {
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)dir, 1, NULL, NULL, 0, &c);
    if (rc) oops("connect", rc);
    return c;
}

static H dictionary(const char *dir) {
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\db.add", dir);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) oops("AdsDDCreate", rc);
    return c;
}

static void add_table_chars(H c, const char *name, const char *file, U16 type, U16 chars,
                            const char *index) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))(
        c, (U8 *)name, (U8 *)file, type, chars, (U8 *)index, (U8 *)"");
    printf("AdsDDAddTable %s (type %u, chars %u): %lu\n", name, type, chars, rc);
    if (rc) oops(name, rc);
}

static void add_table(H c, const char *name, const char *file, U16 type, const char *index) {
    add_table_chars(c, name, file, type, ADS_ANSI, index);
}

/* A view whose statement is `length` bytes long. */
static void add_view(H c, const char *name, int length, const char *comment) {
    char sql[4000];
    const char *head = "SELECT id FROM t WHERE s <> '";
    int fill = length - (int)strlen(head) - 1;
    snprintf(sql, sizeof sql, "%s", head);
    memset(sql + strlen(head), 'x', fill);
    sql[strlen(head) + fill] = '\'';
    sql[strlen(head) + fill + 1] = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))(c, (U8 *)name,
                                                                    (U8 *)comment, (U8 *)sql);
    printf("AdsDDAddView %s (%d): %lu\n", name, (int)strlen(sql), rc);
    if (rc) oops(name, rc);
}

/* What the table `name` reports opened by name on the dictionary connection. */
static void report(H c, const char *name) {
    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        c, (U8 *)name, NULL, 0, ADS_ANSI, 0, 0, 0, &t);
    if (rc) { oops(name, rc); return; }
    U16 type = 0, indexes = 0, fields = 0;
    U32 count = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(t, &type);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(t, &indexes);
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &fields);
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 2, &count);
    printf("open %s: type %u, %u fields, %lu records, %u indexes", name, type, fields, count,
           indexes);
    /* Its table type, character type and memo block size as the dictionary
     * reports them. */
    U16 props[] = {204, 212, 215};
    for (int i = 0; i < 3; i++) {
        U8 value[64] = {0};
        U16 len = 2;
        U32 prc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F("AdsDDGetTableProperty"))(
            c, (U8 *)name, props[i], value, &len);
        printf(", %u: rc %lu len %u value %u", props[i], prc, len, value[0] | value[1] << 8);
    }
    printf("\n");
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    if (accent) {
        for (U16 chars = ADS_ANSI; chars <= ADS_OEM; chars++) {
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                c, (U8 *)name, NULL, 0, chars, 0, 0, 0, &t);
            if (rc) { oops(name, rc); continue; }
            U8 value[64] = {0};
            U32 len = sizeof value;
            ((U32(WINAPI *)(H))F("AdsGotoTop"))(t);
            ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, (U8 *)"name", value,
                                                                         &len, 0);
            printf("  opened with chars %u, name:", chars);
            for (U32 i = 0; i < len; i++) printf(" %02x", value[i]);
            printf("\n");
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
        }
    }
}

/* Tables added with a character type argument of their own. */
static struct { U16 type, chars; } tries[] = {
    {ADS_ADT, 0},  {ADS_ADT, 1},  {ADS_ADT, 2},   {ADS_ADT, 3},     {ADS_ADT, 9},
    {ADS_ADT, 118}, {ADS_ADT, 119}, {ADS_ADT, 65535}, {ADS_VFP, 0},  {ADS_VFP, 3},
    {ADS_VFP, 9},  {ADS_VFP, 119}, {ADS_CDX, 3},  {ADS_CDX, 118},  {ADS_CDX, 119},
    {ADS_CDX, 65535}, {ADS_NTX, 9}, {ADS_NTX, 0},
};
#define NTRIES ((int)(sizeof tries / sizeof tries[0]))

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: ddwrite.exe <ace32.dll> <out folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    char dir[MAX_PATH];

    snprintf(dir, sizeof dir, "%s\\viewlong", argv[2]);
    CreateDirectoryA(dir, NULL);
    H c = free_connection(dir);
    make_table(c, dir, "t.adt", ADS_ADT, "id,Integer;s,Character,20", NULL, NULL);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    keep_free(dir);
    c = dictionary(dir);
    add_table(c, "t", "t.adt", ADS_ADT, "");
    add_view(c, "v268", 268, NULL);
    add_view(c, "v269", 269, NULL);
    add_view(c, "v300", 300, NULL);
    add_view(c, "v1000", 1000, NULL);
    add_view(c, "v3000", 3000, "a comment");
    report(c, "v269");
    report(c, "v3000");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    snprintf(dir, sizeof dir, "%s\\dbftables", argv[2]);
    CreateDirectoryA(dir, NULL);
    c = free_connection(dir);
    make_table(c, dir, "c.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10;notes,Memo",
               "c.cdx", "byid");
    make_table(c, dir, "n.dbf", ADS_NTX, "id,Numeric,5,0;name,Character,10", "n_id.ntx",
               NULL);
    make_table(c, dir, "v.dbf", ADS_VFP, "id,Integer;name,Character,10", "v.cdx", "byid");
    make_table(c, dir, "c0.dbf", ADS_CDX, "id,Numeric,5,0;name,Character,10", NULL, NULL);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    keep_free(dir);
    c = dictionary(dir);
    add_table(c, "c", "c.dbf", ADS_CDX, "c.cdx");
    add_table(c, "n", "n.dbf", ADS_NTX, "n_id.ntx");
    add_table(c, "v", "v.dbf", ADS_VFP, "v.cdx");
    add_table(c, "c0", "c0.dbf", ADS_CDX, "");
    report(c, "c");
    report(c, "n");
    report(c, "v");
    report(c, "c0");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);

    snprintf(dir, sizeof dir, "%s\\dbfcorners", argv[2]);
    CreateDirectoryA(dir, NULL);
    c = free_connection(dir);
    const char *plain = "id,Numeric,5,0;name,Character,10";
    accent = 1;
    make_chars = ADS_OEM;
    make_table(c, dir, "oc.dbf", ADS_CDX, plain, "oc.cdx", "byid");
    make_table(c, dir, "on.dbf", ADS_NTX, plain, "on_id.ntx", NULL);
    make_table(c, dir, "ov.dbf", ADS_VFP, "id,Integer;name,Character,10", "ov.cdx", "byid");
    make_table(c, dir, "oco.dbf", ADS_CDX, plain, NULL, NULL);
    make_chars = ADS_ANSI;
    make_table(c, dir, "oca.dbf", ADS_CDX, plain, NULL, NULL);
    accent = 0;
    make_table(c, dir, "vn.dbf", ADS_VFP,
               "id,Integer,NULL;name,Character,10,NULL;n,Numeric,5,0;d,Date,NULL;m,Memo,NULL",
               "vn.cdx", "byid");
    make_table(c, dir, "vn2.dbf", ADS_VFP, "id,Integer;name,Character,10,NULL", "vn2.cdx",
               "byname");
    make_table(c, dir, "d.adt", ADS_ADT, "id,Integer;name,Character,10", "d.adi", "byid");
    make_table(c, dir, "d1.dbf", ADS_CDX, plain, "d1.cdx", "byid");
    make_table(c, dir, "d2.dbf", ADS_NTX, plain, "d2_id.ntx", NULL);
    make_table(c, dir, "d3.dbf", ADS_VFP, "id,Integer;name,Character,10", NULL, NULL);
    for (int k = 0; k < NTRIES; k++) {
        char file[32];
        snprintf(file, sizeof file, "t%d.%s", k, tries[k].type == ADS_ADT ? "adt" : "dbf");
        make_table(c, dir, file, tries[k].type, "id,Numeric,5,0;name,Character,10", NULL, NULL);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    keep_free(dir);
    c = dictionary(dir);
    /* The character type is the one of the table's own file or the argument. */
    accent = 1;
    add_table_chars(c, "oc", "oc.dbf", ADS_CDX, ADS_OEM, "oc.cdx");
    add_table_chars(c, "on1", "on.dbf", ADS_NTX, ADS_OEM, "on_id.ntx");
    add_table_chars(c, "ov", "ov.dbf", ADS_VFP, ADS_OEM, "ov.cdx");
    add_table_chars(c, "oco", "oco.dbf", ADS_CDX, ADS_ANSI, "");
    add_table_chars(c, "oca", "oca.dbf", ADS_CDX, ADS_OEM, "");
    add_table_chars(c, "oc0", "oco.dbf", ADS_CDX, ADS_DEFAULT, "");
    add_table_chars(c, "ocx", "oca.dbf", ADS_CDX, 9, "");
    report(c, "oc");
    report(c, "on1");
    report(c, "ov");
    report(c, "oco");
    report(c, "oca");
    accent = 0;
    add_table(c, "vn", "vn.dbf", ADS_VFP, "vn.cdx");
    add_table(c, "vn2", "vn2.dbf", ADS_VFP, "vn2.cdx");
    report(c, "vn");
    report(c, "vn2");
    /* ADS_DEFAULT (ADS_DATABASE_TABLE) as the table type. */
    add_table(c, "dd", "d.adt", ADS_DEFAULT, "d.adi");
    add_table(c, "d1", "d1.dbf", ADS_DEFAULT, "d1.cdx");
    add_table(c, "d2", "d2.dbf", ADS_DEFAULT, "d2_id.ntx");
    add_table(c, "d3", "d3.dbf", ADS_DEFAULT, "");
    add_table(c, "dmiss", "missing.adt", ADS_DEFAULT, "");
    add_table(c, "dmiss2", "missing", ADS_DEFAULT, "");
    add_table(c, "dbad", "d1.dbf", 9, "");
    add_table(c, "dmiss3", "missing.adt", ADS_ADT, "");
    add_table(c, "dmiss4", "missing.dbf", ADS_CDX, "");
    add_table(c, "dnoidx", "d1.dbf", ADS_CDX, "nope.cdx");
    add_table(c, "dwrongidx", "d2.dbf", ADS_NTX, "d1.cdx");
    add_table(c, "dwrongidx2", "d1.dbf", ADS_CDX, "d2_id.ntx");
    add_table(c, "dwrongidx3", "d3.dbf", ADS_VFP, "d2_id.ntx");
    add_table(c, "dwrongidx4", "d.adt", ADS_ADT, "d1.cdx");
    add_table(c, "dwrongidx5", "d2.dbf", ADS_NTX, "d.adi");
    add_table(c, "dwrongidx6", "d.adt", ADS_ADT, "d2_id.ntx");
    report(c, "dd");
    report(c, "d1");
    report(c, "d2");
    report(c, "d3");
    /* The character type argument on its own: what each table type takes. */
    for (int k = 0; k < NTRIES; k++) {
        char name[16], file[32];
        snprintf(name, sizeof name, "t%d", k);
        snprintf(file, sizeof file, "t%d.%s", k, tries[k].type == ADS_ADT ? "adt" : "dbf");
        add_table_chars(c, name, file, tries[k].type, tries[k].chars, "");
        report(c, name);
    }
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    return 0;
}
