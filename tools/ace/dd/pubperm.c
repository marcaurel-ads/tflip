/* Probes permissions granted ON publications, articles and subscriptions
 * (object types 15, 16, 17): which rights AdsDDGrantPermission accepts for
 * them, the error codes for the rest, what AdsDDGetPermissions and
 * system.permissions report, whether SQL GRANT/REVOKE reach them — and, as
 * the same code path serves every object type, the rules it surfaced for all
 * of them: the stored inherit bit, revokes carrying the grant option, when the
 * with-grant slot is written, revokes with nothing granted. Builds the golden
 * dictionaries fixtures/dictionary/ops/permpub*, permsub*, revkinds, revnone,
 * colrev and colrev0 (.add/.am/.ai). Black-box: only the official ACE is
 * called.
 *
 *     pubperm.exe <ace64.dll> <out folder>  > fixtures/dictionary/pubperm.json
 *
 * Every scenario's dictionary is <out>\<name>\db.add, made with a user bob
 * (password pw), a group G1, a publication pub and a subscription sub (queue
 * q.add) — in that order, so the object ids are fixed. Some scenarios also add
 * table t (t.adt/t.adi in <out>, copied by pubperm.sh) and an article on it.
 * Each JSON line is {"scenario", "op", "rc"} plus "mask" for a get and the
 * rows of a query; tflip replays the grant/revoke/get lines and compares.
 * Built with x86_64-w64-mingw32-gcc and run under Wine (see pubperm.sh). */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(name) GetProcAddress(ace, name)
#define ADS_ADT  3
#define ADS_ANSI 1

static int first = 1;

static void jstr(const char *s) {
    if (!s) { printf("null"); return; }
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') printf("\\%c", *s);
        else if ((U8)*s < 32 || (U8)*s > 126) printf("\\u%04x", (U8)*s);
        else putchar(*s);
    }
    putchar('"');
}

static void begin(const char *scenario, const char *op) {
    printf("%s\n{\"scenario\": \"%s\", \"op\": ", first ? "[" : ",", scenario);
    jstr(op);
    first = 0;
}

static void oops(const char *what, U32 rc) {
    U8 text[1024];
    U16 len = sizeof text;
    U32 code = 0;
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
    fprintf(stderr, "%s: rc=%lu %s\n", what, (unsigned long)rc, text);
}

static U32 grant(H c, U16 type, const char *name, const char *parent, const char *grantee, U32 bits) {
    return ((U32(WINAPI *)(H, U16, U8 *, U8 *, U8 *, U32))F("AdsDDGrantPermission"))
        (c, type, (U8 *)name, (U8 *)parent, (U8 *)grantee, bits);
}

static U32 revoke(H c, U16 type, const char *name, const char *parent, const char *grantee, U32 bits) {
    return ((U32(WINAPI *)(H, U16, U8 *, U8 *, U8 *, U32))F("AdsDDRevokePermission"))
        (c, type, (U8 *)name, (U8 *)parent, (U8 *)grantee, bits);
}

static U32 getperm(H c, const char *grantee, U16 type, const char *name, const char *parent,
                   U16 inherited, U32 *mask) {
    *mask = 0xAAAAAAAA;
    return ((U32(WINAPI *)(H, U8 *, U16, U8 *, U8 *, U16, U32 *))F("AdsDDGetPermissions"))
        (c, (U8 *)grantee, type, (U8 *)name, (U8 *)parent, inherited, mask);
}

/* Record one grant/revoke call: its rc. */
static void rec_change(const char *sc, int rev, H c, U16 type, const char *name,
                       const char *parent, const char *grantee, U32 bits) {
    U32 rc = rev ? revoke(c, type, name, parent, grantee, bits)
                 : grant(c, type, name, parent, grantee, bits);
    char op[160];
    snprintf(op, sizeof op, "%s type=%u name=%s parent=%s grantee=%s bits=0x%lx",
             rev ? "revoke" : "grant", type, name ? name : "(null)", parent ? parent : "(null)",
             grantee, (unsigned long)bits);
    begin(sc, op);
    printf(", \"rc\": %lu}", (unsigned long)rc);
}

/* Record one AdsDDGetPermissions call: its rc and mask. */
static void rec_get(const char *sc, H c, const char *grantee, U16 type, const char *name,
                    const char *parent, U16 inherited) {
    U32 mask;
    U32 rc = getperm(c, grantee, type, name, parent, inherited, &mask);
    char op[160];
    snprintf(op, sizeof op, "get grantee=%s type=%u name=%s parent=%s inherited=%u", grantee,
             type, name ? name : "(null)", parent ? parent : "(null)", inherited);
    begin(sc, op);
    printf(", \"rc\": %lu, \"mask\": \"0x%lx\"}", (unsigned long)rc, (unsigned long)mask);
}

/* Copy the committed t.adt/t.adi into a scenario's folder and free it (see
 * replops.c), so AddTable binds a fresh copy. */
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

/* <out>\<name>\db.add with bob, G1, pub and sub (and, with table, t and an
 * article on it). */
static H base(const char *out, const char *name, int table) {
    char dir[MAX_PATH], path[MAX_PATH];
    snprintf(dir, sizeof dir, "%s\\%s", out, name);
    CreateDirectoryA(dir, NULL);
    snprintf(path, sizeof path, "%s\\db.add", dir);
    H c = 0;
    U32 rc = ((U32(WINAPI *)(U8 *, U16, U8 *, H *))F("AdsDDCreate"))((U8 *)path, 0, NULL, &c);
    if (rc) { oops(name, rc); return 0; }
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *))F("AdsDDCreateUser"))
        (c, (U8 *)"", (U8 *)"bob", (U8 *)"pw", (U8 *)"");
    if (rc) oops("CreateUser", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDCreateUserGroup"))(c, (U8 *)"G1", (U8 *)"");
    if (rc) oops("CreateUserGroup", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U32))F("AdsDDCreatePublication"))
        (c, (U8 *)"pub", (U8 *)"", 0);
    if (rc) oops("CreatePublication", rc);
    rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U8 *, U8 *, U16, U8 *, U32))
          F("AdsDDCreateSubscription"))
        (c, (U8 *)"sub", (U8 *)"pub", (U8 *)"\\\\srv\\t.add", (U8 *)"u", (U8 *)"pw",
         (U8 *)"q.add", 0, (U8 *)"", 0);
    if (rc) oops("CreateSubscription", rc);
    if (table) {
        copy_table(out, name);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U8 *, U8 *))F("AdsDDAddTable"))
            (c, (U8 *)"t", (U8 *)"t.adt", ADS_ADT, ADS_ANSI, (U8 *)"t.adi", (U8 *)"");
        if (rc) oops("AddTable", rc);
        rc = ((U32(WINAPI *)(H, U8 *, U8 *, U8 *, U8 *, U32))F("AdsDDCreateArticle"))
            (c, (U8 *)"pub", (U8 *)"t", (U8 *)"id", (U8 *)"", 0);
        if (rc) oops("CreateArticle", rc);
    }
    return c;
}

/* Put bob in G1, recording the rc. */
static void add_to_group(const char *sc, H c) {
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *))F("AdsDDAddUserToGroup"))(c, (U8 *)"G1", (U8 *)"bob");
    begin(sc, "AddUserToGroup G1 bob");
    printf(", \"rc\": %lu}", (unsigned long)rc);
}

static void disconnect(H c) { ((U32(WINAPI *)(H))F("AdsDisconnect"))(c); }

/* Run `sql` on a statement of `c`; with dump, print the cursor's rows. */
static void rec_sql(const char *sc, H c, const char *sql, int dump) {
    H s = 0, cur = 0;
    ((U32(WINAPI *)(H, H *))F("AdsCreateSQLStatement"))(c, &s);
    U32 rc = ((U32(WINAPI *)(H, U8 *, H *))F("AdsExecuteSQLDirect"))(s, (U8 *)sql, &cur);
    begin(sc, sql);
    printf(", \"rc\": %lu", (unsigned long)rc);
    if (rc) {
        U8 text[1024];
        U16 len = sizeof text;
        U32 code = 0;
        ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, text, &len);
        printf(", \"error\": ");
        jstr((char *)text);
    }
    if (rc == 0 && cur && dump) {
        U16 cols = 0;
        ((U32(WINAPI *)(H, U16 *))F("AdsGetNumFields"))(cur, &cols);
        printf(", \"names\": [");
        for (U16 f = 1; f <= cols; f++) {
            U8 nm[200];
            U16 nl = sizeof nm;
            ((U32(WINAPI *)(H, U16, U8 *, U16 *))F("AdsGetFieldName"))(cur, f, nm, &nl);
            if (f > 1) printf(", ");
            jstr((char *)nm);
        }
        printf("], \"rows\": [");
        ((U32(WINAPI *)(H))F("AdsGotoTop"))(cur);
        int firstrow = 1;
        for (;;) {
            U16 eof = 0;
            ((U32(WINAPI *)(H, U16 *))F("AdsAtEOF"))(cur, &eof);
            if (eof) break;
            printf("%s\n  [", firstrow ? "" : ",");
            firstrow = 0;
            for (U16 f = 1; f <= cols; f++) {
                U8 val[512];
                U32 len = sizeof val - 1;
                U16 empty = 0;
                memset(val, 0, sizeof val);
                ((U32(WINAPI *)(H, U8 *, U16 *))F("AdsIsEmpty"))(cur, (U8 *)(ULONG_PTR)f, &empty);
                U32 frc = ((U32(WINAPI *)(H, U8 *, U8 *, U32 *, U16))F("AdsGetField"))
                    (cur, (U8 *)(ULONG_PTR)f, val, &len, 0);
                if (f > 1) printf(", ");
                if (frc == 0 && !empty) {
                    val[len < sizeof val ? len : sizeof val - 1] = 0;
                    for (int k = (int)strlen((char *)val) - 1; k >= 0 && val[k] == ' '; k--)
                        val[k] = 0; /* the padding of a fixed-width column */
                    jstr((char *)val);
                } else printf("null");
            }
            printf("]");
            ((U32(WINAPI *)(H, long))F("AdsSkip"))(cur, 1);
        }
        printf("]");
    }
    printf("}");
    if (cur) ((U32(WINAPI *)(H))F("AdsCloseTable"))(cur);
    ((U32(WINAPI *)(H))F("AdsCloseSQLStatement"))(s);
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: pubperm.exe <ace64.dll> <out folder>\n");
        return 2;
    }
    ace = LoadLibraryA(argv[1]);
    if (!ace) { fprintf(stderr, "cannot load %s\n", argv[1]); return 1; }
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1); /* local server */
    const char *out = argv[2];
    H c;

    /* bits: each single right, the grant option and ALL on each object type,
     * granted then revoked, the mask read after each step. */
    if ((c = base(out, "bits", 1))) {
        static const U32 bits[] = {0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200,
                                   0x400, 0x80000000, 0x80000100, 0x8FFFFFFF, 0xFFFFFFFF, 0};
        struct { U16 type; const char *name, *parent; } objs[] = {
            {15, "pub", ""}, {16, "t", "pub"}, {17, "sub", ""},
        };
        for (size_t o = 0; o < sizeof objs / sizeof *objs; o++) {
            rec_get("bits", c, "bob", objs[o].type, objs[o].name, objs[o].parent, 0);
            rec_get("bits", c, "G1", objs[o].type, objs[o].name, objs[o].parent, 0);
            for (size_t b = 0; b < sizeof bits / sizeof *bits; b++) {
                rec_change("bits", 0, c, objs[o].type, objs[o].name, objs[o].parent, "bob", bits[b]);
                rec_get("bits", c, "bob", objs[o].type, objs[o].name, objs[o].parent, 0);
                rec_change("bits", 1, c, objs[o].type, objs[o].name, objs[o].parent, "bob", bits[b]);
                rec_get("bits", c, "bob", objs[o].type, objs[o].name, objs[o].parent, 0);
            }
        }
        /* Unknown names, an unknown grantee, null names. */
        rec_change("bits", 0, c, 15, "nopub", "", "bob", 0x100);
        rec_change("bits", 0, c, 17, "nosub", "", "bob", 0x100);
        rec_change("bits", 0, c, 15, "pub", "", "ghost", 0x100);
        rec_change("bits", 0, c, 15, NULL, NULL, "bob", 0x100);
        rec_change("bits", 0, c, 17, NULL, NULL, "bob", 0x100);
        rec_change("bits", 0, c, 15, "PUB", "", "BOB", 0x100);
        rec_get("bits", c, "bob", 15, "pub", "", 0);
        rec_get("bits", c, "bob", 15, "nopub", "", 0);
        rec_get("bits", c, "ghost", 15, "pub", "", 0);
        rec_get("bits", c, "DB:Admin", 15, "pub", "", 0);
        rec_get("bits", c, "DB:Public", 15, "pub", "", 0);
        rec_get("bits", c, "DB:Admin", 17, "sub", "", 0);
        rec_get("bits", c, "DB:Public", 17, "sub", "", 0);
        /* Group grant inherited by a member. */
        add_to_group("bits", c);
        rec_change("bits", 0, c, 17, "sub", "", "G1", 0x200);
        rec_get("bits", c, "G1", 17, "sub", "", 0);
        rec_get("bits", c, "bob", 17, "sub", "", 0);
        rec_get("bits", c, "bob", 17, "sub", "", 1);
        /* The other object types with no permissions, for reference. */
        static const U16 others[] = {0, 2, 3, 5, 7, 13, 14, 20, 21, 22};
        for (size_t i = 0; i < sizeof others / sizeof *others; i++) {
            rec_change("bits", 0, c, others[i], "t", "", "bob", 0x100);
            rec_get("bits", c, "bob", others[i], "t", "", 0);
        }
        disconnect(c);
    }

    /* sql: SQL GRANT/REVOKE forms on publications/subscriptions, and the
     * system.permissions rows. */
    if ((c = base(out, "sql", 0))) {
        const char *stmts[] = {
            "GRANT ALTER ON pub TO bob",
            "GRANT ALTER ON PUBLICATION pub TO bob",
            "GRANT DROP ON SUBSCRIPTION sub TO bob",
            "GRANT ALTER ON [pub] TO bob",
            "REVOKE ALTER ON PUBLICATION pub FROM bob",
            "REVOKE DROP ON SUBSCRIPTION sub FROM bob",
        };
        for (size_t i = 0; i < sizeof stmts / sizeof *stmts; i++) {
            rec_sql("sql", c, stmts[i], 0);
            rec_get("sql", c, "bob", 15, "pub", "", 0);
            rec_get("sql", c, "bob", 17, "sub", "", 0);
        }
        rec_change("sql", 0, c, 15, "pub", "", "bob", 0x100);
        rec_change("sql", 0, c, 17, "sub", "", "G1", 0x200);
        add_to_group("sql", c);
        rec_sql("sql", c, "SELECT * FROM system.permissions WHERE Object_Type IN (15, 17)", 1);
        rec_sql("sql", c,
                "SELECT * FROM system.effectivepermissions WHERE Object_Type IN (15, 17)", 1);
        disconnect(c);
    }

    /* inh: the stored inherit bit and the grant-option revoke, on a table and a
     * publication alike — does a revoke of INHERIT stick, does a later grant
     * put it back, does a group grant reach a user without it, does a revoke
     * carrying WITH_GRANT drop only the grant option? */
    if ((c = base(out, "inh", 1))) {
        struct { U16 type; const char *name; U32 right; } objs[] = {
            {1, "t", 0x1}, {15, "pub", 0x100},
        };
        add_to_group("inh", c);
        for (size_t o = 0; o < 2; o++) {
            U16 ty = objs[o].type;
            const char *nm = objs[o].name;
            U32 r = objs[o].right;
            rec_change("inh", 0, c, ty, nm, "", "G1", r);
            rec_get("inh", c, "bob", ty, nm, "", 1);
            rec_change("inh", 0, c, ty, nm, "", "bob", 0x80000000 | r);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_change("inh", 1, c, ty, nm, "", "bob", 0x80000000 | r);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_change("inh", 1, c, ty, nm, "", "bob", r);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_change("inh", 1, c, ty, nm, "", "bob", 0x8);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_get("inh", c, "bob", ty, nm, "", 1);
            rec_change("inh", 0, c, ty, nm, "", "bob", 0x200);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_get("inh", c, "bob", ty, nm, "", 1);
            rec_change("inh", 0, c, ty, nm, "", "bob", 0x8);
            rec_get("inh", c, "bob", ty, nm, "", 0);
            rec_get("inh", c, "bob", ty, nm, "", 1);
            rec_change("inh", 0, c, ty, nm, "", "G1", 0x8);
            rec_change("inh", 0, c, ty, nm, "", "bob", 0x80);
            rec_change("inh", 0, c, ty, nm, "", "bob", 0);
        }
        /* A column grant with the grant option, then its revokes. */
        rec_change("inh", 0, c, 4, "s", "t", "bob", 0x80000001);
        rec_get("inh", c, "bob", 4, "s", "t", 0);
        rec_change("inh", 1, c, 4, "s", "t", "bob", 0x80000001);
        rec_get("inh", c, "bob", 4, "s", "t", 0);
        rec_change("inh", 1, c, 4, "s", "t", "bob", 0x1);
        rec_get("inh", c, "bob", 4, "s", "t", 0);
        rec_sql("inh", c, "SELECT * FROM system.permissions WHERE Grantee = 'bob'", 1);
        rec_sql("inh", c, "SELECT * FROM system.effectivepermissions WHERE Grantee = 'bob'", 1);
        disconnect(c);
    }

    /* order: where publications and subscriptions sit in system.permissions
     * among every other object kind. */
    if ((c = base(out, "order", 1))) {
        rec_sql("order", c, "CREATE VIEW v AS SELECT id FROM t", 0);
        rec_sql("order", c, "CREATE PROCEDURE p (x INTEGER, y INTEGER OUTPUT) BEGIN "
                "INSERT INTO __output SELECT x FROM __input; END;", 0);
        rec_sql("order", c, "CREATE FUNCTION f() RETURNS INTEGER BEGIN RETURN 1; END", 0);
        rec_sql("order", c, "CREATE PACKAGE pk", 0);
        rec_sql("order", c, "EXECUTE PROCEDURE sp_CreateLink ('lnk', 'db.add', TRUE, FALSE, "
                "FALSE, '', '')", 0);
        rec_sql("order", c, "SELECT * FROM system.permissions WHERE Grantee = 'bob'", 1);
        disconnect(c);
    }

    /* Goldens. permpub: ALTER then DROP to bob on pub (accumulating), ALTER
     * to G1. */
    if ((c = base(out, "permpub", 0))) {
        rec_change("permpub", 0, c, 15, "pub", "", "bob", 0x100);
        rec_change("permpub", 0, c, 15, "pub", "", "bob", 0x200);
        rec_change("permpub", 0, c, 15, "pub", "", "G1", 0x100);
        disconnect(c);
    }
    /* permsub: ALL to bob on sub, DROP with the grant option to G1. */
    if ((c = base(out, "permsub", 0))) {
        rec_change("permsub", 0, c, 17, "sub", "", "bob", 0xFFFFFFFF);
        rec_change("permsub", 0, c, 17, "sub", "", "G1", 0x80000200);
        disconnect(c);
    }
    /* permpubrev: ALTER|DROP to bob on pub with the grant option, revoke DROP. */
    if ((c = base(out, "permpubrev", 0))) {
        rec_change("permpubrev", 0, c, 15, "pub", "", "bob", 0x80000300);
        rec_change("permpubrev", 1, c, 15, "pub", "", "bob", 0x200);
        disconnect(c);
    }
    /* permpubrevg: ALTER|DROP to bob on pub with the grant option, then a
     * revoke of ALTER carrying the grant option. */
    if ((c = base(out, "permpubrevg", 0))) {
        rec_change("permpubrevg", 0, c, 15, "pub", "", "bob", 0x80000300);
        rec_change("permpubrevg", 1, c, 15, "pub", "", "bob", 0x80000100);
        disconnect(c);
    }
    /* permsubrevnone: revoke ALTER on sub from bob, never granted. */
    if ((c = base(out, "permsubrevnone", 0))) {
        rec_change("permsubrevnone", 1, c, 17, "sub", "", "bob", 0x100);
        disconnect(c);
    }
    /* permpubinh: ALTER to bob on pub, revoke INHERIT, then grant DROP. */
    if ((c = base(out, "permpubinh", 0))) {
        rec_change("permpubinh", 0, c, 15, "pub", "", "bob", 0x100);
        rec_change("permpubinh", 1, c, 15, "pub", "", "bob", 0x8);
        rec_change("permpubinh", 0, c, 15, "pub", "", "bob", 0x200);
        disconnect(c);
    }
    /* When is the with-grant slot 3 written, kept or dropped?
     * permpubri: ALTER to bob, revoke INHERIT (a revoke of a bit never held
     * otherwise). permpubrg: ALTER|DROP, revoke DROP, grant DROP again.
     * permpubgg: ALTER with the grant option, then DROP without it.
     * permpubgrg: ALTER with the grant option, revoke ALTER, grant DROP. */
    if ((c = base(out, "permpubri", 0))) {
        rec_change("permpubri", 0, c, 15, "pub", "", "bob", 0x100);
        rec_change("permpubri", 1, c, 15, "pub", "", "bob", 0x8);
        disconnect(c);
    }
    if ((c = base(out, "permpubrg", 0))) {
        rec_change("permpubrg", 0, c, 15, "pub", "", "bob", 0x300);
        rec_change("permpubrg", 1, c, 15, "pub", "", "bob", 0x200);
        rec_change("permpubrg", 0, c, 15, "pub", "", "bob", 0x200);
        disconnect(c);
    }
    if ((c = base(out, "permpubgg", 0))) {
        rec_change("permpubgg", 0, c, 15, "pub", "", "bob", 0x80000100);
        rec_change("permpubgg", 0, c, 15, "pub", "", "bob", 0x200);
        disconnect(c);
    }
    if ((c = base(out, "permpubgrg", 0))) {
        rec_change("permpubgrg", 0, c, 15, "pub", "", "bob", 0x80000100);
        rec_change("permpubgrg", 1, c, 15, "pub", "", "bob", 0x100);
        rec_change("permpubgrg", 0, c, 15, "pub", "", "bob", 0x200);
        disconnect(c);
    }
    /* revkinds: on each kind of object, bob is granted ALTER (a column UPDATE)
     * then has it revoked, to see on which kinds a revoke writes the with-grant
     * slot 3 when the grant had none. (G1 gets the same on the user bob.)
     * valid: the same objects, for which rights each kind accepts. */
    const char *kinds[] = {"revkinds", "valid"};
    for (int k = 0; k < 2; k++) {
        const char *sc = kinds[k];
        if (!(c = base(out, sc, 1))) continue;
        rec_sql(sc, c, "CREATE VIEW v AS SELECT id FROM t", 0);
        rec_sql(sc, c, "CREATE PROCEDURE p (x INTEGER, y INTEGER OUTPUT) BEGIN "
                "INSERT INTO __output SELECT x FROM __input; END;", 0);
        rec_sql(sc, c, "CREATE FUNCTION f() RETURNS INTEGER BEGIN RETURN 1; END", 0);
        rec_sql(sc, c, "CREATE PACKAGE pk", 0);
        rec_sql(sc, c, "EXECUTE PROCEDURE sp_CreateLink ('lnk', 'db.add', TRUE, FALSE, "
                "FALSE, '', '')", 0);
        struct { U16 type; const char *name, *parent, *grantee; U32 right; } objs[] = {
            {1, "t", "", "bob", 0x100},   {4, "s", "t", "bob", 0x2}, {6, "v", "", "bob", 0x100},
            {10, "p", "", "bob", 0x100},  {18, "f", "", "bob", 0x100}, {19, "pk", "", "bob", 0x100},
            {12, "lnk", "", "bob", 0x100}, {9, "G1", "", "bob", 0x100}, {11, "", "", "bob", 0x100},
            {15, "pub", "", "bob", 0x100}, {17, "sub", "", "bob", 0x100}, {8, "bob", "", "G1", 0x100},
        };
        size_t n = sizeof objs / sizeof *objs;
        if (k == 0) {
            for (size_t o = 0; o < n; o++) {
                rec_change(sc, 0, c, objs[o].type, objs[o].name, objs[o].parent,
                           objs[o].grantee, objs[o].right);
                rec_change(sc, 1, c, objs[o].type, objs[o].name, objs[o].parent,
                           objs[o].grantee, objs[o].right);
            }
        } else {
            /* INHERIT, CREATE, no bits and ALL with the grant option, to bob. */
            static const U32 tries[] = {0x8, 0x80, 0, 0x8FFFFFFF};
            for (size_t o = 0; o < n; o++)
                for (size_t i = 0; i < 4; i++)
                    rec_change(sc, 0, c, objs[o].type, objs[o].name, objs[o].parent, "bob",
                               tries[i]);
            rec_change(sc, 0, c, 8, "bob", "", "bob", 0x100);
            rec_change(sc, 0, c, 9, "G1", "", "G1", 0x100);
            rec_change(sc, 0, c, 8, "bob", "", "G1", 0x8);
            rec_change(sc, 0, c, 15, "pub", "", "G1", 0x8FFFFFFF);
            rec_change(sc, 0, c, 15, "pub", "", "G1", 0xFFFFFFFF);
            rec_change(sc, 0, c, 15, "pub", "", "bob", 0x180);
            rec_change(sc, 0, c, 15, "pub", "", "bob", 0x480);
            rec_change(sc, 0, c, 15, "nopub", "", "bob", 0x1);
            rec_change(sc, 1, c, 15, "pub", "", "G1", 0x8);
        }
        disconnect(c);
    }
    /* revnone: revokes with no grant before — ALTER on t from bob, ALTER on
     * pub from G1, UPDATE on column s from G1. */
    if ((c = base(out, "revnone", 1))) {
        rec_change("revnone", 1, c, 1, "t", "", "bob", 0x100);
        rec_change("revnone", 1, c, 15, "pub", "", "G1", 0x100);
        rec_change("revnone", 1, c, 4, "s", "t", "G1", 0x2);
        disconnect(c);
    }
    /* colrev: column revokes against a grant made with the grant option —
     * bob: READ|UPDATE on s with it, then READ revoked with it; G1: the same
     * grant, then UPDATE revoked plainly. */
    if ((c = base(out, "colrev", 1))) {
        rec_change("colrev", 0, c, 4, "s", "t", "bob", 0x80000003);
        rec_change("colrev", 1, c, 4, "s", "t", "bob", 0x80000001);
        rec_get("colrev", c, "bob", 4, "s", "t", 0);
        rec_change("colrev", 0, c, 4, "s", "t", "G1", 0x80000003);
        rec_change("colrev", 1, c, 4, "s", "t", "G1", 0x2);
        rec_get("colrev", c, "G1", 4, "s", "t", 0);
        disconnect(c);
    }
    /* colrev0: a column's only grant (READ, with the grant option) revoked
     * plainly, leaving every per-column byte zero. */
    if ((c = base(out, "colrev0", 1))) {
        rec_change("colrev0", 0, c, 4, "s", "t", "bob", 0x80000001);
        rec_change("colrev0", 1, c, 4, "s", "t", "bob", 0x1);
        disconnect(c);
    }
    /* permpuballg: ALL with the grant option to bob on pub and to G1 on sub. */
    if ((c = base(out, "permpuballg", 0))) {
        rec_change("permpuballg", 0, c, 15, "pub", "", "bob", 0x8FFFFFFF);
        rec_change("permpuballg", 0, c, 17, "sub", "", "G1", 0x8FFFFFFF);
        disconnect(c);
    }

    printf("\n]\n");
    return 0;
}
