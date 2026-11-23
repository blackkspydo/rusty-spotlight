//! Homebrew: search every formula and cask, ranked by how well it matches and
//! how many people install it, and install from the panel.
//!
//! The catalogue is Homebrew's public JSON (about 50 MB for formulae and
//! casks, plus 30-day install counts). It is fetched and boiled down to a
//! small index file by a short-lived copy of this program (`--brew-index`),
//! so the launcher itself never holds those 50 MB. The index is read when a
//! `brew` search starts and dropped when the panel closes.
//!
//! Nothing here runs unless the brew plugin is on and a `brew` search is typed.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use launcher_core::brew::{Package, parse_count, parse_index};
use serde::Deserialize;

/// The index is rebuilt when it is older than this.
const MAX_AGE: Duration = Duration::from_secs(7 * 86_400);
const API: &str = "https://formulae.brew.sh/api";

/// The `brew` program, where Homebrew installs it.
pub fn program() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].into_iter().map(PathBuf::from).find(|p| p.is_file())
}

fn index_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/rusty-spotlight/brew-index.tsv"))
}

/// `brew` or `brew <words>` → the words. `brewery` is an ordinary search.
pub fn keyword(query: &str) -> Option<String> {
    let text = query.trim_start();
    let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    word.eq_ignore_ascii_case("brew").then(|| rest.trim().to_string())
}

// ---------------------------------------------------------------- the index

#[derive(Deserialize)]
struct Formula {
    name: String,
    desc: Option<String>,
    homepage: Option<String>,
    versions: Option<Versions>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Deserialize)]
struct Versions {
    stable: Option<String>,
}

#[derive(Deserialize)]
struct Cask {
    token: String,
    #[serde(default)]
    name: Vec<String>,
    desc: Option<String>,
    homepage: Option<String>,
    version: Option<String>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Deserialize)]
struct Analytics {
    items: Vec<Count>,
}

#[derive(Deserialize)]
struct Count {
    formula: Option<String>,
    cask: Option<String>,
    count: String,
}

fn download(url: &str, to: &Path) -> Result<Vec<u8>, String> {
    let status = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--compressed", "--max-time", "180", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("could not fetch {url}"));
    }
    let bytes = std::fs::read(to).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(to);
    bytes
}

fn counts(bytes: &[u8]) -> HashMap<String, u64> {
    let Ok(parsed) = serde_json::from_slice::<Analytics>(bytes) else { return HashMap::new() };
    let mut map = HashMap::new();
    for item in parsed.items {
        if let Some(name) = item.formula.or(item.cask) {
            // Options such as `node --HEAD` are counted separately; add them up.
            let name = name.split_whitespace().next().unwrap_or("").to_string();
            *map.entry(name).or_insert(0) += parse_count(&item.count);
        }
    }
    map
}

/// Fetch Homebrew's catalogue and write the index. Run as `--brew-index`, in
/// a process of its own. Returns how many packages were written.
pub fn build_index() -> Result<usize, String> {
    let path = index_path().ok_or("no home folder")?;
    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = |name: &str| dir.join(format!("brew-{name}.{}.tmp", std::process::id()));

    let mut packages: Vec<Package> = Vec::new();
    {
        let installs = counts(&download(&format!("{API}/analytics/install-on-request/30d.json"), &tmp("fa"))?);
        let bytes = download(&format!("{API}/formula.json"), &tmp("f"))?;
        let formulae: Vec<Formula> = serde_json::from_slice(&bytes).map_err(|e| format!("formula.json: {e}"))?;
        for f in formulae.into_iter().filter(|f| !f.disabled) {
            let version = f.versions.and_then(|v| v.stable).unwrap_or_default();
            let n = installs.get(&f.name).copied().unwrap_or(0);
            packages.push(Package::new(&f.name, "", f.desc.as_deref().unwrap_or(""), f.homepage.as_deref().unwrap_or(""), &version, false, n));
        }
    }
    {
        let installs = counts(&download(&format!("{API}/analytics/cask-install/30d.json"), &tmp("ca"))?);
        let bytes = download(&format!("{API}/cask.json"), &tmp("c"))?;
        let casks: Vec<Cask> = serde_json::from_slice(&bytes).map_err(|e| format!("cask.json: {e}"))?;
        for c in casks.into_iter().filter(|c| !c.disabled) {
            let n = installs.get(&c.token).copied().unwrap_or(0);
            let name = c.name.first().map(String::as_str).unwrap_or("");
            packages.push(Package::new(&c.token, name, c.desc.as_deref().unwrap_or(""), c.homepage.as_deref().unwrap_or(""), c.version.as_deref().unwrap_or(""), true, n));
        }
    }
    if packages.len() < 1000 {
        return Err(format!("only {} packages came back; keeping the old index", packages.len()));
    }
    let text: String = packages.iter().map(|p| p.to_line() + "\n").collect();
    let partial = tmp("index");
    std::fs::write(&partial, text).map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(packages.len())
}

thread_local! {
    /// The `--brew-index` child, while one runs.
    static BUILDING: RefCell<Option<Child>> = const { RefCell::new(None) };
    /// When the index was last found fresh, or a fetch last failed: until a
    /// little later, nothing is checked and nothing is fetched again.
    static SETTLED_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// A fetch that failed is not tried again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// A fresh index is not looked at again for this long.
const RECHECK_AFTER: Duration = Duration::from_secs(600);

fn index_is_fresh() -> bool {
    index_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < MAX_AGE)
}

/// Whether the index is being built right now. Reaps the child when it ends.
pub fn building() -> bool {
    BUILDING.with_borrow_mut(|slot| {
        let ended = match slot {
            Some(child) => match child.try_wait() {
                Ok(None) => return true,
                Ok(Some(status)) => status.success(),
                Err(_) => false,
            },
            None => return false,
        };
        *slot = None;
        if !ended {
            eprintln!("[rs] brew: the catalogue fetch failed");
            SETTLED_UNTIL.set(Some(Instant::now() + RETRY_AFTER));
        }
        false
    })
}

/// Start rebuilding the index if it is missing or over a week old and no
/// rebuild is running. Returns whether one is running afterwards.
pub fn refresh_if_stale() -> bool {
    if building() {
        return true;
    }
    // Called for every presentation of a brew search: keep it to a compare.
    if SETTLED_UNTIL.get().is_some_and(|until| Instant::now() < until) {
        return false;
    }
    if index_is_fresh() {
        SETTLED_UNTIL.set(Some(Instant::now() + RECHECK_AFTER));
        return false;
    }
    let Ok(me) = std::env::current_exe() else { return false };
    match Command::new(me).arg("--brew-index").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(child) => {
            eprintln!("[rs] brew: fetching the catalogue");
            BUILDING.set(Some(child));
            true
        }
        Err(e) => {
            eprintln!("[rs] brew: could not start the catalogue fetch: {e}");
            false
        }
    }
}

/// Stop a fetch in progress (the plugin was switched off).
pub fn cancel_fetch() {
    if let Some(mut child) = BUILDING.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The packages, and which of them are installed on this Mac.
pub struct Catalogue {
    pub packages: Vec<Package>,
    /// The most installed, for an empty search; worked out once.
    pub popular: Vec<usize>,
    installed: HashSet<(bool, String)>,
}

impl Catalogue {
    pub fn installed(&self, p: &Package) -> bool {
        self.installed.contains(&(p.cask, p.token.clone()))
    }

    /// Look again at what is installed, after an install or removal.
    pub fn rescan(&mut self) {
        self.installed = installed();
    }
}

/// Read the index. `None` until it has been built once.
pub fn load() -> Option<Catalogue> {
    let text = std::fs::read_to_string(index_path()?).ok()?;
    // The index is a file anyone running as this user could have edited, and
    // its text came off the network. A token is later shown as a command to
    // copy, and a home page is opened: only plain names and web addresses
    // get past this point.
    let packages: Vec<Package> = parse_index(&text)
        .into_iter()
        .filter(|p| valid_token(&p.token))
        .map(|mut p| {
            if !web_address(&p.homepage) {
                p.homepage.clear();
            }
            p
        })
        .collect();
    if packages.is_empty() {
        return None;
    }
    let popular = launcher_core::brew::popular(&packages, 16);
    Some(Catalogue { packages, popular, installed: installed() })
}

/// What is installed: the folder names under Homebrew's Cellar and Caskroom.
/// Reading two folders is far quicker than asking `brew list`.
fn installed() -> HashSet<(bool, String)> {
    let mut set = HashSet::new();
    let Some(prefix) = program().and_then(|p| p.parent().and_then(Path::parent).map(Path::to_path_buf)) else { return set };
    for (folder, cask) in [("Cellar", false), ("Caskroom", true)] {
        if let Ok(entries) = std::fs::read_dir(prefix.join(folder)) {
            for entry in entries.flatten() {
                set.insert((cask, entry.file_name().to_string_lossy().into_owned()));
            }
        }
    }
    set
}

// ---------------------------------------------------------------- jobs

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Install,
    Uninstall,
    Upgrade,
}

impl Verb {
    fn argument(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Uninstall => "uninstall",
            Verb::Upgrade => "upgrade",
        }
    }

    /// "Installing", "Removing", "Upgrading".
    pub fn doing(self) -> &'static str {
        match self {
            Verb::Install => "Installing",
            Verb::Uninstall => "Removing",
            Verb::Upgrade => "Upgrading",
        }
    }

    /// "Installed", "Removed", "Upgraded".
    pub fn done(self) -> &'static str {
        match self {
            Verb::Install => "Installed",
            Verb::Uninstall => "Removed",
            Verb::Upgrade => "Upgraded",
        }
    }

    /// "install", "remove", "upgrade".
    pub fn failed(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Uninstall => "remove",
            Verb::Upgrade => "upgrade",
        }
    }
}

/// The shell command a job stands for, to show and to copy.
pub fn command_line(verb: Verb, token: &str, cask: bool) -> String {
    format!("brew {}{} {token}", verb.argument(), if cask { " --cask" } else { "" })
}

/// One `brew install`, `uninstall` or `upgrade`, running in the background.
/// It is not stopped when the panel closes: a half-finished install is worse
/// than one that completes unseen.
pub struct Job {
    pub verb: Verb,
    pub token: String,
    pub title: String,
    last: Arc<Mutex<String>>,
    rx: Receiver<bool>,
}

impl Job {
    /// The latest line `brew` printed.
    pub fn last_line(&self) -> String {
        self.last.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// `Some(succeeded)` once `brew` has finished.
    pub fn finished(&self) -> Option<bool> {
        match self.rx.try_recv() {
            Ok(ok) => Some(ok),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(false),
        }
    }
}

/// Only http and https addresses are ever opened.
pub fn web_address(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://")) && !url.contains(char::is_whitespace)
}

/// Only names Homebrew itself could have given are ever passed to `brew`.
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with('-')
        && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+'))
}

pub fn start(verb: Verb, token: &str, title: &str, cask: bool) -> Result<Job, String> {
    let brew = program().ok_or("Homebrew is not installed")?;
    if !valid_token(token) {
        return Err(format!("{token:?} is not a package name"));
    }
    let mut command = Command::new(brew);
    command.arg(verb.argument());
    if cask {
        command.arg("--cask");
    }
    // No terminal: a step that would ask for a password fails instead of hanging.
    let mut child = command
        .arg(token)
        .env("NONINTERACTIVE", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("HOMEBREW_NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run brew: {e}"))?;
    let last = Arc::new(Mutex::new(String::new()));
    let (tx, rx) = channel();
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let note = |last: &Arc<Mutex<String>>, line: String| {
        let line = line.trim().trim_start_matches("==> ").to_string();
        if !line.is_empty() {
            if let Ok(mut slot) = last.lock() {
                *slot = line;
            }
        }
    };
    // Errors are what matter when it fails, so stderr is kept apart and wins.
    let errors = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = stderr {
        let errors = errors.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.trim_start().starts_with("Error") || line.trim_start().starts_with("sudo") {
                    note(&errors, line);
                }
            }
        });
    }
    let for_thread = last.clone();
    std::thread::spawn(move || {
        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                note(&for_thread, line);
            }
        }
        let ok = child.wait().is_ok_and(|status| status.success());
        if !ok {
            let error = errors.lock().map(|e| e.clone()).unwrap_or_default();
            if !error.is_empty() {
                if let Ok(mut slot) = for_thread.lock() {
                    *slot = error;
                }
            }
        }
        let _ = tx.send(ok);
    });
    Ok(Job { verb, token: token.to_string(), title: title.to_string(), last, rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brew_is_a_keyword_and_brewery_is_not() {
        assert_eq!(keyword("brew").as_deref(), Some(""));
        assert_eq!(keyword("  Brew  ripgrep ").as_deref(), Some("ripgrep"));
        assert_eq!(keyword("brew visual studio").as_deref(), Some("visual studio"));
        assert_eq!(keyword("brewery"), None);
        assert_eq!(keyword("homebrew"), None);
    }

    #[test]
    fn only_package_names_reach_brew() {
        for ok in ["ripgrep", "node@20", "visual-studio-code", "gtk+3", "python@3.13"] {
            assert!(valid_token(ok), "{ok}");
        }
        for bad in ["", "--force", "-v", "a b", "x;rm -rf /", "$(id)", "a/b"] {
            assert!(!valid_token(bad), "{bad}");
        }
        assert!(web_address("https://git-scm.com") && web_address("http://x.test/a"));
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "", "https://a b", "ssh://host"] {
            assert!(!web_address(bad), "{bad}");
        }
        assert_eq!(command_line(Verb::Install, "iterm2", true), "brew install --cask iterm2");
        assert_eq!(command_line(Verb::Upgrade, "git", false), "brew upgrade git");
    }

    #[test]
    fn install_counts_are_read_and_options_added_up() {
        let json = br#"{"items":[{"number":1,"formula":"gh","count":"333,609"},{"number":2,"formula":"node --HEAD","count":"10"},{"number":3,"formula":"node","count":"246,376"},{"number":4,"cask":"codex","count":"5"}]}"#;
        let map = counts(json);
        assert_eq!((map["gh"], map["node"], map["codex"]), (333_609, 246_386, 5));
        assert!(counts(b"not json").is_empty());
    }

    #[test]
    fn the_catalogue_shapes_parse() {
        let f: Vec<Formula> = serde_json::from_str(r#"[{"name":"git","desc":"VCS","homepage":"https://git-scm.com","versions":{"stable":"2.50","head":"HEAD"},"disabled":false,"extra":[1,2]},{"name":"old","desc":null,"homepage":null,"versions":null,"disabled":true}]"#).unwrap();
        assert_eq!((f[0].name.as_str(), f[0].versions.as_ref().unwrap().stable.as_deref(), f[1].disabled), ("git", Some("2.50"), true));
        let c: Vec<Cask> = serde_json::from_str(r#"[{"token":"0-ad","name":["0 A.D."],"desc":"Game","homepage":"https://play0ad.com/","version":"0.28.0","artifacts":[]}]"#).unwrap();
        assert_eq!((c[0].token.as_str(), c[0].name[0].as_str(), c[0].disabled), ("0-ad", "0 A.D.", false));
    }
}
