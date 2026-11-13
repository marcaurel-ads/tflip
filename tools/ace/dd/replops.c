/* Builds the golden data dictionaries for the replication write tests
 * (fixtures/dictionary/ops/pub*.{add,am,ai}, art*.*, sub*.*), with the real
 * Advantage Client Engine. Black-box: tflip's own output is compared to these
 * byte-for-byte (the login secrets masked). Mirrors tools/ace/dd/variants.c.
 *
 *     replops.exe <ace64.dll> <out folder>
 *
 * Each scenario makes <out>\<name>.add (no password, not encrypted) and runs
 * its DD calls; copy the <name>.{add,am,ai} into fixtures/dictionary/ops/.
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see replops.sh). */
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

#define ADS_DD_USER_DEFINED_PROP       3
#define ADS_DD_COMMENT                 1
#define ADS_DD_PUBLICATION_OPTIONS     1500
#define ADS_DD_ARTICLE_INCLUDE_COLUMNS 1604
#define ADS_DD_ARTICLE_EXCLUDE_COLUMNS 1605
#define ADS_DD_ARTICLE_INSERT_MERGE    1607
#define ADS_DD_ARTICLE_UPDATE_MERGE    1608
#define ADS_IDENTIFY_BY_ALL            0x02
#define ADS_ADT                        3
#define ADS_ANSI                       1

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: rc=%lu %lu %s\n", what, (unsigned long)rc, (unsigned long)code, text);
}

/* Create a dictionary <out>\<folder>\<base>.add (its own folder, so a
 * scenario's added table is a fresh copy) and return its connection. */
static H make_as(const char *out, const char *folder, const char *base) {
    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\%s", out, folder);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\%s.add", dir, base);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) { oops(folder, rc); return 0; }
    return c;
}

/* The common case: the dictionary is named after its folder. */
static H make(const char *out, const char *name) {
    return make_as(out, name, name);
}

/* Copy the committed t.adt/t.adi into a scenario's folder, so AddTable binds a
 * fresh copy. */
static void copy_table(const char *out, const char *name) {
    char src[MAX_PATH], dst[MAX_PATH];
    const char *files[] = {"t.adt", "t.adi"};
    for (int i = 0; i < 2; i++) {
        snprintf(src, sizeof src, "%s\\%s", out, files[i]);
        snprintf(dst, sizeof dst, "%s\\%s\\%s", out, name, files[i]);
        CopyFileA(src, dst, FALSE);
    }
    /* The committed t.adt is bound to another dictionary; free it so AddTable
     * will take it (object id 1 at 0x14 and 0 at 0x160, back-reference at 0x5c
     * cleared), as AdsDDFreeTable would. */
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

static U32 create_pub(H c, const char *name, const char *comment, U32 options) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))
        (c, (U8 *)name, (U8 *)comment, options);
    if (rc) oops("CreatePublication", rc);
    return rc;
}

static U32 set_pub(H c, const char *name, U16 id, void *val, U16 len) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetPublicationProperty"))
        (c, (U8 *)name, id, val, len);
    if (rc) oops("SetPublicationProperty", rc);
    return rc;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: replops.exe <ace64.dll> <out folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1);  /* local server */
    const char *out = argv[2];
    H c;

    /* pubadd: one publication with a comment (create options must be 0). */
    if ((c = make(out, "pubadd"))) {
        create_pub(c, "mypub", "a comment", 0);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* pubset: a publication made bare, then options, user-defined and a first
     * comment set on it. */
    if ((c = make(out, "pubset"))) {
        create_pub(c, "p", "", 0);
        U32 opt = 0x20;
        set_pub(c, "p", ADS_DD_PUBLICATION_OPTIONS, &opt, sizeof opt);
        set_pub(c, "p", ADS_DD_USER_DEFINED_PROP, (void *)"userprop", (U16)strlen("userprop") + 1);
        set_pub(c, "p", ADS_DD_COMMENT, (void *)"later", (U16)strlen("later") + 1);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* pubdel: two publications, the first deleted. */
    if ((c = make(out, "pubdel"))) {
        create_pub(c, "p1", "", 0);
        create_pub(c, "p2", "", 0);
        U32 rc = ((U32(WINAPI *)(H, U8 *))F("AdsDDDeletePublication"))(c, (U8 *)"p1");
        if (rc) oops("DeletePublication", rc);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* The article scenarios all start from a dict with table t (id, s) added
     * and a publication "pub"; the article publishes t. t.adt/t.adi must be in
     * the out folder (replops.sh copies them). */
    struct { const char *name; } arts[] = {
        {"artadd"}, {"artall"}, {"artinc"}, {"artexc"}, {"artdel"}, {"art100"},
    };
    for (size_t i = 0; i < sizeof arts / sizeof *arts; i++) {
        const char *nm = arts[i].name;
        if (!(c = make(out, nm))) continue;
        copy_table(out, nm);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))
            (c, (U8 *)"t", (U8 *)"t.adt", ADS_ADT, ADS_ANSI, (U8 *)"t.adi", (U8 *)"");
        if (rc) oops("AddTable", rc);
        create_pub(c, "pub", "", 0);
        if (!strcmp(nm, "artadd")) {
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
                (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"id > 5", 0);
            if (rc) oops("CreateArticle", rc);
        } else if (!strcmp(nm, "artall")) {
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
                (c, (U8 *)"pub", (U8 *)"t", NULL, (U8 *)"s = 'x'", ADS_IDENTIFY_BY_ALL);
            if (rc) oops("CreateArticle all", rc);
        } else if (!strcmp(nm, "artinc")) {
            ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
                (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"", 0);
            U16 yes = 1;
            ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
                (c, (U8 *)"pub", (U8 *)"t", ADS_DD_ARTICLE_INCLUDE_COLUMNS, (void *)"id;s",
                 (U16)strlen("id;s") + 1);
            ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
                (c, (U8 *)"pub", (U8 *)"t", ADS_DD_ARTICLE_INSERT_MERGE, &yes, sizeof yes);
            ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
                (c, (U8 *)"pub", (U8 *)"t", ADS_DD_ARTICLE_UPDATE_MERGE, &yes, sizeof yes);
        } else if (!strcmp(nm, "artexc")) {
            ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
                (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"", 0);
            ((U32(WINAPI *)(H, U8 *, U8 *, U16, void *, U16))F("AdsDDSetArticleProperty"))
                (c, (U8 *)"pub", (U8 *)"t", ADS_DD_ARTICLE_EXCLUDE_COLUMNS, (void *)"s",
                 (U16)strlen("s") + 1);
        } else if (!strcmp(nm, "artdel")) {
            ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
                (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"id > 5", 0);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDDeleteArticle"))(c, (U8 *)"pub", (U8 *)"t");
            if (rc) oops("DeleteArticle", rc);
        } else if (!strcmp(nm, "art100")) {
            WCHAR wf[64];
            MultiByteToWideChar(CP_UTF8, 0, "id > 5", -1, wf, 64);
            rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, WCHAR *, U32))F("AdsDDCreateArticle100"))
                (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", wf, 0);
            if (rc) oops("CreateArticle100", rc);
        }
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    /* Subscription scenarios. The dict is named db.add (as the tflip test's is),
     * so the queue file's dict back-reference matches. */
    if ((c = make_as(out, "subadd", "db"))) {
        create_pub(c, "pub", "", 0);
        U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
                  F("AdsDDCreateSubscription"))
            (c, (U8 *)"sub", (U8 *)"pub", (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw",
             (U8 *)"q.add", 1, (U8 *)"cmt", 0);
        if (rc) oops("CreateSubscription", rc);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }
    if ((c = make_as(out, "subset", "db"))) {
        create_pub(c, "pub", "", 0);
        ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
         F("AdsDDCreateSubscription"))
            (c, (U8 *)"sub", (U8 *)"pub", (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw",
             (U8 *)"q.add", 0, (U8 *)"", 0);
        U16 yes = 1;
        U32 opt = 0x08;
        ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
            (c, (U8 *)"sub", 1709, &yes, sizeof yes); /* paused */
        ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
            (c, (U8 *)"sub", 1710, &yes, sizeof yes); /* comm TCP/IP */
        ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
            (c, (U8 *)"sub", 1716, (void *)"conn", (U16)strlen("conn") + 1); /* connstr */
        ((U32(WINAPI *)(H, U8 *, U16, void *, U16))F("AdsDDSetSubscriptionProperty"))
            (c, (U8 *)"sub", 3, (void *)"ud", (U16)strlen("ud") + 1); /* user-defined */
        (void)opt;
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }
    if ((c = make_as(out, "subdel", "db"))) {
        create_pub(c, "pub", "", 0);
        ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
         F("AdsDDCreateSubscription"))
            (c, (U8 *)"sub", (U8 *)"pub", (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw",
             (U8 *)"q.add", 1, (U8 *)"cmt", 0);
        U32 rc = ((U32(WINAPI *)(H, U8 *))F("AdsDDDeleteSubscription"))(c, (U8 *)"sub");
        if (rc) oops("DeleteSubscription", rc);
        ((U32(WINAPI *)(H))F("AdsDisconnect"))(c);
    }

    return 0;
}
