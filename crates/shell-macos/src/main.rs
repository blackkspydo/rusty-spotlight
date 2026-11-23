//! A small Spotlight-style launcher. See docs/REQUIREMENTS.md and docs/PLAN.md.
//!
//! Knobs kept while spike S1's manual checks are outstanding:
//!   RS_LEVEL     floating | modal | status | popup (default) | screensaver
//!   RS_BEHAVIOR  a (default: all spaces + fullscreen auxiliary + stationary)
//!                b (can join all applications)   c (move to active space)
//!   RS_MATERIAL  glass (default) | blur
//!   RS_HOTKEY    overrides `hotkey` in config.toml, e.g. `option+space`
//!   RS_DEBUG     any value: log show timings to stderr

mod apps;
mod ask;
mod brew;
mod clipboard;
mod config;
mod files;
mod hotkey;
mod keytap;
mod plugins;
mod rates;
mod settings;
mod spaces;
mod store;
mod theme;
mod ui;
mod watch;

use std::cell::RefCell;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use ui::Controller;

thread_local! {
    static CONTROLLER: RefCell<Option<Retained<Controller>>> = const { RefCell::new(None) };
}

fn diagnostics(args: &[String]) -> bool {
    // rusty-spotlight --apps            list the discovered apps
    // rusty-spotlight --rank <query>    show the ranked results for a query
    if args.len() >= 3 && args[1] == "--icon-colours" {
        // rusty-spotlight --icon-colours OUT.html   compare glass colours per app
        let items = apps::scan(&config::load(None).config.aliases);
        let list: Vec<(String, String)> = items.iter().map(|i| (i.title.clone(), i.id.clone())).collect();
        theme::diagnose(&list, &args[2]);
        eprintln!("wrote {}", args[2]);
        return true;
    }
    if args.len() >= 2 && args[1] == "--brew-index" {
        // rusty-spotlight --brew-index   fetch Homebrew's catalogue and write the search index.
        // The launcher runs this as a child, so the 50 MB of JSON never sit in its own memory.
        match brew::build_index() {
            Ok(n) => eprintln!("indexed {n} packages"),
            Err(e) => {
                eprintln!("brew index: {e}");
                std::process::exit(1);
            }
        }
        return true;
    }
    if args.len() >= 3 && args[1] == "--brew" {
        // rusty-spotlight --brew <query>   show how packages rank for a query
        let Some(catalogue) = brew::load() else {
            eprintln!("no index yet; run --brew-index");
            return true;
        };
        let t = Instant::now();
        let found = launcher_core::brew::search(&args[2], &catalogue.packages, 8);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        for i in found {
            let p = &catalogue.packages[i];
            println!("{}\t{}\t{}\t{}{}", p.installs, if p.cask { "cask" } else { "formula" }, p.token, p.desc, if catalogue.installed(p) { "  [installed]" } else { "" });
        }
        eprintln!("{} packages searched in {ms:.2} ms", catalogue.packages.len());
        return true;
    }
    if args.len() >= 2 && args[1] == "--plugins" {
        // rusty-spotlight --plugins [query]   which plugins are on, and the row a query leads to
        let config = config::load(None).config;
        let mut registry = plugins::Registry::default();
        registry.configure(&config);
        for meta in plugins::all() {
            println!("{}\t{}", if registry.on(meta.id) { "on " } else { "off" }, meta.id);
        }
        if let Some(text) = args.get(2) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let t = Instant::now();
            let answer = registry.special(&plugins::Query { text: text.trim(), now, zone: &ui::zone_offset });
            let us = t.elapsed().as_secs_f64() * 1e6;
            match answer {
                Some(a) => println!("{}\t{}\t({us:.0} µs)", a.id, a.title),
                None => println!("no plugin answers\t({us:.0} µs)"),
            }
        }
        return true;
    }
    if args.len() < 2 || (args[1] != "--apps" && args[1] != "--rank") {
        return false;
    }
    let t = Instant::now();
    let items = apps::scan(&config::load(None).config.aliases);
    let scan_ms = t.elapsed().as_secs_f64() * 1000.0;
    if args[1] == "--apps" {
        for i in &items {
            println!("{}\t{}", i.title, i.id);
        }
    } else {
        let q = args.get(2).map(String::as_str).unwrap_or("");
        let usage = store::load();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let t = Instant::now();
        let ranked = launcher_core::Ranker::default().rank(q, &items, &usage, now, 8);
        let rank_us = t.elapsed().as_secs_f64() * 1e6;
        for r in &ranked {
            println!("{:?}\t{:.2}\t{}", r.tier, r.weight, items[r.index].title);
        }
        eprintln!("rank: {rank_us:.0} µs over {} apps", items.len());
    }
    eprintln!("scan: {} apps in {scan_ms:.1} ms", items.len());
    true
}

fn toggle_panel() {
    // Clone the handle out so no borrow is held while AppKit runs.
    let controller = CONTROLLER.with_borrow(|c| c.clone());
    if let Some(controller) = controller {
        controller.toggle();
    }
}

/// The panel hid itself (a click elsewhere, another app taking the keyboard):
/// let go of what is only held while it is open.
pub(crate) fn panel_hidden() {
    let controller = CONTROLLER.with_borrow(|c| c.clone());
    if let Some(controller) = controller {
        controller.dismiss();
    }
}

fn show_mode(mode: hotkey::Mode) {
    let controller = CONTROLLER.with_borrow(|c| c.clone());
    if let Some(controller) = controller {
        controller.show_mode(mode);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if diagnostics(&args) {
        return;
    }

    let mtm = MainThreadMarker::new().expect("must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Menu-bar agent: no Dock icon, never the active app.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let controller = Controller::new(mtm);
    if args.get(1).map(String::as_str) == Some("--snapshot-settings") {
        settings::snapshot(&controller, args.get(2).map(String::as_str).unwrap_or("."));
        return;
    }

    // Hotkey. A failure here must leave the app usable from the menu bar (FR-1).
    // RS_HOTKEY overrides the configuration file, for testing.
    let wanted = std::env::var("RS_HOTKEY").unwrap_or_else(|_| config::load(None).config.hotkey);
    hotkey::init(&wanted, toggle_panel, show_mode);
    for mode in hotkey::Mode::ALL {
        if let Err(e) = hotkey::set_mode(mode, &controller.mode_hotkey(mode)) {
            eprintln!("[rs] {e}");
        }
    }

    // Menu bar item: Open, Settings, Quit. It can be hidden from Settings.
    settings::install(&controller, mtm);
    app.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*controller)));

    // A rate that arrives while the panel is open is shown as a new presentation.
    rates::set_on_update(|| {
        let controller = CONTROLLER.with_borrow(|c| c.clone());
        if let Some(controller) = controller {
            controller.rates_updated();
        }
    });
    CONTROLLER.set(Some(controller));
    watch::start();
    app.run();
}
