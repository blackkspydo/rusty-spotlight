//! System Settings search: every pane, and every section a pane lists in its
//! search terms ("Night Shift", "Text Replacements", "Microphone"), each with
//! the link that opens it. The shell builds the list from the panes installed
//! on this Mac; this ranks it. No I/O here.

use crate::text::fold;

#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// "Night Shift", or a pane's own name for the pane itself.
    pub title: String,
    /// The pane it is in: "Displays". Equal to `title` for a pane.
    pub pane: String,
    /// `x-apple.systempreferences:<pane id>[?<section>]`.
    pub url: String,
    /// The pane's icon, as a type identifier (`com.apple.graphic-icon.bluetooth`); may be empty.
    pub icon: String,
    folded: String,
    /// Letters and digits only, so "wifi" finds "Wi‑Fi".
    squashed: String,
    words: Vec<String>,
    keywords: Vec<String>,
    /// Ranks one step lower: for panes with hundreds of narrow sections
    /// (Accessibility), whose names would otherwise crowd out common ones.
    demoted: bool,
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn split_words(folded: &str) -> Vec<String> {
    folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

impl Setting {
    pub fn new(title: &str, pane: &str, url: &str, keywords: &[String]) -> Self {
        let folded = fold(title);
        let mut words = split_words(&folded);
        // A section is also found by its pane's name: "displays night".
        if pane != title {
            words.extend(split_words(&fold(pane)));
        }
        let keywords = keywords.iter().map(|k| fold(k.trim())).filter(|k| !k.is_empty()).collect();
        Self { title: title.to_string(), pane: pane.to_string(), url: url.to_string(), icon: String::new(), squashed: squash(&folded), folded, words, keywords, demoted: false }
    }

    pub fn with_icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_string();
        self
    }

    pub fn demoted(mut self) -> Self {
        self.demoted = true;
        self
    }

    /// Whether this is a pane rather than a section inside one.
    pub fn is_pane(&self) -> bool {
        self.title == self.pane
    }
}

/// How strongly a setting matches: titles beat search words, and a pane
/// beats a section of the same strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Strength {
    Keyword = 1,
    Word = 2,
    Prefix = 3,
    Exact = 4,
}

fn word_matches(word: &str, s: &Setting) -> Option<Strength> {
    if s.words.iter().any(|w| w.starts_with(word)) {
        return Some(Strength::Word);
    }
    // Search words are matched whole, or by prefix once three letters are typed:
    // "on" must not find every setting whose words include "only".
    let keyword = |k: &String| k == word || (word.chars().count() >= 3 && k.split(' ').any(|part| part.starts_with(word)));
    s.keywords.iter().any(keyword).then_some(Strength::Keyword)
}

fn strength(query: &str, words: &[&str], s: &Setting) -> Option<Strength> {
    let found = plain_strength(query, words, s)?;
    Some(match (s.demoted, found) {
        (false, found) => found,
        (true, Strength::Exact) => Strength::Prefix,
        (true, Strength::Prefix) => Strength::Word,
        (true, _) => Strength::Keyword,
    })
}

fn plain_strength(query: &str, words: &[&str], s: &Setting) -> Option<Strength> {
    let squashed = squash(query);
    if s.folded == query || (!squashed.is_empty() && s.squashed == squashed) {
        return Some(Strength::Exact);
    }
    if s.folded.starts_with(query) || (squashed.chars().count() >= 2 && s.squashed.starts_with(&squashed)) {
        return Some(Strength::Prefix);
    }
    // A phrase from the search words: "tap to click", "hot corners".
    if query.contains(' ') && s.keywords.iter().any(|k| k.starts_with(query) || k.contains(&format!(" {query}"))) {
        return Some(Strength::Keyword);
    }
    // Every word typed must match; the weakest decides.
    let mut weakest = Strength::Exact;
    for word in words {
        weakest = weakest.min(word_matches(word, s)?);
    }
    Some(weakest.min(Strength::Word))
}

/// The best `limit` settings for `query`, strongest first, with how strong.
pub fn search(query: &str, settings: &[Setting], limit: usize) -> Vec<(usize, Strength)> {
    let q = fold(query);
    let q = q.trim();
    if q.chars().count() < 2 {
        return Vec::new();
    }
    // "and" is how "&" is typed: "privacy and security".
    let words: Vec<&str> = q.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && *w != "and").collect();
    let mut found: Vec<(usize, Strength)> =
        settings.iter().enumerate().filter_map(|(i, s)| strength(q, &words, s).map(|st| (i, st))).collect();
    found.sort_by(|a, b| {
        let (x, y) = (&settings[a.0], &settings[b.0]);
        b.1.cmp(&a.1)
            .then_with(|| y.is_pane().cmp(&x.is_pane()))
            .then_with(|| x.demoted.cmp(&y.demoted))
            .then_with(|| x.title.len().cmp(&y.title.len()))
            .then_with(|| x.title.cmp(&y.title))
    });
    // The same title can be listed by two panes' search terms; show it once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|(i, _)| seen.insert(settings[*i].folded.clone()));
    found.truncate(limit);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Setting> {
        let k = |words: &str| words.split(", ").map(str::to_string).collect::<Vec<_>>();
        let pane = |name: &str, id: &str, words: &str| Setting::new(name, name, &format!("x-apple.systempreferences:{id}"), &k(words));
        let section = |title: &str, pane: &str, url: &str, words: &str| Setting::new(title, pane, url, &k(words));
        vec![
            pane("Wi‑Fi", "com.apple.wifi-settings-extension", "wireless, network, internet"),
            pane("Bluetooth", "com.apple.BluetoothSettings", "wireless, pair, headphones"),
            pane("Displays", "com.apple.Displays-Settings.extension", "monitor, screen, resolution"),
            section("Night Shift", "Displays", "x-apple.systempreferences:com.apple.Displays-Settings.extension?nightShiftSection", "warm, color, night, blue light"),
            pane("Keyboard", "com.apple.Keyboard-Settings.extension", "keys, typing"),
            section("Text Replacements", "Keyboard", "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?TextReplacements", "abbreviation, shortcut, expand"),
            section("Keyboard Shortcuts", "Keyboard", "x-apple.systempreferences:com.apple.Keyboard-Settings.extension?Shortcuts", "hotkey, shortcut"),
            pane("Privacy & Security", "com.apple.settings.PrivacySecurity.extension", "privacy, security, permissions"),
            section("Microphone", "Privacy & Security", "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone", "mic, audio input, recording"),
            pane("Sound", "com.apple.Sound-Settings.extension", "volume, audio, output, input, only"),
        ]
    }

    fn titles(query: &str) -> Vec<String> {
        let s = all();
        search(query, &s, 8).into_iter().map(|(i, _)| s[i].title.clone()).collect()
    }

    #[test]
    fn names_come_first_then_sections_then_search_words() {
        assert_eq!(titles("bluetooth")[0], "Bluetooth");
        assert_eq!(titles("night")[0], "Night Shift");
        assert_eq!(titles("keyboard"), ["Keyboard", "Keyboard Shortcuts", "Text Replacements"]);
        // Only search words mention it.
        assert_eq!(titles("mic"), ["Microphone"]);
        assert_eq!(titles("monitor"), ["Displays"]);
        assert_eq!(titles("abbreviation"), ["Text Replacements"]);
    }

    #[test]
    fn punctuation_does_not_matter() {
        assert_eq!(titles("wifi")[0], "Wi‑Fi");
        assert_eq!(titles("wi-fi")[0], "Wi‑Fi");
        assert_eq!(titles("privacy and")[0], "Privacy & Security");
    }

    #[test]
    fn a_section_is_found_with_its_pane_name() {
        assert_eq!(titles("displays night")[0], "Night Shift");
        assert!(titles("keyboard night").is_empty());
    }

    #[test]
    fn short_or_partial_search_words_do_not_flood_results() {
        assert!(titles("x").is_empty());
        // "on" is not a prefix of "only" among search words; it is two letters.
        assert!(!titles("on").contains(&"Sound".to_string()));
        assert!(titles("onl").contains(&"Sound".to_string()));
    }

    #[test]
    fn phrases_are_found_among_search_words() {
        let s = vec![Setting::new("Trackpad", "Trackpad", "u", &["tap to click".to_string(), "scroll".to_string()])];
        assert_eq!(search("tap to click", &s, 4).len(), 1);
        assert_eq!(search("tap to", &s, 4).len(), 1);
        assert!(search("tap to scroll", &s, 4).is_empty());
    }

    #[test]
    fn a_demoted_section_gives_way_to_a_common_one() {
        let s = vec![
            Setting::new("Night", "Accessibility", "a", &[]).demoted(),
            Setting::new("Night Shift options", "Displays", "d", &[]),
        ];
        let order: Vec<&str> = search("night", &s, 4).into_iter().map(|(i, _)| s[i].title.as_str()).collect();
        assert_eq!(order, ["Night Shift options", "Night"]);
    }

    #[test]
    fn strengths_are_reported_for_placing_results() {
        let s = all();
        let strength_of = |q: &str| search(q, &s, 1)[0].1;
        assert_eq!(strength_of("bluetooth"), Strength::Exact);
        assert_eq!(strength_of("blue"), Strength::Prefix);
        assert_eq!(strength_of("shift"), Strength::Word);
        assert_eq!(strength_of("monitor"), Strength::Keyword);
    }
}
