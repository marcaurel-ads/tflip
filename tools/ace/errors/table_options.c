/* Records what the original engine answers when AdsCreateTable,
 * AdsCreateTable90, AdsOpenTable and AdsOpenTable90 are given table,
 * character, lock and rights options outside what ace.h defines, for
 * fixtures/errors/table_options.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o table_options.exe table_options.c
 *     cd <empty folder> && wine table_options.exe <ace32.dll> <the folder as a Windows path>
 *
 * Each row is {"call": <function>, "type", "char", "lock", "rights": <the
 * options given>, "rc": <rc>, "handle": <whether a handle came back>,
 * "last": <AdsGetLastError's code>, "text": <its text>, "table_type" and
 * "char_type": what AdsGetTableType and AdsGetTableCharType report for the
 * handle, 0 without one, "collation": AdsOpenTable90's collation, null for
 * none}. Then come opens with collations, and last the opens on a
 * dictionary connection (calls "AdsOpenTable (dictionary)"): of the
 * dictionary's table t (dt.adt) as ADS_DEFAULT, and of the free table
 * b3.adt by its name. Before each
 * call an error is provoked (AdsGetRecordCount with filter option 0; its
 * code is the first row's "last"), so a call that leaves the last error
 * alone shows that.
 * The creates make a table of their own each (c<n>); the opens open b<type>
 * (b1.dbf NTX, b2.dbf CDX, b3.adt, b4.dbf VFP; b2.dbf for other types),
 * which the probe creates first, shared and read-only. A handle that comes
 * back is closed at once. The folder must be empty, and the current
 * directory: the original resolves relative table names against the
 * current directory, not the connection's folder (docs/DIFFERENCES.md).
 * Character types 3 to 118 (ADS_MAX_CHAR_SETS) are ace.h's VFP collations. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static int first = 1, made = 0;
static H conn, keep;
/* While the dictionary rows are made: the connection the opens use, and
 * their label. */
static H open_conn;
static const char *open_label;
typedef U32(WINAPI *CreateFn)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *);

static void provoke(void) {
    U32 n;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(keep, 0, &n);
}

static void row_with(const char *call, int type, int chars, int lock, int rights, U32 rc, H table,
                     const char *collation) {
    U32 code = 0;
    U8 text[600] = "";
    U16 len = sizeof text, table_type = 0, char_type = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    if (table) {
        ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(table, &table_type);
        ((U32(WINAPI *)(H, U16 *))F("AdsGetTableCharType"))(table, &char_type);
    }
    printf("%s{\"call\": \"%s\", \"type\": %d, \"char\": %d, \"lock\": %d, \"rights\": %d, "
           "\"rc\": %lu, \"handle\": %d, \"last\": %lu, \"text\": \"",
           first ? "" : ",\n", call, type, chars, lock, rights, rc, table != 0, code);
    for (U8 *p = text; *p; p++) {
        if (*p == '"' || *p == '\\') putchar('\\');
        putchar(*p);
    }
    printf("\", \"table_type\": %u, \"char_type\": %u, \"collation\": ", table_type, char_type);
    if (collation) printf("\"%s\"}", collation);
    else printf("null}");
    first = 0;
    if (table) ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
}

static void row(const char *call, int type, int chars, int lock, int rights, U32 rc, H table) {
    row_with(call, type, chars, lock, rights, rc, table, NULL);
}

static void create(int v90, int type, int chars, int lock, int rights) {
    char name[16];
    H table = 0;
    U32 rc;
    snprintf(name, sizeof name, "c%d", made++);
    provoke();
    if (v90)
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, U8 *, H *))F(
            "AdsCreateTable90"))(conn, (U8 *)name, NULL, type, chars, lock, rights, 0,
                                 (U8 *)"ID,N,4,0", 0, NULL, &table);
    else
        rc = ((CreateFn)F("AdsCreateTable"))(conn, (U8 *)name, NULL, type, chars, lock, rights,
                                             0, (U8 *)"ID,N,4,0", &table);
    row(v90 ? "AdsCreateTable90" : "AdsCreateTable", type, chars, lock, rights, rc, table);
}

static const char *base_name(int type) {
    switch (type) {
    case 1: return "b1.dbf";
    case 3: return "b3.adt";
    case 4: return "b4.dbf";
    default: return "b2.dbf";
    }
}

static void open_with(int v90, int type, int chars, int lock, int rights, const char *collation) {
    H table = 0;
    U32 rc;
    U8 *name = (U8 *)(open_label && type == 0 ? "t" : base_name(type));
    H c = open_conn ? open_conn : conn;
    provoke();
    if (v90)
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, H *))F(
            "AdsOpenTable90"))(c, name, NULL, type, chars, lock, rights, 6, (U8 *)collation,
                               &table);
    else
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            c, name, NULL, type, chars, lock, rights, 6, &table);
    row_with(open_label ? open_label : v90 ? "AdsOpenTable90" : "AdsOpenTable", type, chars, lock,
             rights, rc, table, collation);
}

static void open_table(int v90, int type, int chars, int lock, int rights) {
    open_with(v90, type, chars, lock, rights, NULL);
}

/* AdsOpenTable90 with collations, which can stand for the character type. */
static const char *collations[] = {"", "ansi", "ANSI", "oem", "GENERAL_VFP_CI_AS_1252",
                                   "nonsense"};
static const int collation_chars[] = {0, 1, 2, 3, 118, 119, 0x100};

/* Each option alone over these values, the others valid; then pairs of bad
 * options, which show the order of the checks. */
static const int values[] = {0, 1, 2, 3, 4, 5, 7, 117, 118, 119, 0xFF, 0x100, 0x101, 0xFFFF};
#define N (int)(sizeof values / sizeof *values)

static void matrix(void (*call)(int, int, int, int, int), int v90, int type) {
    for (int i = 0; i < N; i++) call(v90, type, values[i], 1, 1);
    for (int i = 0; i < N; i++) call(v90, type, 1, values[i], 1);
    for (int i = 0; i < N; i++) call(v90, type, 1, 1, values[i]);
    call(v90, type, 0, 2, 1);
    call(v90, type, 0, 1, 0);
    call(v90, type, 1, 2, 0);
    call(v90, type, 0, 2, 0);
}

/* The opens on a dictionary connection; nonzero if the dictionary could not
 * be made. */
static int dictionary(const char *folder) {
    char path[600];
    H dd = 0, t = 0;
    snprintf(path, sizeof path, "%s\\test.add", folder);
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1); /* local; else AdsDDCreate is 6420 */
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &dd);
    if (rc) return printf("AdsDDCreate: %lu\n", rc), 1;
    rc = ((CreateFn)F("AdsCreateTable"))(conn, (U8 *)"dt.adt", NULL, 3, 1, 1, 1, 0,
                                         (U8 *)"ID,N,4,0", &t);
    if (rc) return printf("create dt.adt: %lu\n", rc), 1;
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))(
        dd, (U8 *)"t", (U8 *)"dt.adt", 3, 1, NULL, NULL);
    if (rc) return printf("AdsDDAddTable: %lu\n", rc), 1;
    open_conn = dd;
    open_label = "AdsOpenTable (dictionary)";
    matrix(open_table, 0, 0);
    matrix(open_table, 0, 3);
    open_conn = 0;
    open_label = NULL;
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(dd);
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    for (int type = 1; type <= 4; type++) {
        H base = 0;
        rc = ((CreateFn)F("AdsCreateTable"))(conn, (U8 *)base_name(type), NULL, type, 1, 1, 1, 0,
                                             (U8 *)"ID,N,4,0", &base);
        if (rc) return printf("create %s: %lu\n", base_name(type), rc), 1;
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(base);
    }
    rc = ((CreateFn)F("AdsCreateTable"))(conn, (U8 *)"keep.adt", NULL, 3, 1, 1, 1, 0,
                                         (U8 *)"ID,N,4,0", &keep);
    if (rc) return printf("create keep.adt: %lu\n", rc), 1;

    printf("[\n");
    provoke();
    row("provoked", 0, 0, 0, 0, 0, 0);
    for (int v90 = 0; v90 < 2; v90++) {
        for (int type = 1; type <= 4; type++) matrix(create, v90, type);
        for (int i = 0; i < N; i++) create(v90, values[i], 1, 1, 1);
        create(v90, 7, 0, 1, 1);
        create(v90, 7, 1, 1, 0);
        for (int type = 1; type <= 4; type++) matrix(open_table, v90, type);
        for (int i = 0; i < N; i++) open_table(v90, values[i], 1, 1, 1);
        open_table(v90, 7, 0, 1, 1);
        open_table(v90, 7, 1, 1, 0);
    }
    for (int type = 1; type <= 4; type++)
        for (int c = 0; c < (int)(sizeof collations / sizeof *collations); c++)
            for (int k = 0; k < (int)(sizeof collation_chars / sizeof *collation_chars); k++)
                open_with(1, type, collation_chars[k], 1, 1, collations[c]);
    if (dictionary(argv[2])) return 1;
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(keep);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
