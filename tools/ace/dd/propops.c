/* Builds the golden data dictionaries for the user / group / procedure
 * property-setter tests (fixtures/dictionary/ops/{userprop,groupprop,procprop}.*)
 * with the real Advantage Client Engine. Black-box: tflip's own output is
 * compared byte-for-byte (user login secrets masked). Mirrors replops.c.
 *
 *     propops.exe <ace64.dll> <out folder>
 *
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see propops.sh). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

#define ADS_DD_COMMENT             1
#define ADS_DD_USER_DEFINED_PROP   3
#define ADS_DD_PROC_SCRIPT         805
#define ADS_DD_REQUIRE_OLD_PASSWORD 1105

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: rc=%lu %lu %s\n", what, (unsigned long)rc, (unsigned long)code, text);
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

static U32 set_user(H c, const char *name, U16 id, void *val, U16 len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetUserProperty"))
        (c, (U8 *)name, id, val, len);
    if (rc) oops("SetUserProperty", rc);
    return rc;
}
static U32 set_group(H c, const char *name, U16 id, void *val, U16 len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetUserGroupProperty"))
        (c, (U8 *)name, id, val, len);
    if (rc) oops("SetUserGroupProperty", rc);
    return rc;
}
static U32 set_proc(H c, const char *name, U16 id, void *val, U16 len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetProcedureProperty"))
        (c, (U8 *)name, id, val, len);
    if (rc) oops("SetProcedureProperty", rc);
    return rc;
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: propops.exe <ace64.dll> <out>\n"); return 2; }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    const char *out = argv[2];
    H c;

    /* userprop: a user, then its comment, user-defined property and the
     * require-old-password flag set. */
    if ((c = make(out, "userprop"))) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))
            (c, (U8 *)"", (U8 *)"alice", (U8 *)"secret", (U8 *)"");
        if (rc) oops("CreateUser", rc);
        set_user(c, "alice", ADS_DD_COMMENT, (void *)"user note", (U16)strlen("user note") + 1);
        set_user(c, "alice", ADS_DD_USER_DEFINED_PROP, (void *)"ud", 3);
        U16 require = 0; /* do not require the old password */
        set_user(c, "alice", ADS_DD_REQUIRE_OLD_PASSWORD, &require, sizeof require);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* groupprop: a group, then its comment and user-defined property set. */
    if ((c = make(out, "groupprop"))) {
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))
            (c, (U8 *)"g1", (U8 *)"");
        if (rc) oops("CreateUserGroup", rc);
        set_group(c, "g1", ADS_DD_COMMENT, (void *)"group note", (U16)strlen("group note") + 1);
        set_group(c, "g1", ADS_DD_USER_DEFINED_PROP, (void *)"gd", 3);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* procprop: a SQL-script procedure, then its comment and a new script set
     * (procedures reject the user-defined property, 5126). */
    if ((c = make(out, "procprop"))) {
        const char *s1 = " INSERT INTO __output VALUES (1); ";
        const char *s2 = " INSERT INTO __output VALUES (2); ";
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U32, U8 *, U8 *, U8 *))F("AdsDDAddProcedure"))
            (c, (U8 *)"p", (U8 *)s1, (U8 *)"", 0x4, (U8 *)"a,Integer;", (U8 *)"", (U8 *)"");
        if (rc) oops("AddProcedure", rc);
        set_proc(c, "p", ADS_DD_COMMENT, (void *)"proc note", (U16)strlen("proc note") + 1);
        set_proc(c, "p", ADS_DD_PROC_SCRIPT, (void *)s2, (U16)strlen(s2) + 1);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }
    return 0;
}
