/* Makes fixtures/collation/mismatch/custom: an ADT table cu.adt of one
 * CHAR(5) field with five records, and its structural index cu.adi with a
 * plain order A and a custom order B, built under the collation the engine's
 * adslocal.cfg gives (run_mismatch.sh runs it under ANSI_CHAR_SET=German),
 * for the case of reindexing on opening an index file with a custom order.
 *
 *     mismatch_custom.exe <ace32.dll> <empty folder> */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

int main(int argc, char **argv) {
    if (argc != 3) return fprintf(stderr, "usage: mismatch_custom <ace32.dll> <folder>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H connection = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection))
        return fprintf(stderr, "connect failed\n"), 1;
    char path[1024];
    snprintf(path, sizeof path, "%s\\cu.adt", argv[2]);
    H table = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(connection, (U8 *)path, NULL, 3, 1, 1, 2, 0, (U8 *)"c,Char,5;", &table);
    if (rc) return fprintf(stderr, "AdsCreateTable: %lu\n", rc), 1;
    const char *values[] = {"b", "\xe4", "a", "z", "\xf6"};
    for (int i = 0; i < 5; i++) {
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetString"))(table, (U8 *)"c", (U8 *)values[i],
                                                               0);
    }
    ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    H order = 0;
    /* ADS_COMPOUND, then ADS_COMPOUND | ADS_CUSTOM */
    for (int i = 0; i < 2; i++) {
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
            table, (U8 *)"cu.adi", (U8 *)(i ? "B" : "A"), (U8 *)"c", (U8 *)"", (U8 *)"",
            i ? 6 : 2, 0, &order);
        if (rc) return fprintf(stderr, "AdsCreateIndex61: %lu\n", rc), 1;
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    return 0;
}
