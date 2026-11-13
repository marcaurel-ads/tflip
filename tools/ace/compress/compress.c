/* Probes the communications-compression options: what AdsConnect101 answers
 * for each `Compression=` value and AdsConnect60 for each ADS_COMPRESS_*
 * option, what the connection then reports (AdsGetConnectionProperty), and
 * the server's COMPRESSION setting as AdsMgGetConfigInfo reports it
 * (ADS_MGMT_CONFIG_PARAMS.usCompressionLevel, offset 0x358).
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o compress.exe compress.c
 *     wine compress.exe <ace32.dll> <folder> <LOCAL|REMOTE>
 *
 * Prints one line per call. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static U32 err(const char *what, U32 rc) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    text[0] = 0;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    else len = 0;
    printf("%-60s rc=%lu %.*s\n", what, rc, (int)(len > 90 ? 90 : len), text);
    return rc;
}

static void props(H c) {
    for (U16 p = 0; p <= 8; p++) {
        U8 v[300];
        U32 len = sizeof v;
        memset(v, 0, sizeof v);
        U32 rc =
            ((U32(WINAPI *)(H, U16, void *, U32 *))F("AdsGetConnectionProperty"))(c, p, v, &len);
        printf("    prop %u rc=%lu len=%lu %02x%02x%02x%02x '%.40s'\n", p, rc, len, v[0], v[1],
               v[2], v[3], (char *)v);
    }
}

static void config(H c) {
    static U8 params[2000], memory[200];
    U16 ps = 864, ms = 56;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16 *, U8 *, U16 *))F("AdsMgGetConfigInfo"))(
        c, params, &ps, memory, &ms);
    printf("    config rc=%lu compression=%u internet=%u port=%u\n", rc,
           params[0x358] | params[0x359] << 8, params[0x34f], params[0x350] | params[0x351] << 8);
}

int main(int argc, char **argv) {
    ace = LoadLibraryA(argv[1]);
    const char *dir = argv[2], *type = argv[3];
    int local = !strcmp(type, "LOCAL");
    const char *values[] = {NULL, "ALWAYS", "always", "Never", "INTERNET", "internet ", "BOGUS",
                            "", "1", "TRUE", "ON", "OFF", "NONE"};
    for (unsigned i = 0; i < sizeof values / sizeof *values; i++) {
        char cs[600], what[120];
        H c = 0;
        /* CommType means nothing locally; the evaluation server takes TCP. */
        const char *comm = local ? "" : "CommType=TCP_IP;";
        if (values[i])
            snprintf(cs, sizeof cs, "Data Source=%s;ServerType=%s;%sCompression=%s;", dir, type,
                     comm, values[i]);
        else
            snprintf(cs, sizeof cs, "Data Source=%s;ServerType=%s;%s", dir, type, comm);
        snprintf(what, sizeof what, "AdsConnect101 Compression=%s",
                 values[i] ? values[i] : "(none)");
        if (!err(what, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &c))) {
            if (i == 0 || i == 1 || i == 3) {
                props(c);
                config(c);
            }
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
        }
    }
    U16 types = local ? 1 : 2;
    U32 options[] = {0, 4, 8, 0xC, 0x10, 0x4 | 0x1000};
    for (unsigned i = 0; i < sizeof options / sizeof *options; i++) {
        char what[120];
        H c = 0;
        snprintf(what, sizeof what, "AdsConnect60 options=0x%lx", options[i]);
        if (!err(what, ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))(
                           (U8 *)dir, types, NULL, NULL, options[i], &c)))
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }
    /* An unreachable server: is a bad value refused before connecting? */
    const char *unreachable[] = {"BOGUS", "ALWAYS"};
    for (unsigned i = 0; i < 2; i++) {
        char cs[300], what[120];
        H c = 0;
        snprintf(cs, sizeof cs,
                 "Data Source=\\\\127.0.0.1:6299\\data;ServerType=REMOTE;CommType=TCP_IP;"
                 "Compression=%s;",
                 unreachable[i]);
        snprintf(what, sizeof what, "AdsConnect101 unreachable Compression=%s", unreachable[i]);
        err(what, ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &c));
    }
    return 0;
}
