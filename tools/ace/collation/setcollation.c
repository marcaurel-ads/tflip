/* What AdsSetCollation accepts, for the collation-name check (7147).
 *
 *     setcollation.exe <ace32.dll> <data folder>
 *
 * Built with i686-w64-mingw32-gcc; run on real Windows (or Wine) with the
 * engine's collation files (ansi.chr, adscollate.adt) beside the DLL. It
 * connects to the data folder as a free-table (local) connection and, for a
 * battery of names, prints AdsSetCollation's return code and the name
 * AdsGetCollation reads back.
 *
 * Findings (ADS 12.0, native VM):
 *   - A name in adscollate.adt is accepted regardless of case and read back
 *     in the stored, upper-case spelling (german_vfp_ci_as_1252 ->
 *     GERMAN_VFP_CI_AS_1252).
 *   - A name not in the repository -- a short alias (GERMAN), another table
 *     type (GERMAN_ADT_CI_AS_1252) or nonsense (Bogus) -- fails with 7147
 *     ("The collation name could not be found in the repository.  <name>"),
 *     leaving the collation unchanged.
 *   - An empty name is accepted and clears the collation.
 * AdsStmtSetTableCollation checks the same repository (tools/ace/sql/sql.c,
 * the `collation` command). */
#include <windows.h>
#include <stdio.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(n) GetProcAddress(ace, n)

static void last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf(" err=\"%.*s\"", (int)len, text);
}

int main(int argc, char **argv) {
    if (argc != 3) return fprintf(stderr, "usage: setcollation <ace32.dll> <folder>\n"), 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H connection = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &connection);
    if (rc) return fprintf(stderr, "connect: %lu\n", rc), 1;
    const char *names[] = {
        "GERMAN_VFP_CI_AS_1252", "german_vfp_ci_as_1252", "GENERAL_VFP_CI_AS_1252",
        "UNIQWT_VFP_CS_AS_1252", "RUSSIAN_VFP_CI_AS_1251", "GERMAN",
        "GERMAN_ADT_CI_AS_1252", "GERMAN_VFP_CI_AS_9999",  "Bogus", "",
    };
    for (int i = 0; i < (int)(sizeof names / sizeof *names); i++) {
        rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetCollation"))(connection, (U8 *)names[i]);
        printf("set \"%s\" rc=%lu", names[i], rc);
        if (rc) last_error();
        U8 got[128];
        U16 len = sizeof got;
        U32 grc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetCollation"))(connection, got, &len);
        printf(" | get rc=%lu val=\"%.*s\"\n", grc, grc ? 0 : (int)len, got);
    }
    return 0;
}
