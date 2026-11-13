/* Builds a data dictionary with replication objects (publications, articles
 * and subscriptions), for deriving their on-disk layout and the get/set
 * property contract of the real engine. Black-box: it only calls the official
 * ACE and dumps what comes back.
 *
 *     replication.exe <ace64.dll> <folder>
 *
 * Creates <folder>\repl\repl.add (no password, not encrypted), two tables,
 * a publication with two articles, a second publication, and two
 * subscriptions, each property set to a recognisable value. Then it reads
 * every known property of every object back through the Get* APIs and prints
 * "<object> <propid> rc=<rc> len=<n> <hex>" so the get contract is captured
 * exactly. Run add.py on the .add afterwards for the stored slot layout.
 *
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see replication.sh). */
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

/* Publication, article and subscription property IDs (ace.h). */
#define ADS_DD_PUBLICATION_OPTIONS     1500
#define ADS_DD_ARTICLE_FILTER             1600
#define ADS_DD_ARTICLE_ID_COLUMNS         1601
#define ADS_DD_ARTICLE_ID_COLUMN_NUMBERS  1602
#define ADS_DD_ARTICLE_FILTER_SHORT       1603
#define ADS_DD_ARTICLE_INCLUDE_COLUMNS    1604
#define ADS_DD_ARTICLE_EXCLUDE_COLUMNS    1605
#define ADS_DD_ARTICLE_INC_COLUMN_NUMBERS 1606
#define ADS_DD_ARTICLE_INSERT_MERGE       1607
#define ADS_DD_ARTICLE_UPDATE_MERGE       1608
#define ADS_DD_ARTICLE_FILTER_W           1609
#define ADS_DD_SUBSCR_PUBLICATION_NAME    1700
#define ADS_DD_SUBSCR_TARGET              1701
#define ADS_DD_SUBSCR_USERNAME            1702
#define ADS_DD_SUBSCR_PASSWORD            1703
#define ADS_DD_SUBSCR_FORWARD             1704
#define ADS_DD_SUBSCR_ENABLED             1705
#define ADS_DD_SUBSCR_QUEUE_NAME          1706
#define ADS_DD_SUBSCR_OPTIONS             1707
#define ADS_DD_SUBSCR_QUEUE_NAME_RELATIVE 1708
#define ADS_DD_SUBSCR_PAUSED              1709
#define ADS_DD_SUBSCR_COMM_TCP_IP         1710
#define ADS_DD_SUBSCR_COMM_TCP_IP_V6      1711
#define ADS_DD_SUBSCR_COMM_UDP_IP         1712
#define ADS_DD_SUBSCR_COMM_IPX            1713
#define ADS_DD_SUBSCR_OPTIONS_INTERNAL    1714
#define ADS_DD_SUBSCR_COMM_TLS            1715
#define ADS_DD_SUBSCR_CONNECTION_STR      1716

#define ADS_IDENTIFY_BY_PRIMARY         0x00000001
#define ADS_IDENTIFY_BY_ALL             0x00000002

static void last_error(const char *what) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: %lu %s\n", what, (unsigned long)code, text);
}

static int sql(H c, const char *s) {
    H stmt = 0, cursor = 0;
    U32 rc = ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &stmt);
    if (!rc) rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(stmt, (U8 *)s, &cursor);
    if (rc) { fprintf(stderr, "sql: %s: ", s); last_error("AdsExecuteSQLDirect"); }
    if (cursor) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cursor);
    if (stmt) ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(stmt);
    return rc != 0;
}

/* Read one property via the matching Get*, trying buffer sizes 2, 4 then big
 * (fixed-size properties demand an exact buffer, error 5133 otherwise). Print
 * as a hex line. */
static void get_pub(H c, const char *pub, U16 id) {
    U16 sizes[] = {2, 4, 2048};
    for (int s = 0; s < 3; s++) {
        U8 buf[2048]; U16 len = sizes[s];
        memset(buf, 0, sizeof buf);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F("AdsDDGetPublicationProperty"))
            (c, (U8 *)pub, id, buf, &len);
        if (rc == 5133 && s < 2) continue;
        printf("PUB %s %u rc=%lu len=%u ", pub, id, (unsigned long)rc, len);
        for (U16 i = 0; i < len && i < sizeof buf; i++) printf("%02x", buf[i]);
        printf("\n");
        return;
    }
}

static void get_art(H c, const char *pub, const char *obj, U16 id) {
    U16 sizes[] = {2, 4, 2048};
    for (int s = 0; s < 3; s++) {
        U8 buf[2048]; U16 len = sizes[s];
        memset(buf, 0, sizeof buf);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16 *))F("AdsDDGetArticleProperty"))
            (c, (U8 *)pub, (U8 *)obj, id, buf, &len);
        if (rc == 5133 && s < 2) continue;
        printf("ART %s/%s %u rc=%lu len=%u ", pub, obj, id, (unsigned long)rc, len);
        for (U16 i = 0; i < len && i < sizeof buf; i++) printf("%02x", buf[i]);
        printf("\n");
        return;
    }
}

static void get_sub(H c, const char *sub, U16 id) {
    U16 sizes[] = {2, 4, 2048};
    for (int s = 0; s < 3; s++) {
        U8 buf[2048]; U16 len = sizes[s];
        memset(buf, 0, sizeof buf);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16 *))F("AdsDDGetSubscriptionProperty"))
            (c, (U8 *)sub, id, buf, &len);
        if (rc == 5133 && s < 2) continue;
        printf("SUB %s %u rc=%lu len=%u ", sub, id, (unsigned long)rc, len);
        for (U16 i = 0; i < len && i < sizeof buf; i++) printf("%02x", buf[i]);
        printf("\n");
        return;
    }
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: replication.exe <ace64.dll> <folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);  /* local server */

    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\repl", argv[2]);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\repl.add", dir);

    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) { last_error("AdsDDCreate"); return 1; }

    /* Two tables for the articles to publish. */
    sql(c, "CREATE TABLE t1 (id INTEGER, name CHAR(20), qty INTEGER, note CHAR(40))");
    sql(c, "CREATE TABLE t2 (code CHAR(8), amount DOUBLE)");
    /* A primary key so IDENTIFY_BY_PRIMARY has something to use. */
    sql(c, "CREATE INDEX pk1 ON t1 (id)");
    sql(c, "EXECUTE PROCEDURE sp_ModifyTableProperty('t1', 'TABLE_PRIMARY_KEY', 'pk1')");

    /* --- Publications --- */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))
        (c, (U8 *)"pub_one", (U8 *)"first publication comment", 0);
    if (rc) last_error("AdsDDCreatePublication pub_one");
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))
        (c, (U8 *)"pub_two", (U8 *)"second", 0);
    if (rc) last_error("AdsDDCreatePublication pub_two");

    /* Publication option: set a recognisable value. */
    { U32 opt = 0x12345678;
      rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetPublicationProperty"))
          (c, (U8 *)"pub_one", ADS_DD_PUBLICATION_OPTIONS, &opt, sizeof opt);
      if (rc) last_error("set PUBLICATION_OPTIONS"); }

    /* --- Articles --- */
    /* ANSI variant: row-ident columns + a horizontal filter. */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
        (c, (U8 *)"pub_one", (U8 *)"t1", (U8 *)"id", (U8 *)"qty > 10", 0);
    if (rc) last_error("AdsDDCreateArticle t1");
    /* 100 variant: wide filter, identify by primary key. */
    { WCHAR wfilter[64];
      MultiByteToWideChar(CP_UTF8, 0, "amount < 99.5", -1, wfilter, 64);
      rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, WCHAR *, U32))F("AdsDDCreateArticle100"))
          (c, (U8 *)"pub_one", (U8 *)"t2", (U8 *)"code", wfilter, ADS_IDENTIFY_BY_ALL);
      if (rc) last_error("AdsDDCreateArticle100 t2"); }

    /* Article properties: include/exclude column lists, merge flags. */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
        (c, (U8 *)"pub_one", (U8 *)"t1", ADS_DD_ARTICLE_INCLUDE_COLUMNS, (void *)"id;name;qty",
         (U16)strlen("id;name;qty") + 1);
    if (rc) last_error("set ARTICLE_INCLUDE_COLUMNS");
    { U16 yes = 1;
      rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
          (c, (U8 *)"pub_one", (U8 *)"t1", ADS_DD_ARTICLE_INSERT_MERGE, &yes, sizeof yes);
      if (rc) last_error("set ARTICLE_INSERT_MERGE");
      rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
          (c, (U8 *)"pub_one", (U8 *)"t1", ADS_DD_ARTICLE_UPDATE_MERGE, &yes, sizeof yes);
      if (rc) last_error("set ARTICLE_UPDATE_MERGE"); }
    /* An article on t2 with an exclude list instead of include. */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
        (c, (U8 *)"pub_one", (U8 *)"t2", ADS_DD_ARTICLE_EXCLUDE_COLUMNS, (void *)"amount",
         (U16)strlen("amount") + 1);
    if (rc) last_error("set ARTICLE_EXCLUDE_COLUMNS");

    /* --- Subscriptions --- */
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
          F("AdsDDCreateSubscription"))
        (c, (U8 *)"sub_one", (U8 *)"pub_one", (U8 *)"\\\\server\\target.add",
         (U8 *)"adsuser", (U8 *)"adspassword", (U8 *)"queue1.add", 1,
         (U8 *)"subscription comment", 0);
    if (rc) last_error("AdsDDCreateSubscription sub_one");
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
          F("AdsDDCreateSubscription"))
        (c, (U8 *)"sub_two", (U8 *)"pub_two", (U8 *)"Z:\\other\\target.add",
         (U8 *)"u2", (U8 *)"p2", (U8 *)"q2.add", 0, (U8 *)"second sub", 0);
    if (rc) last_error("AdsDDCreateSubscription sub_two");
    /* Three more subscriptions to isolate the remaining comm-type bits. */
    for (int k = 0; k < 3; k++) {
        const char *nm = (const char *[]){"sub_v6", "sub_ipx", "sub_tls"}[k];
        const char *q = (const char *[]){"qv6.add", "qipx.add", "qtls.add"}[k];
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
              F("AdsDDCreateSubscription"))
            (c, (U8 *)nm, (U8 *)"pub_two", (U8 *)"t.add", (U8 *)"u", (U8 *)"p",
             (U8 *)q, 0, (U8 *)"", 0);
        if (rc) last_error(nm);
    }

    /* Subscription properties: flip booleans / set a connection string, so
     * each slot (and the comm-flag bits in the options dword) can be pinned. */
    { U16 yes = 1, no = 0;
#define SET_SUB(name, id, valp) \
      do { if (((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty")) \
          (c, (U8 *)name, id, valp, sizeof *valp)) last_error("set " #id " " name); } while (0)
      SET_SUB("sub_one", ADS_DD_SUBSCR_ENABLED, &yes);
      SET_SUB("sub_one", ADS_DD_SUBSCR_PAUSED, &yes);
      SET_SUB("sub_one", ADS_DD_SUBSCR_COMM_TCP_IP, &yes);
      SET_SUB("sub_two", ADS_DD_SUBSCR_ENABLED, &no);
      SET_SUB("sub_two", ADS_DD_SUBSCR_COMM_UDP_IP, &yes);
      SET_SUB("sub_v6", ADS_DD_SUBSCR_COMM_TCP_IP_V6, &yes);
      SET_SUB("sub_ipx", ADS_DD_SUBSCR_COMM_IPX, &yes);
      SET_SUB("sub_tls", ADS_DD_SUBSCR_COMM_TLS, &yes);
#undef SET_SUB
    }
    /* User-defined property (common id 3) on a publication and a subscription,
     * to find its slot for each kind. */
    ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetPublicationProperty"))
        (c, (U8 *)"pub_two", 3, (void *)"pubUD", (U16)strlen("pubUD") + 1);
    ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
        (c, (U8 *)"sub_two", 3, (void *)"subUD", (U16)strlen("subUD") + 1);
    ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
        (c, (U8 *)"pub_one", (U8 *)"t2", 3, (void *)"artUD", (U16)strlen("artUD") + 1);
    ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
        (c, (U8 *)"sub_one", ADS_DD_SUBSCR_CONNECTION_STR,
         (void *)"Server=1.2.3.4:6262", (U16)strlen("Server=1.2.3.4:6262") + 1);

    /* --- Read everything back through the Get* APIs. --- */
    for (U16 id = 1500; id <= 1500; id++) { get_pub(c, "pub_one", id); get_pub(c, "pub_two", id); }
    for (U16 id = 1600; id <= 1609; id++) { get_art(c, "pub_one", "t1", id); get_art(c, "pub_one", "t2", id); }
    for (U16 id = 1700; id <= 1716; id++) { get_sub(c, "sub_one", id); get_sub(c, "sub_two", id); }
    get_pub(c, "pub_two", 3);
    get_sub(c, "sub_two", 3);
    get_art(c, "pub_one", "t2", 3);
    for (int k = 0; k < 3; k++) {
        const char *nm = (const char *[]){"sub_v6", "sub_ipx", "sub_tls"}[k];
        get_sub(c, nm, 1714);
    }

    ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    fflush(stdout);
    return 0;
}
