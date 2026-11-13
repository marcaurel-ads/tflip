/* Probes how the Advantage Client Engine stores the *encrypted* properties of
 * a data dictionary (admin and user passwords), so it can be studied
 * black-box, without disassembly (see docs/REFERENCES.md "Deliberately
 * excluded"). These properties are encrypted even in a dictionary that is not
 * itself encrypted, with a key the client never supplies -- so the open
 * question is whether a fixed keystream is reused (then known-plaintext breaks
 * it) or a per-object salt is mixed in (then a binary-resident secret is in
 * play and it stays unreadable).
 *
 *     dict.exe <ace32.dll> <data folder> <admin pw> <user pw> [tag]  > out.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). It:
 *
 *   1. creates a fresh, UNencrypted dictionary d<tag>.add in the data folder
 *      (AdsDDCreate with usEncrypt = 0);
 *   2. sets the admin password to <admin pw> (ADS_DD_ADMIN_PASSWORD);
 *   3. creates a user U1 with password <user pw>;
 *   4. closes it, and reads the raw .add file back as hex.
 *
 * The decisive comparison (run.sh drives it): run twice with the SAME admin
 * and user passwords but different tags. If the encrypted bytes in the two
 * .add files are identical, the keystream is reused across dictionaries and a
 * known-plaintext attack recovers it with no key needed; if they differ, each
 * dictionary is salted and the fixed secret cannot be had from the files.
 * A third run with different passwords locates which bytes carry the password. */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define ADS_DD_ADMIN_PASSWORD 101

static int first_item = 1;

static void hex(const char *key, const U8 *bytes, size_t n) {
    printf("%s\"%s\": \"", first_item ? "" : ", ", key);
    for (size_t i = 0; i < n; i++) printf("%02x", bytes[i]);
    printf("\"");
    first_item = 0;
}

static void num(const char *key, long v) {
    printf("%s\"%s\": %ld", first_item ? "" : ", ", key, v);
    first_item = 0;
}

static void str(const char *key, const char *s) {
    printf("%s\"%s\": \"", first_item ? "" : ", ", key);
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    printf("\"");
    first_item = 0;
}

static void last_error(void) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    num("err", code);
    text[len < sizeof text ? len : sizeof text - 1] = 0;
    str("errtext", (char *)text);
}

static long read_file(const char *path, U8 *buf, long max) {
    FILE *f = fopen(path, "rb");
    if (!f) return -1;
    long n = (long)fread(buf, 1, max, f);
    fclose(f);
    return n;
}

int main(int argc, char **argv) {
    if (argc < 5) {
        fprintf(stderr, "usage: dict <ace32.dll> <data folder> <admin pw> <user pw> [tag]\n");
        return 2;
    }
    const char *data_folder = argv[2];
    const char *admin_pw = argv[3];
    const char *user_pw = argv[4];
    const char *tag = argc > 5 ? argv[5] : "";
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);

    char file[MAX_PATH], path[MAX_PATH];
    snprintf(file, sizeof file, "d%s.add", tag);
    snprintf(path, sizeof path, "%s\\%s", data_folder, file);

    printf("{");
    str("file", file);
    str("admin_pw", admin_pw);
    str("user_pw", user_pw);

    /* 1. Create a fresh, unencrypted dictionary. The wall-clock second around
     * the create is recorded so the time-separated runs (run.sh) can be
     * correlated with the per-dictionary variation. */
    num("create_time", (long)time(NULL));
    H dict = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0 /*unencrypted*/,
                                                                     (U8 *)"probe", &dict);
    num("ddcreate_rc", rc);
    if (rc) {
        last_error();
        return printf("}\n"), 1;
    }

    /* 2. Admin password. */
    rc = ((U32(WINAPI *)(H, U16, void *, U16))F("AdsDDSetDatabaseProperty"))(
        dict, ADS_DD_ADMIN_PASSWORD, (void *)admin_pw, (U16)strlen(admin_pw));
    num("set_admin_pw_rc", rc);
    if (rc) last_error();

    /* 3. A user with a known password. */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))(
        dict, NULL, (U8 *)"U1", (U8 *)user_pw, (U8 *)"probe user");
    num("create_user_rc", rc);
    if (rc) last_error();

    ((U32(WINAPI *)(H))F("AdsDisconnect"))(dict);

    /* 4. The raw .add file (an ADT-format dictionary table we created). */
    static U8 filebuf[1 << 18];
    long flen = read_file(path, filebuf, sizeof filebuf);
    num("file_length", flen);
    hex("file_hex", filebuf, flen < 0 ? 0 : flen);

    printf("}\n");
    return 0;
}
