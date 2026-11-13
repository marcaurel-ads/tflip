/* Records the keys the original engine builds for NChar text (ICU sort keys,
 * :en_US), for working out and checking tflip's (fixtures/seek/nkeys.txt).
 *
 *     nkeys.exe <ace32.dll> <empty folder> <strings file> <length>  > keys.txt
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_nkeys.sh). Each
 * line of the strings file is a string as UTF-16LE hex ("-" for the empty
 * one). The probe creates an ADT table with one NChar(<length>) field and an
 * index on it, appends a record per string (AdsSetStringW), and prints, per
 * record, "<string hex> <AdsExtractKey hex>" ("<string hex> rc <code>" if a
 * call fails). */
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
#define MAX_STRINGS 70000

static char *lines[MAX_STRINGS];

int main(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr, "usage: nkeys.exe <ace32.dll> <folder> <strings> <length>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    const char *folder = argv[2];
    FILE *in = fopen(argv[3], "r");
    if (!in) { fprintf(stderr, "cannot open %s\n", argv[3]); return 2; }
    int n = 0;
    char line[1024];
    while (n < MAX_STRINGS && fgets(line, sizeof line, in)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!line[0]) continue;
        lines[n++] = _strdup(line);
    }
    fclose(in);

    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)folder, 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    char path[MAX_PATH], adi[MAX_PATH], fields[64];
    snprintf(path, sizeof path, "%s\\nkeys.adt", folder);
    snprintf(adi, sizeof adi, "%s\\nkeys.adi", folder);
    snprintf(fields, sizeof fields, "n,NChar,%d", atoi(argv[4]));
    H t = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, H *))F(
        "AdsCreateTable"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, (U8 *)fields, &t);
    if (rc) { fprintf(stderr, "AdsCreateTable rc=%lu\n", rc); return 1; }
    H order = 0;
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *))F("AdsCreateIndex"))(
        t, (U8 *)adi, (U8 *)"N", (U8 *)"n", NULL, NULL, 0, &order);
    if (rc) { fprintf(stderr, "AdsCreateIndex rc=%lu\n", rc); return 1; }
    static U32 wrote[MAX_STRINGS];
    for (int i = 0; i < n; i++) {
        WCHAR text[256];
        U32 len = 0;
        const char *s = lines[i];
        if (strcmp(s, "-") != 0) {
            for (; s[0] && s[1] && s[2] && s[3] && len < 255; s += 4) {
                unsigned lo, hi;
                sscanf(s, "%2x%2x", &lo, &hi);
                text[len++] = (WCHAR)(lo | hi << 8);
            }
        }
        ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
        wrote[i] = ((U32(WINAPI *)(H, U8 *, WCHAR *, U32))F("AdsSetStringW"))(
            t, (U8 *)"n", text, len);
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(t);
    }
    for (int i = 0; i < n; i++) {
        if (wrote[i]) { printf("%s rc %lu\n", lines[i], wrote[i]); continue; }
        ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(t, (U32)(i + 1));
        U8 key[512];
        U16 klen = sizeof key;
        rc = ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsExtractKey"))(order, key, &klen);
        if (rc) { printf("%s rc %lu\n", lines[i], rc); continue; }
        printf("%s ", lines[i]);
        for (int k = 0; k < klen; k++) printf("%02x", key[k]);
        printf("\n");
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
