/* Makes data dictionaries that differ from a base one in one property each,
 * for fixtures/dictionary/variants/: where the dictionary keeps each
 * property shows in the bytes that differ.
 *
 *     variants.exe <ace32.dll> <folder> <variants.txt>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see variants.sh).
 * Each line of variants.txt is "<name>: <statement>; <statement>; ...":
 * the dictionary <folder>\<name>\<name>.add is created (no password, not
 * encrypted), the base statements run, then the variant's, each through
 * AdsExecuteSQLDirect. A statement that fails is reported on stderr and the
 * program exits with 1. Lines starting with '#' are comments. */
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

/* What every dictionary has before its variant's statements. */
static const char *base[] = {
    "CREATE TABLE t (id INTEGER, s CHAR(10))",
};

static int run(H c, const char *variant, const char *sql) {
    H stmt = 0, cursor = 0;
    U32 rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)sql, &cursor);
    if (rc) {
        U8 text[1024];
        U16 len = sizeof text;
        U32 code = 0;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        fprintf(stderr, "%s: %s: %lu %s\n", variant, sql, rc, text);
    }
    if (cursor) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cursor);
    if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
    return rc != 0;
}

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: variants.exe <ace32.dll> <folder> <variants.txt>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s\n", argv[1]);
        return 1;
    }
    /* The local server, which AdsDDCreate needs to find. */
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    FILE *list = fopen(argv[3], "r");
    if (!list) {
        fprintf(stderr, "cannot read %s\n", argv[3]);
        return 1;
    }
    char line[4096];
    int failed = 0;
    while (fgets(line, sizeof line, list)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0] || line[0] == '#') continue;
        char *colon = strchr(line, ':');
        if (!colon) continue;
        *colon = 0;
        const char *name = line;
        char dir[MAX_PATH], path[MAX_PATH];
        snprintf(dir, sizeof dir, "%s\\%s", argv[2], name);
        CreateDirectoryA(dir, NULL);
        snprintf(path, sizeof path, "%s\\%s.add", dir, name);
        H c = 0;
        U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
        if (rc) {
            fprintf(stderr, "%s: AdsDDCreate: %lu\n", name, rc);
            failed = 1;
            continue;
        }
        for (size_t i = 0; i < sizeof base / sizeof *base; i++) failed |= run(c, name, base[i]);
        for (char *sql = strtok(colon + 1, ";"); sql; sql = strtok(NULL, ";")) {
            while (*sql == ' ') sql++;
            if (*sql) failed |= run(c, name, sql);
        }
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }
    fclose(list);
    return failed;
}
