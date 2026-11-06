# Lumen

A Windows launcher opened with **Win+Space**. Type to find applications, files,
spreadsheets and Outlook mail. Enter opens, Ctrl+Enter reveals a local file,
Tab completes, and Escape closes. Click the search filters to switch scope.

## Install on Windows

Download [Lumen-Setup-x64.exe](https://github.com/seanmcmahon101/lumen/releases/latest/download/Lumen-Setup-x64.exe)
and run it on Windows 10 or 11. Setup installs Lumen into your own Windows
profile and adds a Start menu shortcut. It requires no administrator rights.
Start Lumen and press **Win+Space** to search. To have it open at login, search
for **Start Lumen at login** in Lumen and press Enter.

On multiple monitors, Lumen opens in the centre of the active app's display and
dims the others until you close it. When the desktop has focus, Lumen uses the
display under the pointer.

Lumen checks GitHub for a newer installer at startup and every six hours. When
**Update available** appears, click it to open the release page and download
the installer.

For an update or uninstall, first search for **Quit Lumen** and press Enter.
Run the new installer to update, or remove Lumen from **Settings → Apps**.

## Search examples

- `excel budget` — spreadsheets only, including common misspellings.
- `excel finance revenue` — match workbook names, parent folders, worksheet names and cell text together.
- `ext:xlsm forecast` — a specific file extension.
- `mail invoice` — Outlook messages matching all words.
- `type:app code` — applications only.

Search runs on a debounced background worker. Obsolete mail searches are cancelled;
old responses cannot replace newer queries. Applications appear before the file
scan completes. Files become searchable before workbook content finishes indexing.

## Excel coverage

The profile, Documents, Desktop, Downloads and configured OneDrive locations are
scanned, excluding build caches and hidden/system files. Mapped network drives are
indexed separately, with top-level folders published first. Each drive has a
20,000-entry, five-level, 30-second traversal budget (an in-flight Windows network
read can exceed that budget without blocking the UI). Network files are indexed
by name and path only. Exact folder matches outrank incidental file prefixes. Indexing refreshes every
15 minutes, with a 250,000-entry and 20-level traversal limit. Cloud-only files can
match by filename; content indexing does not download them.

XLSX, XLS, XLSM, XLSB and ODS contents are read with Calamine; CSV/TSV text is also
indexed. Limits: 16 MB source file, first 32 worksheets, roughly 128 KB indexed text
per file, 512 characters per cell and 32 MB total indexed content. Password-protected,
unreadable or oversized files remain searchable by name and folder. Formulas are
not evaluated. Search opens the workbook, not a specific cell. Saved EML text is
searchable; encoded MIME bodies/attachments are not decoded. MSG is filename-only.

## Outlook

1. Query the existing **Windows Search** mail index (subjects, senders and indexed
   body text). Lumen does not create a second mailbox index or store credentials.
2. If that returns no results or fails, attach to an already-running **classic
   Outlook** through COM. Search up to 150 recent messages per Inbox/Sent folder
   across stores, with a two-second scan budget and six-result limit. This is a
   deliberately limited fallback, not a full-mailbox search.
3. **Search Outlook online** opens the query in Microsoft 365 in your browser.

The mail subprocess has a seven-second total deadline and cancels when the query
changes. COM never launches Outlook or configures an account. New Outlook does not
provide classic Outlook COM; local results depend on what Windows has indexed.
The online action uses your browser's Microsoft 365 account and searches only when
you activate it. Inbox contents are not sent to an external search service.

## Build and test

With Rust and a Windows linker installed:

```powershell
$env:PATH = 'C:\msys64\mingw64\bin;' + $env:PATH # for the GNU Rust toolchain
cargo test
cargo build --release
.\target\release\lumen.exe --show "excel budget"
```

Tests cover ranking, filters, spelling tolerance, folder/content matching,
workbook extraction, mail scope parsing and calculator behavior.
