/* Records which static collations the engine picks when adslocal.cfg leaves
 * ANSI_CHAR_SET (or OEM_CHAR_SET) empty, under the host's locale.
 *
 *     sysdefault.exe <ace32.dll> <empty data folder>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_sysdefault.sh,
 * which sets the locale and adslocal.cfg) or on Windows. It prints the host's
 * locale as Windows reports it, then writes a DBF table `t.dbf` with one
 * CHAR(1) field holding each byte 1..255 (record n holds byte n) by hand, so
 * no character conversion touches the bytes, and indexes it on the field
 * twice: opened ANSI and opened OEM (ADS_OEM). For each it prints the bytes in
 * the NTX order's order (how the static collation in effect weighs them). Then
 * it does the same with an ADT table (always ANSI) and prints, per byte
 * 1..255 in turn, its ADI key (AdsExtractKey: the byte's weight) and what
 * AdsEvalStringExpr makes of UPPER(c) and LOWER(c), and the collation's CRC
 * the ADI file records at 0x14. Bytes are hex:
 *
 *     lcid system=0409 user=0409 thread=0409 acp=1252 oemcp=437 ui=0409/0409
 *     ansi <255 bytes>
 *     oem <255 bytes>
 *     weights <255 bytes>
 *     upper <255 bytes>
 *     lower <255 bytes>
 *     crc <the u32 at 0x14 of the ADI, little-endian, as a number>
 */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

#define COUNT 255 /* bytes 1..255 */

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

static void check(U32 rc, const char *what) {
    if (!rc) return;
    U32 code = 0;
    U8 text[600] = "";
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s failed %lu: %s\n", what, rc, text);
    exit(1);
}

/* A dBase III table with one field C (CHAR(1)) and a record per byte. */
static void write_dbf(const char *path) {
    FILE *f = fopen(path, "wb");
    if (!f) { perror(path); exit(1); }
    U8 head[32] = {0x03, 126, 10, 23};
    U32 n = COUNT;
    U16 hlen = 32 + 32 + 1, rlen = 2;
    memcpy(head + 4, &n, 4);
    memcpy(head + 8, &hlen, 2);
    memcpy(head + 10, &rlen, 2);
    fwrite(head, 1, 32, f);
    U8 field[32] = {'C', 0};
    field[11] = 'C';
    field[16] = 1;
    fwrite(field, 1, 32, f);
    fputc(0x0d, f);
    for (int b = 1; b <= COUNT; b++) {
        fputc(' ', f);
        fputc(b, f);
    }
    fputc(0x1a, f);
    fclose(f);
}

static void walk(H table, H index, const char *label) {
    printf("%s ", label);
    check(((U32(WINAPI *)(H))F("AdsGotoTop"))(index), "AdsGotoTop");
    for (int n = 0; n < COUNT; n++) {
        U32 rec = 0;
        check(((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2 /* ADS_IGNOREFILTERS */, &rec),
              "AdsGetRecordNum");
        printf("%02lx", rec & 0xff);
        check(((U32(WINAPI *)(H, U32))F("AdsSkip"))(index, 1), "AdsSkip");
    }
    printf("\n");
}

static void ntx_order(H conn, const char *dbf, const char *label, U16 chartype, const char *ntx) {
    H table = 0, index = 0;
    check(((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
              conn, (U8 *)dbf, NULL, 1 /* ADS_NTX */, chartype, 0, 1, 1 /* exclusive */, &table),
          "AdsOpenTable");
    check(((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
              table, (U8 *)ntx, (U8 *)"c", (U8 *)"c", (U8 *)"", (U8 *)"", 0, &index),
          "AdsCreateIndex");
    walk(table, index, label);
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
}

/* An ADT table with the same bytes, indexed. */
static void adt(H conn, const char *dir) {
    char path[MAX_PATH], adi[MAX_PATH];
    snprintf(path, sizeof path, "%s\\w.adt", dir);
    snprintf(adi, sizeof adi, "%s\\w.adi", dir);
    H table = 0, index = 0;
    check(((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F("AdsCreateTable"))(
              conn, (U8 *)path, NULL, 3 /* ADS_ADT */, 1, 0, 1, 0, (U8 *)"c,Char,1;", &table),
          "AdsCreateTable");
    for (int b = 1; b <= COUNT; b++) {
        U8 c = (U8)b;
        check(((U32(WINAPI *)(H))F("AdsAppendRecord"))(table), "AdsAppendRecord");
        check(((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"c", &c, 1),
              "AdsSetField");
        check(((U32(WINAPI *)(H))F("AdsWriteRecord"))(table), "AdsWriteRecord");
    }
    check(((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
              table, (U8 *)"w.adi", (U8 *)"c", (U8 *)"c", (U8 *)"", (U8 *)"", 0, &index),
          "AdsCreateIndex");
    printf("weights ");
    for (U32 rec = 1; rec <= COUNT; rec++) {
        check(((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, rec), "AdsGotoRecord");
        U8 key[64];
        U16 len = sizeof key;
        check(((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(index, key, &len), "AdsExtractKey");
        printf("%02x", len ? key[0] : 0);
    }
    printf("\n");
    static const char *const exprs[] = {"UPPER(c)", "LOWER(c)"};
    for (int e = 0; e < 2; e++) {
        printf("%s ", e ? "lower" : "upper");
        for (U32 rec = 1; rec <= COUNT; rec++) {
            check(((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, rec), "AdsGotoRecord");
            U8 s[16];
            U16 len = sizeof s;
            check(((U32(WINAPI *)(H, U8 *, U8 *, U16 *))F("AdsEvalStringExpr"))(table, (U8 *)exprs[e], s,
                                                                               &len),
                  "AdsEvalStringExpr");
            printf("%02x", len ? s[0] : 0);
        }
        printf("\n");
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    FILE *f = fopen(adi, "rb");
    U8 head[0x18] = {0};
    if (f && fread(head, 1, sizeof head, f) == sizeof head) {
        U32 crc;
        memcpy(&crc, head + 0x14, 4);
        printf("crc %lu\n", crc);
    }
    if (f) fclose(f);
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: sysdefault <ace32.dll> <empty data folder>\n");
        return 2;
    }
    printf("lcid system=%04lx user=%04lx thread=%04lx acp=%u oemcp=%u ui=%04x/%04x\n",
           (unsigned long)GetSystemDefaultLCID(), (unsigned long)GetUserDefaultLCID(),
           (unsigned long)GetThreadLocale(), GetACP(), GetOEMCP(), GetSystemDefaultUILanguage(),
           GetUserDefaultUILanguage());
    fflush(stdout);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return printf("cannot load %s\n", argv[1]), 1;
    char dbf[MAX_PATH];
    snprintf(dbf, sizeof dbf, "%s\\t.dbf", argv[2]);
    write_dbf(dbf);
    H conn = 0;
    check(((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
              (U8 *)argv[2], 1 /* ADS_LOCAL_SERVER */, NULL, NULL, 0, &conn),
          "AdsConnect60");
    ntx_order(conn, dbf, "ansi", 1 /* ADS_ANSI */, "a.ntx");
    ntx_order(conn, dbf, "oem", 2 /* ADS_OEM */, "o.ntx");
    adt(conn, argv[2]);
    fflush(stdout);
    /* The engine's own cleanup can fault under Wine; the process ends here. */
    return 0;
}
