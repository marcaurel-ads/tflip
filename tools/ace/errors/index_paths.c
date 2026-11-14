/* Records which folder an Advantage Client Engine opens an index file named
 * relatively from, for fixtures/errors/index_paths.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o index_paths.exe index_paths.c
 *     cd <W> && wine index_paths.exe <ace32.dll> <C> <layout>
 *
 * C is the folder the probe connects to (a Windows path) and W the current
 * directory; the layout names the run: "same" with C the current directory,
 * "split" with another. For each table type (ADT, NTX, CDX) the probe makes
 * the table t<type> of one field ID N(4) in C\tables, by its full path, with
 * one record, and orders on ID (compound for ADT and CDX) in the files
 *
 *     x1 in C\tables (the table's folder)    x2 in C
 *     x3 in W                                x4 in C\tables\data
 *     x5 in W\data                           x6 in C\data
 *
 * each also by its full path (x<n>_<type>.<adi|ntx|cdx>), and gives the rows
 *
 *     {"layout", "type", "create", "rc", "file"}
 *
 * of AdsCreateIndex61 of an order in b<type> (a bare name) and data\d<type>,
 * "file" being where AdsGetIndexFilename says it is: "C\..." or "W\..."
 * (C first where both are the same folder). With all of them closed, it
 * gives for each name below the row
 *
 *     {"layout", "type", "name", "rc", "orders"}
 *
 * of AdsOpenIndex of the name, closing what it opened. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static const char *names[] = {
    "x1_%u.%s",       "x2_%u.%s",        "x3_%u.%s",        "data\\x4_%u.%s",
    "data\\x5_%u.%s", "data\\x6_%u.%s",  "tables\\x1_%u.%s", "data/x5_%u.%s",
    "sub\\x1_%u.%s",  ".\\x1_%u.%s",     NULL,
};

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    const char *c = argv[2], *layout = argv[3];
    char w[MAX_PATH];
    GetCurrentDirectoryA(MAX_PATH, w);
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)c, 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name, *ext; U16 type; U32 options; } types[] = {
        {"ADT", "adi", 3, 2}, {"NTX", "ntx", 1, 0}, {"CDX", "cdx", 2, 2}};
    int first = 1;
    for (int t = 0; t < 3; t++) {
        char path[MAX_PATH];
        H table = 0;
        snprintf(path, sizeof path, "%s\\tables\\t%u", c, types[t].type);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
            "AdsCreateTable"))(conn, (U8 *)path, NULL, types[t].type, 1, 0, 1, 0,
                               (U8 *)"ID,N,4,0", &table);
        if (rc) return printf("create %s: %lu\n", path, rc), 1;
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        const char *folders[] = {NULL, "%s\\tables", "%s", "%s", "%s\\tables\\data",
                                 "%s\\data", "%s\\data"};
        for (int x = 1; x <= 6; x++) {
            char folder[MAX_PATH], file[MAX_PATH];
            H order = 0;
            snprintf(folder, sizeof folder, folders[x], x == 3 || x == 5 ? w : c);
            snprintf(file, sizeof file, "%s\\x%d_%u.%s", folder, x, types[t].type, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)file, (U8 *)"K", (U8 *)"ID", (U8 *)"", (U8 *)"",
                                     types[t].options, 512, &order);
            if (rc) return printf("index %s: %lu\n", file, rc), 1;
        }
        static const char *creates[] = {"b%u.%s", "data\\d%u.%s", NULL};
        for (int n = 0; creates[n]; n++) {
            char name[64], file[MAX_PATH] = "";
            H order = 0;
            U16 len = sizeof file;
            snprintf(name, sizeof name, creates[n], types[t].type, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F(
                "AdsCreateIndex61"))(table, (U8 *)name, (U8 *)"K", (U8 *)"ID", (U8 *)"", (U8 *)"",
                                     types[t].options, 512, &order);
            if (rc == 0) {
                ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetIndexFilename"))(order, 3, (U8 *)file,
                                                                              &len);
                file[len < sizeof file ? len : sizeof file - 1] = 0;
            }
            const char *at = file, *root = "";
            if (_strnicmp(file, c, strlen(c)) == 0) at = file + strlen(c), root = "C";
            else if (_strnicmp(file, w, strlen(w)) == 0) at = file + strlen(w), root = "W";
            printf("%s{\"layout\": \"%s\", \"type\": \"%s\", \"create\": \"", first ? "" : ",\n",
                   layout, types[t].name);
            for (const char *p = name; *p; p++) printf(*p == '\\' ? "\\\\" : "%c", *p);
            printf("\", \"rc\": %lu, \"file\": \"%s", rc, root);
            for (const char *p = at; *p; p++) printf(*p == '\\' ? "\\\\" : "%c", *p);
            printf("\"}");
            first = 0;
        }
        ((U32(WINAPI *)(H))F("AdsCloseAllIndexes"))(table);
        for (int n = 0; names[n]; n++) {
            char name[64];
            H orders[8];
            U16 count = 8;
            snprintf(name, sizeof name, names[n], types[t].type, types[t].ext);
            rc = ((U32(WINAPI *)(H, U8 *, H *, U16 *))F("AdsOpenIndex"))(table, (U8 *)name,
                                                                         orders, &count);
            if (rc) count = 0;
            for (int o = 0; o < count; o++) ((U32(WINAPI *)(H))F("AdsCloseIndex"))(orders[o]);
            printf("%s{\"layout\": \"%s\", \"type\": \"%s\", \"name\": \"", first ? "" : ",\n",
                   layout, types[t].name);
            for (const char *p = name; *p; p++) printf(*p == '\\' ? "\\\\" : "%c", *p);
            printf("\", \"rc\": %lu, \"orders\": %u}", rc, count);
            first = 0;
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    printf("\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
