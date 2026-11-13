/* Records what an Advantage Client Engine reports about data dictionaries,
 * for fixtures/dictionary/dd.json.
 *
 *     dd.exe <ace32.dll> <data folder> <dictionary> ...  > dd.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on copies of the
 * dictionary fixtures (see run.sh). Each dictionary (a path relative to the
 * data folder) is connected to as ADSSYS. Then, for it:
 *
 * - the connection: its type and path, and connects as other users;
 * - every object of every type AdsDDFindFirstObject lists, under no parent
 *   and under each table;
 * - every property of the database and of each object, through the
 *   property getter of its type (triggers by their qualified names), into
 *   a large buffer, a 3-byte one, and for a numeric property the size it
 *   asks for, or 4 and 2 bytes (the password properties are left out);
 * - each table opened by its name, and some names that are not tables: the
 *   open's return code, then the table's file, alias, record count and
 *   indexes, with whether each is the primary key;
 * - the tables AdsFindFirstTable and AdsFindFirstTable62 list for the
 *   masks "*.adt", "*" and none.
 *
 * Each call is a JSON object:
 *
 *     {"dd": "dictionary/dict.add", "fn": "AdsDDGetTableProperty",
 *      "args": ["customers", 202], "rc": 0, "len": 13, "value": "..."}
 *
 * Outputs start as a sentinel (0x5A bytes) and are recorded whatever the
 * return code: "len" is the length the call wrote back, "value" the bytes
 * it wrote (as hex, sentinel bytes at the end trimmed). The data folder
 * shows as "<data>" in values, and lengths count it as 6 bytes. */
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
#define SENTINEL 0x5A
#define BIG 8000

static int first = 1;
static const char *dd_name = "";

static void json_string(const char *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

/* Starts a call's object: `args` is a JSON array's contents. */
static void begin(const char *fn, const char *args, U32 rc) {
    printf("%s\n{\"dd\": ", first ? "[" : ",");
    json_string(dd_name);
    printf(", \"fn\": \"%s\", \"args\": [%s], \"rc\": %lu", fn, args, rc);
    first = 0;
}

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
}

static size_t written(const U8 *buf, size_t size) {
    while (size && buf[size - 1] == SENTINEL) size--;
    return size;
}

/* A JSON string for an argument list. */
static char *quoted(char *out, const char *s) {
    char *p = out;
    *p++ = '"';
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') *p++ = '\\';
        *p++ = *s;
    }
    *p++ = '"';
    *p = 0;
    return out;
}

static U8 buf[BIG + 16];

/* The property getters: those of one name and those of two (table and
 * field, index or index file). */
typedef U32(WINAPI *Get1)(H, U8 *, U16, void *, U16 *);
typedef U32(WINAPI *Get2)(H, U8 *, U8 *, U16, void *, U16 *);
typedef U32(WINAPI *GetDb)(H, U16, void *, U16 *);

/* The data folder, which values show as "<data>". */
static const char *data_folder = "";

/* Whether the last value held the data folder. */
static int had_folder = 0;

/* Writes the `n` bytes at `bytes` as "value", with the data folder as
 * "<data>", and `len` as "len", less the bytes that saves. */
static void value(const U8 *bytes, size_t n, long len) {
    static U8 out[BIG + 16];
    size_t folder = strlen(data_folder), m = 0;
    had_folder = 0;
    for (size_t i = 0; i < n;) {
        if (folder && i + folder <= n && !_strnicmp((const char *)bytes + i, data_folder, folder)) {
            memcpy(out + m, "<data>", 6);
            m += 6;
            i += folder;
            len -= (long)folder - 6;
            had_folder = 1;
        } else out[m++] = bytes[i++];
    }
    printf(", \"len\": %ld", len);
    hex("value", out, m);
}

/* A property into a large buffer and then into a 3-byte one: a path that
 * does not fit starts as "<da", and its length counts "<data>". */
static void record_property(const char *fn, const char *args, U32 rc, U16 size, U16 len) {
    begin(fn, args, rc);
    printf(", \"size\": %u", size);
    if (size == 3 && had_folder && rc == 5005) {
        printf(", \"len\": %ld", (long)len - (long)strlen(data_folder) + 6);
        hex("value", (const U8 *)"<da", 3);
    } else value(buf, written(buf, size < BIG ? size + 4u : BIG), len);
    printf("}");
}

/* The sizes a property is read into: a large buffer, a 3-byte one, and
 * when the large one is refused as the wrong length (the numeric
 * properties take their exact size), the size the call asked for, or 4
 * and 2 bytes if it did not say. */
#define SIZES(call)                                                        \
    do {                                                                   \
        U16 sizes[] = {BIG, 3, 0, 0};                                      \
        for (int k = 0; k < 4 && sizes[k]; k++) {                          \
            memset(buf, SENTINEL, sizeof buf);                             \
            U16 len = sizes[k];                                            \
            U32 rc = call;                                                 \
            record_property(fn, args, rc, sizes[k], len);                  \
            if (k == 0 && rc == 5133 && len && len < BIG) sizes[2] = len;  \
            if (k == 0 && rc == 5133 && len == BIG) {                      \
                sizes[2] = 4;                                              \
                sizes[3] = 2;                                              \
            }                                                              \
        }                                                                  \
    } while (0)

static void get_db(H c, U16 id) {
    const char *fn = "AdsDDGetDatabaseProperty";
    char args[40];
    snprintf(args, sizeof args, "%u", id);
    SIZES(((GetDb)F(fn))(c, id, buf, &len));
}

static void get1(const char *fn, H c, const char *name, U16 id) {
    char args[600], q[500];
    snprintf(args, sizeof args, "%s, %u", quoted(q, name), id);
    SIZES(((Get1)F(fn))(c, (U8 *)name, id, buf, &len));
}

static void get2(const char *fn, H c, const char *parent, const char *name, U16 id) {
    char args[1200], q1[500], q2[500];
    snprintf(args, sizeof args, "%s, %s, %u", quoted(q1, parent), quoted(q2, name), id);
    SIZES(((Get2)F(fn))(c, (U8 *)parent, (U8 *)name, id, buf, &len));
}

/* The common properties (ADS_DD_COMMENT ... ADS_DD_OPTIONS) and the
 * type's own, from `from` to `to`, leaving out `skip`. */
static void props1(const char *fn, H c, const char *name, U16 from, U16 to, U16 skip1,
                   U16 skip2) {
    for (U16 id = 1; id <= 7; id++) get1(fn, c, name, id);
    for (U16 id = from; id <= to; id++)
        if (id != skip1 && id != skip2) get1(fn, c, name, id);
}

static void props2(const char *fn, H c, const char *parent, const char *name, U16 from,
                   U16 to) {
    for (U16 id = 1; id <= 7; id++) get2(fn, c, parent, name, id);
    for (U16 id = from; id <= to; id++) get2(fn, c, parent, name, id);
}

#define MAX_NAMES 64
typedef struct {
    int n;
    char names[MAX_NAMES][210];
} Names;

typedef U32(WINAPI *FindFirst)(H, U16, U8 *, U8 *, U16 *, H *);
typedef U32(WINAPI *FindNext)(H, H, U8 *, U16 *);

/* Lists the objects of `type` under `parent` (NULL for none), recording
 * each call, into `out`. */
static void find(H c, U16 type, const char *parent, Names *out) {
    out->n = 0;
    char args[600], q[500];
    if (parent) snprintf(args, sizeof args, "%u, %s", type, quoted(q, parent));
    else snprintf(args, sizeof args, "%u, null", type);
    U8 name[260];
    memset(name, SENTINEL, sizeof name);
    U16 len = 250;
    H handle = 0;
    U32 rc = ((FindFirst)F("AdsDDFindFirstObject"))(c, type, (U8 *)parent, name, &len, &handle);
    begin("AdsDDFindFirstObject", args, rc);
    printf(", \"len\": %u", len);
    hex("value", name, written(name, 254));
    printf("}");
    if (rc) return;
    for (;;) {
        if (out->n < MAX_NAMES) {
            memcpy(out->names[out->n], name, len < 209 ? len : 209);
            out->names[out->n][len < 209 ? len : 209] = 0;
            out->n++;
        }
        memset(name, SENTINEL, sizeof name);
        len = 250;
        rc = ((FindNext)F("AdsDDFindNextObject"))(c, handle, name, &len);
        begin("AdsDDFindNextObject", args, rc);
        printf(", \"len\": %u", len);
        hex("value", name, written(name, 254));
        printf("}");
        if (rc) break;
    }
    rc = ((U32(WINAPI *)(H, H))F("AdsDDFindClose"))(c, handle);
    begin("AdsDDFindClose", args, rc);
    printf("}");
}

/* A short name buffer: what AdsDDFindFirstObject does with it. */
static void find_short(H c, U16 type) {
    U8 name[16];
    memset(name, SENTINEL, sizeof name);
    U16 len = 3;
    H handle = 0;
    U32 rc = ((FindFirst)F("AdsDDFindFirstObject"))(c, type, NULL, name, &len, &handle);
    char args[40];
    snprintf(args, sizeof args, "%u, null, \"short\"", type);
    begin("AdsDDFindFirstObject", args, rc);
    printf(", \"len\": %u", len);
    hex("value", name, written(name, 8));
    printf("}");
    if (handle) ((U32(WINAPI *)(H, H))F("AdsDDFindClose"))(c, handle);
}

static void number(const char *fn, const char *args, U32 rc, U32 value) {
    begin(fn, args, rc);
    printf(", \"value\": %lu}", value);
}

static void text(const char *fn, const char *args, U32 rc, const U8 *s, U16 len) {
    begin(fn, args, rc);
    value(s, written(s, len + 2u), len);
    printf("}");
}

typedef U32(WINAPI *OpenTable)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);

/* Opens `name` on `c` and records what the table reports. */
static void open_table(H c, const char *name, U16 rights) {
    char args[600], q[500];
    snprintf(args, sizeof args, "%s, %u", quoted(q, name), rights);
    H t = 0;
    U32 rc = ((OpenTable)F("AdsOpenTable"))(c, (U8 *)name, NULL, 0, 1, 0, rights, 0, &t);
    begin("AdsOpenTable", args, rc);
    printf("}");
    if (rc) return;
    U8 s[600];
    U16 len;
    for (U16 option = 1; option <= 2; option++) {
        memset(s, SENTINEL, sizeof s);
        len = 500;
        rc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetTableFilename"))(t, option, s, &len);
        char a[700];
        snprintf(a, sizeof a, "%s, %u", args, option);
        text("AdsGetTableFilename", a, rc, s, len);
    }
    memset(s, SENTINEL, sizeof s);
    len = 500;
    rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetTableAlias"))(t, s, &len);
    text("AdsGetTableAlias", args, rc, s, len);
    U32 count = 0;
    rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(t, 1, &count);
    number("AdsGetRecordCount", args, rc, count);
    U16 fields = 0;
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(t, &fields);
    number("AdsGetNumFields", args, rc, fields);
    U16 type = 0;
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetTableType"))(t, &type);
    number("AdsGetTableType", args, rc, type);
    U16 indexes = 0;
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(t, &indexes);
    number("AdsGetNumIndexes", args, rc, indexes);
    for (U16 i = 1; i <= indexes; i++) {
        H x = 0;
        rc = ((U32(WINAPI *)(H, U16, H *))F("AdsGetIndexHandleByOrder"))(t, i, &x);
        char a[700];
        snprintf(a, sizeof a, "%s, %u", args, i);
        memset(s, SENTINEL, sizeof s);
        len = 500;
        rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(x, s, &len);
        text("AdsGetIndexName", a, rc, s, len);
        memset(s, SENTINEL, sizeof s);
        len = 500;
        rc = ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetIndexFilename"))(x, 2, s, &len);
        text("AdsGetIndexFilename", a, rc, s, len);
        U16 primary = 0x5A5A;
        rc = ((U32(WINAPI *)(H, U16 *))F("AdsIsIndexPrimaryKey"))(x, &primary);
        number("AdsIsIndexPrimaryKey", a, rc, primary);
        U32 flags = 0;
        rc = ((U32(WINAPI *)(H, U32 *))F("AdsGetIndexFlags"))(x, &flags);
        number("AdsGetIndexFlags", a, rc, flags);
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
}

typedef U32(WINAPI *Connect60)(U8 *, U16, U8 *, U8 *, U32, H *);

static U32 connect_as(const char *path, const char *user, const char *password, H *c) {
    return ((Connect60)F("AdsConnect60"))((U8 *)path, 1, (U8 *)user, (U8 *)password, 0, c);
}

/* The tables AdsFindFirstTable and AdsFindFirstTable62 list. */
static void find_tables(H c, const char *mask) {
    char args[40];
    snprintf(args, sizeof args, mask ? "\"%s\"" : "null", mask);
    U8 name[300], dd[300];
    U16 len = 250, dd_len = 250;
    H handle = 0;
    memset(name, SENTINEL, sizeof name);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *, H *))F("AdsFindFirstTable"))(
        c, (U8 *)mask, name, &len, &handle);
    for (;;) {
        text("AdsFindFirstTable", args, rc, name, len);
        if (rc) break;
        memset(name, SENTINEL, sizeof name);
        len = 250;
        rc = ((U32(WINAPI *)(H, H, U8 *, U16 *))F("AdsFindNextTable"))(c, handle, name, &len);
    }
    if (handle) ((U32(WINAPI *)(H, H))F("AdsFindClose"))(c, handle);
    handle = 0;
    len = 250;
    memset(name, SENTINEL, sizeof name);
    memset(dd, SENTINEL, sizeof dd);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *, U8 *, U16 *, H *))F("AdsFindFirstTable62"))(
        c, (U8 *)mask, dd, &dd_len, name, &len, &handle);
    for (;;) {
        text("AdsFindFirstTable62", args, rc, name, len);
        text("AdsFindFirstTable62 dd", args, rc, dd, dd_len);
        if (rc) break;
        memset(name, SENTINEL, sizeof name);
        memset(dd, SENTINEL, sizeof dd);
        len = 250;
        dd_len = 250;
        rc = ((U32(WINAPI *)(H, H, U8 *, U16 *, U8 *, U16 *))F("AdsFindNextTable62"))(
            c, handle, dd, &dd_len, name, &len);
    }
    if (handle) ((U32(WINAPI *)(H, H))F("AdsFindClose"))(c, handle);
}

static void dictionary(const char *data, const char *rel) {
    data_folder = data;
    dd_name = rel;
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\%s", data, rel);
    for (char *p = path; *p; p++)
        if (*p == '/') *p = '\\';

    /* Connects as other users. */
    static const char *users[][3] = {
        {"none", NULL, NULL}, {"ADSSYS, wrong", "ADSSYS", "wrong"},
        {"adssys, lower case", "adssys", "adssyspw"}, {"unknown user", "nobody", "x"}};
    for (int k = 0; k < 4; k++) {
        H other = 0;
        U32 rc = connect_as(path, users[k][1], users[k][2], &other);
        char args[80];
        snprintf(args, sizeof args, "\"%s\"", users[k][0]);
        begin("AdsConnect60", args, rc);
        printf("}");
        if (!rc) ((U32(WINAPI *)(H))F("AdsDisconnect"))(other);
    }

    H c = 0;
    U32 rc = connect_as(path, "ADSSYS", "adssyspw", &c);
    begin("AdsConnect60", "\"ADSSYS\"", rc);
    printf("}");
    if (rc) return;

    U16 type = 0;
    rc = ((U32(WINAPI *)(H, U16 *))F("AdsGetConnectionType"))(c, &type);
    number("AdsGetConnectionType", "", rc, type);

    get_db(c, 1);
    get_db(c, 2);
    get_db(c, 3);
    for (U16 id = 4; id <= 7; id++) get_db(c, id);
    for (U16 id = 100; id <= 133; id++)
        if (id != 101) get_db(c, id);

    Names tables, list, children;
    find(c, 1, NULL, &tables);
    for (U16 t = 2; t <= 22; t++) {
        if (t == 1 || t == 4 || t == 5 || t == 3) continue;
        find(c, t, NULL, &list);
    }
    find_short(c, 1);

    for (int i = 0; i < tables.n; i++) {
        const char *table = tables.names[i];
        props1("AdsDDGetTableProperty", c, table, 200, 223, 0, 0);
        find(c, 4, table, &children);
        for (int k = 0; k < children.n; k++)
            props2("AdsDDGetFieldProperty", c, table, children.names[k], 300, 313);
        find(c, 3, table, &children);
        for (int k = 0; k < children.n; k++)
            props2("AdsDDGetIndexFileProperty", c, table, children.names[k], 900, 903);
        find(c, 5, table, &children);
        for (int k = 0; k < children.n; k++)
            props2("AdsDDGetIndexProperty", c, table, children.names[k], 400, 415);
        find(c, 14, table, &children);
    }

    struct {
        U16 type;
        const char *fn;
        U16 from, to, skip1, skip2;
    } kinds[] = {
        {2, "AdsDDGetRefIntegrityProperty", 500, 508, 0, 0},
        {6, "AdsDDGetViewProperty", 700, 704, 0, 0},
        {8, "AdsDDGetUserProperty", 1101, 1105, 1101, 1104},
        {9, "AdsDDGetUserGroupProperty", 600, 600, 0, 0},
        {10, "AdsDDGetProcedureProperty", 800, 806, 0, 0},
        {12, "AdsDDGetLinkProperty", 1300, 1303, 0, 0},
        /* Triggers by their qualified names (table::trigger). */
        {20, "AdsDDGetTriggerProperty", 1400, 1409, 0, 0},
    };
    for (size_t k = 0; k < sizeof kinds / sizeof *kinds; k++) {
        find(c, kinds[k].type, NULL, &list);
        for (int i = 0; i < list.n; i++)
            props1(kinds[k].fn, c, list.names[i], kinds[k].from, kinds[k].to, kinds[k].skip1,
                   kinds[k].skip2);
    }
    /* Names that are no object of the type. */
    props1("AdsDDGetTableProperty", c, "nope", 200, 202, 0, 0);
    get2("AdsDDGetFieldProperty", c, "nope", "id", 306);

    for (int i = 0; i < tables.n; i++) {
        open_table(c, tables.names[i], 1);
        open_table(c, tables.names[i], 2);
    }
    /* Other names: a table's file, upper case, a view, none. */
    char file[260];
    if (tables.n) {
        snprintf(file, sizeof file, "%s.adt", tables.names[0]);
        open_table(c, file, 1);
        CharUpperA(strcpy(file, tables.names[0]));
        open_table(c, file, 1);
    }
    find(c, 6, NULL, &list);
    for (int i = 0; i < list.n; i++) open_table(c, list.names[i], 1);
    open_table(c, "nope", 1);

    find_tables(c, "*.adt");
    find_tables(c, "*");
    find_tables(c, NULL);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: dd.exe <ace32.dll> <data folder> <dictionary> ...\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s\n", argv[1]);
        return 1;
    }
    for (int i = 3; i < argc; i++) dictionary(argv[2], argv[i]);
    printf("\n]\n");
    return 0;
}
