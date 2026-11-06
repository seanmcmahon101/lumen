# Lumen

A Windows launcher opened with **Win+Space**. Type to find applications, files,
spreadsheets and Outlook mail. Enter opens, Ctrl+Enter reveals a local file,
Tab completes, and Escape closes. Click the search filters to switch scope.
Lumen remembers what you open for each search, so typing the same letters again
puts that result first.

## Install on Windows

Download [Lumen-Setup-x64.exe](https://github.com/seanmcmahon101/lumen/releases/latest/download/Lumen-Setup-x64.exe)
and run it on Windows 10 or 11. Setup installs Lumen into your own Windows
profile and adds a Start menu shortcut. It requires no administrator rights.
Start Lumen from the Start menu to open the search bar; after that, press
**Win+Space**. Starting Lumen again while it runs opens the search bar too.
Lumen keeps an icon in the notification area: click it to search, or right-click
it for **Start at login** and **Quit Lumen**. You can also search for
**Start Lumen at login** in Lumen and press Enter. At login Lumen starts hidden
and waits for Win+Space.

On multiple monitors, Lumen opens in the centre of the active app's display and
dims the others until you close it. When the desktop has focus, Lumen uses the
display under the pointer.

Lumen checks GitHub for a newer installer at startup, every six hours, and when
you open it if the last check was over an hour ago. When
**Update available** appears, click it to download the installer for that
version, for example `Lumen-Setup-0.4.3-x64.exe`.

Run the new installer to update, or remove Lumen from **Settings → Apps**. Both
close a running Lumen for you.

## Search examples

- `excel budget` — spreadsheets only, including common misspellings.
- `excel finance revenue` — match workbook names, parent folders, worksheet names and cell text together.
- `ext:xlsm forecast` — a specific file extension.
- `mail invoice` — Outlook messages matching all words.
- `type:app code` — applications only.
- `bluetooth`, `dark mode`, `env vars` — open a Windows Settings page or tool.
- `lock`, `restart`, `shut down`, `sign out`, `empty recycle bin` — system commands.
  Restart, Shut down and Sign out ask for a second Enter first.

Settings and commands named exactly appear above files; partial names such as
`blue` only fill rows that files leave empty.

Source code and scripts (`.c`, `.go`, `.py`, `.bat`, `.ps1`…), and anything inside
package caches, SDKs, vendored code or unpacked versioned releases (`go\pkg\mod`,
`dotnet\sdk\10.0.203`, `Python-3.14.8`), rank below documents. They still appear
when nothing better matches, or when typed by exact name.

Each keystroke searches the whole index at once, split across every CPU core; on
a profile of about 115,000 files and 32 MB of workbook text, a search takes 2–6 ms.
The previous results stay on screen until the new ones arrive, and Enter pressed
before they arrive opens the top new result. Mail searches wait for typing to pause.
Obsolete mail searches are cancelled; old responses cannot replace newer queries.
Applications appear before the file scan completes. Files become searchable before
workbook content finishes indexing.

## Excel coverage

The profile, Documents, Desktop, Downloads and configured OneDrive locations are
scanned, excluding build caches and hidden/system files. Mapped network drives are
indexed separately, with top-level folders published first. Each drive has a
20,000-entry, five-level, 30-second traversal budget (an in-flight Windows network
read can exceed that budget without blocking the UI). Network files are indexed
by name and path only. Exact folder matches outrank incidental file prefixes. Indexing refreshes every
15 minutes, with a 250,000-entry and 20-level traversal limit. Cloud-only files can
match by filename; content indexing does not download them.

Documents, Desktop, Downloads and OneDrive are watched: files created, renamed,
moved or deleted there show up in search, or leave it, within about a second.
A saved workbook's text is read again straight away. During the 15-minute refresh,
search keeps using the previous index, and text from workbooks whose size, date
and attributes have not changed is reused rather than read again.

XLSX, XLS, XLSM, XLSB and ODS contents are read with Calamine; CSV/TSV text is also
indexed. Each distinct word is kept once per file, so repeated labels and values
do not use up the limits. Limits: 16 MB source file, first 32 worksheets, 128 KB of
distinct words per file, 512 characters per cell and 32 MB total indexed content. Password-protected,
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

To time searches over your own profile:

```powershell
cargo test --release -- --ignored --nocapture search_speed
```
