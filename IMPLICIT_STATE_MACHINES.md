# Lumen: implicit state machine audit

Audited on 8 November 2026, against commit `16dc4659c893e333f619c0245f7599ef92ad4343` on `main`, version 0.5.1.

At the audited revision, the source contained **18 runtime workflow families** whose states were encoded in flags, optional values, collection membership, channels, operating-system resources or control flow. Nested machines are grouped with their owning workflow. Smaller derived policies and algorithms, and the two packaging workflows, are recorded separately below.

**Implementation follow-up**

The four priority findings below have been fixed:

- Native hotkey dismissal and reopening cancel deferred activation and command confirmation, including a hide/show between UI frames. Activation rechecks visibility before dispatch to reject a dismissal during frame processing. Editing the query also cancels deferred activation.
- Exact index lookup honors the same replacement and deletion overlay as ordinary search, keeping learned picks from restoring deleted paths.
- Directory watches subscribe to last-write/size changes and apply modified-file events. Modified folders do not trigger the recursive scan used for newly added folders.
- Failed content reads remain eligible for the next refresh. Successful reads and deliberate exclusions remain reusable for an unchanged file.

Seven regression tests exercise these behaviors, including real Windows notifications and a temporary exclusive file lock. Each failed before its corresponding fix and passed afterward. The full test suite passes with 34 passed and one deliberately ignored profiling test, and the Windows release build passes. The inventory and findings below remain a historical snapshot; their source links point to the audited commit.

Original audit scope: all 11 Rust source files, their embedded PowerShell, existing tests, the installer and release workflow. The initial audit was static: interaction findings came from source, and existing tests were inspected without running the suite at that stage. State names below describe behavior at the audited revision; proposed names and changes are identified as recommendations.

**The interactions to address first**

1. **Hotkey dismissal retains consent and deferred actions.** `win::toggle()` calls `win::hide()` directly. Only `Lumen::hide()` clears `pending` and `armed`. The hidden branch in `logic()` closes monitor dimmers and returns, leaving those fields intact. Reopening the same query can retain an armed Restart/Shut down/Sign out command, or execute an Enter queued before dismissal. See R02, R06 and R07.
2. **Learning can bring back a deleted result.** Normal search honors deletion markers, but the learned-result fallback calls `Index::get()`, which reads the scanned index without checking those markers. A remembered path can therefore be prepended after normal search excluded it. See R13 and A02.
3. **An in-place save has no watcher transition.** The watch requests filename/directory-name notifications and parses add/remove/rename actions. It neither subscribes to last-write changes nor handles modified actions. Such a save waits for a periodic scan to refresh content. See R12.
4. **Failed content extraction becomes reusable state.** `enrich()` sets `read = true` before attempting metadata or workbook reads. `reuse_content()` treats that flag and an unchanged stamp as reusable, including an earlier transient failure. See R14.

These are separate source-derived behaviors. The additional edge cases in the inventory are design concerns to validate, rather than claims of reproduced failures.

| ID | Workflow | Current state encoding | Logical states |
| --- | --- | --- | --- |
| R01 | Process startup and relaunch | Named mutex/event, arguments, `frames`, `show_at_launch`, `pinned` | New process; primary starting; primary ready; secondary signalling; exit |
| R02 | Launcher session and monitor overlay | `VISIBLE`, `JUST_SHOWN`, `seen_focus`, native window, monitor/dimmers/backdrop | Hidden; show requested; awaiting focus; focus observed; dismissed |
| R03 | Win+Space chord | `SPACE_HELD`, Windows key state | Released; intercepted chord held |
| R04 | Asynchronous search | Query/generation keys, `searching`, result buffers, channels | Empty; requested; searching with or without preview; current answer ready |
| R05 | Result selection | `selected`, `selection_by_user`, previous row identity | Automatic selection; user selection; retained or reset selection |
| R06 | Deferred activation | `pending: Option<(query, mode)>` | None; waiting for results; execute or discard |
| R07 | Destructive-command confirmation | `armed: Option<(query, title)>` | Unarmed; armed for one query/command; execute or cancel |
| R08 | Autocomplete and caret | Text-edit cursor, selected item, `ghost_shown` | No visible suggestion; painted suggestion; accepted completion |
| R09 | Shell icon cache | Cache absence/`None`/texture, `pending_icons`, channels | Unrequested; pending; loaded; cached failure |
| R10 | Local index rebuild | `building`, `rebuild_again`, snapshots, generation | Idle; collecting apps; walking files; enriching; rerun requested |
| R11 | Mapped-drive scan, per root | `scanning_drives`, `mapped`, queue/time/count budgets | Idle; scanning; partial snapshot; completed or budget limited |
| R12 | Directory watch, per root | Directory handle, blocking read loop, buffer | Opening; waiting; applying batch; overflow recovery; stopped |
| R13 | Per-path change reconciliation | `added`, `added_at`, `shadowed`, `removed` | Scanned; overlaid; tombstoned; settled into a scan |
| R14 | Per-file content indexing | `stamp`, `read`, `content`, `detail` | Unattempted; attempted; extracted/skipped/failed; reused |
| R15 | Outlook lookup | Child process/status, reader thread, deadline, provider variables | Index lookup; COM fallback; success/empty; cancelled/timed out/failed |
| R16 | Update discovery and handoff | Last-check time, open/check channels, `available_update` | Due; checking; cooldown; offer known; handoff; stopped |
| R17 | Start-at-login preference | Registry value presence and command contents | Disabled; legacy enabled; background enabled |
| R18 | Tray icon and popup menu | Native window/icon handles, shell registration, menu lifetime | Starting; registered; shell registration lost; re-added; menu open; exiting |

**R01 — Process startup and relaunch**

Evidence: [main.rs:19](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/main.rs#L19), [win.rs:59](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L59), [app.rs:223](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L223), [app.rs:798](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L798).

The first process creates the named mutex and starts the relaunch listener. A second process signals the existing instance unless it was launched with `--background`, then exits. Ordinary primary startup requests a visible launcher; login startup remains hidden. `--show` separately schedules a delayed show and pins the window against focus-loss dismissal.

There is a nested paint-staging machine: before first paint -> frame 1 requests another paint -> frame 2 corrects eframe's automatic visibility -> normal operation. `frames` encodes this one-time protocol.

Invariant: a secondary interactive launch should reach a primary ready to receive the show request. Edge: the mutex becomes visible before the named event is created, so an early secondary signal can miss the listener. Mutex/event creation failures have no represented recovery state. Explicit startup phases would make readiness and visibility staging easier to reason about.

**R02 — Launcher session, focus and monitor resources**

Evidence: [win.rs:30](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L30), [win.rs:45](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L45), [win.rs:81](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L81), [win.rs:123](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L123), [app.rs:299](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L299), [app.rs:806](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L806), [app.rs:853](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L853).

`show()` captures the target-monitor choice, sets `VISIBLE` and publishes `JUST_SHOWN`. The UI consumes that signal, opens the display overlay, captures the backdrop, shows/focuses the native window and resets `seen_focus`. Focus loss dismisses the launcher only after focus has been observed, except in pinned mode.

The nested resource machine is Closed -> Active(monitor rectangle, zero or more dimmer windows) -> Closed. Opening first closes the previous overlay; failed screenshot capture still permits an active overlay with a plain fallback background. `Drop` also closes owned dimmers. A native hide is followed by resource cleanup when the UI observes hidden state.

The visibility/focus states are orthogonal to search, deferred activation and confirmation. The current dismissal routes have different effects:

| Dismissal | Window hidden | Monitor dimmers closed | Pending/armed cleared |
| --- | --- | --- | --- |
| Escape, backdrop click, focus loss, activation | Immediately requested | In `Lumen::hide()` | Yes |
| Win+Space while visible | Immediately requested | When hidden UI logic runs | No |

Source-derived trace: arm a command -> hotkey hide -> reopen unchanged query -> `refresh()` sees the same search query and preserves `armed` -> the next activation can execute. The equivalent deferred-Enter trace can activate a result when reopening.

Recommendation: one session-dismissal transition should cancel session-owned actions for every dismissal source. The seam between native window operations and session policy is the useful place to concentrate this behavior.

**R03 — Win+Space chord latch**

Evidence: [hook.rs:17](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/hook.rs#L17), [hook.rs:36](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/hook.rs#L36).

Released + physical Space-down while Win is held -> Held, mask the Start-menu action, invoke the callback once and swallow the event. Repeated chord key-downs stay Held and are swallowed. Physical Space-up while latched -> Released and swallowed. Injected events leave the latch unchanged; Windows supplies current Win-key state.

This is already a compact two-state machine. Its invariant is one callback per intercepted press. A missing Space-up can leave the latch set until another physical Space-up; the current code has no hook-reset transition. Hook installation/message-loop lifetime is a nested Starting -> Listening -> Stopped protocol with silent installation failure.

**R04 — Search coordination**

Evidence: [app.rs:84](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L84), [app.rs:157](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L157), [app.rs:306](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L306), [app.rs:836](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L836).

Query edit or index-generation change -> construct a `(query, generation)` key -> enqueue a search and mark a nonempty query searching. Ordinary file searches retain previous hits as a preview; empty/mail queries clear them. The worker drains queued requests to the newest one, waits 70 ms for mail, drains again, searches, applies learned ranking and sends an answer.

Different-text answers are ignored. A same-text answer can refresh the preview even when its generation differs; only an answer matching the current search key ends the wait. The generation is the request's label, not a snapshot lock held during searching. Empty queries skip the visible waiting state.

This machine has independent presentation and progress dimensions: Ready(old preview) can coexist with Searching(new request). `last_query` also serves as a render/rebuild invalidation marker, so its `None` value does not exclusively mean uninitialized.

Edges: repeated text/generation combinations have no unique request/session identifier; channel send failures are ignored and a worker failure has no UI error transition. Recommendation: give each request a monotonic identity and keep current request, accepted preview and worker outcome in one search module.

**R05 — Keyboard selection ownership**

Evidence: [app.rs:307](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L307), [app.rs:407](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L407), [app.rs:549](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L549).

New query -> Automatic(row 0). Arrow key -> UserSelected(row), clearing command confirmation. Same-query result refresh -> retain a user selection by matching title/subtitle, falling back to row 0 if the row disappears. Automatic selection resets to row 0 on refresh.

Selection ownership affects R06: a manually selected result can execute during a search, while automatic selection defers Enter. Pointer clicks execute their clicked row directly.

The important invariant is selection of the same logical item across refreshes. Title/subtitle is a presentation identity and can collide; a stable item/action identity would make that invariant explicit.

**R06 — Deferred Enter**

Evidence: [app.rs:115](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L115), [app.rs:579](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L579), [app.rs:870](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L870).

None + Enter during a non-mail search with automatic selection -> Waiting(query, mode). Once searching ends, `take()` consumes the pending activation: matching query plus nonempty results executes the currently selected row; otherwise it is discarded. `Lumen::hide()` cancels it.

The stored token identifies query text and mode, without a session, request identity or selected-item identity. Query edits leave the token stored until completion; editing away and back can make old intent match again. Arrow selection changes can also change the row ultimately executed. Hotkey hiding retains the token as described in R02.

Recommendation: specify which events cancel queued intent, and bind the remaining intent to a request and session. A suitable event set includes QueryChanged, SelectionChanged, Dismissed and SearchCompleted.

**R07 — Destructive-command confirmation**

Evidence: [app.rs:295](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L295), [app.rs:311](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L311), [app.rs:485](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L485), [app.rs:750](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L750), [system.rs:84](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/system.rs#L84).

Unarmed + activation of Restart/Shut down/Sign out -> Armed(query, title), with no system command yet. A subsequent activation of that query/title executes. This applies to row clicks as well as Enter. A different query, keyboard navigation or `Lumen::hide()` clears confirmation; activating a different confirmable command arms that command.

Confirmation has no expiry and no session identity. Same-query index refreshes preserve it. The hotkey-dismissal path also preserves it. An explicit confirmation state should attach consent to the active session and a stable command identity.

The existing system test checks which commands require confirmation; it does not exercise this two-activation lifecycle.

**R08 — Autocomplete and caret handshake**

Evidence: [app.rs:279](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L279), [app.rs:463](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L463), [app.rs:539](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L539), [app.rs:655](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L655).

The selected result determines a possible suffix or learned-name suggestion. Caret at end plus no text selection permits painting it and sets `ghost_shown`. Right Arrow accepts only when that flag and the current caret conditions permit it. Tab accepts a selected result's completion independently of whether a ghost is visible. Acceptance replaces the term while preserving parsed filter text, moves the caret and triggers search refresh.

The hidden state lives partly in egui's text-edit memory and partly in a flag from the previous draw. The invariant to check is that an accepted Right Arrow suggestion matches the currently displayed query and result. This can remain a derived view with a small explicit acceptance guard rather than a new long-lived enum.

**R09 — Shell icon loading**

Evidence: [app.rs:134](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L134), [app.rs:521](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L521), [app.rs:716](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L716), [app.rs:847](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L847), [win.rs:309](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L309).

Absent from both collections -> Unrequested. A frame with remaining budget inserts the key into `pending_icons` and sends work -> Pending. A reply removes pending membership and inserts either a texture -> Loaded, or `None` -> FailedCached. A three-request frame budget postpones additional requests.

The tri-state map plus separate pending set forms four states. Intended invariant: a key is never both cached and pending. Cached failures have no retry transition; a send failure leaves the key pending because its result is ignored. One per-key state value would make failed, retryable and outstanding requests easier to distinguish.

**R10 — Local index rebuild and rerun**

Evidence: [index.rs:176](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L176), [index.rs:203](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L203), [app.rs:209](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L209).

Idle + rebuild -> Building. A rebuild during Building sets `rebuild_again` and returns. Completion clears `building`, consumes the rerun flag and starts another build if requested. Startup, a 15-minute timer and watcher overflow request builds.

Inside a build: discover apps, falling back to Start-menu shortcuts if empty -> publish apps on an empty initial index -> walk local files -> reuse unchanged text -> publish file names -> settle older watcher changes -> enrich remaining entries up to the content budget -> publish enriched index. Each publication changes the generation. Refreshes retain the preceding snapshot during the initial app/file collection; the names snapshot is published before remaining enrichment finishes.

This is a rebuild lifecycle with a separate rerun bit, not one boolean meaning complete/incomplete data. `is_building()` also includes active drive scans. There is no Failed outcome or worker-panic recovery that resets `building`. App enumeration uses a blocking child process without the mail worker's supervision.

Recommendation: retain independent local and per-drive progress, while making local phase and queued rerun policy explicit. Keep publication and watcher reconciliation in that module.

**R11 — Mapped-drive scan, per drive**

Evidence: [index.rs:204](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L204), [index.rs:490](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L490), [index.rs:502](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L502).

Drive discovered + absent from `scanning_drives` -> insert membership and start its worker. Already scanning -> skip the duplicate. The worker publishes top-level entries, then periodic cumulative snapshots, then its final snapshot. Finishing removes membership and repaints.

Logical terminal outcomes include exhausted traversal, 20,000-entry cap, depth cap and 30-second budget. They all publish ordinary snapshots. I/O can outlast the checked deadline, as the README already explains. There is no recorded distinction between complete, partial and inaccessible.

Edges: a disconnected drive omitted from later discovery leaves its older `mapped` entry, since discovery does not prune missing roots. A worker panic can leave membership set. A rediscovered but inaccessible drive that reaches the end of `scan_mapped()` publishes an empty snapshot.

**R12 — Directory watch and overflow recovery**

Evidence: [watch.rs:16](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/watch.rs#L16), [watch.rs:25](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/watch.rs#L25), [watch.rs:39](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/watch.rs#L39), [watch.rs:87](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/watch.rs#L87).

At startup, existing nonoverlapping roots are selected once. Each worker opens a directory handle -> waits for notifications -> parses/applies a batch -> waits again. A zero-byte buffer result requests a full rebuild and resumes waiting. A failed read exits and closes the handle; open failure exits directly.

Stopped is terminal in the current process: there is no reopen/retry, nor periodic registration of roots absent at startup. Renames are deliberately modeled as independent removal and addition events, with no pairing machine.

The event mask requests `FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME`; the parser accepts add/remove/rename only. An in-place write consequently has no transition to content re-indexing. A write-notification event and a retryable watch lifecycle are distinct improvements.

**R13 — Per-path change overlay**

Evidence: [index.rs:105](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L105), [index.rs:275](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L275), [index.rs:284](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L284), [index.rs:340](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L340), [index.rs:390](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L390).

An added/updated entry becomes an overlay entry and shadows its scanned copy. Removing a path drops matching overlay entries and descendants, then records a timestamped tombstone hiding scanned descendants. Adding that exact path again removes its exact tombstone. A later full scan settles changes older than the scan's start, retaining newer changes.

There is a nested two-stage update: publish names immediately -> enrich supported files -> replace an overlay entry only if its path location and stamp still match. This guards against overwriting a newer version while extraction runs.

Invariants include equal lengths for `added` and `added_at`, matching `shadowed` membership, and consistent effective visibility across reads. `search()` consults tombstones; `get()` does not. The learned-ranking fallback can therefore resurrect a deleted scanned path.

Recommendation: expose one effective-entry lookup policy to both search and exact-path lookup, and colocate per-entry metadata instead of maintaining parallel vectors.

**R14 — Content extraction and reuse**

Evidence: [index.rs:30](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L30), [index.rs:255](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L255), [index.rs:663](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L663), [index.rs:903](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L903).

New scanned entry -> `read = false`. Entering `enrich()` immediately sets it true. Extraction may yield text, skip a noneligible/oversized/cloud file, or fail. All those outcomes share `read = true`. On refresh, an old read entry with the same size/write-time/attributes stamp supplies its content/detail and marks the new entry read. A changed stamp causes another attempt; entries left beyond the aggregate budget stay unread.

The boolean therefore means Attempted, although its field comment describes content freshness. A transient sharing or parsing failure can be retained across unchanged scans and never retried during that process.

Recommendation: distinguish Unattempted, Indexed(stamp), Skipped(reason, stamp) and RetryableFailure. Content freshness and retry eligibility are the important dimensions. Existing tests exercise extraction and reuse after stamp changes, with no transient-failure recovery sequence.

**R15 — Outlook lookup and process supervision**

Evidence: [app.rs:169](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L169), [mail.rs:16](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/mail.rs#L16), [mail.rs:21](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/mail.rs#L21), [mail.rs:79](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/mail.rs#L79).

There are two nested machines:

- Provider selection: query Windows Search -> keep its items; if the item list is empty -> attach to running classic Outlook -> bounded Inbox/Sent scan -> hits/empty/unavailable. An empty list can follow an empty result or an index failure. `$indexed` and `$items` jointly encode provider success; an earlier successful-but-empty index query can still yield a successful empty outcome if COM fails.
- Process supervision: cancellation check -> spawn child and stdout reader -> poll -> collect/parse success, or kill/wait/join on cancellation, seven-second timeout or polling error. COM scanning has its own two-second and result/item budgets.

Cancellation compares the worker's query text to current query text. Cancellation, timeout, child failure and invalid output all become `Err(())`; the UI supplies one unavailable note for those errors when their query is still relevant. Browser search is a separate user-activated action, rather than an automatic provider fallback.

The deadline is checked while the child is running; reader joins and cleanup are blocking operations outside a separately enforced deadline. Separate outcome variants and request identity would make cancellation and provider availability easier to validate.

**R16 — Update polling, offer and handoff**

Evidence: [update.rs:22](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/update.rs#L22), [update.rs:47](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/update.rs#L47), [app.rs:689](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L689), [app.rs:832](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L832).

Scheduling: NeverChecked -> check immediately -> Cooldown(last attempt). A launcher-open message permits another check after an hour; a six-hour receive timeout also wakes the loop. Disconnection stops it. The attempt time is stored before checking, so failure also enters cooldown.

Presentation: Unknown -> KnownAvailable(update) on a positive channel message. No current/unavailable/failure message clears or updates the offer.

Handoff: click offer -> hide launcher and spawn recheck -> choose newly returned update or known fallback -> open installer URL/release page in the browser. The app tracks browser handoff, with no internal download/install lifecycle.

Recommendation: model CheckOutcome(Current, Available, Failed) separately from scheduler and remembered offer. Existing tests cover version/asset eligibility, rather than timing or offer transitions.

**R17 — Start-at-login preference and migration**

Evidence: [win.rs:434](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L434), [app.rs:369](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L369), [app.rs:510](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L510), [tray.rs:111](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L111).

Missing registry value -> Disabled. Present command without `--background` -> LegacyEnabled. Startup migration appends that argument -> BackgroundEnabled. The tray or search command toggles between disabled and the current executable's background command; uninstall removes the value.

Registry reads are the source of truth; there is no independent persistent UI flag. Read errors look like disabled, and write/delete errors are discarded, so attempted and successful transitions have no distinct outcomes. Preserve registry truth while returning an observable write result.

**R18 — Tray icon and popup menu**

Evidence: [tray.rs:30](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L30), [tray.rs:35](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L35), [tray.rs:58](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L58), [tray.rs:73](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L73), [tray.rs:93](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/tray.rs#L93).

Worker startup registers a hidden top-level window, creates/caches an icon and asks the shell to add it. Explorer restart loses shell registration; `TaskbarCreated` invokes add again with the cached native icon. Quit requests shell removal and exits the process immediately.

The nested menu machine is Absent -> Created/foreground -> waiting for choice -> destroyed -> dispatch Open, toggle startup, Quit or cancellation. Its state lives on the native call stack.

Icon existence, listener-window existence and shell registration are different dimensions. The code stores the first two and discards registration results, so a nonzero icon handle does not establish a registered tray icon. Existing tests cover icon pixels only.

**Smaller stateful policies and algorithms**

These complete the inventory without treating every data classification as an application lifecycle.

| ID | Policy/protocol | State and transitions | Evidence and implication |
| --- | --- | --- | --- |
| A01 | Query scope encoded in text | All/app/spreadsheet/mail scope and extension constraints derive from prefixes/tokens; filter clicks rewrite the query | [index.rs:358](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L358), [index.rs:473](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L473), [mail.rs:7](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/mail.rs#L7), [app.rs:667](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L667). Parsing is case-insensitive and accepts aliases, while chip highlighting/removal recognizes fewer literal prefixes. For example, `sheets budget` searches spreadsheets while showing All active; `Mail invoice` can show no chip active. Derive chip state and scope changes from one parsed query model. |
| A02 | Learned result and use counts | A normalized query/prefix goes from unbound to bound-to-path and is overwritten by later choices; count increments after activation; JSON load/write persists it | [win.rs:353](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L353), [win.rs:392](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L392), [app.rs:193](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L193), [app.rs:513](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L513). A learned path can bypass ordinary matching. Its lookup needs R13's deletion policy. Memory and the two JSON files can diverge when writes fail; those failures are discarded. |
| A03 | External action handoff | Select -> optional R07 confirmation -> hide -> dispatch open/reveal/admin/copy/system/startup/quit -> learn/invalidate for returning paths | [app.rs:485](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L485), [win.rs:289](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L289), [mail.rs:142](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/mail.rs#L142), [system.rs:112](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/system.rs#L112). There is no ongoing success/failure/retry state. History learning follows attempted dispatch, including a failed shell open; reveal also learns. Empty Recycle Bin delegates confirmation to Windows. |
| A04 | One-time initialization and fallback selection | Context/callback registration is unset -> initialized; dimmer class is unattempted -> cached registration success/failure; app/font discovery falls through alternate sources | [app.rs:127](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L127), [hook.rs:18](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/hook.rs#L18), [win.rs:185](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/win.rs#L185), [index.rs:239](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L239), [app.rs:933](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/app.rs#L933). These are small initialization protocols; cached dimmer-class failure is terminal. The icon worker additionally initializes COM and uninitializes it after its receive loop when initialization succeeded. |
| A05 | Calculator and query parsers | Calculator cursor and `work` evolve while grammar calls track precedence/nesting; filter parsing changes from first token to subsequent tokens; initials/edit matching track positions | [calc.rs:3](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/calc.rs#L3), [calc.rs:40](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/calc.rs#L40), [index.rs:473](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L473), [index.rs:632](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L632), [index.rs:748](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L748). These are local algorithmic state. The recursive calculator is stack-based, so a flat finite-state lifecycle would be a poor representation. |
| A06 | Bounded builders and traversals | Directory queues go from pending to visited/exhausted/capped; word accumulation goes from accepting to full; ranking heaps fill and replace weakest entries | [index.rs:422](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L422), [index.rs:722](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L722), [index.rs:794](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L794), [index.rs:843](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/src/index.rs#L843). These are bounded algorithms inside R10/R11/R14. Completion reasons and partial results matter to the owning workflows; a separate persistent enum for each loop adds little value. |

`Action`, `Mode`, `Kind` and `system::Target` are explicit classifications of operations/data. They supply events or payloads to these workflows; their variants alone do not describe temporal lifecycles.

**Packaging workflows**

| Workflow | Implicit transitions | Evidence |
| --- | --- | --- |
| Installer, upgrade and uninstall | Stop running process -> wait -> replace executable/create shortcut -> optionally launch; uninstall stops process -> removes files/shortcut/login registration | [lumen.iss:32](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/installer/lumen.iss#L32). The stop result is ignored and a fixed 500 ms wait stands in for verified process termination. |
| Release publication | Tag checkout -> test/build -> verify version and build installer -> installation/upgrade/uninstall checks -> optional signing -> publish versioned and compatibility assets | [release.yml:16](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/.github/workflows/release.yml#L16), [release.yml:49](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/.github/workflows/release.yml#L49), [release.yml:83](https://github.com/seanmcmahon101/lumen/blob/16dc4659c893e333f619c0245f7599ef92ad4343/.github/workflows/release.yml#L83). The step graph is explicit; signing mode is implicit in secret availability and signed-file presence. CI checks the install lifecycle using a long-lived stand-in executable. |

**Where making state explicit would help most**

Start with a launcher-session module owning dismissal, queued intent and confirmation, and a search module owning request identity and preview acceptance. Keep native window effects behind those transitions. This concentrates the behavior currently spread across the hook, window helpers and UI callbacks.

For indexing, keep independent local-rebuild, per-drive, per-path and per-file machines. Their lifetimes overlap, so one application-wide state enum would create combinations without improving locality. Unify effective entry lookup first; then distinguish successful content indexing from retryable failure.

Simple two-state latches, derived caret/scope views and local parser loops can retain compact representations. Explicit transitions and invariants matter more than adopting a state-machine library.

**Verification opportunities**

The existing tests cover ranking/filter parsing, workbook extraction/reuse, sequential watcher-overlay changes, mapped-drive publication order, mail-scope parsing, history prefix keys, calculator behavior, release eligibility and tray icon rendering. The release workflow also checks installation, upgrade and uninstall. There are no direct runtime lifecycle tests in `app.rs`, `hook.rs` or `watch.rs`.

The highest-value behavioral checks for follow-up work are:

1. Arm a mock destructive command, dismiss through each route, reopen and require fresh confirmation.
2. Queue Enter, then dismiss or edit away and back before completion; verify cancelled intent never executes.
3. Learn a file, tombstone it, then search its learned query before a full scan; verify it stays absent.
4. Save a watched file in place and verify its changed text becomes searchable.
5. Make extraction fail transiently without changing its stamp, recover access and verify retry.
6. Refresh results while navigating and verify selected identity, preview freshness and pending activation agree.
7. Interleave rebuild requests, watcher changes, overflow and drive scans; verify one active local build, retained newer deltas and eventual rerun.

These are proposed checks, not tests added or executed by this audit.
