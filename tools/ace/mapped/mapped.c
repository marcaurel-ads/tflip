/* What does ace32.dll do with a path through a mapped drive?
 *
 *     mapped.exe <ace32.dll> <path> [<table>]
 *
 * For each server-type mask (1 LOCAL, 2 REMOTE, 3 both) connects to <path>
 * with AdsConnect60 and prints the return code, the error text, the
 * connection type, AdsGetServerName and AdsGetConnectionPath; with <table>,
 * also opens it as an ADT and prints the return code. Run it with <path> on a
 * drive mapped to a share (`net use Y: \\host\share`) and compare with the
 * same folder named by its UNC path.
 *
 * Black-box: only the documented ACE API is called; no bytes are inspected. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(n) GetProcAddress(ace, n)
typedef U32(WINAPI *Connect60)(U8 *, U16, U8 *, U8 *, U32, H *);
typedef U32(WINAPI *GetText)(H, U8 *, U16 *);
typedef U32(WINAPI *GetConnType)(H, U16 *);
typedef U32(WINAPI *GetLastErr)(U32 *, U8 *, U16 *);
typedef U32(WINAPI *OpenTable)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *);
typedef U32(WINAPI *Close)(H);

static void last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof(text);
    text[0] = 0;
    ((GetLastErr)F("AdsGetLastError"))(&code, text, &len);
    printf("  last error %lu: %s\n", code, (char *)text);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: mapped <ace32.dll> <path> [<table>]\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s\n", argv[1]);
        return 1;
    }
    for (U16 types = 1; types <= 3; types++) {
        H c = 0;
        U32 rc = ((Connect60)F("AdsConnect60"))((U8 *)argv[2], types, NULL, NULL, 0, &c);
        printf("connect(\"%s\", types=%u) rc=%lu\n", argv[2], types, rc);
        if (rc != 0) {
            last_error();
            continue;
        }
        U16 kind = 0;
        U32 rk = ((GetConnType)F("AdsGetConnectionType"))(c, &kind);
        printf("  type rc=%lu %u\n", rk, kind);
        U8 buf[512];
        U16 len = sizeof(buf);
        buf[0] = 0;
        U32 rs = ((GetText)F("AdsGetServerName"))(c, buf, &len);
        printf("  server rc=%lu \"%s\"\n", rs, (char *)buf);
        len = sizeof(buf);
        buf[0] = 0;
        U32 rp = ((GetText)F("AdsGetConnectionPath"))(c, buf, &len);
        printf("  path rc=%lu \"%s\"\n", rp, (char *)buf);
        if (argc > 3) {
            H t = 0;
            /* ADS_ADT, ADS_ANSI, ADS_PROPRIETARY_LOCKING, ADS_CHECK_RIGHTS, shared */
            U32 ro = ((OpenTable)F("AdsOpenTable"))(c, (U8 *)argv[3], NULL, 3, 1, 1, 1, 0, &t);
            printf("  open(\"%s\") rc=%lu\n", argv[3], ro);
            if (ro != 0)
                last_error();
            else
                ((Close)F("AdsCloseTable"))(t);
        }
        ((Close)F("AdsDisconnect"))(c);
    }
    return 0;
}
