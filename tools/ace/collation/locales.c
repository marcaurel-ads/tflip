/* Lists, for every locale Windows (or Wine) knows, the static collation the
 * original engine derives from it when ANSI_CHAR_SET is empty (see
 * sysdefault.c and the README): each byte's weight is its rank when the 256
 * single-byte strings are sorted by CompareStringA under the locale, equal
 * ones by their byte; its case tables are LCMapStringA's.
 *
 *     locales.exe > locales.txt
 *
 * Built with i686-w64-mingw32-gcc; it calls no Advantage code. One line per
 * locale: the LCID (hex), its name (LOCALE_SNAME), its language abbreviation
 * (LOCALE_SABBREVLANGNAME), its ANSI code page, and the 256 weights,
 * upper-case and lower-case bytes, each as hex. gen_locales.py turns the
 * output into crates/tflip-engine/src/system_collations.bin. */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>

static LCID lcid;

static int compare(int a, int b) {
    char x = (char)a, y = (char)b;
    int r = CompareStringA(lcid, 0, &x, 1, &y, 1);
    if (r != CSTR_EQUAL) return r - CSTR_EQUAL;
    return a - b;
}

static BOOL CALLBACK locale(LPSTR text) {
    lcid = strtoul(text, NULL, 16);
    char name[LOCALE_NAME_MAX_LENGTH] = "", abbrev[16] = "", acp[16] = "";
    GetLocaleInfoA(lcid, 0x5c /* LOCALE_SNAME */, name, sizeof name);
    GetLocaleInfoA(lcid, LOCALE_SABBREVLANGNAME, abbrev, sizeof abbrev);
    GetLocaleInfoA(lcid, LOCALE_IDEFAULTANSICODEPAGE, acp, sizeof acp);
    int order[256];
    for (int i = 0; i < 256; i++) order[i] = i;
    for (int i = 1; i < 256; i++) /* a stable insertion sort */
        for (int j = i; j > 0 && compare(order[j - 1], order[j]) > 0; j--) {
            int t = order[j];
            order[j] = order[j - 1];
            order[j - 1] = t;
        }
    unsigned char weights[256], upper[256], lower[256];
    for (int i = 0; i < 256; i++) weights[order[i]] = (unsigned char)i;
    for (int i = 0; i < 256; i++) {
        /* Byte by byte: a double-byte code page's lead byte stays itself. */
        char b = (char)i, u = b, l = b;
        if (i) {
            LCMapStringA(lcid, LCMAP_UPPERCASE, &b, 1, &u, 1);
            LCMapStringA(lcid, LCMAP_LOWERCASE, &b, 1, &l, 1);
        }
        upper[i] = (unsigned char)u;
        lower[i] = (unsigned char)l;
    }
    printf("%04lx %s %s %s ", (unsigned long)lcid, name[0] ? name : "-", abbrev[0] ? abbrev : "-", acp);
    for (int i = 0; i < 256; i++) printf("%02x", weights[i]);
    putchar(' ');
    for (int i = 0; i < 256; i++) printf("%02x", upper[i]);
    putchar(' ');
    for (int i = 0; i < 256; i++) printf("%02x", lower[i]);
    putchar('\n');
    return TRUE;
}

int main(void) {
    EnumSystemLocalesA(locale, LCID_SUPPORTED);
    return 0;
}
