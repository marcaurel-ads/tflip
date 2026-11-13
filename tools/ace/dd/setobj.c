/* Probes AdsDDSetObjectAccessRights: decodes its access-string grammar (which
 * letter sets which permission bit), how it infers the object type (its
 * signature has no type argument), and that it replaces rather than merges.
 * Reads results back with AdsDDGetPermissions. Black-box, against the real ACE.
 *
 *     setobj.exe <ace64.dll> <out folder>
 *
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see setobj.sh). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: rc=%lu %s\n", what, (unsigned long)rc, text);
}

static H make(const char *out, const char *name) {
    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\%s", out, name);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\%s.add", dir, name);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) { oops(name, rc); return 0; }
    return c;
}

static U32 set_rights(H c, const char *object, const char *grantee, const char *access) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDSetObjectAccessRights"))
        (c, (U8 *)object, (U8 *)grantee, (U8 *)access);
}
static U32 get_mask(H c, const char *grantee, U16 type, const char *object) {
    U32 mask = 0;
    ((U32(WINAPI *)(H, U8 *, U16, U8 *, U8 *, U16, U32 *))F("AdsDDGetPermissions"))
        (c, (U8 *)grantee, type, (U8 *)object, NULL, 0, &mask);
    return mask;
}

/* Decode each letter A..Z on an object: print its set rc and resulting mask. */
static void decode(H c, const char *object, U16 type, const char *grantee) {
    printf("letters on %s (type %u) for %s:\n", object, type, grantee);
    for (char ch = 'A'; ch <= 'Z'; ch++) {
        char s[2] = {ch, 0};
        U32 rc = set_rights(c, object, grantee, s);
        U32 mask = get_mask(c, grantee, type, object);
        if (rc || mask) printf("  %c -> rc=%lu mask=0x%08lx\n", ch, (unsigned long)rc, (unsigned long)mask);
    }
}

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: setobj.exe <ace.dll> <out>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "LoadLibrary %s failed\n", argv[1]); return 2; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    const char *out = argv[2];

    H c = make(out, "setobj");
    if (!c) return 1;

    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *))F("AdsDDAddView"))
        (c, (U8 *)"myview", NULL, (U8 *)"SELECT 1 AS n FROM system.iota");
    if (rc) oops("AddView", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))
        (c, (U8 *)"myproc", (U8 *)"lib.dll", (U8 *)"go", 1, NULL, NULL, NULL);
    if (rc) oops("AddProcedure", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))
        (c, NULL, (U8 *)"BOB", (U8 *)"pw", NULL);
    if (rc) oops("CreateUser", rc);

    decode(c, "myview", 6, "BOB");
    decode(c, "myproc", 10, "BOB");

    /* Object-type inference: same access string, let the engine find the object
     * by name across types. Grant "R" on the view and "A" on the proc by name
     * only, read back with each type. */
    printf("inference: set \"RA\" on myview and myproc by name:\n");
    set_rights(c, "myview", "BOB", "RA");
    printf("  myview as view(6)=0x%08lx\n", (unsigned long)get_mask(c, "BOB", 6, "myview"));
    set_rights(c, "myproc", "BOB", "RA");
    printf("  myproc as proc(10)=0x%08lx\n", (unsigned long)get_mask(c, "BOB", 10, "myproc"));

    /* Replace semantics: set "W" then "R"; W's bits should be gone. */
    printf("replace: W then R on myview:\n");
    set_rights(c, "myview", "BOB", "W");
    printf("  after W=0x%08lx\n", (unsigned long)get_mask(c, "BOB", 6, "myview"));
    set_rights(c, "myview", "BOB", "R");
    printf("  after R=0x%08lx\n", (unsigned long)get_mask(c, "BOB", 6, "myview"));

    /* An unknown object and an unknown grantee. */
    printf("unknown object rc=%lu\n", (unsigned long)set_rights(c, "nope", "BOB", "R"));
    /* Its text is "No object matching the search condition was found in the
     * data dictionary.  The given object was not found." */
    {
        U8 text[1024];
        U16 len = sizeof text;
        U32 code = 0;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf("  %s\n", text);
    }
    printf("unknown grantee rc=%lu\n", (unsigned long)set_rights(c, "myview", "nobody", "R"));

    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    fflush(stdout);
    return 0;
}
