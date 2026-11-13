/* Records which timestamp the Advantage Client Engine reads from a text as
 * CTOTS()'s argument, as a TimeStamp field's value and as an ADS_STRINGKEY
 * seek key in an order on that field, for
 * tflip_expr::text::parse_timestamp_with.
 *
 *     tstexts.exe <ace32.dll> <data folder>  < texts.txt  > tstexts.tsv
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run_tstexts.sh).
 * Each input line is a text, `format <format>` (AdsSetDateFormat) or
 * `epoch <year>` (AdsSetEpoch) for the lines after it, or a `#` comment.
 *
 * CTOTS('<text>') is read back with DTOS() and with HOUR(), MINUTE(),
 * SECOND() and FRAC_SECOND() as milliseconds. The seek key's timestamp is
 * found by moving the table's one record about: a hard seek on the record
 * left empty tells an empty key; else a binary search over (day, ms) for
 * the smallest record value a soft seek lands on, then a hard seek on it
 * for whether that is the key itself.
 *
 * Each output line, tab-separated: the format, the epoch, the text,
 * CTOTS's DTOS (in brackets, as it is, or E<code>) and milliseconds
 * (or E<code>), AdsSetField's return code on the TimeStamp field and the
 * Julian day and milliseconds it has after it (it had 1990-01-01
 * 01:01:01, day 2447893 3661000), then the seek's return code and its
 * key: `empty`, the Julian day and milliseconds (`~` before them when no
 * record value is the key: then the key lies just before that value), or
 * `>max`. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef long S32;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

static H table, order;

static void put(long long v) {
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
    U32 a = ((U32(WINAPI *)(H, U8 *, S32))F("AdsSetJulian"))(table, (U8 *)"ts", (S32)(v / 86400000));
    U32 b = ((U32(WINAPI *)(H, U8 *, S32))F("AdsSetMilliseconds"))(table, (U8 *)"ts",
                                                            (S32)(v % 86400000));
    U32 c = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    if (a || b || c) fprintf(stderr, "put %lld: %lu %lu %lu\n", v, a, b, c);
}

static U32 seek(const char *text, U16 type, U16 *found) {
    *found = 0;
    return ((U32(WINAPI *)(H, U8 *, U16, U16, U16, U16 *))F("AdsSeek"))(
        order, (U8 *)text, (U16)strlen(text), 2, type, found);
}

static int at_eof(void) {
    U16 eof = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(table, &eof);
    return eof;
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: tstexts.exe <ace32.dll> <folder>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    H conn = 0;
    if (((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &conn)) {
        fprintf(stderr, "connect failed\n");
        return 1;
    }
    char path[MAX_PATH];
    snprintf(path, sizeof path, "%s\\tstexts.adt", argv[2]);
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(conn, (U8 *)path, NULL, 3, 1, 1, 1, 0, (U8 *)"ts,TimeStamp", 0,
                             &table);
    if (rc) { fprintf(stderr, "CreateTable rc=%lu\n", rc); return 1; }
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, U32, H *))F("AdsCreateIndex61"))(
        table, NULL, (U8 *)"ts", (U8 *)"ts", (U8 *)"", (U8 *)"", 2, 512, &order);
    if (rc) { fprintf(stderr, "CreateIndex rc=%lu\n", rc); return 1; }
    ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
    ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    char format[64] = "MM/DD/CCYY";
    int epoch = 1900;
    char line[256], expr[512];
    while (fgets(line, sizeof line, stdin)) {
        line[strcspn(line, "\r\n")] = 0;
        if (line[0] == '#') continue;
        if (strncmp(line, "format ", 7) == 0) {
            strcpy(format, line + 7);
            ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))((U8 *)format);
            continue;
        }
        if (strncmp(line, "epoch ", 6) == 0) {
            epoch = atoi(line + 6);
            ((U32(WINAPI *)(U16))F("AdsSetEpoch"))((U16)epoch);
            continue;
        }
        printf("%s\t%d\t%s", format, epoch, line);
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);

        U8 s[64];
        U16 len = sizeof s - 1;
        snprintf(expr, sizeof expr, "DTOS(TSTOD(CTOTS('%s')))", line);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16 *))F("AdsEvalStringExpr"))(table, (U8 *)expr, s,
                                                                         &len);
        if (rc) printf("\tE%lu", rc);
        else printf("\t[%.*s]", (int)len, s);
        double ms = 0;
        snprintf(expr, sizeof expr,
                 "((HOUR(CTOTS('%s'))*60+MINUTE(CTOTS('%s')))*60+SECOND(CTOTS('%s')))*1000"
                 "+FRAC_SECOND(CTOTS('%s'))",
                 line, line, line, line);
        rc = ((U32(WINAPI *)(H, U8 *, double *))F("AdsEvalNumericExpr"))(table, (U8 *)expr, &ms);
        if (rc) printf("\tE%lu", rc);
        else printf("\t%.0f", ms);

        put(2447893LL * 86400000 + 3661000);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsSetField"))(table, (U8 *)"ts", (U8 *)line,
                                                                   strlen(line));
        S32 day = -1, time = -1;
        ((U32(WINAPI *)(H, U8 *, S32 *))F("AdsGetJulian"))(table, (U8 *)"ts", &day);
        ((U32(WINAPI *)(H, U8 *, S32 *))F("AdsGetMilliseconds"))(table, (U8 *)"ts", &time);
        printf("\t%lu\t%ld %ld", rc, day, time);
        ((U32(WINAPI *)(H))F("AdsCancelUpdate"))(table);

        U16 found;
        ((U32(WINAPI *)(H, U8 *))F("AdsSetEmpty"))(table, (U8 *)"ts");
        ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
        rc = seek(line, 2, &found);
        printf("\t%lu", rc);
        if (rc) { printf("\t\n"); continue; }
        if (found) { printf("\tempty\n"); continue; }
        /* Values from day 0 to 9999-12-31's end. */
        long long lo = 0, hi = 5373484LL * 86400000 + 86399999;
        put(hi);
        seek(line, 1, &found);
        if (at_eof()) { printf("\t>max\n"); continue; }
        while (lo < hi) {
            long long mid = lo + (hi - lo) / 2;
            put(mid);
            seek(line, 1, &found);
            if (at_eof()) lo = mid + 1;
            else hi = mid;
        }
        put(lo);
        seek(line, 2, &found);
        printf("\t%s%lld %lld\n", found ? "" : "~", lo / 86400000, lo % 86400000);
    }
    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
    return 0;
}
