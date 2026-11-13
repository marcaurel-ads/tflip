/* Records whether an Advantage Client Engine evaluates both branches of
 * IIF/IF (a division by zero in the branch not taken), in expressions,
 * index expressions (on creation, append and update), AOFs and filters, and
 * what TRANSFORM answers to the pictures that carry a percent sign.
 *
 *     iif.exe <ace32.dll> <empty data folder>
 *
 * Built with i686-w64-mingw32-gcc and run under Wine; prints one line per
 * call, "<table type> <kind> <expression> = <answer>" or "rc=<code>". */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define CALL(name, type, ...) ((U32(WINAPI *) type)F(name))(__VA_ARGS__)

static H conn;
static const char *kind;

static void num(H t, const char *e) {
    double d = 0;
    U32 rc = CALL("AdsEvalNumericExpr", (H, U8 *, double *), t, (U8 *)e, &d);
    if (rc) printf("%s N %s rc=%lu\n", kind, e, rc);
    else printf("%s N %s = %.17g\n", kind, e, d);
}
static void lgc(H t, const char *e) {
    U16 b = 0;
    U32 rc = CALL("AdsEvalLogicalExpr", (H, U8 *, U16 *), t, (U8 *)e, &b);
    if (rc) printf("%s L %s rc=%lu\n", kind, e, rc);
    else printf("%s L %s = %d\n", kind, e, b);
}
static void str(H t, const char *e) {
    U8 s[500];
    U16 len = sizeof s;
    U32 rc = CALL("AdsEvalStringExpr", (H, U8 *, U8 *, U16 *), t, (U8 *)e, s, &len);
    if (rc) printf("%s S %s rc=%lu\n", kind, e, rc);
    else printf("%s S %s = [%.*s]\n", kind, e, len, s);
}

static H make(const char *name, int type, int rows, const double *ns) {
    H t = 0;
    char path[1024];
    snprintf(path, sizeof path, "%s.%s", name, type == 3 ? "adt" : "dbf");
    const char *fields = type == 3 ? "n,Double,8,2;x,Double,8,2" : "n,N,8,0;x,N,8,0";
    U32 rc = CALL("AdsCreateTable71", (H, U8 *, U8 *, U16, U16, U16, U16, U32, U8 *, U32, H *),
                  conn, (U8 *)path, NULL, type, 1, 1, 1, 0, (U8 *)fields, 0, &t);
    if (rc) { printf("create rc=%lu\n", rc); exit(1); }
    for (int i = 0; i < rows; i++) {
        CALL("AdsAppendRecord", (H), t);
        CALL("AdsSetDouble", (H, U8 *, double), t, (U8 *)"n", ns[i]);
        CALL("AdsSetDouble", (H, U8 *, double), t, (U8 *)"x", 10.0);
        CALL("AdsWriteRecord", (H), t);
    }
    return t;
}

static void run(int type) {
    kind = type == 3 ? "ADT" : "DBF";
    char name[64];
    double with0[] = {2, 0, 4};
    snprintf(name, sizeof name, "%s_a", kind);
    H t = make(name, type, 3, with0);
    CALL("AdsGotoRecord", (H, U32), t, 2); /* n = 0 */
    num(t, "x/n");
    num(t, "IIF(n>0,x/n,0)");
    num(t, "IF(n>0,x/n,0)");
    num(t, "IIF(n=0,0,x/n)");
    num(t, "IIF(.T.,1,x/0)");
    num(t, "IIF(.F.,x/0,1)");
    num(t, "IIF(.F.,x/n,1)");
    lgc(t, "IIF(n>0,x/n>1,.F.)");
    lgc(t, "IF(n>0,x/n>1,.F.)");
    lgc(t, "IIF(n>0 .AND. x/n>1,.T.,.F.)");
    lgc(t, "n>0 .AND. x/n>1");
    str(t, "IIF(n>0,STR(x/n,5),'zero')");
    num(t, "COALESCE(n,x/0)");
    num(t, "COALESCE(n,x/n)");
    num(t, "IIF(.T.,1,x/n)");
    num(t, "IIF(.T.,1,1/0)");
    lgc(t, ".T. .OR. x/n>1");
    CALL("AdsGotoRecord", (H, U32), t, 1);
    num(t, "IIF(n>0,x/n,0)");
    U32 rc = CALL("AdsSetAOF", (H, U8 *, U16), t, (U8 *)"IIF(n>0,x/n,0)>1", 1);
    U32 cnt = 0;
    if (!rc) CALL("AdsGetRecordCount", (H, U16, U32 *), t, 2, &cnt);
    printf("%s AOF rc=%lu count=%lu\n", kind, rc, cnt);
    CALL("AdsClearAOF", (H), t);
    rc = CALL("AdsSetAOF", (H, U8 *, U16), t, (U8 *)"x/n>1", 1);
    cnt = 0;
    if (!rc) CALL("AdsGetRecordCount", (H, U16, U32 *), t, 2, &cnt);
    printf("%s AOF plain x/n>1 rc=%lu count=%lu\n", kind, rc, cnt);
    CALL("AdsClearAOF", (H), t);
    rc = CALL("AdsSetFilter", (H, U8 *), t, (U8 *)"IIF(n>0,x/n,0)>1");
    printf("%s filter rc=%lu:", kind, rc);
    rc = CALL("AdsGotoTop", (H), t);
    printf(" top rc=%lu", rc);
    for (int i = 0; i < 5; i++) {
        U16 eof = 0;
        U32 recno = 0;
        CALL("AdsAtEOF", (H, U16 *), t, &eof);
        CALL("AdsGetRecordNum", (H, U16, U32 *), t, 2, &recno);
        printf(" [recno %lu eof %d]", recno, eof);
        if (eof) break;
        rc = CALL("AdsSkip", (H, long), t, 1);
        if (rc) printf(" skip rc=%lu", rc);
    }
    printf("\n");
    CALL("AdsClearFilter", (H), t);
    H ord = 0;
    rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), t,
              (U8 *)(type == 3 ? "ia.adi" : "ia.cdx"), (U8 *)"q", (U8 *)"IIF(n>0,x/n,0)", NULL,
              NULL, 0, &ord);
    printf("%s index over existing n=0 rc=%lu\n", kind, rc);
    rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), t,
              (U8 *)(type == 3 ? "ic.adi" : "ic.cdx"), (U8 *)"q", (U8 *)"n", (U8 *)"IIF(n>0,x/n,0)>1",
              NULL, 0, &ord);
    printf("%s index with FOR over existing n=0 rc=%lu\n", kind, rc);
    rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), t,
              (U8 *)(type == 3 ? "id.adi" : "id.cdx"), (U8 *)"q", (U8 *)"n", NULL,
              (U8 *)"IIF(n>0,x/n,0)>1", 0, &ord);
    printf("%s index with WHILE over existing n=0 rc=%lu\n", kind, rc);
    CALL("AdsCloseTable", (H), t);

    double without0[] = {2, 4};
    snprintf(name, sizeof name, "%s_b", kind);
    t = make(name, type, 2, without0);
    ord = 0;
    rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), t,
              (U8 *)(type == 3 ? "ib.adi" : "ib.cdx"), (U8 *)"q", (U8 *)"IIF(n>0,x/n,0)", NULL,
              NULL, 0, &ord);
    printf("%s index over n>0 rows rc=%lu\n", kind, rc);
    rc = CALL("AdsAppendRecord", (H), t);
    CALL("AdsSetDouble", (H, U8 *, double), t, (U8 *)"n", 0.0);
    U32 rc2 = CALL("AdsSetDouble", (H, U8 *, double), t, (U8 *)"x", 10.0);
    U32 rc3 = CALL("AdsWriteRecord", (H), t);
    printf("%s append n=0 rc=%lu set=%lu write=%lu\n", kind, rc, rc2, rc3);
    CALL("AdsGotoRecord", (H, U32), t, 1);
    CALL("AdsSetDouble", (H, U8 *, double), t, (U8 *)"n", 0.0);
    rc3 = CALL("AdsWriteRecord", (H), t);
    printf("%s update to n=0 write=%lu\n", kind, rc3);
    U32 n = 0;
    CALL("AdsGetRecordCount", (H, U16, U32 *), t, 2, &n);
    printf("%s records now %lu\n", kind, n);
    CALL("AdsCloseTable", (H), t);
}

static void transforms(void) {
    kind = "DBF";
    double ns[] = {5};
    H t = make("tr", 2, 1, ns);
    const char *pics[] = {"@BR 999%", "999%", "@B 999%", "@R 999%", "@R 99%", "@BR 99%",
                          "999", "@B 999", "@R 999", "###", "9,999", "999-999", "999.99%", "@B 99.9",
                          "@Z 999", "@( 999", "@C 999", "@X 999", "$999", "99.9%", "@ 999",
                          "@B999", "%999", "@B %999", "9%99", "@R 9%99", NULL};
    const char *vals[] = {"5", "50", "100", "7", "999", "0", "12.5", "-5", "1234", "1234.567",
                          "0.5", NULL};
    for (int p = 0; pics[p]; p++)
        for (int v = 0; vals[v]; v++) {
            char e[200];
            snprintf(e, sizeof e, "TRANSFORM(%s,\"%s\")", vals[v], pics[p]);
            str(t, e);
        }
    /* The key length and key of an index on a picture: the width the
     * expression's type has. */
    const char *keys[] = {"TRANSFORM(n,\"@BR 999%\")", "TRANSFORM(n,\"999%\")",
                          "TRANSFORM(n,\"@B 999\")", "TRANSFORM(n,\"999\")", NULL};
    for (int i = 0; keys[i]; i++) {
        H ord = 0;
        char tag[16];
        snprintf(tag, sizeof tag, "tr%d", i);
        U32 rc = CALL("AdsCreateIndex", (H, U8 *, U8 *, U8 *, U8 *, U8 *, U32, H *), t,
                      (U8 *)"tr.cdx", (U8 *)tag, (U8 *)keys[i], NULL, NULL, 0, &ord);
        U16 len = 0;
        U8 key[200];
        U16 klen = sizeof key;
        if (!rc) {
            CALL("AdsGotoTop", (H), t);
            CALL("AdsGetKeyLength", (H, U16 *), ord, &len);
            CALL("AdsExtractKey", (H, U8 *, U16 *), ord, key, &klen);
        }
        printf("INDEX %s rc=%lu keylen=%u key=[%.*s]\n", keys[i], rc, len, klen, key);
    }
    CALL("AdsCloseTable", (H), t);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    ace = LoadLibraryA(argv[1]);
    if (!ace) return 1;
    CALL("AdsSetServerType", (U16), 1);
    U32 rc = CALL("AdsConnect26", (U8 *, U16, H *), (U8 *)argv[2], 1, &conn);
    if (rc) return printf("connect %lu\n", rc), 1;
    run(3);
    run(2);
    transforms();
    return 0;
}
