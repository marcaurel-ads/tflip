/* Records how an Advantage Client Engine's AdsSetStringFromCodePage converts
 * the four double-byte code pages (932 Shift-JIS, 936 GBK, 949 UHC, 950 Big5)
 * for fixtures/codepage/dbcs.json. Companion to codepage.c, which does the
 * single-byte pages.
 *
 *     codepage2.exe <ace32.dll> <data folder>  > dbcs.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run2.sh). The folder
 * is connected to as a free-table folder; a table with a Unicode (NChar) field
 * is created and, for each page:
 *
 *   - every single byte 0x00..0xFF is set on its own and read back as UTF-16
 *     ("single": for each byte, the return code, the number of UTF-16 units it
 *     produced, and the first two units), so which bytes are stand-alone
 *     characters, which are lead bytes (a lone lead reads back U+FFFD), and
 *     which are undefined is seen;
 *   - each byte is classified as a lead byte or not ("lead_byte": a byte is a
 *     lead, consuming two bytes, iff <byte,'A'> yields one unit);
 *   - for every lead byte, each two-byte sequence <lead,trail> for trail
 *     0x00..0xFF is set and read back ("leads": per lead, the UTF-16 unit for
 *     each trail, U+FFFD where the pair does not map). A lead byte always
 *     consumes two bytes and yields exactly one unit, so no count is needed;
 *     a lead with no mappings at all is omitted (U+FFFD for every trail).
 *
 * The engine converts through its bundled ICU (aicu32.dll), not the host's
 * NLS, so these are the engine's own tables, reproducible under Wine. A field
 * stores a NUL as a space, so byte 0x00 reads back as U+0020; its true value
 * is U+0000 (the reader compensates, as for the single-byte table). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

/* Sets `n` bytes through AdsSetStringFromCodePage and reads the field back as
 * UTF-16. Returns the set return code; on success fills *pn with the unit
 * count and u0/u1 with the first two units. */
static U32 convert(H t, U32 cp, U8 *bytes, U32 n, U32 *pn, U16 *u0, U16 *u1) {
    U32 rc = ((U32(WINAPI *)(H, U32, U8 *, U8 *, U32))F("AdsSetStringFromCodePage"))(
        t, cp, (U8 *)"u", bytes, n);
    *pn = 0;
    *u0 = 0;
    *u1 = 0;
    if (rc == 0) {
        U16 wide[64];
        U32 wlen = 64;
        memset(wide, 0, sizeof wide);
        U32 rw = ((U32(WINAPI *)(H, U8 *, U16 *, U32 *, U16))F("AdsGetFieldW"))(
            t, (U8 *)"u", wide, &wlen, 0);
        if (rw == 0) {
            /* The field is padded with spaces; count only the units before the
             * trailing run of U+0020 (a genuine trailing space is rare in these
             * conversions and would only undercount, which the note flags). */
            U32 end = wlen;
            while (end > 0 && wide[end - 1] == 0x0020) end--;
            *pn = end;
            if (end >= 1) *u0 = wide[0];
            if (end >= 2) *u1 = wide[1];
        }
    }
    return rc;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: codepage2.exe <ace32.dll> <data folder>\n");
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
        stmt, (U8 *)"CREATE TABLE cp (u NCHAR(16))", &cur);
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

    U32 pages[] = {932, 936, 949, 950};
    int npages = sizeof pages / sizeof pages[0];

    printf("{\n\"note\": \"AdsSetStringFromCodePage for the four double-byte code "
           "pages, probed with tools/ace/codepage (codepage2.c). Per page: "
           "single_rc/single_n/single_u record setting each byte 0x00..0xFF on "
           "its own (return code, UTF-16 unit count, first unit); lead_byte[b] "
           "is 1 when byte b consumes two bytes (a lead byte); leads gives, for "
           "each lead byte, the UTF-16 unit of <lead,trail> for trail "
           "0x00..0xFF, U+FFFD where unmapped. Byte 0x00 reads back as U+0020 (a "
           "stored NUL is a space); its true value is U+0000.\",\n\"pages\": [");
    for (int p = 0; p < npages; p++) {
        U32 cp = pages[p];
        U32 n;
        U16 u0, u1;

        printf("%s\n {\"code_page\": %lu,\n", p ? "," : "", cp);

        /* Single bytes: rc, unit count, first unit. */
        printf("  \"single_rc\": [");
        U32 srcs[256], sns[256];
        U16 sus[256];
        for (int b = 0; b < 256; b++) {
            U8 one = (U8)b;
            srcs[b] = convert(t, cp, &one, 1, &n, &u0, &u1);
            sns[b] = n;
            sus[b] = u0;
            printf("%s%lu", b ? "," : "", srcs[b]);
        }
        printf("],\n  \"single_n\": [");
        for (int b = 0; b < 256; b++) printf("%s%lu", b ? "," : "", sns[b]);
        printf("],\n  \"single_u\": [");
        for (int b = 0; b < 256; b++) printf("%s%u", b ? "," : "", sus[b]);
        printf("],\n");

        /* Lead-byte classification: a byte is a lead (consumes two bytes) iff
         * <byte, 'A'> yields a single unit — the engine took the 'A' as the
         * trail. A stand-alone character or an undefined single byte leaves the
         * 'A' as a second unit. This separates true lead bytes and lead bytes
         * with no mapping (all trails U+FFFD, still two bytes) from undefined
         * single bytes (one byte, U+FFFD). */
        printf("  \"lead_byte\": [");
        int is_lead[256];
        for (int b = 0; b < 256; b++) {
            U8 two[2] = {(U8)b, 0x41};
            U32 lrc = convert(t, cp, two, 2, &n, &u0, &u1);
            is_lead[b] = (lrc == 0 && n == 1) ? 1 : 0;
            printf("%s%d", b ? "," : "", is_lead[b]);
        }
        printf("],\n");

        /* Lead bytes 0x81..0xFE: for each trail 0x00..0xFF, the unit count and
         * first unit (and, when two units came back, the second). */
        printf("  \"leads\": [");
        int firstlead = 1;
        for (int lead = 0x81; lead <= 0xFE; lead++) {
            if (!is_lead[lead]) continue; /* only genuine lead bytes have a row */
            U16 tus[256];
            int any = 0;
            for (int tr = 0; tr < 256; tr++) {
                U8 two[2] = {(U8)lead, (U8)tr};
                U32 trc = convert(t, cp, two, 2, &n, &u0, &u1);
                tus[tr] = (trc == 0 && n == 1) ? u0 : 0xFFFD;
                if (tus[tr] != 0xFFFD) any = 1;
            }
            if (!any) continue; /* a lead that maps nothing: U+FFFD by default */
            printf("%s\n   {\"lead\": %d, \"u\": [", firstlead ? "" : ",", lead);
            firstlead = 0;
            for (int tr = 0; tr < 256; tr++) printf("%s%u", tr ? "," : "", tus[tr]);
            printf("]}");
        }
        printf("\n  ]\n }");
    }
    printf("\n]\n}\n");
    fflush(stdout);
    /* The DLL's own cleanup faults under Wine here; the process exits anyway. */
    return 0;
}
