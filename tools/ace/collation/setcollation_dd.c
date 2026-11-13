/* What AdsSetCollation and AdsStmtSetTableCollation accept on a dictionary
 * connection, against a free-table one.
 *
 *     setcollation_dd.exe <ace32.dll> <data folder> <names file> [<server path>]
 *
 * The data folder gets a fresh dictionary (collation.add). For every line of
 * the names file (read as it is, no trimming; "\e" alone stands for the empty
 * name) it prints the return code of AdsSetCollation (and the name
 * AdsGetCollation then reads back) on a free-table connection to the folder
 * and on the dictionary connection, and of AdsStmtSetTableCollation on a
 * statement of each, with the error text of a refusal. With a server path
 * (`\\127.0.0.1:6262\data`, the data folder as the server sees it) the
 * connections are made to that server over TCP/IP instead. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(n) GetProcAddress(ace, n)

static void err(char *out) {
    U32 code = 0;
    U8 text[600];
    U16 len = sizeof text;
    text[0] = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    sprintf(out, "%lu \"%.*s\"", code, (int)len, text);
}

static void one(const char *what, H c, H stmt, const char *name) {
    char e[700] = "";
    U32 rc = ((U32(WINAPI *)(H, U8 *))F("AdsSetCollation"))(c, (U8 *)name);
    if (rc) err(e);
    U8 got[128] = "";
    U16 len = sizeof got;
    ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsGetCollation"))(c, got, &len);
    printf("  %s set rc=%lu get=\"%.*s\" %s\n", what, rc, (int)len, got, e);
    e[0] = 0;
    rc = ((U32(WINAPI *)(H, U8 *))F("AdsStmtSetTableCollation"))(stmt, (U8 *)name);
    if (rc) err(e);
    printf("  %s stmt rc=%lu %s\n", what, rc, e);
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: setcollation_dd <ace32.dll> <folder> <names>\n"), 2;
    int remote = argc > 4;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    char dict[600], other[600];
    snprintf(dict, sizeof dict, "%s\\collation.add", argv[2]);
    H d = 0, free_c = 0, dict_c = 0;
    U32 rc;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    DeleteFileA(dict);
    snprintf(other, sizeof other, "%s\\collation.ai", argv[2]);
    DeleteFileA(other);
    snprintf(other, sizeof other, "%s\\collation.am", argv[2]);
    DeleteFileA(other);
    rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)dict, 0, (U8 *)"collation", &d);
    if (rc) return fprintf(stderr, "AdsDDCreate %lu\n", rc), 1;
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(d);
    if (remote) {
        char cs[700];
        snprintf(cs, sizeof cs, "Data Source=%s\\collation.add;ServerType=REMOTE;CommType=TCP_IP;User ID=ADSSYS;Password=", argv[4]);
        rc = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &dict_c);
        snprintf(cs, sizeof cs, "Data Source=%s;ServerType=REMOTE;CommType=TCP_IP", argv[4]);
        U32 rc2 = ((U32(WINAPI *)(U8 *, H *, H *))F("AdsConnect101"))((U8 *)cs, NULL, &free_c);
        if (rc || rc2) return fprintf(stderr, "connect %lu %lu\n", rc, rc2), 1;
    } else {
        rc = ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)dict, 1, (U8 *)"ADSSYS", (U8 *)"", 0, &dict_c);
        if (rc) return fprintf(stderr, "dict connect %lu\n", rc), 1;
        rc = ((U32(WINAPI *)(U8 *, U16, H *))F("AdsConnect26"))((U8 *)argv[2], 1, &free_c);
        if (rc) return fprintf(stderr, "free connect %lu\n", rc), 1;
    }
    H fs = 0, ds = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(free_c, &fs);
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(dict_c, &ds);
    FILE *f = fopen(argv[3], "r");
    char line[512];
    while (fgets(line, sizeof line, f)) {
        size_t n = strlen(line);
        while (n && (line[n - 1] == '\n' || line[n - 1] == '\r')) line[--n] = 0;
        if (!strcmp(line, "\\e")) line[0] = 0;
        printf("name \"%s\"\n", line);
        one("free", free_c, fs, line);
        one("dict", dict_c, ds, line);
    }
    return 0;
}
