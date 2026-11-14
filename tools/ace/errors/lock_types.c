/* Records which locking an Advantage Client Engine reports for a table
 * created or opened with each lock type, for fixtures/errors/lock_types.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o lock_types.exe lock_types.c
 *     cd <empty folder> && wine lock_types.exe <ace32.dll> <the folder as a Windows path>
 *
 * The original resolves relative table names against the current directory,
 * so the folder must be the current one too. For each table type (NTX, CDX,
 * ADT, VFP) and each lock type (ADS_COMPATIBLE_LOCKING 0,
 * ADS_PROPRIETARY_LOCKING 1), the probe makes the table l<type>_<lock> of
 * one field ID N(4) with that lock type, closes it, and opens it again with
 * each lock type. Each row is
 *
 *     {"type", "step": "create"|"open", "lock", "rc", "lock_type"}
 *
 * "lock_type" being what AdsGetTableLockType reports (9 where the call was
 * not made). */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static int first = 1;

static void row(const char *type, const char *step, U16 lock, U32 rc, H table) {
    U16 lock_type = 9;
    if (rc == 0) {
        ((U32(WINAPI *)(H, U16 *))F("AdsGetTableLockType"))(table, &lock_type);
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    }
    printf("%s{\"type\": \"%s\", \"step\": \"%s\", \"lock\": %u, \"rc\": %lu, \"lock_type\": %u}",
           first ? "" : ",\n", type, step, lock, rc, lock_type);
    first = 0;
}

int main(int argc, char **argv) {
    if (argc != 3 || !argv[2][0]) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
        (U8 *)argv[2], 1, NULL, NULL, 0, &conn);
    if (rc) return printf("connect: %lu\n", rc), 1;
    static const struct { const char *name; U16 type; } types[] = {
        {"NTX", 1}, {"CDX", 2}, {"ADT", 3}, {"VFP", 4}};
    printf("[\n");
    for (int t = 0; t < 4; t++) {
        for (U16 lock = 0; lock < 2; lock++) {
            char name[16];
            H table = 0;
            snprintf(name, sizeof name, "l%u_%u", types[t].type, lock);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
                "AdsCreateTable"))(conn, (U8 *)name, NULL, types[t].type, 1, lock, 1, 0,
                                   (U8 *)"ID,N,4,0", &table);
            row(types[t].name, "create", lock, rc, table);
            for (U16 open = 0; open < 2; open++) {
                table = 0;
                rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F(
                    "AdsOpenTable"))(conn, (U8 *)name, NULL, types[t].type, 1, open, 1, 0,
                                     &table);
                row(types[t].name, "open", open, rc, table);
            }
        }
    }
    printf("\n]\n");
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
