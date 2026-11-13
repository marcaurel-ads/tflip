/* Records which date the Advantage Client Engine reads from a text in a date
 * format (AdsSetDateFormat), for tflip_expr::text::parse_date.
 *
 *     dates.exe <ace32.dll> <data folder>  < texts.txt  > dates.txt
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_dates.sh).
 * Makes a free table with a Date and a TimeStamp field; each input line is a
 * text, `format <format>` to set the date format for the lines after it, or
 * a `#` comment.
 * Each text is set with AdsSetDate and with AdsSetField on the Date field,
 * and with AdsSetField on the TimeStamp field; each output line is the
 * format, the text and, for each setter, its return code and the Julian day
 * AdsGetJulian then gives (for the TimeStamp, AdsGetField's text),
 * tab-separated. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef long S32;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: dates.exe <ace32.dll> <folder>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0, t = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\dates.adt", argv[2]);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, (U8 *)"d,Date;ts,TimeStamp", 0, &t);
    if (rc) { fprintf(stderr, "CreateTable rc=%lu\n", rc); return 1; }
    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(t);
    char format[64] = "MM/DD/CCYY";
    char line[128];
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\r\n")] = 0;
        if (line[0] == '#') continue;
        if (strncmp(line, "format ", 7) == 0) {
            strcpy(format, line + 7);
            ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))((U8 *)format);
            continue;
        }
        printf("%s\t%s", format, line);
        for (int i = 0; i < 2; i++) {
            /* A day no text gives, to see whether a failed set leaves it. */
            ((U32(WINAPI *)(H, U8 *, S32))F("AdsSetJulian"))(t, (U8 *)"d", 1000);
            U32 src = ((U32(WINAPI *)(H, U8 *, U8 *, U16))F(i ? "AdsSetField" : "AdsSetDate"))(
                t, (U8 *)"d", (U8 *)line, (U16)strlen(line));
            S32 julian = -1;
            ((U32(WINAPI *)(H, U8 *, S32 *))F("AdsGetJulian"))(t, (U8 *)"d", &julian);
            printf("\t%lu\t%ld", src, julian);
        }
        ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(t, (U8 *)"ts", (U8 *)"01/01/1990 01:01:01", 19);
        U32 src = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(
            t, (U8 *)"ts", (U8 *)line, strlen(line));
        U8 out[64];
        U32 len = sizeof out - 1;
        memset(out, 0, sizeof out);
        ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(t, (U8 *)"ts", out, &len, 0);
        printf("\t%lu\t%.*s", src, (int)len, out);
        printf("\n");
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(t);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
