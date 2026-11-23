//! Homebrew package search: an index of formulae and casks, and a ranking
//! that weighs how well a package matches the query against how many people
//! install it. No I/O here; the shell fetches and stores the index.

use crate::text::fold;

/// One formula or cask.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    /// What `brew install` takes: `ripgrep`, `visual-studio-code`.
    pub token: String,
    /// A cask's display name ("Visual Studio Code"); empty for a formula.
    pub name: String,
    pub desc: String,
    pub homepage: String,
    pub version: String,
    pub cask: bool,
    /// Installs over the last 30 days, from Homebrew's public analytics.
    pub installs: u64,
    /// The folded text the search runs on, built once.
    folded: Folded,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Folded {
    token: String,
    name: String,
    desc: String,
}

impl Package {
    pub fn new(token: &str, name: &str, desc: &str, homepage: &str, version: &str, cask: bool, installs: u64) -> Self {
        Self {
            token: token.to_string(),
            name: name.to_string(),
            desc: desc.to_string(),
            homepage: homepage.to_string(),
            version: version.to_string(),
            cask,
            installs,
            folded: Folded { token: fold(token), name: fold(name), desc: fold(desc) },
        }
    }

    /// What to call it in a list: a cask's display name, else the token.
    pub fn title(&self) -> &str {
        if self.name.is_empty() { &self.token } else { &self.name }
    }

    /// One line of the index file. Tabs and newlines in the text become spaces.
    pub fn to_line(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            if self.cask { 'c' } else { 'f' },
            clean(&self.token),
            clean(&self.name),
            clean(&self.desc),
            clean(&self.homepage),
            clean(&self.version),
            self.installs
        )
    }

    fn from_line(line: &str) -> Option<Self> {
        let f: Vec<&str> = line.split('\t').collect();
        let [kind, token, name, desc, homepage, version, installs] = f.as_slice() else { return None };
        if token.is_empty() {
            return None;
        }
        Some(Self::new(token, name, desc, homepage, version, *kind == "c", installs.parse().unwrap_or(0)))
    }
}

/// Read an index file; lines that do not parse are skipped.
pub fn parse_index(text: &str) -> Vec<Package> {
    text.lines().filter_map(Package::from_line).collect()
}

/// A count as Homebrew's analytics write it: `"333,609"`.
pub fn parse_count(text: &str) -> u64 {
    text.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

/// How well one query word matches one package, 0 when it does not.
fn word_relevance(word: &str, p: &Folded) -> u32 {
    let starts_a_word = |text: &str, separators: &[char]| {
        text.split(|c: char| separators.contains(&c)).skip(1).any(|part| part.starts_with(word))
    };
    if p.token == word || p.name == word {
        100
    } else if p.token.starts_with(word) || p.name.starts_with(word) {
        75
    } else if starts_a_word(&p.token, &['-', '@', '_', '.']) || starts_a_word(&p.name, &[' ', '-']) {
        // Nearly as good as the name starting with it: "chrome" should find
        // google-chrome ahead of a little-used chrome-something.
        68
    } else if p.token.contains(word) || p.name.contains(word) {
        45
    } else if p.desc.split(|c: char| !c.is_alphanumeric()).any(|part| part.starts_with(word)) {
        25
    } else {
        0
    }
}

/// Relevance first, popularity second: every query word must match, the
/// weakest word sets the tier, and installs lift a package by at most one tier.
fn score(words: &[&str], whole: &str, p: &Package) -> Option<f64> {
    let mut tier = u32::MAX;
    for word in words {
        match word_relevance(word, &p.folded) {
            0 => return None,
            r => tier = tier.min(r),
        }
    }
    // The whole query naming the package beats any single-word match.
    if p.folded.token == whole || p.folded.name == whole {
        tier = 120;
    }
    // 10 installs → 5, 1,000 → 15, 100,000 → 25: a far more popular package
    // rises above a slightly better match, but never above an exact one.
    let popularity = ((p.installs as f64 + 1.0).log10() * 5.0).min(30.0);
    Some(tier as f64 + popularity)
}

/// The best `limit` packages for `query`, as indexes into `packages`.
pub fn search(query: &str, packages: &[Package], limit: usize) -> Vec<usize> {
    let whole = fold(query);
    let whole = whole.trim();
    let words: Vec<&str> = whole.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f64, usize)> =
        packages.iter().enumerate().filter_map(|(i, p)| score(&words, whole, p).map(|s| (s, i))).collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| packages[a.1].token.len().cmp(&packages[b.1].token.len()))
            .then_with(|| packages[a.1].token.cmp(&packages[b.1].token))
    });
    scored.into_iter().take(limit).map(|(_, i)| i).collect()
}

/// The most installed packages, for when nothing has been typed yet.
pub fn popular(packages: &[Package], limit: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..packages.len()).collect();
    order.sort_by(|a, b| packages[*b].installs.cmp(&packages[*a].installs).then_with(|| packages[*a].token.cmp(&packages[*b].token)));
    order.truncate(limit);
    order
}

/// `1234567` → `1.2M`, `45300` → `45K`, `812` → `812`.
pub fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}K", n as f64 / 1000.0),
        10_000..=999_999 => format!("{:.0}K", n as f64 / 1000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packages() -> Vec<Package> {
        vec![
            Package::new("git", "", "Distributed revision control system", "https://git-scm.com", "2.50", false, 134_583),
            Package::new("gh", "", "GitHub command-line tool", "https://cli.github.com", "2.60", false, 333_609),
            Package::new("git-lfs", "", "Git extension for versioning large files", "", "3.6", false, 30_000),
            Package::new("lazygit", "", "Simple terminal UI for git commands", "", "0.44", false, 25_000),
            Package::new("gitui", "", "Blazing fast terminal-ui for git written in rust", "", "0.26", false, 4_000),
            Package::new("legit", "", "Command-line interface for Git, optimized for workflow simplicity", "", "1.2", false, 12),
            Package::new("ripgrep", "", "Search tool like grep and The Silver Searcher", "", "14.1", false, 40_000),
            Package::new("visual-studio-code", "Microsoft Visual Studio Code", "Open-source code editor", "", "1.99", true, 60_000),
            Package::new("github", "GitHub Desktop", "Desktop client for GitHub repositories", "", "3.4", true, 9_000),
            Package::new("node", "", "Platform built on V8 to build network applications", "", "23", false, 246_376),
            Package::new("node@20", "", "Platform built on V8 to build network applications", "", "20", false, 20_000),
            Package::new("nodenv", "", "Manage multiple NodeJS versions", "", "1.5", false, 3_000),
        ]
    }

    fn found(query: &str) -> Vec<String> {
        let all = packages();
        search(query, &all, 8).into_iter().map(|i| all[i].token.clone()).collect()
    }

    #[test]
    fn an_exact_name_comes_first_however_popular_the_others_are() {
        assert_eq!(found("git")[0], "git");
        assert_eq!(found("node")[0], "node");
        // gh is installed far more than git, and is not what "git" names.
        assert!(!found("git").contains(&"gh".to_string()) || found("git").iter().position(|t| t == "gh") > Some(0));
    }

    #[test]
    fn a_prefix_beats_a_substring_and_popularity_orders_within_a_tier() {
        let order = found("git");
        let at = |t: &str| order.iter().position(|x| x == t).unwrap_or(99);
        // Prefix matches: git-lfs (30K), github cask (9K), gitui (4K).
        assert!(at("git-lfs") < at("gitui"), "{order:?}");
        // Substring matches rank below prefixes; lazygit (25K) above legit (12).
        assert!(at("gitui") < at("legit"), "{order:?}");
        assert!(at("lazygit") < at("legit"), "{order:?}");
    }

    #[test]
    fn far_more_installs_can_lift_a_weaker_match_one_tier() {
        // "lazygit" only contains "git" (45) but is installed 25,000 times
        // (+22); "gitui" starts with it (75) with 4,000 (+18). The prefix
        // still wins: popularity never jumps two tiers.
        let order = found("git");
        let at = |t: &str| order.iter().position(|x| x == t).unwrap();
        assert!(at("gitui") < at("lazygit"), "{order:?}");
    }

    #[test]
    fn casks_are_found_by_display_name_and_descriptions_are_searched_last() {
        assert_eq!(found("visual studio")[0], "visual-studio-code");
        assert_eq!(found("github desktop")[0], "github");
        assert_eq!(found("code")[0], "visual-studio-code");
        // Only the description mentions grep's rival.
        assert_eq!(found("silver searcher"), ["ripgrep"]);
        assert_eq!(found("revision control"), ["git"]);
        assert!(found("zzzz").is_empty());
        assert!(found("  ").is_empty());
    }

    #[test]
    fn a_much_used_package_beats_a_little_used_one_whose_name_starts_with_the_word() {
        let all = vec![
            Package::new("chrome-remote-desktop-host", "Chrome Remote Desktop", "Remote access", "", "1", true, 300),
            Package::new("google-chrome", "Google Chrome", "Web browser", "", "1", true, 38_332),
            Package::new("chrome-cli", "", "Control Google Chrome from the command-line", "", "1", false, 55),
        ];
        let order: Vec<&str> = search("chrome", &all, 8).into_iter().map(|i| all[i].token.as_str()).collect();
        assert_eq!(order, ["google-chrome", "chrome-remote-desktop-host", "chrome-cli"]);
    }

    #[test]
    fn every_word_must_match() {
        assert_eq!(found("git large"), ["git-lfs"]);
        assert!(found("git banana").is_empty());
    }

    #[test]
    fn versions_after_an_at_sign_match_as_words() {
        let order = found("node");
        assert_eq!(&order[..2], ["node", "node@20"]);
        assert_eq!(found("20"), ["node@20"]);
    }

    #[test]
    fn the_index_round_trips() {
        let all = packages();
        let text: String = all.iter().map(|p| p.to_line() + "\n").collect();
        assert_eq!(parse_index(&text), all);
        let odd = Package::new("x", "A\tB", "line\none", "", "1", true, 5);
        let back = parse_index(&odd.to_line());
        assert_eq!((back[0].name.as_str(), back[0].desc.as_str()), ("A B", "line one"));
        assert!(parse_index("garbage\n\tno token\n").is_empty());
    }

    #[test]
    fn counts_and_popular() {
        assert_eq!(parse_count("333,609"), 333_609);
        assert_eq!(parse_count(""), 0);
        assert_eq!((short_count(812), short_count(4_000), short_count(45_300), short_count(1_234_567)), ("812".into(), "4.0K".into(), "45K".into(), "1.2M".into()));
        let all = packages();
        let top: Vec<&str> = popular(&all, 3).into_iter().map(|i| all[i].token.as_str()).collect();
        assert_eq!(top, ["gh", "node", "git"]);
    }
}
