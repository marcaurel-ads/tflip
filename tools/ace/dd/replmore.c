/* Probes what is left of the replication objects: the refusals of
 * AdsDDDeletePublication, AdsDDCreateSubscription and AdsDDCreateArticle, the
 * queue name without an extension, and what the system tables and
 * AdsDDFindFirstObject say of publications, articles and subscriptions.
 * Prints "<step> rc=<n> <code> <text>" lines, and the rows of queries.
 *
 *     replmore.exe <ace64.dll> <out folder>
 *
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see replmore.sh). */
#include <windows.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)

static void result(const char *step, U32 rc) {
    U8 text[1024] = "";
    U16 len = sizeof text;
    U32 code = 0;
    if (rc) ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    printf("%s rc=%lu %lu %s\n", step, (unsigned long)rc, (unsigned long)code, text);
}

static H make(const char *out, const char *name) {
    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\%s", out, name);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\db.add", dir);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) result("create", rc);
    return c;
}

/* A fresh copy of the committed t.adt/t.adi, freed from its old dictionary
 * so AddTable takes it. */
static void copy_table(const char *out, const char *name) {
    char src[MAX_PATH], dst[MAX_PATH];
    const char *files[] = {"t.adt", "t.adi"};
    for (int i = 0; i < 2; i++) {
        snprintf(src, sizeof src, "%s\\%s", out, files[i]);
        snprintf(dst, sizeof dst, "%s\\%s\\%s", out, name, files[i]);
        CopyFileA(src, dst, FALSE);
    }
    snprintf(dst, sizeof dst, "%s\\%s\\t.adt", out, name);
    FILE *f = fopen(dst, "r+b");
    if (f) {
        U32 one = 1, zero = 0;
        char blank[260] = {0};
        fseek(f, 0x14, SEEK_SET); fwrite(&one, 4, 1, f);
        fseek(f, 0x5c, SEEK_SET); fwrite(blank, sizeof blank, 1, f);
        fseek(f, 0x160, SEEK_SET); fwrite(&zero, 4, 1, f);
        fclose(f);
    }
}

static U32 pub(H c, const char *n, const char *cmt) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))(c, (U8 *)n, (U8 *)cmt, 0);
}
static U32 sub(H c, const char *n, const char *p, const char *q, const char *cmt) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
            F("AdsDDCreateSubscription"))
        (c, (U8 *)n, (U8 *)p, (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw", (U8 *)q, 1, (U8 *)cmt, 0);
}
static U32 art(H c, const char *p, const char *t) {
    return ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
        (c, (U8 *)p, (U8 *)t, (U8 *)"id", (U8 *)"", 0);
}
static void add_table(H c) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))
        (c, (U8 *)"t", (U8 *)"t.adt", 3, 1, (U8 *)"t.adi", (U8 *)"");
    result("addtable", rc);
}
static void disc(H c) { ((U32(WINAPI *)(H))F("AdsDisconnect"))(c); }
static U32 del_pub(H c, const char *n) {
    return ((U32(WINAPI *)(H, U8 *))F("AdsDDDeletePublication"))(c, (U8 *)n);
}
static U32 del_sub(H c, const char *n) {
    return ((U32(WINAPI *)(H, U8 *))F("AdsDDDeleteSubscription"))(c, (U8 *)n);
}

/* The rows of a query, every field as text, tab-separated. */
static void query(H c, const char *step, const char *s) {
    H stmt = 0, cur = 0;
    U32 rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)s, &cur);
    result(step, rc);
    if (!rc && cur) {
        U16 n = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &n);
        printf("  cols");
        for (U16 i = 1; i <= n; i++) {
            U8 nm[100];
            U16 nl = sizeof nm;
            ((U32(WINAPI *)(H, U32, U8 *, U16 *))F("AdsGetFieldName"))(cur, i, nm, &nl);
            nm[nl] = 0;
            printf(" %s", nm);
        }
        printf("\n");
        for (int guard = 0; guard < 50; guard++) {
            U16 eof = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            if (eof) break;
            printf("  row");
            for (U16 i = 1; i <= n; i++) {
                U8 v[600];
                U32 len = sizeof v - 1;
                U32 r = ((U32(WINAPI *)(H, ULONG_PTR, U8 *, U32 *, U16))F("AdsGetString"))(cur, i, v, &len, 0);
                if (r) printf("\t<%lu>", (unsigned long)r);
                else { v[len] = 0; printf("\t%s", v); }
            }
            printf("\n");
            ((U32(WINAPI *)(H, int))F("AdsSkip"))(cur, 1);
        }
        ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
    }
    if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
}

static void find(H c, const char *step, U16 type, const char *parent) {
    U8 name[300];
    U16 len = sizeof name;
    H h = 0;
    U32 rc = ((U32(WINAPI *)(H, U16, U8 *, U8 *, U16 *, H *))F("AdsDDFindFirstObject"))
        (c, type, (U8 *)parent, name, &len, &h);
    result(step, rc);
    while (!rc) {
        name[len] = 0;
        printf("  found %s\n", name);
        len = sizeof name;
        rc = ((U32(WINAPI *)(H, H, U8 *, U16 *))F("AdsDDFindNextObject"))(c, h, name, &len);
    }
    if (h) ((U32(WINAPI *)(H, H))F("AdsDDFindClose"))(c, h);
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);
    const char *out = argv[2];
    H c;

    if ((c = make(out, "refuse"))) {
        copy_table(out, "refuse");
        add_table(c);
        result("pub p", pub(c, "p", ""));
        result("pub p2", pub(c, "p2", "cmt of p2"));
        result("art p/t", art(c, "p", "t"));
        result("art nopub/t", art(c, "nopub", "t"));
        result("art p/notable", art(c, "p", "notable"));
        result("art p/'' table", art(c, "p", ""));
        result("art ''/t", art(c, "", "t"));
        result("art p/t again", art(c, "p", "t"));
        result("sub s1 p q.add", sub(c, "s1", "p", "q.add", "c1"));
        result("sub s2 nopub", sub(c, "s2", "nopub", "q2.add", ""));
        result("sub s3 empty pub", sub(c, "s3", "", "q3.add", ""));
        result("sub s1 dup", sub(c, "s1", "p", "q4.add", ""));
        result("sub s5 queue q5", sub(c, "s5", "p", "q5", "c5"));
        result("sub s6 queue .adt", sub(c, "s6", "p", "q6.adt", ""));
        result("sub s7 queue q7.dat", sub(c, "s7", "p", "q7.dat", ""));
        result("sub s8 queue empty", sub(c, "s8", "p", "", ""));
        result("sub s9 queue q.add again", sub(c, "s9", "p", "q.add", ""));
        result("sub s10 queue dir", sub(c, "s10", "p", "sub\\q10.add", ""));
        result("delete p used", del_pub(c, "p"));
        result("delete nopub", del_pub(c, "nopub"));
        result("delete p2 unused", del_pub(c, "p2"));
        result("delete sub s1", del_sub(c, "s1"));
        result("delete sub nosub", del_sub(c, "nosub"));
        result("delete p still used", del_pub(c, "p"));
        find(c, "find pub", 15, NULL);
        find(c, "find art", 16, NULL);
        find(c, "find art of p", 16, "p");
        find(c, "find sub", 17, NULL);
        find(c, "find sub of p", 17, "p");
        disc(c);
    }

    if ((c = make(out, "tables"))) {
        copy_table(out, "tables");
        add_table(c);
        pub(c, "p", "pcmt");
        pub(c, "p2", "");
        art(c, "p", "t");
        sub(c, "s1", "p", "q1.add", "c1");
        sub(c, "s2", "p2", "q2.add", "");
        find(c, "find pub", 15, NULL);
        find(c, "find art", 16, NULL);
        find(c, "find art of p", 16, "p");
        find(c, "find art of t", 16, "t");
        find(c, "find sub", 17, NULL);
        find(c, "find sub of p", 17, "p");
        find(c, "find sub of p2", 17, "p2");
        find(c, "find table", 1, NULL);
        query(c, "q publications", "SELECT * FROM system.publications");
        query(c, "q articles", "SELECT * FROM system.articles");
        query(c, "q subscriptions", "SELECT * FROM system.subscriptions");
        query(c, "q tables", "SELECT name FROM system.tables");
        disc(c);
    }
    return 0;
}
