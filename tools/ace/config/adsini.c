/* Does ace32.dll itself resolve an ads.ini [Databases] alias?
 *
 *     adsini.exe <ace32.dll> <alias-or-path> <server-types>
 *
 * Connects with AdsConnect26 (server types: 1 LOCAL, 2 REMOTE, 3 both) and
 * prints the folder the connection resolved to, via AdsGetConnectionPath.
 * Run from a directory holding an ads.ini with a [Databases] section; the
 * engine reads that file during connect (seen under WINEDEBUG=+file), but the
 * resolved path shows whether it SUBSTITUTES the alias. It does not: a name is
 * always taken as a folder relative to the current directory, so an alias such
 * as `AllTypesRemote=\\host:port\share;A` resolves to `.\AllTypesRemote`, not
 * the aliased path. Alias resolution lives in the TDataset/ARC Delphi layer
 * (adscnnct.pas `GetAliasInfo`), which hands ACE an already-resolved path.
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
typedef U32(WINAPI *Connect26)(U8 *, U16, H *);
typedef U32(WINAPI *GetConnPath)(H, U8 *, U16 *);

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: adsini <ace32.dll> <alias-or-path> <server-types>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s\n", argv[1]);
        return 1;
    }
    U16 types = (U16)atoi(argv[3]);
    H c = 0;
    U32 rc = ((Connect26)F("AdsConnect26"))((U8 *)argv[2], types, &c);
    U8 buf[512];
    U16 len = sizeof(buf);
    buf[0] = 0;
    U32 rc2 = ((GetConnPath)F("AdsGetConnectionPath"))(c, buf, &len);
    printf("conn(\"%s\", types=%u) rc=%lu  path(rc=%lu)=\"%s\"\n", argv[2], types, rc, rc2,
           (char *)buf);
    return 0;
}
