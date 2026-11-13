/* Records the text the Advantage Client Engine gives a Double field's value
 * (AdsGetField), for the large-double cases of tflip_engine::format.
 *
 *     doubles.exe <ace32.dll> <data folder>  < values.txt  > doubles.txt
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_doubles.sh).
 * Makes a free table with Double fields of 15, 2 and 0 decimals; each input
 * line is a double's 16 hex digits (its bits), set with AdsSetDouble on each
 * field, or `t <text>`, set with AdsSetField; each output line is the bits
 * (with a text, the bits AdsGetDouble then gives) and the three texts,
 * tab-separated. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: doubles.exe <ace32.dll> <folder>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0, t = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\doubles.adt", argv[2]);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0,
        (U8 *)"d15,Double,8,15;d2,Double,8,2;d0,Double,8,0", 0, &t);
    if (rc) { fprintf(stderr, "CreateTable rc=%lu\n", rc); return 1; }
    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    const char *fields[] = {"d15", "d2", "d0"};
    char line[64];
    while (fgets(line, sizeof line, stdin)) {
        unsigned long long bits = 0;
        double v = 0;
        int text = line[0] == 't' && line[1] == ' ';
        line[strcspn(line, "\r\n")] = 0;
        if (text) {
            printf("%s", line + 2);
        } else {
            if (sscanf(line, "%llx", &bits) != 1) continue;
            memcpy(&v, &bits, 8);
            printf("%016llx", bits);
        }
        for (int i = 0; i < 3; i++) {
            U32 src;
            if (text) {
                src = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
                    t, (U8 *)fields[i], (U8 *)line + 2, strlen(line + 2));
                double got = 0;
                ((U32(WINAPI *)(H, U8 *, double *))F("AdsGetDouble"))(t, (U8 *)fields[i], &got);
                memcpy(&bits, &got, 8);
                printf("\t%016llx", bits);
            } else {
                src = ((U32(WINAPI *)(H, U8 *, double))F("AdsSetDouble"))(t, (U8 *)fields[i], v);
            }
            U8 out[64];
            U32 len = sizeof out - 1;
            memset(out, 0, sizeof out);
            U32 grc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                t, (U8 *)fields[i], out, &len, 0);
            if (src || grc) printf("\trc=%lu/%lu", src, grc);
            else printf("\t%.*s", (int)len, out);
        }
        printf("\n");
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
