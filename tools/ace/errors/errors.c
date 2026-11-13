/* Records the original engine's error texts, for fixtures/errors/texts.json.
 *
 *     errors.exe <ace32.dll>  > texts.json
 *
 * Built with i686-w64-mingw32-gcc and run under Wine. AdsGetErrorString
 * is called for every code from 0 to 10000; codes it has a text of its own
 * for are recorded as {"<code>": "<text>"}, in Windows-1252 escaped as
 * \u00XX, or {"<code>": {"rc": <rc>}} if it fails; codes it has no text
 * for (AE_STRING_NOT_FOUND, or "Error code <code> is unknown.") are left
 * out, but 9999999 comes first. Then a short buffer, as
 * {"short": [rc, len, written]}. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef U32(WINAPI *GetErrorString)(U32, U8 *, U16 *);

static void json(const U8 *s, size_t n) {
    putchar('"');
    for (size_t i = 0; i < n; i++) {
        U8 c = s[i];
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 32 || c >= 127) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

int main(int argc, char **argv) {
    HMODULE ace = LoadLibraryA(argv[1]);
    GetErrorString get = (GetErrorString)GetProcAddress(ace, "AdsGetErrorString");
    U8 big[200];
    U16 bn = sizeof big;
    U32 brc = get(9999999, big, &bn);
    printf("{\"9999999\": ");
    if (brc) printf("{\"rc\": %lu}", brc); else json(big, bn);
    for (U32 code = 0; code <= 10000; code++) {
        U8 text[1200];
        U16 n = sizeof text;
        U32 rc = get(code, text, &n);
        if (rc == 5055) continue; /* AE_STRING_NOT_FOUND: no text */
        if (rc != 0) {
            printf(",\n\"%lu\": {\"rc\": %lu}", code, rc);
            continue;
        }
        char unknown[64];
        int u = snprintf(unknown, sizeof unknown, "Error code %lu is unknown.", code);
        if (n == u && !memcmp(text, unknown, n)) continue;
        printf(",\n\"%lu\": ", code);
        json(text, n);
    }
    U8 text[10];
    memset(text, 0x5A, sizeof text);
    U16 n = 5;
    U32 rc = get(5004, text, &n);
    printf(",\n\"short\": [%lu, %u, ", rc, n);
    json(text, 7);
    printf("]\n}\n");
    return 0;
}
