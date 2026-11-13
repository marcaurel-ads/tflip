/* Records which `Parameter N` the original engine adds to the error text of
 * the administrator calls (`AdsMg*`) given a NULL pointer, and what
 * AdsGetHandleType says of an AdsMgConnect handle, for
 * fixtures/errors/param_n_mg.json.
 *
 *     i686-w64-mingw32-gcc -O1 -Wall -o param_n_mg.exe param_n_mg.c
 *     wine param_n_mg.exe <ace32.dll> <server name> <cases file> <a folder as a Windows path, for l>
 *
 * The server name is 127.0.0.1 (the evaluation server on its default port; see
 * param_n_mg.sh). The cases file has one call a line, its name and its
 * arguments, each a word as in param_n.c:
 *
 *     m                the handle of AdsMgConnect
 *     c                the handle of AdsConnect60 to the server
 *     l                the handle of AdsConnect60 to the folder argv[4], local
 *     x                a connection handle that was disconnected (again at
 *                      each line !dead, as new connections may have taken its place)
 *     0 1 99 ...       a number
 *     str:id           a pointer to the text after the colon
 *     pstr:text        as str:, but a pointer that can be NULL
 *     pbuf             a pointer to a buffer of 100000 bytes
 *     pl16:N pl32:N    a pointer to a 16 or 32 bits number, N
 *     p16 p32          a pointer to a number, 0
 *
 * Each call runs as it is and then once for each of its pointers (those
 * that start with p, but not str:) replaced by NULL. The result of each is
 * {"call", "null", "rc", "last", "text"}; before each call another failure
 * is made (a NULL date format), which allocates no handle, so that a call that leaves the last error shows that. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;
static HMODULE ace;
#define F(name) ((void *)GetProcAddress(ace, name))

static H conn, mgmt, dead, local;
static int first = 1;

static H call(void *fn, int n, H *a) {
    switch (n) {
    case 0: return ((U32(WINAPI *)(void))fn)();
    case 1: return ((U32(WINAPI *)(H))fn)(a[0]);
    case 2: return ((U32(WINAPI *)(H, H))fn)(a[0], a[1]);
    case 3: return ((U32(WINAPI *)(H, H, H))fn)(a[0], a[1], a[2]);
    case 4: return ((U32(WINAPI *)(H, H, H, H))fn)(a[0], a[1], a[2], a[3]);
    case 5: return ((U32(WINAPI *)(H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4]);
    case 6: return ((U32(WINAPI *)(H, H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4], a[5]);
    default: return ((U32(WINAPI *)(H, H, H, H, H, H, H))fn)(a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
    }
}

static void json(const U8 *s) {
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if (*s < 32 || *s >= 127) printf("\\u%04x", *s);
        else putchar(*s);
    }
    putchar('"');
}

static int is_pointer(const char *w) { return w[0] == 'p'; }

static void run(const char *line, char words[][64], int nwords, int null) {
    static U8 buf[100000];
    U16 n16[8] = {0};
    U32 n32[8] = {0};
    char strings[8][64];
    H args[8];
    memset(buf, 0, sizeof buf);
    for (int i = 1; i < nwords; i++) {
        const char *w = words[i];
        H v;
        if (!strcmp(w, "m")) v = mgmt;
        else if (!strcmp(w, "c")) v = conn;
        else if (!strcmp(w, "x")) v = dead;
        else if (!strcmp(w, "l")) v = local;
        else if (!strcmp(w, "pbuf")) v = (H)buf;
        else if (!strcmp(w, "p16")) v = (H)&n16[i];
        else if (!strcmp(w, "p32")) v = (H)&n32[i];
        else if (!strncmp(w, "pl16:", 5)) { n16[i] = atoi(w + 5); v = (H)&n16[i]; }
        else if (!strncmp(w, "pl32:", 5)) { n32[i] = atoi(w + 5); v = (H)&n32[i]; }
        else if (!strncmp(w, "str:", 4) || !strncmp(w, "pstr:", 5)) {
            snprintf(strings[i], 64, "%s", w + (w[0] == 'p' ? 5 : 4));
            v = (H)strings[i];
        } else v = (H)strtoul(w, NULL, 10);
        if (i == null) v = 0;
        args[i - 1] = v;
    }
    ((U32(WINAPI *)(U8 *))F("AdsSetDateFormat"))(NULL);
    void *fn = F(words[0]);
    if (!fn) {
        printf("%s{\"call\": \"%s\", \"missing\": 1}", first ? "" : ",\n", line);
        first = 0;
        return;
    }
    U32 rc = (U32)call(fn, nwords - 1, args);
    U32 code = 0;
    U8 text[1200] = "";
    U16 len = sizeof text;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s{\"call\": \"%s\", \"null\": %d, \"rc\": %lu, \"last\": %lu, \"text\": ", first ? "" : ",\n",
           line, null, rc, code);
    json(text);
    /* What the call wrote, for the calls that succeed. */
    if (rc == 0 && null != 2 && (!strcmp(words[0], "AdsGetHandleType") || !strcmp(words[0], "AdsMgGetServerType"))) {
        printf(", \"type\": %u", n16[2]);
    }
    printf("}");
    first = 0;
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: param_n_mg.exe <ace32.dll> <server> <cases>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    U32 rc = ((U32(WINAPI *)(U8 *, U8 *, U8 *, H *))F("AdsMgConnect"))((U8 *)argv[2], (U8 *)"ADSSYS", NULL, &mgmt);
    if (rc) return fprintf(stderr, "AdsMgConnect %lu\n", rc), 1;
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 2, NULL, NULL, 0, &conn))
        return fprintf(stderr, "connect\n"), 1;
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 2, NULL, NULL, 0, &dead))
        return fprintf(stderr, "connect\n"), 1;
    if (argc > 4 && ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[4], 1, NULL, NULL, 0, &local))
        return fprintf(stderr, "local connect\n"), 1;
    ((U32(WINAPI *)(H))F("AdsDisconnect"))(dead);
    FILE *cases = fopen(argv[3], "r");
    char line[400];
    printf("[\n");
    while (fgets(line, sizeof line, cases)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        if (!strcmp(line, "!dead")) {
            /* x as a handle that was disconnected just now: the calls
             * before it may have taken the place of the earlier one. */
            ((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 2, NULL, NULL, 0, &dead);
            ((U32(WINAPI *)(H))F("AdsDisconnect"))(dead);
            continue;
        }
        char words[10][64];
        int n = 0;
        char copy[400];
        snprintf(copy, sizeof copy, "%s", line);
        for (char *w = strtok(copy, " "); w && n < 10; w = strtok(NULL, " ")) snprintf(words[n++], 64, "%s", w);
        run(line, words, n, 0);
        for (int i = 1; i < n; i++)
            if (is_pointer(words[i])) run(line, words, n, i);
    }
    printf("\n]\n");
    return 0;
}
