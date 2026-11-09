# Lumen

<img width="780" height="624" alt="image" src="https://github.com/user-attachments/assets/640bc96f-ce8e-4741-85e5-7a31022f20c6" />


A Windows launcher opened with **Win+Space**. Type to find applications, files,
spreadsheets and Outlook mail. Enter opens, Ctrl+Enter reveals a local file,
Tab completes, and Escape closes. Click the search filters to switch scope.
Lumen remembers what you open for each search, so typing the same letters again
puts that result first.

Type **/** to browse commands, or **/help** (also **F1**) for searchable help,
examples and keyboard shortcuts. Enter chooses a command; Tab completes it.
Escape returns from a command to your previous search, then closes Lumen.

## Commands and shortcuts

| Command | Use |
| --- | --- |
| `/help [topic]` | Find commands and examples, such as `/help excel` or `/help keyboard`. |
| `/pins [filter]` | Open pinned apps, files and folders. Pins also appear before typing. |
| `/pin`, `/unpin` | Pin or unpin the result selected before you started the command. `/pin C:\Work` also accepts a full path. **Ctrl+D** toggles a result's pin. |
| `/recent [filter]` | Items opened through Lumen, newest first. Up to 30 are retained; five unpinned recents appear on the empty search bar. |
| `/alias name [path or URL]` | Give the selected item an explicit shortcut. Select your invoices folder, type `/alias inv`, and press Enter. Typing `inv` in ordinary search then puts it first. A full path or URL can also be supplied. |
| `/aliases`, `/unalias name` | Browse shortcuts or remove one. Removing a shortcut leaves the original item in place. |
| `/windows [title]` | Find an open desktop window by title and switch to it, including minimized windows. The list refreshes when Lumen opens. |
| `/actions [search]` | Actions for the selected result, or the first match for a supplied search. **Ctrl+K** opens the same list. Open, reveal, copy path, run as administrator, pin, create an alias, preview, and open a terminal appear when applicable. |
| `/preview [search]` | Preview a selected spreadsheet, or find one with `/preview excel budget`. **Ctrl+P** previews the selected result. Shows up to eight excerpts with real worksheet and cell references; Enter on an excerpt copies its text. |
| `/calc expression` | Calculate and copy the answer, for example `/calc 125 * 1.2`. Ordinary arithmetic also works without a command. |
| `/convert value unit in unit` | Offline conversion, for example `/convert 10 miles in km`, `/convert 2 GiB in MiB`, or `/convert 32 F in C`. Also works without the command. |
| `/date [calculation]` | Today's local date, `today + 30 days`, `today + 2 weeks`, or `2026-12-25 - today`. Use `YYYY-MM-DD` for dates; Enter copies the answer. Date calculations also work in ordinary search. |
| `/settings` | Change the theme, the opening shortcut, search sources, profile coverage, additional folders and exclusions. The theme previews as you choose it. Save applies changes and refreshes search in the background when search settings changed. |
| `/index [full folder path]` | Add a folder to search, for example `/index D:\Projects`, or select a folder result and type `/index`. Lumen indexes it in the background and watches it for changes; remove folders in `/settings`. |
| `/reindex`, `/status` | Refresh the index or inspect its progress, approximate size, folders and enabled sources. |
| `/perf` | View recent and p95 opening, local search and result-display timings, working-set memory, and CPU since the previous `/perf` snapshot. Leave Lumen idle between snapshots to inspect idle CPU. |

Explicit aliases take priority over learned ranking. Pins (up to 64), aliases
(up to 128), recents and preferences are saved locally; writes run off the UI
thread. Available opening shortcuts are Win+Space, Alt+Space, Ctrl+Space and
Ctrl+Alt+Space. The default is Win+Space.

Conversions cover length (`m`, `km`, `cm`, `mm`, `mi`, `ft`, `in`, `yd`), area
(`m2`, `km2`, `ft2`, `acre`, `ha`), mass (`kg`, `g`, `mg`, `lb`, `oz`, `st`),
volume (`l`, `ml`, `ukgal`, `usgal`), time (`ms`, `s`, `min`, `h`, `d`, `weeks`),
data and temperature (`C`, `F`, `K`). Data units use bytes: KB/MB/GB are decimal;
KiB/MiB/GiB are binary. Currency conversion requires live rates and is not included.

Previews are read only when requested, on a background worker. They share the
16 MB source-file and cloud-placeholder exclusions of content indexing, skip
network files, and inspect at most 32 worksheets / 100,000 cells with a two-second
cell-scan budget. The preview opens the workbook rather than navigating to a cell.

## Startup and performance

A versioned index snapshot is saved under `%LOCALAPPDATA%\Lumen`. At startup,
Lumen loads it in the background and makes it searchable while reconciling current
files. A missing, corrupt, incompatible or differently configured snapshot falls
back to a fresh scan. Cached results may briefly include files changed while Lumen
was closed. Unchanged workbook text is reused. Network drives refresh separately.

Outlook searches use an isolated **native COM helper**, querying the existing
Windows Search index and attaching to running classic Outlook as a bounded
fallback. Searching does not start PowerShell. A cancelled or timed-out helper is
terminated without blocking the launcher; the seven-second deadline still applies.

`/perf` retains at most 128 samples per timing. Opening measures the request to the
end of the next UI draw; result display measures query dispatch to a draw with its
results ready. These are application timings, not physical display latency. CPU is
reported as a percentage of one core over the interval between `/perf` snapshots.
There is no diagnostic polling while idle. Performance targets should be checked
on the intended Windows hardware; earlier search figures below are prior measurements.

On 8 November 2026, a release benchmark over 103,148 items and 32 MB of content
returned all results in about 1.7–3.6 ms for the tested multi-letter queries; a
broad `b` query took 23.6 ms. A 69.6 MB snapshot decoded in memory in 103 ms,
compared with a 3.42-second fresh file walk plus 322 ms of content extraction.
Snapshot decode excludes disk I/O and these figures are specific to this PC.

All matching indexed applications, files and spreadsheets are available in a
scrollable list. Use the mouse wheel, a trackpad or the scrollbar to browse;
the search bar and shortcuts stay in place. Up/Down moves the selection,
Page Up/Page Down moves a page, and Ctrl+Home/Ctrl+End jumps to the first/last
result. The list follows keyboard selection and starts at the top for a new
search. When results overflow, a counter shows the visible range and total. Up
to eight rows are visible at once, fewer on a short display; only visible rows
are drawn. The selected row shows what Enter will do.

Lumen follows Windows' light or dark app mode and accent color, and uses
Segoe UI Variable on Windows 11 (Segoe UI on Windows 10). Choose Light or Dark
in `/settings` to override the mode. Opening dims the desktop over 0.2 seconds
unless Windows animation effects are turned off.

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

Windows code signing is being set up through the SignPath Foundation; current
downloads are unsigned. See the [Code signing policy](SIGNING.md) and
[Privacy policy](PRIVACY.md). A trusted signature identifies the publisher,
but new releases can still show SmartScreen warnings while reputation builds.

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
`blue` appear after matching files.

Source code and scripts (`.c`, `.go`, `.py`, `.bat`, `.ps1`…) rank below documents.
They still appear when nothing better matches, or when typed by exact name.

Package caches, SDKs, vendored code and unpacked versioned releases (`go\pkg\mod`,
`sdk`, `vendor`, `third_party`, `testdata`, `externals`, and folders such as
`libc@v1.55.3`, `Python-3.14.8` or `10.0.203`) are listed by name but not scanned
inside. On a developer's PC they were most of the index; skipping them made the
index about a fifth of its size.

Each keystroke searches the whole index at once, split across every CPU core; on
a profile of about 115,000 files and 32 MB of workbook text, a search takes 2–6 ms.
The previous results stay on screen until the new ones arrive, and Enter pressed
before they arrive opens the top new result. Closing Lumen or changing the query
cancels that queued Enter; closing also resets power-command confirmation.
Mail searches wait for typing to pause.
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
A saved workbook's text is read again straight away, including when a save edits
the existing file in place. During the 15-minute refresh,
search keeps using the previous index, and text from workbooks whose size, date
and attributes have not changed is reused rather than read again. Temporary read
failures are retried on later refreshes.

XLSX, XLS, XLSM, XLSB and ODS contents are read with Calamine; CSV/TSV text is also
indexed. Each distinct word is kept once per file, so repeated labels and values
do not use up the limits. Limits: 16 MB source file, first 32 worksheets, 128 KB of
distinct words per file, 512 characters per cell and 32 MB total indexed content. Password-protected,
unreadable or oversized files remain searchable by name and folder. Formulas are
not evaluated. Search opens the workbook, not a specific cell. Saved EML text is
searchable; encoded MIME bodies/attachments are not decoded. MSG is filename-only.

## Outlook

1. Query the existing **Windows Search** mail index (subjects, senders and indexed
   body text), returning up to 1,000 matches, newest first, in the scrollable list.
   At that limit, Lumen asks you to narrow the search. Lumen does not create a
   second mailbox index or store credentials.
2. If that returns no results or fails, attach to an already-running **classic
   Outlook** through COM. Search up to 150 recent messages per Inbox/Sent folder
   across stores, with a two-second scan budget. This is a
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

For a separate development preview alongside an installed copy, use
`lumen.exe --isolated --show "/help"`. This mode skips the global hotkey,
notification-area icon, single-instance signalling and startup migration.
Set `LUMEN_DATA_DIR` to a test folder to keep preferences, shortcuts and the cache
separate from your normal Lumen data.

Tests cover ranking, filters, spelling tolerance, folder/content matching,
workbook extraction, mail scope parsing and calculator behavior.

To time searches over your own profile:

```powershell
cargo test --release -- --ignored --nocapture search_speed
```

## License

Lumen is released under the [MIT license](LICENSE).
