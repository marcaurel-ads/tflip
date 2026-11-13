/* Probe: what does AdsImageToClipboard put on the Windows clipboard?
 *
 *   clipboard.exe <ace32.dll> <writable data folder>
 *
 * Creates a one-field ADS_IMAGE table, stores a known 2x2 24-bpp BMP in it,
 * calls AdsImageToClipboard, then opens the clipboard and dumps every format
 * present: the format id, its registered name (if any), GlobalSize and the
 * first bytes. That tells us which clipboard format(s) the engine sets
 * (CF_DIB / CF_BITMAP / CF_DIBV5 / a private format) and the exact payload
 * shape, so tflip can replicate it clean-room. JSON to stdout. */
#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef unsigned long U32;
typedef unsigned short U16;
typedef unsigned char U8;
typedef ULONG_PTR H;

static HMODULE ace;
#define F(n) GetProcAddress(ace, n)

static void last_error(const char *tag, U32 rc) {
    U32 code = 0xDEAD;
    U8 buf[600];
    U16 len = sizeof buf;
    memset(buf, 0, sizeof buf);
    ((U32(WINAPI *)(U32 *, U8 *, U16 *))F("AdsGetLastError"))(&code, buf, &len);
    printf("  {\"step\": \"%s\", \"rc\": %lu, \"lasterr\": %lu, \"msg\": \"", tag, rc, code);
    for (U16 i = 0; i < len && buf[i]; i++)
        printf((buf[i] == '"' || buf[i] == '\\') ? "\\%c" : (buf[i] < 32 ? " " : "%c"), buf[i]);
    printf("\"},\n");
}

/* A valid 2x2, 24-bpp bottom-up BMP: 14-byte file header + 40-byte info header
 * + 16 bytes of pixel data (each 6-byte row padded to 8). 70 bytes total. */
static U8 bmp[70];
static void make_bmp(void) {
    memset(bmp, 0, sizeof bmp);
    bmp[0] = 'B';
    bmp[1] = 'M';
    *(U32 *)(bmp + 2) = 70;   /* bfSize */
    *(U32 *)(bmp + 10) = 54;  /* bfOffBits: 14 + 40 */
    *(U32 *)(bmp + 14) = 40;  /* biSize */
    *(long *)(bmp + 18) = 2;  /* biWidth */
    *(long *)(bmp + 22) = 2;  /* biHeight */
    *(U16 *)(bmp + 26) = 1;   /* biPlanes */
    *(U16 *)(bmp + 28) = 24;  /* biBitCount */
    *(U32 *)(bmp + 34) = 16;  /* biSizeImage */
    /* pixels: row0 red,green  row1 blue,white (BGR order) */
    U8 px[16] = {0, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 255, 255, 255, 255, 0};
    memcpy(bmp + 54, px, 16);
}

static void dump_clipboard(void) {
    if (!OpenClipboard(NULL)) {
        printf("  {\"step\": \"OpenClipboard\", \"err\": %lu},\n", GetLastError());
        return;
    }
    printf("  {\"step\": \"formats\", \"list\": [\n");
    U32 fmt = 0;
    int first = 1;
    while ((fmt = EnumClipboardFormats(fmt)) != 0) {
        char name[128];
        int nl = GetClipboardFormatNameA(fmt, name, sizeof name);
        if (nl <= 0) {
            /* a standard format: label the common ones */
            const char *std = "";
            switch (fmt) {
            case CF_TEXT: std = "CF_TEXT"; break;
            case CF_BITMAP: std = "CF_BITMAP"; break;
            case CF_METAFILEPICT: std = "CF_METAFILEPICT"; break;
            case CF_DIB: std = "CF_DIB"; break;
            case CF_PALETTE: std = "CF_PALETTE"; break;
            case CF_ENHMETAFILE: std = "CF_ENHMETAFILE"; break;
            case CF_DIBV5: std = "CF_DIBV5"; break;
            default: std = "?"; break;
            }
            strncpy(name, std, sizeof name - 1);
            name[sizeof name - 1] = 0;
        }
        HANDLE data = GetClipboardData(fmt);
        SIZE_T size = data ? GlobalSize(data) : 0;
        printf("%s    {\"fmt\": %lu, \"name\": \"%s\", \"size\": %llu, \"bytes\": \"", first ? "" : ",\n",
               fmt, name, (unsigned long long)size);
        first = 0;
        if (data && fmt != CF_BITMAP && fmt != CF_PALETTE && fmt != CF_ENHMETAFILE &&
            fmt != CF_METAFILEPICT) {
            U8 *p = (U8 *)GlobalLock(data);
            if (p) {
                SIZE_T show = size < 80 ? size : 80;
                for (SIZE_T i = 0; i < show; i++) printf("%02x", p[i]);
                GlobalUnlock(data);
            }
        } else if (data && fmt == CF_BITMAP) {
            BITMAP bm;
            if (GetObject(data, sizeof bm, &bm))
                printf("HBITMAP %ldx%ld %ubpp", bm.bmWidth, bm.bmHeight, bm.bmBitsPixel);
        }
        printf("\"}");
    }
    printf("\n  ]},\n");
    CloseClipboard();
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc < 3) {
        fprintf(stderr, "usage: clipboard.exe <ace32.dll> <data folder> [bmp|junk|empty]\n");
        return 2;
    }
    const char *mode = argc >= 4 ? argv[3] : "bmp";
    ace = LoadLibraryA(argv[1]);
    if (!ace) {
        fprintf(stderr, "cannot load %s (err %lu)\n", argv[1], GetLastError());
        return 2;
    }
    make_bmp();
    printf("[\n");

    /* Local server only: this is the full engine, which would otherwise try the
     * remote ADS (error 6420). */
    ((U32(WINAPI *)(U16))F("AdsSetServerType"))(1 /*ADS_LOCAL_SERVER*/);

    char path[600];
    snprintf(path, sizeof path, "%s\\clip.adt", argv[2]);
    DeleteFileA(path);
    { char m[600]; snprintf(m, sizeof m, "%s\\clip.adm", argv[2]); DeleteFileA(m);
      snprintf(m, sizeof m, "%s\\clip.adi", argv[2]); DeleteFileA(m); }

    H table = 0;
    U32 rc = ((U32(WINAPI *)(H, U8 *, U8 *, U16, U16, U16, U16, U16, U8 *, U32, H *))F(
        "AdsCreateTable71"))(0, (U8 *)path, NULL, 3 /*ADT*/, 1 /*ansi*/, 1, 1, 0,
                             (U8 *)"pic,Image", 0, &table);
    last_error("AdsCreateTable71", rc);
    if (rc) { printf("  {\"fatal\": \"create failed\"}\n]\n"); return 1; }

    rc = ((U32(WINAPI *)(H))F("AdsAppendRecord"))(table);
    last_error("AdsAppendRecord", rc);
    /* AdsSetBinary(hTable, field, binaryType, totalLength, offset, buf, len) */
    if (!strcmp(mode, "empty")) {
        printf("  {\"step\": \"SetBinary\", \"mode\": \"empty-skip\"},\n");
    } else if (!strcmp(mode, "junk")) {
        U8 junk[20] = {'n', 'o', 't', 'a', 'b', 'm', 'p', 1, 2, 3, 4, 5, 6, 7, 8, 9};
        rc = ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
            table, (U8 *)"pic", 7, sizeof junk, 0, junk, sizeof junk);
        last_error("AdsSetBinary(junk)", rc);
    } else if (!strcmp(mode, "bmhdr")) {
        U8 hdr[14] = {'B', 'M'};  /* valid-length file header, empty body */
        rc = ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
            table, (U8 *)"pic", 7, sizeof hdr, 0, hdr, sizeof hdr);
        last_error("AdsSetBinary(bmhdr)", rc);
    } else if (!strcmp(mode, "bmdib")) {
        U8 d[54] = {'B', 'M'};  /* file header + zeroed 40-byte info header, bitcount 0 */
        d[14] = 40;
        rc = ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
            table, (U8 *)"pic", 7, sizeof d, 0, d, sizeof d);
        last_error("AdsSetBinary(bmdib)", rc);
    } else {
        rc = ((U32(WINAPI *)(H, U8 *, U16, U32, U32, U8 *, U32))F("AdsSetBinary"))(
            table, (U8 *)"pic", 7 /*ADS_IMAGE*/, sizeof bmp, 0, bmp, sizeof bmp);
        last_error("AdsSetBinary", rc);
    }
    rc = ((U32(WINAPI *)(H))F("AdsWriteRecord"))(table);
    last_error("AdsWriteRecord", rc);
    rc = ((U32(WINAPI *)(H))F("AdsGotoTop"))(table);
    last_error("AdsGotoTop", rc);

    rc = ((U32(WINAPI *)(H, U8 *))F("AdsImageToClipboard"))(table, (U8 *)"pic");
    last_error("AdsImageToClipboard", rc);

    dump_clipboard();

    ((U32(WINAPI *)(H))F("AdsCloseTable"))(table);
    printf("  {\"done\": true}\n]\n");
    return 0;
}
