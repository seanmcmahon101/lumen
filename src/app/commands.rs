use super::*;
use crate::command::{self, COMMANDS};
use crate::library::Saved;

impl Lumen {
    pub(super) fn accept_preview(
        &mut self,
        request: crate::preview::Request,
        lines: Result<Vec<crate::preview::Line>, String>,
    ) {
        if self.preview_key.as_ref() == Some(&request) {
            self.preview_lines = Some(lines);
            if command::parse(&self.query).is_some_and(|(name, _)| name == "preview") {
                self.last_query = None;
            }
        }
    }
    pub(super) fn saved_item(&self, saved: &Saved) -> Item {
        // Pins must be available before indexing and must not scan 250,000 entries on the UI thread.
        self.entry_item(&saved.entry())
    }

    pub(super) fn navigate(&mut self, query: String) {
        if query.trim().eq_ignore_ascii_case("/perf") {
            self.perf_rows = crate::metrics::snapshot();
        }
        if !self.query.trim_start().starts_with('/') {
            self.return_query = self.query.clone();
            self.preview_terms = self.query.clone();
            self.command_target = self
                .results
                .get(self.selected)
                .filter(|i| i.saved.is_some())
                .cloned();
        }
        self.query = query;
        self.cancel_actions();
        self.refresh();
        self.selected = 0;
        self.selection_by_user = false;
        if let Some(ctx) = win::CTX.get() {
            self.set_cursor(ctx, false);
            ctx.memory_mut(|m| m.request_focus(Self::query_id()));
        }
    }

    pub(super) fn home_results(&mut self) {
        for (i, saved) in self.library.pins.iter().enumerate() {
            let mut item = self.saved_item(saved);
            item.header = (i == 0).then_some("Pinned");
            self.results.push(item);
        }
        let recent: Vec<Saved> = self
            .library
            .recent
            .iter()
            .filter(|s| !self.library.pinned(&s.path))
            .take(5)
            .cloned()
            .collect();
        for (i, saved) in recent.iter().enumerate() {
            let mut item = self.saved_item(saved);
            item.header = (i == 0).then_some("Recent");
            self.results.push(item);
        }
        if self.results.is_empty() {
            self.results.push(simple(
                "Welcome to Lumen",
                "Type to search, or open /help for commands and examples",
                "Help",
                G_HELP,
                Action::Query("/help".into()),
            ));
        }
    }

    fn command_item(command: &command::Command) -> Item {
        let query = format!(
            "/{}{}",
            command.name,
            if command.usage.contains(' ')
                && !matches!(
                    command.name,
                    "help"
                        | "pins"
                        | "recent"
                        | "aliases"
                        | "windows"
                        | "actions"
                        | "preview"
                        | "date"
                )
            {
                " "
            } else {
                ""
            }
        );
        let mut item = simple(
            command.usage,
            command.description,
            "",
            command_glyph(command.name).unwrap_or(G_SEARCH),
            Action::Query(query.clone()),
        );
        item.complete = Some(query);
        item
    }

    fn info(&mut self, title: &str, detail: &str) {
        self.results
            .push(simple(title, detail, "", G_INFO, Action::Noop));
    }

    /// Starts a group: the next row pushed gets `header` above it.
    fn group(&mut self, start: usize, header: &'static str) {
        if let Some(item) = self.results.get_mut(start) {
            item.header = Some(header);
        }
    }

    fn target(&self, argument: &str) -> Option<Item> {
        if !argument.is_empty() {
            return (!self.searching)
                .then(|| self.found.first().map(|e| self.entry_item(e)))
                .flatten();
        }
        self.command_target.clone()
    }

    pub(super) fn slash_results(&mut self, query: &str) -> bool {
        let Some((name, argument)) = command::parse(query) else {
            return false;
        };
        let known = COMMANDS.iter().any(|c| c.name == name);
        if matches!(name.as_str(), "settings" | "reindex" | "status" | "perf")
            && !argument.is_empty()
        {
            self.results.push(simple(
                "This command takes no arguments",
                &format!("Use /{name}"),
                "Command",
                G_SEARCH,
                Action::Query(format!("/{name}")),
            ));
            return true;
        }
        if !known {
            for command in COMMANDS.iter().filter(|c| c.name.starts_with(&name)) {
                self.results.push(Self::command_item(command));
            }
            if self.results.is_empty() {
                self.results.push(simple(
                    "Unknown command",
                    "Open /help to see available commands",
                    "Help",
                    G_SEARCH,
                    Action::Query("/help".into()),
                ));
            }
            return true;
        }
        match name.as_str() {
            "help" => {
                let hotkey = crate::settings::get().hotkey.label();
                let shortcuts = [
                    (
                        "Search apps, files, and folders",
                        "Type a name; Lumen learns which result you open",
                        "budget",
                        G_SEARCH,
                    ),
                    (
                        "Search spreadsheets and their contents",
                        "excel finance revenue · ext:xlsm forecast",
                        "excel budget",
                        G_SHEET,
                    ),
                    (
                        "Search Outlook mail",
                        "mail invoice · subjects, senders, and indexed text",
                        "mail invoice",
                        G_MAIL,
                    ),
                    (
                        "Keyboard shortcuts",
                        "Enter open · Tab complete · Esc back/close · ↑↓ select",
                        "",
                        G_KEYBOARD,
                    ),
                    (
                        "Result actions",
                        "Ctrl+K actions · Ctrl+P preview · Ctrl+D pin/unpin",
                        "/actions",
                        G_MORE,
                    ),
                    (
                        "Open, reveal, or run as administrator",
                        "Ctrl+Enter reveal · Ctrl+Shift+Enter administrator · Ctrl+C copy",
                        "",
                        G_OPEN,
                    ),
                    (
                        "Browse long result lists",
                        "Page Up/Down · Ctrl+Home/End · mouse wheel or scrollbar",
                        "",
                        G_LIST,
                    ),
                    ("Open Lumen", hotkey, "", G_KEYBOARD),
                ];
                for (title, detail, example, glyph) in shortcuts {
                    if !command::matches(&format!("{title} {detail}"), argument) {
                        continue;
                    }
                    self.results.push(simple(
                        title,
                        detail,
                        "",
                        glyph,
                        if example.is_empty() {
                            Action::Noop
                        } else {
                            Action::Query(example.into())
                        },
                    ));
                }
                self.group(0, "Basics");
                let commands = self.results.len();
                for command in COMMANDS.iter().filter(|c| {
                    command::matches(
                        &format!("{} {} {}", c.name, c.description, c.example),
                        argument,
                    )
                }) {
                    self.results.push(Self::command_item(command));
                }
                self.group(commands, "Commands");
                if self.results.is_empty() {
                    self.info(
                        "No matching help",
                        "Try /help excel, /help pin, or /help keyboard.",
                    );
                }
            }
            "pins" | "recent" => {
                let entries = if name == "pins" {
                    &self.library.pins
                } else {
                    &self.library.recent
                };
                for saved in entries
                    .iter()
                    .filter(|s| command::matches(&format!("{} {}", s.name, s.path), argument))
                {
                    let item = self.saved_item(saved);
                    self.results.push(item);
                }
                if self.results.is_empty() {
                    self.info(
                        if name == "pins" {
                            "No pinned items"
                        } else {
                            "No recent items"
                        },
                        if name == "pins" {
                            "Search for an item, then press Ctrl+D or use /pin."
                        } else {
                            "Items you open through Lumen will appear here."
                        },
                    );
                }
            }
            "aliases" => {
                for (alias, saved) in self.library.aliases.iter().filter(|(a, s)| {
                    command::matches(&format!("{a} {} {}", s.name, s.path), argument)
                }) {
                    let mut item = self.saved_item(saved);
                    item.title = format!("{alias} → {}", saved.name);
                    item.label = "Alias".into();
                    self.results.push(item);
                }
                if self.results.is_empty() {
                    self.info(
                        "No matching aliases",
                        "Select an item, type /alias followed by a short name, then press Enter.",
                    );
                }
            }
            "pin" | "unpin" => {
                let target = if argument.is_empty() {
                    self.command_target.as_ref().and_then(|i| i.saved.clone())
                } else {
                    Saved::from_path(argument)
                };
                if let Some(target) = target {
                    let pinned = self.library.pinned(&target.path);
                    if pinned == (name == "pin") {
                        self.info(
                            if pinned {
                                "This item is already pinned"
                            } else {
                                "This item is not pinned"
                            },
                            &target.name,
                        );
                    } else {
                        self.results.push(simple(
                            &format!(
                                "{} {}",
                                if name == "pin" { "Pin" } else { "Unpin" },
                                target.name
                            ),
                            &target.path,
                            "",
                            if name == "pin" { G_PIN } else { G_UNPIN },
                            if name == "pin" {
                                Action::Pin(target.clone())
                            } else {
                                Action::Unpin(target.path.clone())
                            },
                        ));
                    }
                } else {
                    self.info(
                        "Choose an item to pin",
                        "Search for an app, file, or folder first, or use /pin C:\\Work.",
                    );
                }
            }
            "alias" => {
                let end = argument.find(char::is_whitespace).unwrap_or(argument.len());
                let alias = &argument[..end];
                let path = argument[end..].trim();
                let target = if path.is_empty() {
                    self.command_target.as_ref().and_then(|i| i.saved.clone())
                } else {
                    Saved::from_path(path)
                };
                if let (false, Some(target)) = (alias.is_empty(), target) {
                    let replaces = self.library.aliases.contains_key(&alias.to_lowercase());
                    self.results.push(simple(
                        &format!(
                            "{} alias {alias} → {}",
                            if replaces { "Replace" } else { "Save" },
                            target.name
                        ),
                        &target.path,
                        "",
                        G_LINK,
                        Action::SetAlias(alias.into(), target.clone()),
                    ));
                } else {
                    self.info("Give an item a shortcut", "Select a result first, then type /alias inv. Or use /alias inv C:\\Work\\Invoices.");
                }
            }
            "unalias" => {
                if let Some(saved) = self.library.aliases.get(&argument.to_lowercase()) {
                    self.results.push(simple(
                        &format!("Remove alias {argument}"),
                        &format!("{} · the original item stays in place", saved.name),
                        "",
                        G_LINK,
                        Action::RemoveAlias(argument.to_lowercase()),
                    ));
                } else {
                    self.info(
                        "Choose an alias to remove",
                        "Use /aliases to see your shortcuts, then /unalias name.",
                    );
                }
            }
            "windows" => {
                for window in self
                    .windows
                    .iter()
                    .filter(|w| command::matches(&w.title, argument))
                {
                    self.results.push(simple(
                        &window.title,
                        "Switch to this open window",
                        "",
                        G_WINDOWS,
                        Action::Switch(window.clone()),
                    ));
                }
                if self.results.is_empty() {
                    self.info(
                        if self.windows_loading {
                            "Finding open windows…"
                        } else {
                            "No matching windows"
                        },
                        "Try part of a workbook, document, or application window title.",
                    );
                }
            }
            "actions" => {
                if let Some(target) = self.target(argument) {
                    self.action_results(target);
                } else {
                    self.info(
                        if self.searching {
                            "Finding the item…"
                        } else {
                            "Select an item first"
                        },
                        "Search and select a result, then press Ctrl+K, or type /actions budget.",
                    );
                }
            }
            "preview" => {
                if let Some(target) = self.target(argument).and_then(|i| i.saved) {
                    if !crate::settings::get().excel {
                        self.info(
                            "Spreadsheet search is disabled",
                            "Enable Spreadsheets in /settings to preview workbooks.",
                        );
                        return true;
                    }
                    let terms = if argument.is_empty() {
                        self.preview_terms.clone()
                    } else {
                        argument.into()
                    };
                    let request = (target.path.clone(), terms);
                    if self.preview_key.as_ref() != Some(&request) {
                        self.preview_lines = None;
                        self.preview_key = Some(request.clone());
                        let _ = self.preview_tx.send(request);
                    }
                    let mut open = self.saved_item(&target);
                    open.header = Some("Workbook");
                    self.results.push(open);
                    let excerpts = self.results.len();
                    match self.preview_lines.clone() {
                        Some(Ok(lines)) => {
                            for line in lines {
                                let mut item = simple(
                                    &line.location,
                                    &line.text,
                                    "",
                                    G_SHEET,
                                    Action::Copy(line.text.clone()),
                                );
                                item.copy = Some(line.text);
                                self.results.push(item);
                            }
                            self.group(excerpts, "Matching cells");
                        }
                        Some(Err(error)) => self.info("Preview unavailable", &error),
                        None => self.info(
                            "Reading spreadsheet…",
                            "The preview runs in the background; Escape returns to your search.",
                        ),
                    }
                } else {
                    self.info(
                        if self.searching {
                            "Finding the spreadsheet…"
                        } else {
                            "Select a spreadsheet first"
                        },
                        "Search for a workbook and press Ctrl+P, or type /preview excel budget.",
                    );
                }
            }
            "calc" | "convert" | "date" => {
                let answer = match name.as_str() {
                    "convert" => crate::convert::units(argument),
                    "date" => crate::convert::date(argument),
                    _ => crate::calc::eval(argument).map(|n| {
                        let (display, copy) = crate::calc::format(n);
                        crate::convert::Answer {
                            display,
                            copy,
                            note: format!("{argument} · Enter copies"),
                        }
                    }),
                };
                if let Some(answer) = answer {
                    let (glyph, label) = match name.as_str() {
                        "convert" => (G_CONVERT, "Conversion"),
                        "date" => (G_CALENDAR, "Date"),
                        _ => (G_CALC, "Calculator"),
                    };
                    self.results.push(answer_item(answer, glyph, label));
                } else {
                    let command = COMMANDS.iter().find(|c| c.name == name).unwrap();
                    self.info(
                        if argument.is_empty() {
                            command.description
                        } else {
                            "Could not calculate that"
                        },
                        command.usage,
                    );
                    self.results.push(simple(
                        command.example,
                        "Select to try this example",
                        "Example",
                        command_glyph(command.name).unwrap_or(G_CALC),
                        Action::Query(command.example.into()),
                    ));
                    if name == "convert" {
                        self.info("Supported units", "m km cm mm mi ft in yd · kg g lb oz st · l ml ukgal usgal · s min h d · MB MiB GB GiB · C F K");
                    }
                    if name == "date" {
                        self.info(
                            "Date examples",
                            "2026-12-25 - today · today + 2 weeks · use YYYY-MM-DD dates",
                        );
                    }
                }
            }
            "index" => {
                let folder = if argument.is_empty() {
                    self.command_target
                        .as_ref()
                        .and_then(|i| i.saved.as_ref())
                        .filter(|s| s.kind == Kind::Folder)
                        .map(|s| s.path.clone())
                } else {
                    Some(argument.trim_matches('"').replace('/', "\\"))
                };
                let Some(folder) = folder else {
                    self.info(
                        "Choose a folder to add",
                        "Select a folder in the results first, or use /index C:\\Work.",
                    );
                    return true;
                };
                let settings = crate::settings::get();
                let mut searched = crate::index::scan_roots();
                if settings.profile
                    && let Ok(profile) = std::env::var("USERPROFILE")
                {
                    searched.push(profile.into());
                }
                if !std::path::Path::new(&folder).is_absolute() {
                    self.info("Use a full folder path", "For example /index C:\\Work.");
                } else if folder.starts_with("\\\\") {
                    self.info(
                        "Use a mapped drive for network folders",
                        "Map the share to a drive letter, then add that path.",
                    );
                } else if settings.excluded(&folder) {
                    self.info(
                        "This folder is left out of search",
                        "Remove it from Leave out in /settings first.",
                    );
                } else if let Some(root) = searched
                    .iter()
                    .find(|root| crate::settings::within(&folder, &root.to_string_lossy()))
                {
                    self.info(
                        "Lumen already searches this folder",
                        &format!("It is inside {}", root.to_string_lossy()),
                    );
                } else {
                    self.results.push(simple(
                        &format!("Add {folder} to search"),
                        "Lumen indexes it in the background and watches it for changes",
                        "",
                        G_FOLDER,
                        Action::AddFolder(folder),
                    ));
                }
            }
            "settings" => {}
            "reindex" => self.results.push(simple(
                "Refresh the search index",
                "Keep searching while files and spreadsheet contents are refreshed",
                "Index",
                G_REFRESH,
                Action::Reindex,
            )),
            "status" => {
                let (count, cached) = self.index.stats();
                let settings = crate::settings::get();
                self.info(
                    if self.index.is_building() {
                        "Index refresh in progress"
                    } else {
                        "Index is ready"
                    },
                    &format!(
                        "About {} indexed items · {}",
                        thousands(count),
                        if cached {
                            "saved index loaded at startup"
                        } else {
                            "built during this session"
                        }
                    ),
                );
                self.group(0, "Index");
                self.info(
                    "Enabled sources",
                    &format!(
                        "Apps: {} · Files: {} · Spreadsheets: {} · Outlook: {}",
                        on_off(settings.apps),
                        on_off(settings.files),
                        on_off(settings.excel),
                        on_off(settings.mail)
                    ),
                );
                let folders = self.results.len();
                for root in crate::index::scan_roots() {
                    let path = root.to_string_lossy();
                    let name = root.file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
                    self.results.push(simple(&name, &path, "", G_FOLDER, Action::Noop));
                }
                self.group(folders, "Folders searched");
                self.results.push(simple(
                    "Refresh now",
                    "Run /reindex to reconcile the index with your files",
                    "Index",
                    G_REFRESH,
                    Action::Reindex,
                ));
            }
            "perf" => {
                for (title, note) in self.perf_rows.clone() {
                    self.results.push(simple(&title, &note, "", G_SPEED, Action::Noop));
                }
                self.results.push(simple(
                    "Refresh measurements",
                    "Leave Lumen idle between snapshots to inspect idle CPU",
                    "Performance",
                    G_REFRESH,
                    Action::Query("/perf".into()),
                ));
            }
            _ => unreachable!(),
        }
        true
    }

    fn action_results(&mut self, target: Item) {
        let mut open = target.clone();
        open.label = "Enter".into();
        self.results.push(open);
        if let Some(path) = &target.reveal {
            self.results.push(simple(
                "Reveal in File Explorer",
                path,
                "Ctrl+Enter",
                G_REVEAL,
                Action::Reveal(path.clone()),
            ));
        }
        if let Some(path) = &target.copy {
            self.results.push(simple(
                "Copy path",
                path,
                "Ctrl+C",
                G_COPY,
                Action::Copy(path.clone()),
            ));
        }
        if target.admin
            && let Action::Open(path) = &target.action
        {
            self.results.push(simple(
                "Run as administrator",
                &target.title,
                "Ctrl+Shift+Enter",
                G_ADMIN,
                Action::Admin(path.clone()),
            ));
        }
        if let Some(saved) = &target.saved {
            if self.library.pinned(&saved.path) {
                self.results.push(simple(
                    "Unpin this item",
                    &saved.name,
                    "Ctrl+D",
                    G_UNPIN,
                    Action::Unpin(saved.path.clone()),
                ));
            } else {
                self.results.push(simple(
                    "Pin this item",
                    &saved.name,
                    "Ctrl+D",
                    G_PIN,
                    Action::Pin(saved.clone()),
                ));
            }
            self.results.push(simple(
                "Create an alias…",
                &format!("Give {} a short name", saved.name),
                "",
                G_LINK,
                Action::BeginAlias(saved.clone()),
            ));
            if saved.kind != Kind::Email && std::path::Path::new(&saved.path).is_absolute() {
                self.results.push(simple(
                    "Open terminal here",
                    &saved.path,
                    "",
                    G_TERMINAL,
                    Action::Terminal(saved.path.clone()),
                ));
            }
            if crate::preview::supported(&saved.path) && crate::settings::get().excel {
                self.results.push(simple(
                    "Preview worksheet matches",
                    &saved.name,
                    "Ctrl+P",
                    G_PREVIEW,
                    Action::Preview(saved.clone(), self.preview_terms.clone()),
                ));
            }
        }
    }

    pub(super) fn reindex(&self) {
        let ctx = win::CTX.get().cloned();
        self.index.rebuild(move || {
            if let Some(ctx) = &ctx {
                ctx.request_repaint();
            }
        });
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
