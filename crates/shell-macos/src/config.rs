//! User configuration (FR-50): one TOML file, read at start and on each summon
//! when it has changed. A malformed edit keeps the last good configuration.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Modifiers and Space, e.g. `cmd+space` or `option+space`. Read at start.
    pub hotkey: String,
    /// Target when a conversion names only one currency (`100 usd`).
    pub default_currency: String,
    /// Show the magnifying glass in the menu bar. Settings stay reachable from
    /// the launcher ("settings") and by opening the app again in Finder.
    pub menu_bar_icon: bool,
    /// Colour on the glass: selection, aurora, accent, none, or "#rrggbb".
    pub theme: String,
    /// How strong that colour is: mild, medium, strong, or 0 to 1.
    pub theme_strength: String,
    /// The opening and resizing springs. Off: the panel appears at its size.
    pub animations: bool,
    /// `alias = "App name"`: typing the alias ranks that app first.
    pub aliases: BTreeMap<String, String>,
    /// `keyword = "https://…{query}…"`: typing `keyword words` opens the URL.
    pub links: BTreeMap<String, String>,
    pub ask: Ask,
    /// `[plugins.<id>]`: one table per feature; `enabled = false` turns it off.
    pub plugins: BTreeMap<String, Plugin>,
}

/// One feature's table under `[plugins]`. Other keys in it are left alone.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Plugin {
    /// Unset: the feature's own default.
    pub enabled: Option<bool>,
    /// The feature's own settings.
    #[serde(flatten)]
    pub options: toml::Table,
}

impl Config {
    /// Whether the feature `id` is switched on, given its default.
    pub fn plugin_on(&self, id: &str, default: bool) -> bool {
        self.plugins.get(id).and_then(|p| p.enabled).unwrap_or(default)
    }

    fn option(&self, id: &str, key: &str) -> Option<&toml::Value> {
        self.plugins.get(id)?.options.get(key)
    }

    /// `[plugins.<id>] hotkey`: opens the panel straight into that plugin
    /// (clipboard history, file search). Empty: none.
    pub fn plugin_hotkey(&self, id: &str) -> String {
        self.option(id, "hotkey").and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    /// `[plugins.clipboard] keep_days`: unpinned items older than this are
    /// dropped. 0 keeps them until the history is full.
    pub fn clipboard_keep_days(&self) -> u32 {
        self.option("clipboard", "keep_days").and_then(|v| v.as_integer()).map_or(30, |d| d.clamp(0, 3650) as u32)
    }

    /// `[plugins.clipboard] ignore_apps`: bundle identifiers whose copies are
    /// never recorded. Unset: Passwords and Keychain Access.
    pub fn clipboard_ignored_apps(&self) -> Vec<String> {
        match self.option("clipboard", "ignore_apps").and_then(|v| v.as_array()) {
            Some(list) => list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            None => crate::clipboard::DEFAULT_IGNORED_APPS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// `[plugins.files] ambient`: file names are listed under apps as you
    /// type, without the `file` keyword.
    pub fn files_ambient(&self) -> bool {
        self.option("files", "ambient").and_then(|v| v.as_bool()).unwrap_or(false)
    }
}

/// Assistant command-line tools for `? question` (FR-34).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Ask {
    /// Used for a bare `? question`. `?name question` picks another.
    pub default: String,
    /// name = [program, args…]; `{question}` marks where the question goes.
    pub tools: BTreeMap<String, Vec<String>>,
}

impl Default for Ask {
    fn default() -> Self {
        let tool = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let mut tools = BTreeMap::new();
        tools.insert("claude".into(), tool(&["~/.local/bin/claude", "-p", "{question}", "--model", "haiku"]));
        tools.insert(
            "codex".into(),
            tool(&["codex", "exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "{question}"]),
        );
        tools.insert("grok".into(), tool(&["grok", "-p", "{question}", "--no-subagents"]));
        Self { default: "claude".into(), tools }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self { hotkey: "option+space".into(), default_currency: "INR".into(), menu_bar_icon: true, theme: "selection".into(), theme_strength: "mild".into(), animations: true, aliases: BTreeMap::new(), links: BTreeMap::new(), ask: Ask::default(), plugins: BTreeMap::new() }
    }
}

const TEMPLATE: &str = r##"# rusty-spotlight configuration. Saved changes apply the next time the panel opens.

# Most of this can be set in Settings (menu bar icon > Settings…, or type
# "settings" in the launcher), which saves here.

# The shortcut that opens the panel: Command, Option, Control or Shift, plus
# Space, a letter or a digit, e.g. "cmd+space" or "ctrl+opt+k". For "cmd+space",
# first turn off Spotlight's own shortcut in System Settings > Keyboard >
# Keyboard Shortcuts > Spotlight.
hotkey = "option+space"

# Used when a conversion names only one currency, e.g. "100 usd".
default_currency = "INR"

# Show the icon in the menu bar. When it is hidden, type "settings" in the
# launcher, or open Rusty Spotlight again in Finder, to reach the settings.
menu_bar_icon = true

# Colour on the glass. "selection" takes the selected app's colour (or one per
# kind of result) and cross-fades as you move; "aurora" drifts slowly through
# a few colours; "accent" uses the system accent colour; "#7c5cff" is one
# fixed colour; "none" is plain glass.
theme = "selection"
# How strong the colour is: "mild", "medium", "strong", or a number 0 to 1.
theme_strength = "mild"

# The opening and resizing springs. false: the panel appears at its size.
animations = true

# alias = "App name": typing the alias puts that app first.
[aliases]
# ff = "Firefox"

# keyword = "URL". "{query}" is replaced by whatever follows the keyword.
[links]
g = "https://www.google.com/search?q={query}"
yt = "https://www.youtube.com/results?search_query={query}"
gh = "https://github.com/{query}"

# "? question" asks an assistant tool installed on this Mac and shows its answer.
# "?codex question" or "?grok question" picks another one. The question is sent
# to that tool's service; nothing it answers is ever run.
[ask]
default = "claude"

[ask.tools]
claude = ["~/.local/bin/claude", "-p", "{question}", "--model", "haiku"]
codex = ["codex", "exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "{question}"]
grok = ["grok", "-p", "{question}", "--no-subagents"]

# Each feature can be switched off, here or in Settings > Plugins. A feature
# that is off does no work at all: no network, no background activity.
# The features: apps, ask, websites, calculator, currency, units, time, and
# three that start off: files, clipboard and brew.
# [plugins.currency]
# enabled = false

# File and folder names, from the index macOS already keeps. "file notes"
# searches files and folders; "folder notes" searches folders. With
# ambient = true, names are also listed under apps as you type.
# hotkey opens the panel straight into file search, with no keyword to type.
# [plugins.files]
# enabled = true
# ambient = false
# hotkey = ""

# Homebrew: "brew ripgrep" searches every formula and cask, the best match and
# the most installed first; Return installs. The catalogue (about 50 MB) is
# fetched once a week, by a helper that exits when it is done, and only when
# you use a brew search.
# [plugins.brew]
# enabled = true
# hotkey = ""

# Clipboard history: text, images and copied files, kept on this Mac. Type
# "clip", or set a shortcut ("shift+cmd+space" is usual; first untick "Ask Siri
# about active window" in System Settings > Keyboard > Keyboard Shortcuts >
# Screenshots). Copies marked secret by password managers are never kept, nor
# anything copied while one of ignore_apps is in front. Unpinned items are
# dropped after keep_days (0: only when the history is full).
# [plugins.clipboard]
# enabled = true
# hotkey = ""
# keep_days = 30
# ignore_apps = ["com.apple.Passwords", "com.apple.keychainaccess"]
"##;

pub fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/rusty-spotlight/config.toml"))
}

pub struct Loaded {
    pub config: Config,
    pub modified: Option<SystemTime>,
    /// Set when the file could not be parsed; `config` is then the previous one.
    pub error: Option<String>,
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Read the file, creating it with examples on first run.
pub fn load(previous: Option<&Config>) -> Loaded {
    let fallback = previous.cloned().unwrap_or_default();
    let Some(path) = path() else {
        return Loaded { config: fallback, modified: None, error: None };
    };
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, TEMPLATE);
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return Loaded { config: fallback, modified: modified(), error: Some(e.to_string()) },
    };
    match toml::from_str::<Config>(&text) {
        Ok(mut config) => {
            config.default_currency = config.default_currency.to_uppercase();
            Loaded { config, modified: modified(), error: None }
        }
        Err(e) => Loaded { config: fallback, modified: modified(), error: Some(e.message().to_string()) },
    }
}

/// Set one top-level `key = value` line, keeping every other line and comment.
/// `value` is written as given, so strings must carry their own quotes.
pub fn set_value(key: &str, value: &str) -> std::io::Result<()> {
    edit_file(|text| with_value(text, key, value))
}

fn with_value(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Only the top level, before the first [table], holds plain keys.
    let top = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    let existing = lines[..top].iter().position(|l| {
        l.split_once('=').is_some_and(|(k, _)| k.trim() == key)
    });
    match existing {
        Some(i) => lines[i] = line,
        None => {
            // After the last top-level line, set off by blank lines.
            let at = lines[..top].iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
            lines.insert(at, line);
            if lines.get(at + 1).is_some_and(|l| !l.trim().is_empty()) {
                lines.insert(at + 1, String::new());
            }
            if at > 0 && !lines[at - 1].trim().is_empty() {
                lines.insert(at, String::new());
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A TOML string, quoted and escaped: `"a \"b\""`.
pub fn quote(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

/// A key as TOML writes it: bare when it can be, quoted otherwise.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare { key.to_string() } else { quote(key) }
}

fn edit_file(change: impl FnOnce(&str) -> String) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no home folder"))?;
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| TEMPLATE.to_string());
    let new = change(&text);
    // Never write a file the launcher could not read back.
    toml::from_str::<Config>(&new).map_err(|e| std::io::Error::other(e.message().to_string()))?;
    std::fs::write(&path, new)
}

/// The lines of `[table]`: from its header to the next header or the end.
fn table_span(lines: &[String], table: &str) -> Option<(usize, usize)> {
    let header = format!("[{table}]");
    // A header may carry a comment: `[plugins.files] # local`.
    let start = lines.iter().position(|l| l.split('#').next().unwrap_or("").trim() == header)?;
    let end = lines[start + 1..].iter().position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| start + 1 + i);
    Some((start, end))
}

fn is_entry(line: &str) -> bool {
    let t = line.trim_start();
    !t.is_empty() && !t.starts_with('#') && t.contains('=')
}

fn entry_key(line: &str) -> Option<String> {
    let (k, _) = line.split_once('=')?;
    let k = k.trim();
    Some(toml::from_str::<toml::Table>(&format!("{k} = 0")).ok()?.keys().next()?.clone())
}

/// Replace every `key = "value"` line of `[table]` with `entries`, keeping
/// its comments; the table is added at the end if it is missing.
fn with_table(text: &str, table: &str, entries: &[(String, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let new: Vec<String> = entries.iter().map(|(k, v)| format!("{} = {}", toml_key(k), quote(v))).collect();
    match table_span(&lines, table) {
        Some((start, end)) => {
            let mut block: Vec<String> = lines[start + 1..end].iter().filter(|l| !is_entry(l)).cloned().collect();
            // New entries go after the table's own comments, before trailing blank lines.
            let at = block.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
            block.splice(at..at, new);
            lines.splice(start + 1..end, block);
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{table}]"));
            lines.extend(new);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Set one `key = value` line inside `[table]`. `value` is written as given.
fn with_table_value(text: &str, table: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let line = format!("{} = {value}", toml_key(key));
    match table_span(&lines, table) {
        Some((start, end)) => {
            match (start + 1..end).find(|&i| is_entry(&lines[i]) && entry_key(&lines[i]).as_deref() == Some(key)) {
                Some(i) => lines[i] = line,
                None => lines.insert(start + 1, line),
            }
        }
        None => {
            lines.push(String::new());
            lines.push(format!("[{table}]"));
            lines.push(line);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Save `[aliases]` or `[links]` as a whole.
pub fn set_table(table: &str, entries: &[(String, String)]) -> std::io::Result<()> {
    edit_file(|text| with_table(text, table, entries))
}

/// Save one key inside a table, e.g. `default` in `[ask]`.
pub fn set_table_value(table: &str, key: &str, value: &str) -> std::io::Result<()> {
    edit_file(|text| with_table_value(text, table, key, value))
}

/// Whether the file changed since `since`.
pub fn changed_since(since: Option<SystemTime>) -> bool {
    modified() != since
}

/// Build the URL for a link keyword. Only http and https are ever opened (NFR-34).
pub fn link_url(template: &str, query: &str) -> Option<String> {
    let mut encoded = String::new();
    for b in query.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => encoded.push(b as char),
            b' ' => encoded.push_str("%20"),
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    let url = template.replace("{query}", &encoded);
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

/// A typed address such as `github.com/rust-lang` or `https://example.org`.
pub fn bare_url(query: &str) -> Option<String> {
    let q = query.trim();
    if q.contains(char::is_whitespace) || q.is_empty() {
        return None;
    }
    if q.starts_with("https://") || q.starts_with("http://") {
        return (q.len() > 10).then(|| q.to_string());
    }
    let host = q.split('/').next()?;
    let (name, tld) = host.rsplit_once('.')?;
    let ok = !name.is_empty()
        && (2..=24).contains(&tld.len())
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then(|| format!("https://{q}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_value_keeps_the_rest_of_the_file() {
        let text = "# note\nhotkey = \"cmd+space\"\n\n[links]\ng = \"x\"\n";
        let added = with_value(text, "menu_bar_icon", "false");
        assert_eq!(added, "# note\nhotkey = \"cmd+space\"\n\nmenu_bar_icon = false\n\n[links]\ng = \"x\"\n");
        let changed = with_value(&added, "menu_bar_icon", "true");
        assert_eq!(changed, added.replace("menu_bar_icon = false", "menu_bar_icon = true"));
        // A key of the same name inside a table is not the top-level one.
        let in_table = "[links]\nmenu_bar_icon = \"x\"\n";
        assert_eq!(with_value(in_table, "menu_bar_icon", "false"), "menu_bar_icon = false\n\n[links]\nmenu_bar_icon = \"x\"\n");
        assert!(toml::from_str::<Config>(&with_value(in_table, "menu_bar_icon", "false")).is_ok_and(|c| !c.menu_bar_icon));
        assert!(toml::from_str::<Config>(&with_value(TEMPLATE, "menu_bar_icon", "false")).is_ok_and(|c| !c.menu_bar_icon));
    }

    #[test]
    fn links_encode_the_query() {
        assert_eq!(link_url("https://github.com/{query}", "rust-lang/rust").as_deref(), Some("https://github.com/rust-lang/rust"));
        assert_eq!(link_url("https://x.test/?q={query}", "a b&c").as_deref(), Some("https://x.test/?q=a%20b%26c"));
        assert_eq!(link_url("file:///etc/{query}", "passwd"), None);
    }

    #[test]
    fn bare_urls() {
        assert_eq!(bare_url("github.com/rust-lang").as_deref(), Some("https://github.com/rust-lang"));
        assert_eq!(bare_url("https://example.org/a").as_deref(), Some("https://example.org/a"));
        for s in ["terminal", "2.0", "1.2.3", "a b.com", "notes.", ".com"] {
            assert_eq!(bare_url(s), None, "{s}");
        }
    }

    #[test]
    fn ask_queries() {
        let c = Config::default();
        assert_eq!(ask_query(&c, "? list big files"), Some(("claude", "list big files")));
        assert_eq!(ask_query(&c, "?grok list big files"), Some(("grok", "list big files")));
        assert_eq!(ask_query(&c, "?Codex why"), Some(("codex", "why")));
        assert_eq!(ask_query(&c, "?how do I untar"), Some(("claude", "how do I untar")));
        assert_eq!(ask_query(&c, "?"), None);
        assert_eq!(ask_query(&c, "?grok"), None);
        assert_eq!(ask_query(&c, "terminal"), None);
    }

    #[test]
    fn template_parses() {
        let c: Config = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(c.default_currency, "INR");
        assert!(c.links.contains_key("gh"));
        assert_eq!(c.ask.default, "claude");
        assert_eq!(c.ask.tools.len(), 3);
    }

    #[test]
    fn tables_are_rewritten_in_place() {
        let text = "a = 1\n\n# note\n[aliases]\n# ff = \"Firefox\"\nold = \"X\"\n\n[links]\ng = \"https://g/{query}\"\n";
        let entries = vec![("ff".to_string(), "Firefox".to_string()), ("my app".to_string(), "Say \"Hi\"".to_string())];
        let out = with_table(text, "aliases", &entries);
        assert_eq!(out, "a = 1\n\n# note\n[aliases]\n# ff = \"Firefox\"\nff = \"Firefox\"\n\"my app\" = 'Say \"Hi\"'\n\n[links]\ng = \"https://g/{query}\"\n");
        let parsed: Config = toml::from_str(&out).unwrap();
        assert_eq!(parsed.aliases.get("my app").map(String::as_str), Some("Say \"Hi\""));
        assert_eq!(parsed.links.len(), 1);
        // Emptied: the comment stays, the entries go.
        assert!(!with_table(&out, "aliases", &[]).contains("\nff = "));
        // Missing: added at the end.
        assert!(toml::from_str::<Config>(&with_table("a = 1\n", "links", &entries)).is_ok_and(|c| c.links.len() == 2));
    }

    #[test]
    fn a_plugin_is_switched_in_its_own_table() {
        let off = with_table_value(TEMPLATE, "plugins.currency", "enabled", "false");
        let c: Config = toml::from_str(&off).unwrap();
        assert!(!c.plugin_on("currency", true) && c.plugin_on("units", true));
        assert_eq!(c.ask.tools.len(), 3, "the tables before it are intact");
        let on = with_table_value(&off, "plugins.currency", "enabled", "true");
        assert!(toml::from_str::<Config>(&on).unwrap().plugin_on("currency", false));
        assert_eq!(on.matches("[plugins.currency]").count(), 2, "the template's commented example, and the real table");
        // A header with a comment after it is still that table.
        let noted = "[plugins.files] # local settings\nenabled = false\n";
        let out = with_table_value(noted, "plugins.files", "enabled", "true");
        assert_eq!(out, "[plugins.files] # local settings\nenabled = true\n");
        // A second plugin gets its own table and does not disturb the first.
        let two = with_table_value(&off, "plugins.files", "enabled", "true");
        let c: Config = toml::from_str(&two).unwrap();
        assert!(!c.plugin_on("currency", true) && c.plugin_on("files", false));
        // Files written before plugins existed read as before.
        let old: Config = toml::from_str("default_currency = \"EUR\"\n[links]\ng = \"https://g/{query}\"\n").unwrap();
        assert!(old.plugins.is_empty() && old.plugin_on("currency", true));
    }

    #[test]
    fn plugin_options_are_read_from_the_plugin_table() {
        let c = Config::default();
        assert_eq!((c.plugin_hotkey("clipboard").as_str(), c.clipboard_keep_days(), c.files_ambient()), ("", 30, false));
        assert_eq!(c.clipboard_ignored_apps(), ["com.apple.Passwords", "com.apple.keychainaccess"]);
        let c: Config = toml::from_str("[plugins.clipboard]\nenabled = true\nhotkey = \"shift+cmd+space\"\nkeep_days = 7\nignore_apps = [\"com.1password.1password\"]\n[plugins.files]\nambient = true\nhotkey = \"opt+cmd+space\"\n").unwrap();
        assert_eq!((c.plugin_hotkey("files").as_str(), c.plugin_hotkey("units").as_str()), ("opt+cmd+space", ""));
        assert!(c.plugin_on("clipboard", false) && !c.plugin_on("files", false));
        assert_eq!((c.plugin_hotkey("clipboard").as_str(), c.clipboard_keep_days(), c.files_ambient()), ("shift+cmd+space", 7, true));
        assert_eq!(c.clipboard_ignored_apps(), ["com.1password.1password"]);
        // Saving one option leaves the others.
        let text = "[plugins.clipboard]\nenabled = true\nkeep_days = 7\n";
        let out = with_table_value(text, "plugins.clipboard", "hotkey", "\"ctrl+v\"");
        let c: Config = toml::from_str(&out).unwrap();
        assert_eq!((c.plugin_hotkey("clipboard").as_str(), c.clipboard_keep_days()), ("ctrl+v", 7));
    }

    #[test]
    fn a_table_value_is_replaced() {
        let out = with_table_value(TEMPLATE, "ask", "default", "\"grok\"");
        assert_eq!(toml::from_str::<Config>(&out).unwrap().ask.default, "grok");
        assert_eq!(out.matches("default = ").count(), 1);
        assert_eq!(toml::from_str::<Config>(&out).unwrap().ask.tools.len(), 3);
    }

}

/// `? question` or `?name question` → (tool name, question).
pub fn ask_query<'a>(config: &'a Config, query: &'a str) -> Option<(&'a str, &'a str)> {
    let rest = query.trim_start().strip_prefix('?')?;
    let (head, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if head.is_empty() {
        let question = tail.trim();
        return (!question.is_empty()).then_some((config.ask.default.as_str(), question));
    }
    match config.ask.tools.get_key_value(&head.to_lowercase()) {
        Some((name, _)) => {
            let question = tail.trim();
            (!question.is_empty()).then_some((name.as_str(), question))
        }
        // `?how do I…` with no space after the mark: the default tool.
        None => Some((config.ask.default.as_str(), rest.trim())),
    }
}
