//! The global shortcut (FR-1): read from `hotkey` in config.toml, registered
//! through Carbon, mirrored by the key tap, and changeable while running:
//! Settings records a new one and applies it at once.
//!
//! A shortcut is one or more of Command, Option, Control, Shift, plus Space,
//! a letter or a digit: `cmd+space`, `option+space`, `ctrl+shift+k`.

use std::cell::{Cell, RefCell};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;
use objc2_app_kit::NSEventModifierFlags;

use crate::ui::event_log;

/// Keys a shortcut can end in: config name, Carbon code, macOS key code, shown name.
const KEYS: &[(&str, Code, u16, &str)] = &[
    ("space", Code::Space, 49, "Space"),
    ("a", Code::KeyA, 0, "A"), ("b", Code::KeyB, 11, "B"), ("c", Code::KeyC, 8, "C"), ("d", Code::KeyD, 2, "D"),
    ("e", Code::KeyE, 14, "E"), ("f", Code::KeyF, 3, "F"), ("g", Code::KeyG, 5, "G"), ("h", Code::KeyH, 4, "H"),
    ("i", Code::KeyI, 34, "I"), ("j", Code::KeyJ, 38, "J"), ("k", Code::KeyK, 40, "K"), ("l", Code::KeyL, 37, "L"),
    ("m", Code::KeyM, 46, "M"), ("n", Code::KeyN, 45, "N"), ("o", Code::KeyO, 31, "O"), ("p", Code::KeyP, 35, "P"),
    ("q", Code::KeyQ, 12, "Q"), ("r", Code::KeyR, 15, "R"), ("s", Code::KeyS, 1, "S"), ("t", Code::KeyT, 17, "T"),
    ("u", Code::KeyU, 32, "U"), ("v", Code::KeyV, 9, "V"), ("w", Code::KeyW, 13, "W"), ("x", Code::KeyX, 7, "X"),
    ("y", Code::KeyY, 16, "Y"), ("z", Code::KeyZ, 6, "Z"),
    ("0", Code::Digit0, 29, "0"), ("1", Code::Digit1, 18, "1"), ("2", Code::Digit2, 19, "2"), ("3", Code::Digit3, 20, "3"),
    ("4", Code::Digit4, 21, "4"), ("5", Code::Digit5, 23, "5"), ("6", Code::Digit6, 22, "6"), ("7", Code::Digit7, 26, "7"),
    ("8", Code::Digit8, 28, "8"), ("9", Code::Digit9, 25, "9"),
];

/// Modifiers in the order macOS writes them, with config and shown names.
const MODS: &[(Modifiers, NSEventModifierFlags, &str, &str)] = &[
    (Modifiers::CONTROL, NSEventModifierFlags::Control, "ctrl", "Control"),
    (Modifiers::ALT, NSEventModifierFlags::Option, "opt", "Option"),
    (Modifiers::SHIFT, NSEventModifierFlags::Shift, "shift", "Shift"),
    (Modifiers::META, NSEventModifierFlags::Command, "cmd", "Command"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Shortcut {
    pub mods: Modifiers,
    code: Code,
    /// macOS virtual key code, for the key tap.
    pub keycode: u16,
    /// The key alone, as a menu's key equivalent: " " or "k".
    pub key: String,
    /// "Command-Space".
    pub name: String,
    /// "cmd+space", as config.toml stores it.
    pub text: String,
}

impl Shortcut {
    pub fn menu_mods(&self) -> NSEventModifierFlags {
        MODS.iter().filter(|m| self.mods.contains(m.0)).fold(NSEventModifierFlags::empty(), |f, m| f | m.1)
    }
}

fn build(mods: Modifiers, key: &str) -> Option<Shortcut> {
    let &(config_key, code, keycode, shown) = KEYS.iter().find(|k| k.0 == key)?;
    if mods.is_empty() || mods == Modifiers::SHIFT {
        return None; // a bare key or Shift+key would type letters
    }
    let used: Vec<_> = MODS.iter().filter(|m| mods.contains(m.0)).collect();
    let mut name: Vec<&str> = used.iter().map(|m| m.3).collect();
    name.push(shown);
    let mut text: Vec<&str> = used.iter().map(|m| m.2).collect();
    text.push(config_key);
    Some(Shortcut {
        mods,
        code,
        keycode,
        key: if config_key == "space" { " ".into() } else { config_key.into() },
        name: name.join("-"),
        text: text.join("+"),
    })
}

/// `cmd+space`, `option+space`, `ctrl+shift+k`.
pub fn parse(text: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut key = None;
    for part in text.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "cmd" | "command" | "super" | "meta" => mods |= Modifiers::META,
            "opt" | "option" | "alt" => mods |= Modifiers::ALT,
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            other if key.is_none() => key = Some(other.to_string()),
            _ => return None,
        }
    }
    build(mods, &key?)
}

/// A key press recorded in Settings: its modifier flags and key code.
pub fn from_event(flags: NSEventModifierFlags, keycode: u16) -> Result<Shortcut, String> {
    let mut mods = Modifiers::empty();
    for m in MODS {
        if flags.contains(m.1) {
            mods |= m.0;
        }
    }
    let Some(key) = KEYS.iter().find(|k| k.2 == keycode) else {
        return Err("Use Space, a letter or a digit, with Command, Option or Control.".into());
    };
    build(mods, key.0).ok_or_else(|| "Hold Command, Option or Control as well.".into())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Launch,
    Clipboard,
}

struct Registered {
    hotkey: HotKey,
    shortcut: Shortcut,
    slot: Slot,
}

thread_local! {
    static MANAGER: RefCell<Option<GlobalHotKeyManager>> = const { RefCell::new(None) };
    static REGISTERED: RefCell<Vec<Registered>> = const { RefCell::new(Vec::new()) };
    /// Why registration of the launcher shortcut failed, when it did.
    static PROBLEM: RefCell<Option<String>> = const { RefCell::new(None) };
    static ON_LAUNCH: Cell<Option<fn()>> = const { Cell::new(None) };
    static ON_CLIPBOARD: Cell<Option<fn()>> = const { Cell::new(None) };
    static READY: Cell<bool> = const { Cell::new(false) };
    /// Shortcuts lifted while Settings is recording a new one.
    static PAUSED: RefCell<Vec<(Slot, Shortcut)>> = const { RefCell::new(Vec::new()) };
}

/// Register the shortcut from config.toml (or RS_HOTKEY) and route presses
/// to `on_press`. A failure leaves the app usable from the menu bar.
/// `on_clipboard` runs for the separate clipboard shortcut, once one is set.
pub fn init(wanted: &str, on_press: fn(), on_clipboard: fn()) {
    let shortcut = parse(wanted).unwrap_or_else(|| {
        eprintln!("[rs] hotkey {wanted:?} not understood; using option+space");
        parse("option+space").expect("valid")
    });
    ON_LAUNCH.set(Some(on_press));
    ON_CLIPBOARD.set(Some(on_clipboard));
    match GlobalHotKeyManager::new() {
        Ok(manager) => MANAGER.set(Some(manager)),
        Err(e) => {
            PROBLEM.set(Some(format!("Shortcuts unavailable: {e}")));
            eprintln!("[rs] shortcuts unavailable: {e}");
            return;
        }
    }
    // The Carbon handler runs on the main thread's event target.
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        let slot = REGISTERED.with_borrow(|all| {
            all.iter().find(|r| r.hotkey.id() == event.id).map(|r| r.slot)
        });
        let Some(slot) = slot else { return };
        if MainThreadMarker::new().is_none() {
            eprintln!("[rs] hotkey event arrived off the main thread; ignored");
            return;
        }
        match slot {
            Slot::Launch => {
                if crate::keytap::claim() {
                    return; // the key tap already acted on this press
                }
                event_log(if crate::keytap::active() { "hotkey pressed (Carbon; the key tap missed it)" } else { "hotkey pressed" });
                if let Some(open) = ON_LAUNCH.get() {
                    open();
                }
            }
            Slot::Clipboard => {
                event_log("clipboard hotkey pressed");
                if let Some(open) = ON_CLIPBOARD.get() {
                    open();
                }
            }
        }
    }));
    match register(Slot::Launch, &shortcut) {
        Ok(()) => eprintln!("[rs] Hotkey: {}", shortcut.name),
        Err(e) => eprintln!("[rs] {e}"),
    }
    crate::keytap::start(on_press);
    sync_tap();
    READY.set(true);
}

fn register(slot: Slot, shortcut: &Shortcut) -> Result<(), String> {
    let hotkey = HotKey::new(Some(shortcut.mods), shortcut.code);
    let result = MANAGER.with_borrow(|m| match m {
        Some(m) => m.register(hotkey).map_err(|e| e.to_string()),
        None => Err("shortcuts unavailable".into()),
    });
    match result {
        Ok(()) => {
            unregister_slot(slot);
            REGISTERED.with_borrow_mut(|all| {
                all.push(Registered { hotkey, shortcut: shortcut.clone(), slot });
            });
            if slot == Slot::Launch {
                PROBLEM.set(None);
            }
            Ok(())
        }
        Err(e) => {
            let reason = format!("{} is unavailable: {e}", shortcut.name);
            if slot == Slot::Launch {
                PROBLEM.set(Some(reason.clone()));
            }
            Err(reason)
        }
    }
}

fn unregister_slot(slot: Slot) {
    let old = REGISTERED.with_borrow_mut(|all| {
        all.iter().position(|r| r.slot == slot).map(|i| all.remove(i))
    });
    if let Some(old) = old {
        MANAGER.with_borrow(|m| {
            if let Some(m) = m {
                let _ = m.unregister(old.hotkey);
            }
        });
    }
}

fn sync_tap() {
    match current() {
        Some(s) => crate::keytap::watch(s.menu_mods().bits() as u64, s.keycode),
        None => crate::keytap::watch(0, 0),
    }
}

/// Use `shortcut` from now on. On failure the previous one stays.
pub fn change(shortcut: &Shortcut) -> Result<(), String> {
    let previous = current();
    if previous.as_ref() == Some(shortcut) {
        return Ok(());
    }
    unregister_slot(Slot::Launch);
    let result = register(Slot::Launch, shortcut);
    if result.is_err() {
        if let Some(p) = &previous {
            let _ = register(Slot::Launch, p);
        }
    } else {
        event_log(&format!("shortcut is now {}", shortcut.name));
    }
    sync_tap();
    result
}

/// Register the clipboard shortcut from its config text. Empty clears it.
/// Before `init`, this does nothing.
pub fn set_clipboard(text: &str) -> Result<(), String> {
    if !READY.get() {
        return Ok(());
    }
    let text = text.trim();
    if text.is_empty() {
        unregister_slot(Slot::Clipboard);
        return Ok(());
    }
    let shortcut = parse(text).ok_or_else(|| format!("clipboard shortcut {text:?} was not understood"))?;
    if current().as_ref() == Some(&shortcut) {
        return Err(format!("{} already opens the launcher", shortcut.name));
    }
    if REGISTERED.with_borrow(|all| all.iter().any(|r| r.slot == Slot::Clipboard && r.shortcut == shortcut)) {
        return Ok(());
    }
    unregister_slot(Slot::Clipboard);
    register(Slot::Clipboard, &shortcut)
}

/// While Settings records a shortcut, neither shortcut may open the panel.
pub fn pause(paused: bool) {
    if paused {
        let keep = REGISTERED.with_borrow(|all| all.iter().map(|r| (r.slot, r.shortcut.clone())).collect::<Vec<_>>());
        unregister_slot(Slot::Launch);
        unregister_slot(Slot::Clipboard);
        crate::keytap::watch(0, 0);
        PAUSED.set(keep);
    } else {
        for (slot, shortcut) in PAUSED.take() {
            let _ = register(slot, &shortcut);
        }
        sync_tap();
    }
}

pub fn current() -> Option<Shortcut> {
    REGISTERED.with_borrow(|all| all.iter().find(|r| r.slot == Slot::Launch).map(|r| r.shortcut.clone()))
}

/// The shortcut that opens clipboard history, when one is registered.
pub fn clipboard_current() -> Option<Shortcut> {
    REGISTERED.with_borrow(|all| all.iter().find(|r| r.slot == Slot::Clipboard).map(|r| r.shortcut.clone()))
}

/// Why the shortcut is not working, if it is not.
pub fn problem() -> Option<String> {
    PROBLEM.with_borrow(|p| p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_shortcuts() {
        let s = parse("cmd+space").unwrap();
        assert_eq!((s.name.as_str(), s.text.as_str(), s.keycode, s.key.as_str()), ("Command-Space", "cmd+space", 49, " "));
        let s = parse("Shift + Control + k").unwrap();
        assert_eq!((s.name.as_str(), s.text.as_str(), s.keycode), ("Control-Shift-K", "ctrl+shift+k", 40));
        assert_eq!(parse("option+space").unwrap().text, "opt+space");
        for bad in ["space", "shift+a", "cmd", "cmd+f5", "cmd+space+k"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let recorded = from_event(NSEventModifierFlags::Command | NSEventModifierFlags::Option, 49).unwrap();
        assert_eq!(recorded.text, "opt+cmd+space");
        assert!(from_event(NSEventModifierFlags::Command, 122).is_err());
        assert!(from_event(NSEventModifierFlags::Shift, 0).is_err());
    }
}
