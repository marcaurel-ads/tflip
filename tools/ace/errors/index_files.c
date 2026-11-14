/* Records when an Advantage Client Engine takes an index file for the
 * table's auto-open (structural) one in AdsOpenIndex, AdsCloseIndex and
 * AdsCloseAllIndexes, and what becomes of index handles across
 * AdsPackTable, for fixtures/errors/index_files.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o index_files.exe index_files.c
 *     cd <empty folder> && wine index_files.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (ADT, CDX,
 * VFP) and each way of naming the index file (by its name, "t.cdx", or by
 * its full path), the probe runs the scenarios below on a table t<n> of the
 * field K C(4) and the records A, B, C. Each row is
 *
 *     {"type", "path": "name"|"full", "scenario", "step", "rc", ...}
 *
 * with "n" (AdsGetNumIndexes after the step), "error" (the text
 * AdsGetLastError gives after a step that fails; before each AdsCloseIndex
 * and AdsOpenIndex, AdsGetRecordCount with option 0 leaves its own error,
 * so a call that leaves the last error alone shows that one) and, for
 * AdsOpenIndex, "len" and "names" (AdsGetIndexName of each handle written,
 * when it succeeds or the file is open already). Orders are made without
 * ADS_COMPOUND (as OpenADS's tests make them), but in "compound". After a
 * table is closed, a row "closed" says whether its index file is still held
 * (opening it without sharing fails) and gives the DBF header's byte 28
 * ("flags", bit 0: the table has a structural CDX file; -1 for an ADT
 * table). The scenarios:
 *
 *   created   AdsCreateTable, then AdsCreateIndex61 of tags A and B into the
 *             file named after the table (the auto-open one once the table
 *             is reopened); AdsCloseIndex of B's handle, then of A's; then
 *             AdsOpenIndex of the file twice; AdsCloseAllIndexes.
 *   created reopened  that table closed and opened again (exclusive), and
 *             AdsOpenIndex of its file.
 *   reopened  a new table r<n> with tags A and B made the same way, closed
 *             right away and opened again (exclusive); AdsOpenIndex of its
 *             file twice; AdsCloseIndex of A's handle (from
 *             AdsGetIndexHandle); AdsCloseAllIndexes.
 *   other     the reopened table: AdsCreateIndex61 of tag O into o<n>.<ext>,
 *             AdsCloseIndex of it, AdsOpenIndex of o<n> twice, AdsCloseIndex
 *             of its handle twice.
 *   pack      the reopened table, o<n> open again: delete record 2,
 *             AdsPackTable; then AdsGotoTop and AdsCloseIndex of the handles
 *             of A and of O taken before the pack, and AdsGetIndexHandle of A.
 *   later     a new table l<n>, closed and opened again before tags A and
 *             B are made into the file named after it; AdsCloseIndex of B;
 *             AdsOpenIndex of the file on the same handle; then the table
 *             is closed, opened again, and the file opened and A closed.
 *   compound  a new table c<n>, tags A and B made with ADS_COMPOUND into the
 *             file named after it; AdsCloseIndex of B, AdsOpenIndex; the
 *             table closed and opened again, AdsOpenIndex.
 *   created pack  as OpenADS's pack test: a new table p<n>, AdsCreateIndex
 *             of tag A into the file named after it, delete, AdsPackTable,
 *             AdsCloseIndex of the handle. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static int first = 1;
static H current;

/* Leaves an error of its own as the last one (see the header). */
static void provoke(void) {
    U32 count = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(current, 0, &count);
}
static const char *type_name, *path_kind, *scenario;
static char folder[300];

static void row(const char *step, U32 rc, H table) {
    U32 code = 0;
    U8 text[600] = "";
    U16 len = sizeof text;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    U16 n = 777;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumIndexes"))(table, &n);
    printf("%s{\"type\": \"%s\", \"path\": \"%s\", \"scenario\": \"%s\", \"step\": \"%s\", "
           "\"rc\": %lu, \"n\": %u",
           first ? "" : ",\n", type_name, path_kind, scenario, step, rc, n);
    first = 0;
    if (rc) {
        printf(", \"error\": \"");
        for (U8 *c = text; *c; c++) {
            if (*c == '"' || *c == '\\') putchar('\\');
            putchar(*c);
        }
        printf("\"");
    }
}

/* After the table is closed: whether its index file `bag` is still held
 * (opening it without sharing fails), and the DBF header's byte 28, whose
 * bit 0 says the table has a structural CDX file (-1 for an ADT table). */
static void after_close(const char *table_file, const char *bag) {
    HANDLE file = CreateFileA(bag, GENERIC_READ, 0, NULL, OPEN_EXISTING, 0, NULL);
    int held = file == INVALID_HANDLE_VALUE;
    if (!held) CloseHandle(file);
    int flags = -1;
    FILE *dbf = fopen(table_file, "rb");
    if (dbf) {
        unsigned char header[32];
        if (fread(header, 1, 32, dbf) == 32 && strstr(table_file, ".dbf")) flags = header[28];
        fclose(dbf);
    }
    printf("%s{\"type\": \"%s\", \"path\": \"%s\", \"scenario\": \"%s\", \"step\": "
           "\"closed\", \"held\": %d, \"flags\": %d}",
           first ? "" : ",\n", type_name, path_kind, scenario, held, flags);
    first = 0;
}

static void end(void) { printf("}"); }

static U32 create_index_with(H table, const char *bag, const char *tag, U32 options, H *order) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
        "AdsCreateIndex61"))(table, (U8 *)bag, (U8 *)tag, (U8 *)"K", (U8 *)"", (U8 *)"", options,
                             512, order);
}

static U32 create_index(H table, const char *bag, const char *tag, H *order) {
    return create_index_with(table, bag, tag, 0, order);
}

static U32 close_index(H order) {
    provoke();
    return ((U32(WINAPI *)(H))F("AdsCloseIndex"))(order);
}

static H open_index(H table, const char *step, const char *bag) {
    H handles[16] = {0};
    U16 len = 16;
    provoke();
    U32 rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(table, (U8 *)bag, handles,
                                                                   &len);
    row(step, rc, table);
    printf(", \"len\": %u, \"names\": [", len);
    for (int i = 0; (rc == 0 || rc == 5065) && i < len && i < 16; i++) {
        U8 name[64] = "";
        U16 name_len = sizeof name;
        ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetIndexName"))(handles[i], name, &name_len);
        printf("%s\"%s\"", i ? ", " : "", name);
    }
    printf("]");
    end();
    return handles[0];
}

static H handle_of(H table, const char *tag) {
    H order = 0;
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)tag, &order);
    return order;
}

static void fill(H table) {
    static const char *values[] = {"A", "B", "C"};
    for (int i = 0; i < 3; i++) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"K",
                                                                (U8 *)values[i], 1);
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    }
}

static void bag_name(char *out, size_t size, int full, const char *stem, const char *ext) {
    if (full) snprintf(out, size, "%s\\%s.%s", folder, stem, ext);
    else snprintf(out, size, "%s.%s", stem, ext);
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    snprintf(folder, sizeof folder, "%s", argv[2]);
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext, *table_ext; U16 type; } types[] = {
        {"ADT", "adi", "adt", 3},
        {"CDX", "cdx", "dbf", 2},
        {"VFP", "cdx", "dbf", 4},
    };
    printf("[\n");
    int n = 0;
    for (int t = 0; t < 3; t++) {
        type_name = types[t].name;
        const char *ext = types[t].ext;
        for (int full = 0; full < 2; full++, n++) {
            char stem[16], other[16], bag[400], obag[400], pstem[16], pbag[400], rstem[16],
                rbag[400], lstem[16], lbag[400], file[64], plain[64], cstem[16], cbag[400];
            H table = 0, a = 0, b = 0, o = 0;
            path_kind = full ? "full" : "name";
            snprintf(stem, sizeof stem, "t%d", n);
            snprintf(other, sizeof other, "o%d", n);
            snprintf(pstem, sizeof pstem, "p%d", n);
            snprintf(rstem, sizeof rstem, "r%d", n);
            bag_name(bag, sizeof bag, full, stem, ext);
            bag_name(obag, sizeof obag, full, other, ext);
            bag_name(pbag, sizeof pbag, full, pstem, ext);
            bag_name(rbag, sizeof rbag, full, rstem, ext);
            snprintf(lstem, sizeof lstem, "l%d", n);
            bag_name(lbag, sizeof lbag, full, lstem, ext);
            snprintf(cstem, sizeof cstem, "c%d", n);
            bag_name(cbag, sizeof cbag, full, cstem, ext);

            scenario = "created";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)stem, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            current = table;
            if (rc) return printf("create %s: %lu\n", stem, rc), 1;
            fill(table);
            rc = create_index(table, bag, "A", &a);
            row("create A", rc, table), end();
            rc = create_index(table, bag, "B", &b);
            row("create B", rc, table), end();
            row("close B", close_index(b), table), end();
            row("close A", close_index(a), table), end();
            open_index(table, "open", bag);
            open_index(table, "open again", bag);
            row("close all", ((U32(WINAPI *)(H))F("AdsCloseAllIndexes"))(table), table), end();
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            snprintf(file, sizeof file, "%s.%s", stem, types[t].table_ext);
            snprintf(plain, sizeof plain, "%s.%s", stem, ext);
            after_close(file, plain);

            scenario = "created reopened";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)stem, NULL, types[t].type, 1, 0, 1, 1, &table);
            current = table;
            row("opened", rc, table), end();
            open_index(table, "open", bag);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);

            scenario = "reopened";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)rstem, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            current = table;
            if (rc) return printf("create %s: %lu\n", rstem, rc), 1;
            fill(table);
            create_index(table, rbag, "A", &a);
            create_index(table, rbag, "B", &b);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            snprintf(file, sizeof file, "%s.%s", rstem, types[t].table_ext);
            snprintf(plain, sizeof plain, "%s.%s", rstem, ext);
            after_close(file, plain);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)rstem, NULL, types[t].type, 1, 0, 1, 1, &table);
            current = table;
            if (rc) return printf("open %s: %lu\n", rstem, rc), 1;
            row("opened", 0, table), end();
            open_index(table, "open", rbag);
            open_index(table, "open again", rbag);
            row("close A", close_index(handle_of(table, "A")), table), end();
            row("close all", ((U32(WINAPI *)(H))F("AdsCloseAllIndexes"))(table), table), end();

            scenario = "other";
            rc = create_index(table, obag, "O", &o);
            row("create O", rc, table), end();
            row("close O", close_index(o), table), end();
            o = open_index(table, "open", obag);
            open_index(table, "open again", obag);
            row("close O", close_index(o), table), end();
            row("close O again", close_index(o), table), end();

            scenario = "pack";
            o = open_index(table, "open", obag);
            a = handle_of(table, "A");
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
            ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            row("pack", ((U32(WINAPI *)(H))F("AdsPackTable"))(table), table), end();
            row("top A", ((U32(WINAPI *)(H))F("AdsGotoTop"))(a), table), end();
            row("top O", ((U32(WINAPI *)(H))F("AdsGotoTop"))(o), table), end();
            row("close A", close_index(a), table), end();
            row("close O", close_index(o), table), end();
            H again = 0;
            rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsGetIndexHandle"))(table, (U8 *)"A", &again);
            row("handle A", rc, table);
            printf(", \"same\": %d}", again == a);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);

            scenario = "later";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)lstem, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            current = table;
            if (rc) return printf("create %s: %lu\n", lstem, rc), 1;
            fill(table);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)lstem, NULL, types[t].type, 1, 0, 1, 1, &table);
            current = table;
            if (rc) return printf("open %s: %lu\n", lstem, rc), 1;
            rc = create_index(table, lbag, "A", &a);
            row("create A", rc, table), end();
            rc = create_index(table, lbag, "B", &b);
            row("create B", rc, table), end();
            row("close B", close_index(b), table), end();
            open_index(table, "open same", lbag);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            snprintf(file, sizeof file, "%s.%s", lstem, types[t].table_ext);
            snprintf(plain, sizeof plain, "%s.%s", lstem, ext);
            after_close(file, plain);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)lstem, NULL, types[t].type, 1, 0, 1, 1, &table);
            current = table;
            row("opened", rc, table), end();
            open_index(table, "open", lbag);
            row("close A", close_index(handle_of(table, "A")), table), end();
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);

            scenario = "compound";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)cstem, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            current = table;
            if (rc) return printf("create %s: %lu\n", cstem, rc), 1;
            fill(table);
            row("create A", create_index_with(table, cbag, "A", 2, &a), table), end();
            row("create B", create_index_with(table, cbag, "B", 2, &b), table), end();
            row("close B", close_index(b), table), end();
            open_index(table, "open", cbag);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
            snprintf(file, sizeof file, "%s.%s", cstem, types[t].table_ext);
            snprintf(plain, sizeof plain, "%s.%s", cstem, ext);
            after_close(file, plain);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
                conn, (U8 *)cstem, NULL, types[t].type, 1, 0, 1, 1, &table);
            current = table;
            row("opened", rc, table), end();
            open_index(table, "open", cbag);
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);

            scenario = "created pack";
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)pstem, NULL, types[t].type, 1, 0, 1, 0,
                                   (U8 *)"K,C,4,0", &table);
            current = table;
            if (rc) return printf("create %s: %lu\n", pstem, rc), 1;
            fill(table);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
                table, (U8 *)pbag, (U8 *)"A", (U8 *)"K", NULL, NULL, 0, &a);
            row("create A", rc, table), end();
            ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, 2);
            ((U32(WINAPI *)(H))F("AdsDeleteRecord"))(table);
            ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
            row("pack", ((U32(WINAPI *)(H))F("AdsPackTable"))(table), table), end();
            row("top A", ((U32(WINAPI *)(H))F("AdsGotoTop"))(a), table), end();
            row("close A", close_index(a), table), end();
            ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
