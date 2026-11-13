# References

The external sources tflip's reimplementation draws on. tflip is a
clean-room reimplementation: it uses only the SDK headers, published
documentation, and its own black-box observation of the official engine
(see [the clean-room rule](CONTRIBUTING.md#the-clean-room-rule)). This
file is the provenance trail for that — every allowed source, and a note
on what is deliberately left out.

## Primary (the SDK, local)

These ship with the Advantage Client Engine SDK and are the authoritative
source for the API surface.

- **`ace.h`** — the published ACE API: function signatures, constants
  (`ADS_*`, `AE_*`), and field/table type numbers.
- **`aceunpub.pas`** (SDK TDataSet sources) — declares the unpublished ACE
  functions tflip also implements.
- **The TDataSet component sources** (Delphi, shipped with the SDK) — how
  a client of the API calls it: which functions, in which order, and what
  it does with their results.
- **ARC's Delphi source** (shipped with the Advantage Data Architect
  installation, in `ARC/Source/`) — how ARC, the client tflip is most
  often checked with, calls the API, and its comments on why (for
  example the `AdsSetProperty` ids it sets, which no header names).
  Published by the vendor with the program, not recovered from
  `arc32.exe`. Read for behaviour only, and confirmed by probing before
  tflip relies on it; its code is SAP's and is not copied.
- **The Advantage 12 help** (shipped with the installation) — the API
  reference, the SQL and expression references, the error codes, and the
  settings of `ads.ini`, for the version the fixtures are made with (the
  archived DevZone help below is of version 10).
- **Collation data** — `ansi.chr`, `extend.chr`, `adscollate.adt`: the
  static and dynamic collation tables, read directly.

## Vendor documentation (archived)

SAP retired Advantage; the DevZone help and `devzone.advantagedatabase.com`
now redirect to community.sap.com. The original documentation survives on
the Wayback Machine.

- **Advantage DevZone help** (dev guide, SQL engine, API reference) —
  <https://web.archive.org/web/20220322180141/http://devzone.advantagedatabase.com/dz/WebHelp/Advantage10/index.html>
- **Data security / encryption overview** — the dev guide's "Data Security"
  topic in that archived help (RC4 default, AES as a purchased FIPS add-on,
  password-derived keys). High-level only; no byte-level algorithm.
- **Advantage DevZone forum** — the developer Q&A forum, with answers from
  the Advantage engineers. Archived from 2012 to 2021; latest snapshot:
  <https://web.archive.org/web/20210508104106/http://devzone.advantagedatabase.com/forum>
  Use the engineers' answers; forum posts are not authoritative, and
  reverse-engineering discussion there is out of bounds (see below).
- **Advantage newsgroups** — the NNTP groups that preceded the forum
  (`Advantage.*` on solutions.advantagedatabase.com, 2001–2012), as
  mirrored in Sybase's forum archive (nntp-archive.sybase.com, product
  "Advantage DB"). The site is gone; the Wayback Machine holds about 400
  of its threads, one page per thread, for example
  <https://web.archive.org/web/20141027183430/http://nntp-archive.sybase.com/nntp-archive/action/article/%3C470ea886@solutions.advantagedatabase.com%3E>
  The same rules as for the forum apply.
- **SAP Community** (current home of retired Advantage content) —
  <https://community.sap.com/>

## Community (live)

- **Stack Overflow, tag `advantage-database-server`** —
  <https://stackoverflow.com/questions/tagged/advantage-database-server>
  Answers by the Advantage engineers among the rest; the same rules as for
  the forum apply.
- **Joachim Duerr's blog**, by a former Advantage engineer —
  <https://www.jd-engineering.de/category/blog/> what changed between
  versions, and how features are meant to be used.

## Third-party documentation (live)

- **Alaska Software — Advantage database engine docs** —
  <https://doc.alaska-software.com/content/ads_h2_special_adt_table_format_considerations.cxp>
  A licensed ADS consumer's documentation of the ADT/ADI/ADM formats and
  behaviour.
- **ConnectionStrings.com — Sybase Advantage** —
  <https://www.connectionstrings.com/sybase-advantage/> connection-string
  syntax.

## Client libraries

Published drivers whose source shows which part of the API they need. Read
for the calls they make; their code is not copied.

- **`adsdb`**, the Python DB API 2.0 driver (Apache 2.0) —
  <https://code.google.com/archive/p/adsdb>; loads ACE with ctypes and calls
  the DBCAPI. **`adsdb-django`** (BSD), the Django backend on it —
  <https://code.google.com/archive/p/adsdb-django>.
- **`advantage`**, the Ruby driver (gem 0.1.1) —
  <https://rubygems.org/gems/advantage>; a C extension whose `ext/dbcapi.h`
  (Apache 2.0, iAnywhere Solutions) declares the DBCAPI, the `ads_*`
  functions ACE exports. **`activerecord-advantage-adapter`** —
  <https://rubygems.org/gems/activerecord-advantage-adapter>, on top of it.

## What the project generates itself

- **`fixtures/`** — ground truth captured by probing the official engine
  (`tools/ace/*`): tables, indexes, dictionaries, SQL results, and the
  `*.system.json` dictionary dumps. This black-box output, not any
  document, is what tflip is verified against.

## Deliberately excluded

Not used, by the clean-room rule — citing them would taint the project's
provenance and breach the ACE SDK license:

- **Disassembly of the Advantage binaries**, and any documentation derived
  from it. This includes the data dictionary encryption scheme (the
  dictionary key wrapping, password secrets, permission-grant encryption):
  it is not publicly documented at the byte level, and the only available
  descriptions are disassembly-derived, so tflip leaves encrypted fields
  unreadable rather than implement it.
- **Key- or password-extraction threads** (e.g. forum posts about pulling a
  dictionary key out of a third-party executable).
