/* Probe: does AdsFindServers enumerate a running Advantage server?
 *
 *   findservers.exe <ace32.dll> [<remote Data Source for the control connect>]
 *
 * Calls AdsFindServers with a few option masks, reports rc + last error, and
 * (on success) walks the returned cursor, dumping every field of every row.
 * If a Data Source is given, it also does a plain remote AdsConnect60 to prove
 * the server is actually up and reachable, so "discovery broken" can be told
 * apart from "no server running". JSON to stdout. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(n) GetProcAddress(ace, n)

static void last_error(void) {
    U32 code = 0xDEAD;
    U8 buf[600];
    U16 len = sizeof buf;
    memset(buf, 0, sizeof buf);
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, buf, &len);
    printf(", \"lasterr\": %lu, \"lastmsg\": \"", code);
    for (U16 i = 0; i < len && buf[i]; i++)
        printf((buf[i] == '"' || buf[i] == '\\') ? "\\%c" : (buf[i] < 32 ? " " : "%c"), buf[i]);
    printf("\"");
}

static void dump_cursor(H h) {
    U32 count = 0xFFFF;
    U32 rc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(h, 2, &count);
    printf(", \"count_rc\": %lu, \"count\": %lu", rc, count);
    U16 nfields = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(h, &nfields);
    printf(", \"nfields\": %u, \"fields\": [", nfields);
    char names[64][64];
    for (U16 i = 0; i < nfields && i < 64; i++) {
        U16 len = 64;
        ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(h, (U16)(i + 1), (U8 *)names[i],
                                                                   &len);
        names[i][len < 64 ? len : 63] = 0;
        printf("%s\"%s\"", i ? ", " : "", names[i]);
    }
    printf("], \"rows\": [");
    ((U32(WINAPI *)(H))F("AdsGotoTop"))(h);
    U16 eof = 0;
    ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(h, &eof);
    int first = 1;
    for (int guard = 0; !eof && guard < 100; guard++) {
        printf("%s{", first ? "" : ", ");
        first = 0;
        for (U16 i = 0; i < nfields && i < 64; i++) {
            U8 val[512];
            U32 vlen = sizeof val;
            memset(val, 0, sizeof val);
            U32 rc2 = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))(
                h, (U8 *)names[i], val, &vlen, 0);
            printf("%s\"%s\": \"", i ? ", " : "", names[i]);
            if (!rc2)
                for (U32 k = 0; k < vlen && val[k]; k++)
                    printf((val[k] == '"' || val[k] == '\\') ? "\\%c" : (val[k] < 32 ? " " : "%c"),
                           val[k]);
            printf("\"");
        }
        printf("}");
        ((U32(WINAPI *)(H, long))F("AdsSkip"))(h, 1);
        ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(h, &eof);
    }
    printf("]");
}

static void find(const char *label, U32 options) {
    H h = 0xDEADBEEF;  /* sentinel: did the engine write the out-handle at all? */
    U32 rc = ((U32(WINAPI *)(U32, H *))F("AdsFindServers"))(options, &h);
    printf("{\"call\": \"AdsFindServers\", \"label\": \"%s\", \"options\": %lu, \"rc\": %lu",
           label, options, rc);
    last_error();
    printf(", \"htable\": \"0x%llx\", \"wrote_handle\": %s", (unsigned long long)h,
           h == 0xDEADBEEF ? "false" : "true");
    /* Interrogate the handle regardless of rc: if the engine built a usable
     * cursor yet still returned an error code, that is an engine bug. */
    if (h != 0xDEADBEEF) {
        U16 nf = 0xFFFF;
        U32 rcf = ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(h, &nf);
        U32 cnt = 0xFFFFFFFF;
        U32 rcc = ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordCount"))(h, 2, &cnt);
        printf(", \"GetNumFields_rc\": %lu, \"nfields\": %u, \"GetRecordCount_rc\": %lu, "
               "\"count\": %lu",
               rcf, nf, rcc, cnt);
        if (!rcf) dump_cursor(h);
    }
    printf("},\n");
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 2) {
        fprintf(stderr, "usage: findservers.exe <ace32.dll> [<Data Source>]\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s (err %lu)\n", argv[1], GetLastError());
        return 2;
    }
    printf("[\n");
    find("default", 0);
    find("multicast_only", 0x0001); /* ADS_FS_MULTICAST_ONLY */
    find("connect_all", 0x0002);    /* ADS_FS_CONNECT_ALL */
    find("both", 0x0003);
    find("invalid_4", 0x0004);      /* not a defined option: expect 5019 */

    /* Control: a plain remote connect, to show the server is up and reachable. */
    if (argc >= 3) {
        ((U32(WINAPI *)(U16))F("AdsSetServerType"))(0x0002);
        H conn = 0;
        U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
            (U8 *)argv[2], 0x0002, (U8 *)"adssys", (U8 *)"", 0, &conn);
        printf("{\"call\": \"AdsConnect60\", \"target\": \"%s\", \"rc\": %lu", argv[2], rc);
        last_error();
        if (!rc && conn) {
            U8 d[32], t[32];
            U16 dl = sizeof d, tl = sizeof t;
            long sec = 0;
            d[0] = t[0] = 0;
            U32 r2 = ((U32(WINAPI *)(H, U8 *, U16 *, long *, U8 *, U16 *))F("AdsGetServerTime"))(
                conn, d, &dl, &sec, t, &tl);
            printf(", \"servertime_rc\": %lu, \"date\": \"%s\", \"time\": \"%s\"", r2, d, t);
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(conn);
        }
        printf("}\n");
    }
    printf("]\n");
    return 0;
}
