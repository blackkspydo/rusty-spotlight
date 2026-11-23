//! Exchange rates (FR-32). Rates are units of a currency per one US dollar.
//! Refreshed only when a conversion is typed and the cached rate is over an
//! hour old; cached rates keep working offline. Main thread only.
//!
//! Nothing here does anything until `set_enabled(true)` (the currency plugin
//! being switched on): no file is read and no request is made. Switching it
//! off forgets the rates, cancels requests in flight, and makes any reply
//! that still arrives a no-op.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{
    NSData, NSError, NSMutableURLRequest, NSOperationQueue, NSString, NSURL, NSURLResponse, NSURLSession,
    NSURLSessionConfiguration,
};

#[derive(Clone, Debug)]
pub struct Rate {
    pub per_usd: f64,
    /// When the source says the rate was current, seconds since the Unix epoch.
    pub at: f64,
    /// When this app last fetched it. Markets close, so `at` can stay old
    /// however often the rate is fetched; freshness is judged by this.
    pub fetched: f64,
    pub source: String,
}

#[derive(Default)]
struct State {
    rates: HashMap<String, Rate>,
    in_flight: HashSet<String>,
    last_try: HashMap<String, f64>,
    session: Option<Retained<NSURLSession>>,
    on_update: Option<Box<dyn Fn()>>,
    loaded: bool,
    enabled: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

const MAX_AGE: f64 = 3600.0;
const RETRY_AFTER: f64 = 60.0;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/rusty-spotlight/rates.tsv"))
}

fn ensure_loaded(state: &mut State) {
    if state.loaded {
        return;
    }
    state.loaded = true;
    let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let [code, rate, at, fetched, source] = f.as_slice() {
            if let (Ok(per_usd), Ok(at), Ok(fetched)) = (rate.parse::<f64>(), at.parse::<f64>(), fetched.parse::<f64>()) {
                if per_usd.is_finite() && per_usd > 0.0 {
                    state.rates.insert(code.to_string(), Rate { per_usd, at, fetched, source: source.to_string() });
                }
            }
        }
    }
}

fn save(state: &State) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::new();
    for (code, r) in &state.rates {
        out.push_str(&format!("{code}\t{}\t{}\t{}\t{}\n", r.per_usd, r.at, r.fetched, r.source));
    }
    let tmp = path.with_extension("tsv.tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Called on the main thread whenever a fetch stores a new rate.
pub fn set_on_update(f: impl Fn() + 'static) {
    STATE.with_borrow_mut(|s| s.on_update = Some(Box::new(f)));
}

/// Switch rate keeping on or off. Off drops everything held in memory and
/// cancels requests; the saved file stays for the next time it is on.
pub fn set_enabled(on: bool) {
    let session = STATE.with_borrow_mut(|s| {
        if s.enabled == on {
            return None;
        }
        s.enabled = on;
        s.rates.clear();
        s.in_flight.clear();
        s.last_try.clear();
        s.loaded = false;
        s.session.take()
    });
    if let Some(session) = session {
        // Outside the borrow: cancelled tasks may call their handlers.
        session.invalidateAndCancel();
    }
}

pub fn enabled() -> bool {
    STATE.with_borrow(|s| s.enabled)
}

/// The cached rate for `code`, if any. USD is always 1.
pub fn get(code: &str) -> Option<Rate> {
    if !enabled() {
        return None;
    }
    if code == "USD" {
        return Some(Rate { per_usd: 1.0, at: now(), fetched: now(), source: String::new() });
    }
    STATE.with_borrow_mut(|s| {
        ensure_loaded(s);
        s.rates.get(code).cloned()
    })
}

/// Start a fetch for each code whose cached rate is missing or over an hour old.
pub fn refresh(codes: &[&str]) {
    if !enabled() {
        return;
    }
    for code in codes {
        if *code == "USD" {
            continue;
        }
        let due = STATE.with_borrow_mut(|s| {
            ensure_loaded(s);
            let t = now();
            let fresh = s.rates.get(*code).is_some_and(|r| t - r.fetched < MAX_AGE);
            let tried = s.last_try.get(*code).is_some_and(|l| t - l < RETRY_AFTER);
            if fresh || tried || s.in_flight.contains(*code) {
                return false;
            }
            s.in_flight.insert(code.to_string());
            s.last_try.insert(code.to_string(), t);
            true
        });
        if due {
            fetch_yahoo(code.to_string());
        }
    }
}

fn session() -> Retained<NSURLSession> {
    STATE.with_borrow_mut(|s| {
        s.session
            .get_or_insert_with(|| {
                let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
                // Completion handlers run on the main queue, so no locking is needed.
                unsafe {
                    NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                        &config,
                        None,
                        Some(&NSOperationQueue::mainQueue()),
                    )
                }
            })
            .clone()
    })
}

fn get_json(url: &str, done: impl Fn(Option<serde_json::Value>) + 'static) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        done(None);
        return;
    };
    let request = NSMutableURLRequest::requestWithURL(&url);
    request.setTimeoutInterval(8.0);
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str("Mozilla/5.0 (Macintosh) rusty-spotlight")),
        &NSString::from_str("User-Agent"),
    );
    let handler = RcBlock::new(move |data: *mut NSData, _response: *mut NSURLResponse, _error: *mut NSError| {
        let json = unsafe { data.as_ref() }.and_then(|d| serde_json::from_slice(&d.to_vec()).ok());
        done(json);
    });
    let task = unsafe { session().dataTaskWithRequest_completionHandler(&request, &handler) };
    task.resume();
}

fn store(code: &str, per_usd: Option<f64>, at: Option<f64>, source: &str) -> bool {
    let Some(per_usd) = per_usd.filter(|r| r.is_finite() && *r > 0.0) else { return false };
    let callback_due = STATE.with_borrow_mut(|s| {
        s.rates.insert(
            code.to_string(),
            Rate { per_usd, at: at.unwrap_or_else(now), fetched: now(), source: source.to_string() },
        );
        save(s);
        s.on_update.is_some()
    });
    if callback_due {
        // Take the callback out while it runs so it may call back into this module.
        let callback = STATE.with_borrow_mut(|s| s.on_update.take());
        if let Some(callback) = callback {
            callback();
            STATE.with_borrow_mut(|s| {
                if s.on_update.is_none() {
                    s.on_update = Some(callback);
                }
            });
        }
    }
    true
}

fn finish(code: &str) {
    STATE.with_borrow_mut(|s| {
        s.in_flight.remove(code);
    });
}

/// Primary source: Yahoo Finance's chart endpoint (unofficial; see the requirements).
fn fetch_yahoo(code: String) {
    let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/USD{code}=X?interval=1d&range=1d");
    get_json(&url, move |json| {
        if !enabled() {
            return; // switched off while the request was out
        }
        let meta = json.as_ref().map(|j| &j["chart"]["result"][0]["meta"]);
        let price = meta.and_then(|m| m["regularMarketPrice"].as_f64());
        let at = meta.and_then(|m| m["regularMarketTime"].as_f64());
        if store(&code, price, at, "Yahoo Finance") {
            finish(&code);
        } else {
            fetch_fallback(code.clone());
        }
    });
}

/// Fallback: ExchangeRate-API's open endpoint (daily; attribution required).
fn fetch_fallback(code: String) {
    get_json("https://open.er-api.com/v6/latest/USD", move |json| {
        if !enabled() {
            return;
        }
        let rate = json.as_ref().and_then(|j| j["rates"][code.as_str()].as_f64());
        let at = json.as_ref().and_then(|j| j["time_last_update_unix"].as_f64());
        store(&code, rate, at, "Rates By Exchange Rate API");
        finish(&code);
    });
}

/// Convert through the dollar. Returns the amount, the older of the two rate
/// times, and the source(s).
pub fn convert(amount: f64, from: &str, to: &str) -> Option<(f64, f64, String)> {
    let (a, b) = (get(from)?, get(to)?);
    let value = amount / a.per_usd * b.per_usd;
    if !value.is_finite() {
        return None;
    }
    let at = match (from, to) {
        ("USD", _) => b.at,
        (_, "USD") => a.at,
        _ => a.at.min(b.at),
    };
    let source = match (a.source.as_str(), b.source.as_str()) {
        ("", s) | (s, "") => s.to_string(),
        (x, y) if x == y => x.to_string(),
        (x, y) => format!("{x} / {y}"),
    };
    Some((value, at, source))
}
