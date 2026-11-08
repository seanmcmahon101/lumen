//! One catalog drives slash completion and the in-app help.
pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub example: &'static str,
}
pub const COMMANDS: &[Command] = &[
    Command {
        name: "help",
        usage: "/help [topic]",
        description: "Commands, search examples, and keyboard shortcuts",
        example: "/help",
    },
    Command {
        name: "pins",
        usage: "/pins [filter]",
        description: "Open your pinned apps, files, and folders",
        example: "/pins",
    },
    Command {
        name: "pin",
        usage: "/pin [full path]",
        description: "Pin the selected result, or a full path",
        example: "/pin",
    },
    Command {
        name: "unpin",
        usage: "/unpin",
        description: "Unpin the selected result; select it before typing /unpin",
        example: "/unpin",
    },
    Command {
        name: "recent",
        usage: "/recent [filter]",
        description: "Items opened through Lumen, newest first",
        example: "/recent",
    },
    Command {
        name: "alias",
        usage: "/alias name [full path or URL]",
        description: "Give the selected result an explicit shortcut",
        example: "/alias inv C:\\Work\\Invoices",
    },
    Command {
        name: "aliases",
        usage: "/aliases [filter]",
        description: "List your shortcuts; type a name in normal search to open it",
        example: "/aliases",
    },
    Command {
        name: "unalias",
        usage: "/unalias name",
        description: "Remove an explicit shortcut",
        example: "/unalias inv",
    },
    Command {
        name: "windows",
        usage: "/windows [title]",
        description: "Find and switch to an open desktop window",
        example: "/windows budget",
    },
    Command {
        name: "actions",
        usage: "/actions [search]",
        description: "Open, reveal, copy, pin, alias, preview, or open a terminal",
        example: "/actions",
    },
    Command {
        name: "preview",
        usage: "/preview [search]",
        description: "Read worksheet names and matching cells in a selected spreadsheet",
        example: "/preview",
    },
    Command {
        name: "calc",
        usage: "/calc expression",
        description: "Calculate locally; Enter copies the answer",
        example: "/calc 125 * 1.2",
    },
    Command {
        name: "convert",
        usage: "/convert value unit in unit",
        description: "Length, area, mass, volume, time, data, and temperature",
        example: "/convert 10 miles in km",
    },
    Command {
        name: "date",
        usage: "/date [date calculation]",
        description: "Today, days between dates, or adding days/weeks",
        example: "/date today + 30 days",
    },
    Command {
        name: "settings",
        usage: "/settings",
        description: "Hotkey, search sources, folders, and exclusions",
        example: "/settings",
    },
    Command {
        name: "reindex",
        usage: "/reindex",
        description: "Refresh the saved index in the background",
        example: "/reindex",
    },
    Command {
        name: "status",
        usage: "/status",
        description: "Indexing progress, enabled sources, and index size",
        example: "/status",
    },
    Command {
        name: "perf",
        usage: "/perf",
        description: "Measured opening, search, display times, memory, and CPU",
        example: "/perf",
    },
];

pub fn parse(query: &str) -> Option<(String, &str)> {
    let input = query.trim_start().strip_prefix('/')?;
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    Some((input[..end].to_ascii_lowercase(), input[end..].trim()))
}
pub fn search_query(query: &str) -> &str {
    match parse(query) {
        Some((name, argument)) if matches!(name.as_str(), "actions" | "preview") => argument,
        Some(_) => "",
        None => query,
    }
}
pub fn matches(text: &str, filter: &str) -> bool {
    let text = text.to_lowercase();
    filter
        .to_lowercase()
        .split_whitespace()
        .all(|word| text.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slash_queries_do_not_leak_into_file_or_mail_search() {
        assert_eq!(parse(" /HELP Excel"), Some(("help".into(), "Excel")));
        assert_eq!(search_query("/help mail"), "");
        assert_eq!(search_query("/actions excel budget"), "excel budget");
        assert_eq!(search_query("mail invoice"), "mail invoice");
        for (i, command) in COMMANDS.iter().enumerate() {
            assert!(!COMMANDS[..i].iter().any(|c| c.name == command.name));
        }
    }
}
