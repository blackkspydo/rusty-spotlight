//! File and folder search through Spotlight's existing index (`mdfind`).
//! Nothing is indexed here. Results arrive on a background read so typing
//! stays on the main thread; the caller drops the search to cancel it.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub modified: f64,
}

/// `kMDItemFSName` contains `term`. Quotes and wildcards in the term are
/// removed so they cannot change the query.
pub fn spotlight_query(term: &str, folders_only: bool) -> Option<String> {
    let clean: String = term
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '*' | '\n' | '\r' | '\0'))
        .take(80)
        .collect();
    let clean = clean.trim();
    if clean.chars().count() < 2 {
        return None;
    }
    let folder = if folders_only { " && kMDItemContentTypeTree == 'public.folder'" } else { "" };
    Some(format!("kMDItemFSName == \"*{clean}*\"cd && kMDItemFSInvisible != 1{folder}"))
}

pub fn cache_key(term: &str, folders_only: bool) -> String {
    format!("{}:{term}", if folders_only { "d" } else { "f" })
}

/// `file ` / `files ` search files and folders. `folder ` / `folders ` search
/// folders. The word alone, with no space after it, is not a search.
pub fn keyword(query: &str) -> Option<(String, bool)> {
    let text = query.trim_start();
    let lower = text.to_ascii_lowercase();
    let (prefix, folders_only) = [("folders ", true), ("folder ", true), ("files ", false), ("file ", false)]
        .into_iter()
        .find(|(prefix, _)| lower.starts_with(prefix))?;
    Some((text[prefix.len()..].trim().to_string(), folders_only))
}

pub struct Running {
    pub key: String,
    pub generation: u64,
    rx: Receiver<Vec<Hit>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Running {
    pub fn poll(&self) -> Option<Vec<Hit>> {
        match self.rx.try_recv() {
            Ok(hits) => Some(hits),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Vec::new()),
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn start(term: &str, folders_only: bool, generation: u64) -> Option<Running> {
    let query = spotlight_query(term, folders_only)?;
    let home = std::env::var("HOME").ok()?;
    let mut child = Command::new("/usr/bin/mdfind")
        .args(["-onlyin", &home, &query])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let child = Arc::new(Mutex::new(Some(child)));
    let (tx, rx) = channel();
    let for_thread = child.clone();
    let home_for_filter = home.clone();
    std::thread::spawn(move || {
        let hits = read_hits(stdout, &home_for_filter);
        if let Some(mut child) = for_thread.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = tx.send(hits);
    });
    Some(Running { key: cache_key(term, folders_only), generation, rx, child })
}

fn read_hits(stdout: impl std::io::Read, home: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let start = Instant::now();
    let mut raw = 0;
    for line in BufReader::new(stdout).lines() {
        if start.elapsed() > Duration::from_millis(800) || raw >= 400 {
            break;
        }
        let Ok(path) = line else { break };
        raw += 1;
        if path.is_empty() || noisy(&path, home) {
            continue;
        }
        let meta = std::fs::metadata(&path).ok();
        let modified = meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        hits.push(Hit { path, name, modified });
        if hits.len() >= 40 {
            break;
        }
    }
    hits.sort_by(|a, b| b.modified.partial_cmp(&a.modified).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(8);
    hits
}

/// `~/Library`, build folders and VCS metadata match almost every short query
/// and are not what a person is trying to open.
fn noisy(path: &str, home: &str) -> bool {
    let library = format!("{home}/Library");
    if path == library || path.starts_with(&(library.clone() + "/")) {
        return true;
    }
    path.split('/').any(|part| {
        matches!(part, "node_modules" | "target" | ".git" | ".Trash" | "DerivedData" | ".cargo" | "build")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_needs_two_characters_and_drops_quotes() {
        assert!(spotlight_query("a", false).is_none());
        assert!(spotlight_query("  x ", false).is_none());
        let query = spotlight_query("tax \"return\"", false).unwrap();
        assert_eq!(query, "kMDItemFSName == \"*tax return*\"cd && kMDItemFSInvisible != 1");
        assert!(spotlight_query("notes", true).unwrap().contains("public.folder"));
        assert!(!spotlight_query("notes", false).unwrap().contains("public.folder"));
    }

    #[test]
    fn keywords_need_a_trailing_space() {
        assert_eq!(keyword("file"), None);
        assert_eq!(keyword("folder"), None);
        assert_eq!(keyword("files"), None);
        assert_eq!(keyword("file notes"), Some(("notes".into(), false)));
        assert_eq!(keyword("File Notes"), Some(("Notes".into(), false)));
        assert_eq!(keyword("folders tax"), Some(("tax".into(), true)));
        assert_eq!(keyword("file "), Some(("".into(), false)));
        assert_eq!(keyword("  files  report.pdf"), Some(("report.pdf".into(), false)));
    }

    #[test]
    fn library_and_build_folders_are_skipped() {
        let home = "/Users/me";
        assert!(noisy("/Users/me/Library/Caches/x", home));
        assert!(noisy("/Users/me/code/app/node_modules/leftpad/index.js", home));
        assert!(!noisy("/Users/me/Documents/notes.txt", home));
    }
}
