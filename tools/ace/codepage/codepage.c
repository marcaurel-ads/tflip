/* Records how an Advantage Client Engine's AdsSetStringFromCodePage converts
 * each byte of each code page, for fixtures/codepage/setfromcodepage.json.
 *
 *     codepage.exe <ace32.dll> <data folder>  > setfromcodepage.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). The data
 * folder is connected to as a free-table folder (local server); a table with
 * a Unicode (NChar) field is created, one record appended, and for each
 * candidate code page every byte 0x00..0xFF is set on its own through
 * AdsSetStringFromCodePage and read back as Unicode (AdsGetFieldW). A code
 * page the engine does not know fails every byte with the same code (1565);
 * one it knows maps most bytes and fails the undefined ones. Each is a JSON
 * object with the per-byte return codes and, for the bytes that converted,
 * the UTF-16 unit each produced:
 *
 *     {"code_page": 1250, "rcs": [...256...], "units": [...]}
 *
 * so which pages the engine accepts, and exactly how it maps each byte, is
 * seen without depending on the host's code page. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define FIELD(n) ((U8 *)(ULONG_PTR)(n))

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: codepage.exe <ace32.dll> <data folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "LoadLibrary(%s) failed\n", argv[1]);
        return 2;
    }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn);
    if (rc) {
        fprintf(stderr, "AdsConnect26(%s) = %lu\n", argv[2], rc);
        return 2;
    }
    H stmt = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(conn, &stmt);
    ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)"DROP TABLE cp", &cur);
    rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(
        stmt, (U8 *)"CREATE TABLE cp (u NCHAR(8))", &cur);
    if (rc) {
        fprintf(stderr, "CREATE TABLE = %lu\n", rc);
        return 2;
    }
    char path[1024];
    snprintf(path, sizeof path, "%s\\cp", argv[2]);
    H t = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
        conn, (U8 *)path, NULL, 3 /*ADT*/, 1 /*ANSI*/, 1, 1, 1 /*excl*/, &t);
    if (rc) {
        fprintf(stderr, "AdsOpenTable(%s) = %lu\n", path, rc);
        return 2;
    }
    if (((U32(WINAPI *)(H))F("AdsAppendRecord"))(t)) {
        fprintf(stderr, "AdsAppendRecord failed\n");
        return 2;
    }

    /* The single-byte code pages the first pass found the engine accepts
     * (every byte returned the same 1565 for a page it does not know). */
    U32 pages[] = {437,  500,  737,  775,  850,  852,  855,  856,  857,  858,
                   860,  861,  862,  863,  864,  865,  866,  869,  874,  875,
                   1026, 1250, 1251, 1252, 1253, 1254, 1255, 1256, 1257, 1258,
                   932,  936,  949,  950};
    int npages = sizeof pages / sizeof pages[0];

    printf("[");
    for (int p = 0; p < npages; p++) {
        U32 cp = pages[p];
        printf("%s\n  {\"code_page\": %lu, \"rcs\": [", p ? "," : "", cp);
        U32 rcs[256];
        U16 units[256];
        for (int b = 0; b < 256; b++) {
            U8 one = (U8)b;
            rcs[b] = ((U32(WINAPI *)(H, U32, U8 *, U8 *, U32))F("AdsSetStringFromCodePage"))(
                t, cp, (U8 *)"u", &one, 1);
            units[b] = 0;
            if (rcs[b] == 0) {
                U16 wide[32];
                U32 wlen = 16;
                memset(wide, 0, sizeof wide);
                U32 rw = ((U32(WINAPI *)(H, U8 *, U16 *, U32 *, U16))F("AdsGetFieldW"))(
                    t, (U8 *)"u", wide, &wlen, 0);
                units[b] = (rw == 0 && wlen >= 1) ? wide[0] : 0xFFFF;
            }
            printf("%s%lu", b ? "," : "", rcs[b]);
        }
        printf("], \"units\": [");
        for (int b = 0; b < 256; b++) printf("%s%u", b ? "," : "", units[b]);
        printf("]}");
    }
    printf("\n]\n");
    fflush(stdout);
    /* The DLL's own cleanup faults under Wine here; the process exits anyway. */
    return 0;
}
