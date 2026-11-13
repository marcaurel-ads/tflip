# AdsImageToClipboard — clipboard-format probe

`AdsImageToClipboard` copies the BMP stored in an `ADS_IMAGE` field of the
current record onto the Windows clipboard. This probe learns *exactly* what the
original engine puts there, so tflip can reproduce it clean-room.

The clipboard is a local-Windows concept — Wine has no real window station — so
this must run on the native Windows VM (`LOCAL.md`), not under Wine.

## What it does

`clipboard.c` creates a one-field `ADS_IMAGE` table, stores a known 2×2 24-bpp
BMP in it (or malformed data, for the error modes), calls `AdsImageToClipboard`,
then opens the clipboard and dumps every format present — the format id, its
name, `GlobalSize`, and the first bytes. Modes: `bmp` (default), `junk` (not a
BMP), `empty` (NULL image), `bmhdr` (`BM` + a bare file header), `bmdib` (`BM` +
a zeroed info header).

## Build and run on the VM

```sh
i686-w64-mingw32-gcc -O2 -Wall -o clipboard.exe clipboard.c -lgdi32
scp -i ~/.ssh/winvm -P 2222 clipboard.exe Docker@127.0.0.1:clipboard.exe
# Run from a folder holding a local-server ace32.dll + companions (C:\acesdk
# for the original; a copied tflip ace32.dll works the same way):
ssh -i ~/.ssh/winvm -p 2222 Docker@127.0.0.1 \
  'cmd /c "cd /d C:\acesdk && C:\Users\Docker\clipboard.exe ace32.dll C:\Users\Docker\clip"'
```

## What it found (12.0 engine, native Windows)

A **valid BMP** → the engine sets one format, **`CF_DIB` (8)**: the BMP's bytes
with its 14-byte `BITMAPFILEHEADER` stripped (a packed DIB — `BITMAPINFOHEADER`,
colour table if any, pixels). Windows then **auto-synthesises** `CF_BITMAP` (2)
and `CF_DIBV5` (17) from it; the engine does not set those itself. Return 0.

Error cases (return code, with the detail `AdsGetLastError` appends):

| input | rc | message detail |
| --- | --- | --- |
| data length < 14 (incl. empty) | 5091 | `Missing BITMAPFILEHEADER.` |
| does not start with `BM` | 5091 | `Invalid bitmap type.` |
| `BM` but no room for a `BITMAPINFOHEADER` | 5001 | `Unable to allocate memory for bitmap.` |
| a non-`ADS_IMAGE` field | 5066 | `Field: "…"` / `Type must be ADS_IMAGE.` |

tflip's `crate::clipboard` reproduces all of this; the probe was re-run against
tflip's own `ace32.dll` and produced byte-identical output and identical error
codes (see `docs/KNOWN_ISSUES.md`, the clipboard entry).
