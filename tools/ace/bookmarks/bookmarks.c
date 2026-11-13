/* Records what the original engine does with bookmarks, taken through a
 * table handle and through index handles, for fixtures/bookmarks/bookmarks.json.
 *
 *     bookmarks.exe <ace32.dll> <folder with indexed.adt and indexed.adi> <ops file> > bookmarks.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine (see run.sh). The ops
 * file has one command per line (# starts a comment); each one's result is a
 * JSON object with the command, its return code ("rc") and AdsGetLastError's
 * text ("error") when it fails. The handles are `t`, the table, and 0 to 4,
 * the index orders AdsGetAllIndexes returns, in that order; `conn`, the
 * connection; `zero`, 0; and `bad`, a value that is no handle. Bookmarks are
 * kept in named registers of 64 bytes (filled with 0xEE first):
 *
 *     move <h> top|bottom|bof|eof|rec <n>
 *                           AdsGotoTop and AdsGotoBottom on the handle, and
 *                           for bof and eof a skip of -1 and 1 after them;
 *                           AdsGotoRecord of the table for rec ("recno":
 *                           AdsGetRecordNum afterwards)
 *     len <h>               AdsGetBookmarkLength ("len")
 *     get <h> [<name> [<size>]]
 *                           AdsGetBookmark60 into the register (default
 *                           size 64): "len" as it was set, "bytes" hex
 *     get1 <h>              AdsGetBookmark ("value", hex)
 *     lennull <h>           AdsGetBookmarkLength with a NULL length
 *     getnull <h> buf|len   AdsGetBookmark60 with a NULL buffer or length
 *     copy <dst> <src>      a register's bytes and length into another
 *     patch <name> <offset> <hex>
 *                           bytes written into the register; {hN} stands for
 *                           the handle of index N as 4 little-endian bytes
 *     cmp <a> <b>           AdsCompareBookmarks of two registers ("result")
 *     cmpnull a|b|r         the same with a NULL bookmark a, b or result
 *     goto <h> <name>       AdsGotoBookmark60 ("recno" afterwards)
 *     goto1 <h> <n>         AdsGotoBookmark with the value n ("recno")
 *     gotonull <h>          AdsGotoBookmark60 with a NULL bookmark ("recno")
 *
 * The bytes of a bookmark taken through an index have the index's handle in
 * them (offset 8); they are written as {hN} so that the fixture holds no
 * handle value. */
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

/* The next space-separated word of *s, which moves past it; NULL at the end. */
static char *strsep(char **s, const char *delim) {
    char *w = *s;
    if (!w) return NULL;
    char *end = strpbrk(w, delim);
    if (end) *end++ = 0;
    *s = end;
    return w;
}

static H table, conn, idx[8];
static U16 nidx = 8;
static struct {
    char name[16];
    U8 bytes[64];
    U32 len;
} regs[64];
static int nregs;

static void json_text(const U8 *s, size_t n) {
    putchar('"');
    for (size_t i = 0; i < n; i++) {
        U8 c = s[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void result(U32 rc) {
    printf("\"rc\": %lu", rc);
    if (rc) {
        U32 code = 0;
        U8 text[1200];
        U16 len = sizeof text;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf(", \"error\": ");
        json_text(text, len);
    }
}

/* Hex of bytes, with the handle of an index at offset 8 written as {hN}. */
static void hex(const char *key, const U8 *b, size_t n) {
    printf(", \"%s\": \"", key);
    for (size_t i = 0; i < n; i++) {
        if (i == 8 && i + 4 <= n) {
            U32 word;
            memcpy(&word, b + 8, 4);
            int found = -1;
            for (int k = 0; k < nidx; k++)
                if ((U32)idx[k] == word) found = k;
            if (found >= 0) {
                printf("{h%d}", found);
                i += 3;
                continue;
            }
        }
        printf("%02x", b[i]);
    }
    printf("\"");
}

static H handle(const char *name) {
    if (!strcmp(name, "t")) return table;
    if (!strcmp(name, "bad")) return 0x12345;
    if (!strcmp(name, "zero")) return 0;
    if (!strcmp(name, "conn")) return conn;
    return idx[atoi(name)];
}

static int reg(const char *name) {
    for (int i = 0; i < nregs; i++)
        if (!strcmp(regs[i].name, name)) return i;
    snprintf(regs[nregs].name, sizeof regs[nregs].name, "%s", name);
    memset(regs[nregs].bytes, 0xEE, 64);
    regs[nregs].len = 0;
    return nregs++;
}

static size_t from_hex(const char *s, U8 *out) {
    size_t n = 0;
    while (*s) {
        if (!strncmp(s, "{h", 2)) {
            U32 word = (U32)idx[atoi(s + 2)];
            memcpy(out + n, &word, 4);
            n += 4;
            s = strchr(s, '}') + 1;
            continue;
        }
        char d[3] = {s[0], s[1], 0};
        out[n++] = (U8)strtoul(d, NULL, 16);
        s += 2;
    }
    return n;
}

static void recno(void) {
    U32 r = 0;
    ((U32(WINAPI *)(H, U16, U32 *))F("AdsGetRecordNum"))(table, 2, &r);
    printf(", \"recno\": %lu", r);
}

int main(int argc, char **argv) {
    if (argc < 4) return fprintf(stderr, "usage: bookmarks.exe <ace32.dll> <folder> <ops>\n"), 2;
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return fprintf(stderr, "cannot load %s\n", argv[1]), 1;
    if (((U32(WINAPI *)(U8 *, U16, U8 *, U8 *, U32, H *))F("AdsConnect60"))((U8 *)argv[2], 1, NULL,
                                                                           NULL, 0, &conn))
        return fprintf(stderr, "connect\n"), 1;
    char path[600];
    snprintf(path, sizeof path, "%s\\indexed.adt", argv[2]);
    if (((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U32, H *))F("AdsOpenTable"))(
            conn, (U8 *)path, NULL, 3, 1, 1, 1, 2, &table))
        return fprintf(stderr, "open\n"), 1;
    ((U32(WINAPI *)(H, H *, U16 *))F("AdsGetAllIndexes"))(table, idx, &nidx);
    FILE *ops = fopen(argv[3], "r");
    char line[512];
    int first = 1;
    printf("[\n");
    while (fgets(line, sizeof line, ops)) {
        line[strcspn(line, "\r\n")] = 0;
        if (!*line || *line == '#') continue;
        printf("%s{\"cmd\": \"%s\", ", first ? "" : ",\n", line);
        first = 0;
        char *rest = line;
        char *cmd = strsep(&rest, " ");
        char *a = strsep(&rest, " "), *b = strsep(&rest, " "), *c = strsep(&rest, " ");
        if (!strcmp(cmd, "move")) {
            H h = handle(a);
            U32 rc = 0;
            if (!strcmp(b, "top")) rc = ((U32(WINAPI *)(H))F("AdsGotoTop"))(h);
            else if (!strcmp(b, "bottom")) rc = ((U32(WINAPI *)(H))F("AdsGotoBottom"))(h);
            else if (!strcmp(b, "bof")) {
                ((U32(WINAPI *)(H))F("AdsGotoTop"))(h);
                rc = ((U32(WINAPI *)(H, long))F("AdsSkip"))(h, -1);
            } else if (!strcmp(b, "eof")) {
                ((U32(WINAPI *)(H))F("AdsGotoBottom"))(h);
                rc = ((U32(WINAPI *)(H, long))F("AdsSkip"))(h, 1);
            } else rc = ((U32(WINAPI *)(H, U32))F("AdsGotoRecord"))(table, atoi(c));
            result(rc);
            recno();
        } else if (!strcmp(cmd, "len")) {
            U32 len = 99;
            result(((U32(WINAPI *)(H, U32 *))F("AdsGetBookmarkLength"))(handle(a), &len));
            printf(", \"len\": %lu", len);
        } else if (!strcmp(cmd, "get")) {
            int r = reg(b ? b : "_");
            U32 len = c ? (U32)atoi(c) : 64;
            memset(regs[r].bytes, 0xEE, 64);
            U32 rc = ((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetBookmark60"))(handle(a), regs[r].bytes,
                                                                            &len);
            result(rc);
            regs[r].len = rc ? 0 : len;
            printf(", \"len\": %lu", len);
            hex("bytes", regs[r].bytes, rc ? 8 : (len > 64 ? 64 : len));
        } else if (!strcmp(cmd, "lennull")) {
            result(((U32(WINAPI *)(H, U32 *))F("AdsGetBookmarkLength"))(handle(a), NULL));
        } else if (!strcmp(cmd, "getnull")) {
            U8 buf[64];
            U32 len = 64;
            result(((U32(WINAPI *)(H, U8 *, U32 *))F("AdsGetBookmark60"))(
                handle(a), !strcmp(b, "buf") ? NULL : buf, !strcmp(b, "len") ? NULL : &len));
        } else if (!strcmp(cmd, "get1")) {
            H v = 0x77;
            result(((U32(WINAPI *)(H, H *))F("AdsGetBookmark"))(handle(a), &v));
            printf(", \"value\": %lu", (unsigned long)v);
        } else if (!strcmp(cmd, "copy")) {
            int d = reg(a), s = reg(b);
            regs[d].len = regs[s].len;
            memcpy(regs[d].bytes, regs[s].bytes, 64);
            printf("\"rc\": 0");
        } else if (!strcmp(cmd, "patch")) {
            int r = reg(a);
            from_hex(c, regs[r].bytes + atoi(b));
            printf("\"rc\": 0");
            hex("bytes", regs[r].bytes, regs[r].len);
        } else if (!strcmp(cmd, "cmp") || !strcmp(cmd, "cmpnull")) {
            long out = 99;
            U8 *x = regs[reg(a)].bytes, *y = regs[reg(b ? b : a)].bytes;
            long *res = &out;
            if (!strcmp(cmd, "cmpnull")) {
                x = regs[reg("p")].bytes;
                y = regs[reg("p")].bytes;
                if (!strcmp(a, "a")) x = NULL;
                if (!strcmp(a, "b")) y = NULL;
                if (!strcmp(a, "r")) res = NULL;
            }
            result(((U32(WINAPI *)(U8 *, U8 *, long *))F("AdsCompareBookmarks"))(x, y, res));
            printf(", \"result\": %ld", out);
        } else if (!strcmp(cmd, "goto")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsGotoBookmark60"))(handle(a), regs[reg(b)].bytes));
            recno();
        } else if (!strcmp(cmd, "gotonull")) {
            result(((U32(WINAPI *)(H, U8 *))F("AdsGotoBookmark60"))(handle(a), NULL));
            recno();
        } else if (!strcmp(cmd, "goto1")) {
            result(((U32(WINAPI *)(H, H))F("AdsGotoBookmark"))(handle(a), (H)atoi(b)));
            recno();
        } else printf("\"rc\": -1");
        printf("}");
    }
    printf("\n]\n");
    return 0;
}
