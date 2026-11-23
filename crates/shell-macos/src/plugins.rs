//! Plugins: each feature of the launcher is one unit that can be switched off
//! with `[plugins.<id>] enabled = false`.
//!
//! Off means the plugin does not exist: it is stopped, dropped, and never
//! asked about a query, so it does no work and holds no memory. The host
//! (`ui.rs`) owns the panel, the ranking and every row it draws; a plugin only
//! turns a query into plain data (`Answer`). No AppKit type crosses this
//! interface, so a plugin could later be a script speaking the same shapes.
//!
//! Order is fixed by `ALL`, in three passes over the enabled plugins:
//! 1. `claim`: the query is addressed to the plugin (`? …`, a website keyword);
//! 2. `answer`: the query is something the plugin can work out (a sum, a
//!    conversion, a time);
//! 3. `fallback`: nothing else wanted it (a bare web address).
//! The first plugin to return an answer wins; the rest are not asked.

use launcher_core::calc;

use crate::config::{self, Config};
use crate::rates;

/// What Settings shows for a plugin, and its switch's default.
pub struct Meta {
    pub id: &'static str,
    pub name: &'static str,
    /// One line: what it does, and what work it causes when on.
    pub blurb: &'static str,
    pub default_on: bool,
}

/// The typed text, trimmed, and what a plugin may need from the system.
pub struct Query<'a> {
    pub text: &'a str,
    /// Seconds since 1970.
    pub now: f64,
    /// A time zone's offset from UTC in seconds at a moment; `local` is this Mac's.
    pub zone: &'a dyn Fn(&str, i64) -> Option<i32>,
}

/// One row for the top of the list, as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub effect: Effect,
}

/// What Return does on an answer. The host carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Copy `copy`; `expression` and `note` are shown with the result.
    Copy { expression: String, copy: String, note: Option<String> },
    /// Open a web address.
    Open { url: String },
    /// Send `question` to an assistant tool and show what it says.
    Ask { tool: String, question: String },
}

pub trait Plugin {
    /// Called when the plugin is switched on, and again whenever the
    /// configuration changes while it is on.
    fn start(&mut self, _config: &Config) {}
    /// Called once before the plugin is dropped: let go of everything.
    fn stop(&mut self) {}
    fn claim(&mut self, _q: &Query) -> Option<Answer> {
        None
    }
    fn answer(&mut self, _q: &Query) -> Option<Answer> {
        None
    }
    fn fallback(&mut self, _q: &Query) -> Option<Answer> {
        None
    }
}

struct Entry {
    meta: Meta,
    /// `None`: the host does the work itself and only asks whether it is on.
    make: Option<fn() -> Box<dyn Plugin>>,
}

/// Every plugin, in the order they are asked.
static ALL: &[Entry] = &[
    Entry {
        meta: Meta {
            id: "apps",
            name: "Apps",
            blurb: "Finds and opens your apps. Reads the application folders each time the panel opens.",
            default_on: true,
        },
        make: None,
    },
    Entry {
        meta: Meta {
            id: "ask",
            name: "Ask",
            blurb: "“? question” asks an assistant tool on this Mac. Runs the tool only when you press Return.",
            default_on: true,
        },
        make: Some(|| Box::new(Ask::default())),
    },
    Entry {
        meta: Meta {
            id: "websites",
            name: "Websites",
            blurb: "Your keywords (“gh rust”) and typed addresses open in the browser.",
            default_on: true,
        },
        make: Some(|| Box::new(Websites::default())),
    },
    Entry {
        meta: Meta { id: "calculator", name: "Calculator", blurb: "Sums and percentages, worked out as you type.", default_on: true },
        make: Some(|| Box::new(Calculator)),
    },
    Entry {
        meta: Meta {
            id: "currency",
            name: "Currency",
            blurb: "Converts money. Fetches rates from Yahoo Finance when you type a conversion and the saved rate is over an hour old.",
            default_on: true,
        },
        make: Some(|| Box::new(Currency::new(Box::new(LiveRates)))),
    },
    Entry {
        meta: Meta { id: "units", name: "Units", blurb: "Converts length, weight, temperature and more.", default_on: true },
        make: Some(|| Box::new(Units)),
    },
    Entry {
        meta: Meta { id: "time", name: "World time", blurb: "The time in another city, and times converted between zones.", default_on: true },
        make: Some(|| Box::new(WorldTime)),
    },
];

/// Every plugin's description, in order, for Settings.
pub fn all() -> impl Iterator<Item = &'static Meta> {
    ALL.iter().map(|e| &e.meta)
}

/// Whether `id` is switched on in `config`. An unknown id is off.
pub fn is_on(config: &Config, id: &str) -> bool {
    ALL.iter().find(|e| e.meta.id == id).is_some_and(|e| config.plugin_on(id, e.meta.default_on))
}

/// The plugins that are switched on. Main thread only.
#[derive(Default)]
pub struct Registry {
    /// In `ALL`'s order.
    active: Vec<(&'static str, Box<dyn Plugin>)>,
    /// Ids of host-run plugins that are on.
    host: Vec<&'static str>,
}

impl Registry {
    /// Bring the set of running plugins in line with `config`: stop and drop
    /// those switched off, create those switched on, and tell the rest about
    /// the new configuration.
    pub fn configure(&mut self, config: &Config) {
        let mut old = std::mem::take(&mut self.active);
        self.host.clear();
        for entry in ALL {
            let id = entry.meta.id;
            let on = config.plugin_on(id, entry.meta.default_on);
            let Some(make) = entry.make else {
                if on {
                    self.host.push(id);
                }
                continue;
            };
            let running = old.iter().position(|(i, _)| *i == id).map(|i| old.remove(i));
            match (on, running) {
                (true, Some(mut plugin)) => {
                    plugin.1.start(config);
                    self.active.push(plugin);
                }
                (true, None) => {
                    let mut plugin = make();
                    plugin.start(config);
                    self.active.push((id, plugin));
                }
                (false, Some(mut plugin)) => {
                    plugin.1.stop();
                    eprintln!("[rs] plugin {id}: off");
                }
                (false, None) => {}
            }
        }
    }

    /// Whether `id` is running.
    pub fn on(&self, id: &str) -> bool {
        self.host.contains(&id) || self.active.iter().any(|(i, _)| *i == id)
    }

    /// The row the query asks for, if any plugin wants it.
    pub fn special(&mut self, q: &Query) -> Option<Answer> {
        if let Some(a) = self.active.iter_mut().find_map(|(_, p)| p.claim(q)) {
            return Some(a);
        }
        if let Some(a) = self.active.iter_mut().find_map(|(_, p)| p.answer(q)) {
            return Some(a);
        }
        self.active.iter_mut().find_map(|(_, p)| p.fallback(q))
    }
}

// ---------------------------------------------------------------- ask

/// `? question` and `?tool question` (FR-34). The host runs the tool.
#[derive(Default)]
struct Ask {
    config: Config,
}

impl Plugin for Ask {
    fn start(&mut self, config: &Config) {
        self.config = Config { ask: config.ask.clone(), ..Config::default() };
    }

    fn claim(&mut self, q: &Query) -> Option<Answer> {
        let (tool, question) = config::ask_query(&self.config, q.text)?;
        Some(Answer {
            id: format!("ask:{tool}"),
            title: String::new(),
            subtitle: None,
            effect: Effect::Ask { tool: tool.to_string(), question: question.to_string() },
        })
    }
}

// ---------------------------------------------------------------- websites

/// Website keywords (FR-22) and bare addresses.
#[derive(Default)]
struct Websites {
    links: std::collections::BTreeMap<String, String>,
}

fn without_scheme(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").to_string()
}

impl Plugin for Websites {
    fn start(&mut self, config: &Config) {
        self.links = config.links.clone();
    }

    fn stop(&mut self) {
        self.links.clear();
    }

    fn claim(&mut self, q: &Query) -> Option<Answer> {
        let (keyword, rest) = q.text.split_once(char::is_whitespace).unwrap_or((q.text, ""));
        let keyword = keyword.to_lowercase();
        let url = config::link_url(self.links.get(&keyword)?, rest)?;
        let shown = without_scheme(&url);
        Some(Answer {
            id: format!("link:{keyword}"),
            title: if rest.trim().is_empty() { format!("Open {shown}") } else { format!("Open “{}”", rest.trim()) },
            subtitle: Some(shown),
            effect: Effect::Open { url },
        })
    }

    fn fallback(&mut self, q: &Query) -> Option<Answer> {
        let url = config::bare_url(q.text)?;
        Some(Answer { id: "url".into(), title: format!("Open {}", without_scheme(&url)), subtitle: None, effect: Effect::Open { url } })
    }
}

// ---------------------------------------------------------------- calculator

struct Calculator;

impl Plugin for Calculator {
    fn answer(&mut self, q: &Query) -> Option<Answer> {
        let Some(calc::Query::Calc(value)) = calc::parse(q.text) else { return None };
        let text = calc::format_number(value, None);
        Some(Answer {
            id: "calc".into(),
            title: text.clone(),
            subtitle: None,
            effect: Effect::Copy { expression: q.text.to_string(), copy: text, note: None },
        })
    }
}

// ---------------------------------------------------------------- currency

/// Where exchange rates come from. The live one is `rates.rs`; tests count calls.
pub trait RateSource {
    fn start(&mut self);
    /// Forget every rate and cancel anything in flight.
    fn stop(&mut self);
    /// Fetch any of `codes` whose saved rate is missing or old.
    fn refresh(&mut self, codes: &[&str]);
    /// The amount, when the rate was current, and who it is from.
    fn convert(&mut self, amount: f64, from: &str, to: &str) -> Option<(f64, f64, String)>;
}

struct LiveRates;

impl RateSource for LiveRates {
    fn start(&mut self) {
        rates::set_enabled(true);
    }
    fn stop(&mut self) {
        rates::set_enabled(false);
    }
    fn refresh(&mut self, codes: &[&str]) {
        rates::refresh(codes);
    }
    fn convert(&mut self, amount: f64, from: &str, to: &str) -> Option<(f64, f64, String)> {
        rates::convert(amount, from, to)
    }
}

/// Currency conversion (FR-32).
struct Currency {
    rates: Box<dyn RateSource>,
    default: String,
    started: bool,
}

impl Currency {
    fn new(rates: Box<dyn RateSource>) -> Self {
        Self { rates, default: String::new(), started: false }
    }
}

impl Plugin for Currency {
    fn start(&mut self, config: &Config) {
        self.default = config.default_currency.clone();
        if !self.started {
            self.started = true;
            self.rates.start();
        }
    }

    fn stop(&mut self) {
        self.rates.stop();
        self.started = false;
    }

    fn answer(&mut self, q: &Query) -> Option<Answer> {
        let Some(calc::Query::Convert { amount, from, to }) = calc::parse(q.text) else { return None };
        let to = to.unwrap_or_else(|| self.default.clone());
        // A newer rate arrives as a new presentation, never under the number
        // already on screen.
        self.rates.refresh(&[&from, &to]);
        let (title, copy, note) = match self.rates.convert(amount, &from, &to) {
            Some((value, at, source)) => {
                let text = calc::format_number(value, Some(2));
                let unit = self.rates.convert(1.0, &from, &to).map(|u| u.0).unwrap_or(0.0);
                let age = q.now - at;
                let stale = if age > 3.0 * 86_400.0 { " · out of date" } else { "" };
                let note = format!(
                    "1 {from} = {} {to} · {source} · market rate from {}{stale}",
                    calc::format_number(unit, Some(if unit >= 10.0 { 2 } else { 4 })),
                    ago(age)
                );
                (format!("{text} {to}"), text, note)
            }
            None => ("Rate unavailable".to_string(), String::new(), "Waiting for the exchange rate".to_string()),
        };
        Some(Answer {
            id: "convert".into(),
            title,
            subtitle: None,
            effect: Effect::Copy { expression: format!("{} {from} to {to}", calc::format_number(amount, None)), copy, note: Some(note) },
        })
    }
}

fn ago(seconds: f64) -> String {
    let s = seconds.max(0.0);
    if s < 90.0 {
        "just now".into()
    } else if s < 3600.0 {
        format!("{:.0} min ago", s / 60.0)
    } else if s < 86_400.0 {
        format!("{:.0} h ago", s / 3600.0)
    } else {
        format!("{:.0} d ago", s / 86_400.0)
    }
}

// ---------------------------------------------------------------- units

struct Units;

/// Decimal places that give about four significant figures.
fn sig(value: f64) -> usize {
    let magnitude = if value == 0.0 { 0 } else { value.abs().log10().floor() as i32 };
    (3 - magnitude).clamp(0, 6) as usize
}

impl Plugin for Units {
    fn answer(&mut self, q: &Query) -> Option<Answer> {
        let c = launcher_core::units::parse(q.text)?;
        let text = calc::format_number(c.value, Some(sig(c.value)));
        let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
        Some(Answer {
            id: "units".into(),
            title: format!("{text} {}", c.to),
            subtitle: None,
            effect: Effect::Copy { expression: format!("{} {}", calc::format_number(c.amount, None), c.from), copy: text, note: None },
        })
    }
}

// ---------------------------------------------------------------- world time

struct WorldTime;

impl Plugin for WorldTime {
    fn answer(&mut self, q: &Query) -> Option<Answer> {
        let parsed = launcher_core::time::parse(q.text)?;
        let a = launcher_core::time::answer(&parsed, q.now as i64, &q.zone)?;
        Some(Answer {
            id: "time".into(),
            title: a.title.clone(),
            subtitle: None,
            effect: Effect::Copy { expression: a.expression, copy: a.title, note: Some(a.note) },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn zone(id: &str, _t: i64) -> Option<i32> {
        match id {
            "local" | "Asia/Kolkata" => Some(19_800),
            "Asia/Tokyo" => Some(32_400),
            _ => None,
        }
    }

    fn config(toml: &str) -> Config {
        toml::from_str(toml).unwrap()
    }

    /// The id of the row a query leads to.
    fn id(registry: &mut Registry, text: &str) -> Option<String> {
        registry.special(&Query { text, now: 1_800_000_000.0, zone: &zone }).map(|a| a.id)
    }

    /// Every plugin but currency, which would reach for the network.
    const NO_CURRENCY: &str = "[plugins.currency]\nenabled = false\n[links]\ngh = \"https://github.com/{query}\"\n";

    #[test]
    fn queries_reach_the_same_rows_in_the_same_order_as_before() {
        let mut r = Registry::default();
        r.configure(&config(NO_CURRENCY));
        for (query, row) in [
            ("? why is the sky blue", Some("ask:claude")),
            ("?grok why", Some("ask:grok")),
            ("gh rust-lang/rust", Some("link:gh")),
            ("12*7", Some("calc")),
            ("15% of 2400", Some("calc")),
            ("5 km to mi", Some("units")),
            ("6 ft", Some("units")),
            ("time in tokyo", Some("time")),
            ("github.com/rust-lang", Some("url")),
            ("terminal", None),
            ("saf", None),
        ] {
            assert_eq!(id(&mut r, query).as_deref(), row, "{query}");
        }
        // A keyword wins over what follows it, as it did in `special_row`.
        assert_eq!(id(&mut r, "gh 12*7").as_deref(), Some("link:gh"));
    }

    #[test]
    fn a_plugin_switched_off_is_not_asked() {
        let mut r = Registry::default();
        r.configure(&config(NO_CURRENCY));
        assert!(!r.on("currency") && r.on("calculator") && r.on("apps"));
        // The conversion syntax leads nowhere; it is not handed to another plugin.
        assert_eq!(id(&mut r, "100 usd to inr"), None);
        assert_eq!(id(&mut r, "$100"), None);

        r.configure(&config("[plugins.calculator]\nenabled = false\n[plugins.currency]\nenabled = false\n[plugins.websites]\nenabled = false\n[plugins.ask]\nenabled = false\n[plugins.apps]\nenabled = false\n"));
        assert!(!r.on("apps"));
        for query in ["12*7", "? why", "github.com", "g rust"] {
            assert_eq!(id(&mut r, query), None, "{query}");
        }
        assert_eq!(id(&mut r, "5 km to mi").as_deref(), Some("units"));
    }

    #[derive(Default)]
    struct Calls {
        start: u32,
        stop: u32,
        refresh: u32,
        convert: u32,
    }

    struct Counting(Rc<RefCell<Calls>>);

    impl RateSource for Counting {
        fn start(&mut self) {
            self.0.borrow_mut().start += 1;
        }
        fn stop(&mut self) {
            self.0.borrow_mut().stop += 1;
        }
        fn refresh(&mut self, _codes: &[&str]) {
            self.0.borrow_mut().refresh += 1;
        }
        fn convert(&mut self, amount: f64, _from: &str, _to: &str) -> Option<(f64, f64, String)> {
            self.0.borrow_mut().convert += 1;
            Some((amount * 80.0, 1_800_000_000.0 - 7200.0, "Test".into()))
        }
    }

    #[test]
    fn currency_touches_rates_only_for_a_conversion() {
        let calls = Rc::new(RefCell::new(Calls::default()));
        let mut c = Currency::new(Box::new(Counting(calls.clone())));
        c.start(&Config::default());
        c.start(&Config::default()); // a configuration change is not a second start
        assert_eq!(calls.borrow().start, 1);
        let ask = |c: &mut Currency, text: &str| c.answer(&Query { text, now: 1_800_000_000.0, zone: &zone });
        for text in ["12*7", "safari", "5 km to mi", "time in tokyo"] {
            assert!(ask(&mut c, text).is_none(), "{text}");
        }
        assert_eq!((calls.borrow().refresh, calls.borrow().convert), (0, 0));

        let a = ask(&mut c, "100 usd").unwrap();
        assert_eq!(a.title, "8,000.00 INR");
        let Effect::Copy { expression, note, .. } = a.effect else { panic!() };
        assert_eq!(expression, "100 USD to INR");
        assert!(note.unwrap().contains("Test · market rate from 2 h ago"));
        assert_eq!(calls.borrow().refresh, 1);

        c.stop();
        assert_eq!(calls.borrow().stop, 1);
    }

    #[test]
    fn switching_off_stops_the_plugin_and_on_starts_a_new_one() {
        // Uses the live rate source, which only flips a flag until a
        // conversion is typed; none is typed here.
        let mut r = Registry::default();
        r.configure(&Config::default());
        assert!(all().all(|m| r.on(m.id)));
        assert!(rates::enabled());
        r.configure(&config("[plugins.currency]\nenabled = false\n"));
        assert!(!r.on("currency") && !rates::enabled());
        r.configure(&Config::default());
        assert!(r.on("currency") && rates::enabled());
        // The order is `ALL`'s, whatever order plugins were switched in.
        let order: Vec<&str> = r.active.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, ["ask", "websites", "calculator", "currency", "units", "time"]);
    }

    #[test]
    fn every_plugin_is_on_unless_the_file_says_otherwise() {
        let c = Config::default();
        assert!(all().all(|m| is_on(&c, m.id)));
        assert!(!is_on(&c, "no-such-plugin"));
        assert!(!is_on(&config("[plugins.time]\nenabled = false\n"), "time"));
        // A table with only other keys leaves the default.
        assert!(is_on(&config("[plugins.time]\nformat = \"24h\"\n"), "time"));
    }
}
