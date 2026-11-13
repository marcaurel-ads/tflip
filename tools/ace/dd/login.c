/* Records how an Advantage Client Engine authenticates a connection's user
 * and password, for fixtures/dictionary/login.json.
 *
 *     login.exe <ace32.dll> <data folder>  > login.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine on a fresh copy of the
 * `dictionary` fixture (see run_login.sh). That dictionary has ADSSYS with
 * password "adssyspw" and a user "alice" with password "secret"
 * (tools/ace/fixtures/dictionary.sql). For a battery of (user, password)
 * pairs it records only AdsConnect60's return code — the thing login
 * enforcement must match. No property or record bytes are touched, so no
 * password material is ever written out.
 *
 * Each line is {"user", "password", "rc"}; a null user or password is JSON
 * null (AdsConnect60 called with a NULL pointer). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

typedef U32(WINAPI *Connect60)(U8 *, U16, U8 *, U8 *, U32, H *);
typedef U32(WINAPI *Disconnect)(H);

static const char *path;
static int first = 1;

static void json_string_or_null(const char *s) {
    if (!s) {
        printf("null");
        return;
    }
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static void probe(const char *user, const char *password) {
    H c = 0;
    U32 rc = ((Connect60)F("AdsConnect60"))((U8 *)path, 1, (U8 *)user, (U8 *)password, 0, &c);
    printf("%s\n{\"user\": ", first ? "[" : ",");
    json_string_or_null(user);
    printf(", \"password\": ");
    json_string_or_null(password);
    printf(", \"rc\": %lu}", rc);
    first = 0;
    if (rc == 0 && c) ((Disconnect)F("AdsDisconnect"))(c);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: login.exe <ace32.dll> <dict path>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s\n", argv[1]);
        return 2;
    }
    path = argv[2];

    /* ADSSYS (the administrator): right, wrong, empty, null, case. */
    probe("ADSSYS", "adssyspw");
    probe("ADSSYS", "wrong");
    probe("ADSSYS", "");
    probe("ADSSYS", NULL);
    probe("ADSSYS", "ADSSYSPW");
    probe("adssys", "adssyspw");

    /* alice (a regular user, password "secret"). */
    probe("alice", "secret");
    probe("alice", "wrong");
    probe("alice", "");
    probe("alice", NULL);
    probe("alice", "Secret");
    probe("alice", "SECRET");
    probe("ALICE", "secret");
    probe("Alice", "secret");

    /* no user / unknown user. */
    probe(NULL, NULL);
    probe(NULL, "secret");
    probe("nobody", "x");
    probe("", "");

    printf("\n]\n");
    FreeLibrary(ace);
    return 0;
}
