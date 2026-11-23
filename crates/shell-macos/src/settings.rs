//! The menu bar item and the Settings window.
//!
//! Settings is a toolbar window in four tabs: General, Appearance, Search,
//! Advanced. Every control saves to config.toml at once (keeping the file's
//! comments) and the launcher reads it straight back, so a change applies
//! without a restart, the shortcut included.
//!
//! The menu bar icon can be hidden. Settings then stays reachable from the
//! launcher (type "settings") and by opening the app again in Finder.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSColor, NSColorWell,
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventMask, NSEventModifierFlags, NSFont,
    NSFontWeightMedium, NSGlassEffectView, NSImage, NSImageSymbolConfiguration, NSImageView,
    NSMenu, NSMenuItem, NSPopUpButton, NSScrollView, NSSlider, NSStatusBar, NSStatusItem,
    NSTabViewController, NSTabViewControllerTabStyle, NSTabViewItem, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTextAlignment, NSTextField, NSVariableStatusItemLength, NSView, NSViewController,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowToolbarStyle, NSWorkspace,
};
use objc2_foundation::{NSInteger, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::config;
use crate::hotkey::Mode;
use crate::theme;
use crate::ui::{Controller, event_log};

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

// ---------------------------------------------------------------- state

/// Controls that show configuration values, kept to refresh them.
#[derive(Default)]
struct Controls {
    menu_icon: Option<Retained<NSButton>>,
    login: Option<Retained<NSButton>>,
    login_note: Option<Retained<NSTextField>>,
    shortcut: Option<Retained<NSButton>>,
    shortcut_note: Option<Retained<NSTextField>>,
    tap_note: Option<Retained<NSTextField>>,
    tap_button: Option<Retained<NSButton>>,
    theme: Option<Retained<NSPopUpButton>>,
    colour: Option<Retained<NSColorWell>>,
    theme_note: Option<Retained<NSTextField>>,
    strength: Option<Retained<NSSlider>>,
    strength_value: Option<Retained<NSTextField>>,
    animations: Option<Retained<NSButton>>,
    preview_icon: Option<Retained<NSImage>>,
    currency: Option<Retained<NSPopUpButton>>,
    ask: Option<Retained<NSPopUpButton>>,
    /// One switch per plugin, in `plugins::all()`'s order.
    plugins: Vec<Retained<NSButton>>,
    learning_note: Option<Retained<NSTextField>>,
    clipboard: Option<Retained<NSButton>>,
    clipboard_note: Option<Retained<NSTextField>>,
    files_key: Option<Retained<NSButton>>,
    files_note: Option<Retained<NSTextField>>,
    search_files: Option<Retained<NSButton>>,
}

thread_local! {
    static CONTROLLER: RefCell<Option<Retained<Controller>>> = const { RefCell::new(None) };
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
    static STATUS: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static OPEN_ITEM: RefCell<Option<Retained<NSMenuItem>>> = const { RefCell::new(None) };
    static PROBLEM_ITEM: RefCell<Option<Retained<NSMenuItem>>> = const { RefCell::new(None) };
    static ICON_WANTED: Cell<bool> = const { Cell::new(true) };
    static WINDOW: RefCell<Option<Retained<SettingsWindow>>> = const { RefCell::new(None) };
    static CONTROLS: RefCell<Controls> = RefCell::new(Controls::default());
    static PREVIEW: RefCell<Option<theme::Tint>> = const { RefCell::new(None) };
    static LISTS: RefCell<Vec<Retained<PairList>>> = const { RefCell::new(Vec::new()) };
    /// The local key monitor while a shortcut is being recorded.
    static RECORDING: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    /// The plugin whose shortcut is being recorded; `None` for the launcher's.
    static RECORD_MODE: Cell<Option<Mode>> = const { Cell::new(None) };
    /// Set while controls are being filled in, so their actions do not save.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
}

fn controller() -> Option<Retained<Controller>> {
    CONTROLLER.with_borrow(|c| c.clone())
}

/// Save with `write`, then have the launcher read the file back.
fn save(what: &str, write: impl FnOnce() -> std::io::Result<()>) -> bool {
    if SYNCING.get() {
        return false;
    }
    match write() {
        Ok(()) => {
            if let Some(c) = controller() {
                c.apply_settings();
            }
            true
        }
        Err(e) => {
            event_log(&format!("could not save {what}: {e}"));
            false
        }
    }
}

// ---------------------------------------------------------------- menu bar

/// Build the menu bar item: Open (showing the shortcut), Settings…, Quit.
pub fn install(controller: &Controller, mtm: MainThreadMarker) {
    CONTROLLER.set(Some(controller.retain()));
    TARGET.set(Some(Target::new(mtm)));
    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status.button(mtm) {
        // The system magnifying glass, drawn as a template so it follows the
        // menu bar's appearance like the icons beside it.
        match symbol("magnifyingglass", 14.0) {
            Some(image) => {
                image.setTemplate(true);
                button.setImage(Some(&image));
                button.setTitle(&ns(""));
            }
            None => button.setTitle(&ns("⌕")),
        }
    }

    let menu = NSMenu::new(mtm);
    let item = |title: &str, action: Option<Sel>, key: &str| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), action, &ns(key))
    };
    let target: &AnyObject = controller;
    let open = item("Open Rusty Spotlight", Some(sel!(showPanel:)), "");
    unsafe { open.setTarget(Some(target)) };
    menu.addItem(&open);
    let problem = item("", None, "");
    problem.setEnabled(false);
    menu.addItem(&problem);
    let settings = item("Settings…", Some(sel!(showSettings:)), ",");
    unsafe { settings.setTarget(Some(target)) };
    menu.addItem(&settings);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item("Quit Rusty Spotlight", Some(sel!(terminate:)), "q"));
    status.setMenu(Some(&menu));
    status.setVisible(ICON_WANTED.get());
    STATUS.set(Some(status));
    OPEN_ITEM.set(Some(open));
    PROBLEM_ITEM.set(Some(problem));
    shortcut_changed();
}

fn symbol(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns("Rusty Spotlight")))?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe { NSFontWeightMedium });
    image.imageWithSymbolConfiguration(&config)
}

/// The shortcut changed or failed: update the menu and the General tab.
fn shortcut_changed() {
    let current = crate::hotkey::current();
    OPEN_ITEM.with_borrow(|i| {
        if let Some(open) = i {
            match &current {
                // Drawn only, as a reminder: the global shortcut does the work.
                Some(s) => {
                    open.setKeyEquivalent(&ns(&s.key));
                    open.setKeyEquivalentModifierMask(s.menu_mods());
                }
                None => open.setKeyEquivalent(&ns("")),
            }
        }
    });
    let problem = crate::hotkey::problem();
    PROBLEM_ITEM.with_borrow(|i| {
        if let Some(item) = i {
            item.setTitle(&ns(problem.as_deref().unwrap_or("")));
            item.setHidden(problem.is_none());
        }
    });
    CONTROLS.with_borrow(|c| {
        if let Some(button) = &c.shortcut {
            button.setTitle(&ns(&current.map(|s| s.name).unwrap_or_else(|| "None".into())));
        }
        for mode in Mode::ALL {
            if let Some(button) = mode_button(c, mode) {
                button.setTitle(&ns(&crate::hotkey::mode_current(mode).map(|s| s.name).unwrap_or_else(|| "None".into())));
            }
        }
    });
}

/// Show or hide the icon. Called whenever the configuration is read.
pub fn set_icon_visible(visible: bool) {
    if ICON_WANTED.replace(visible) != visible {
        event_log(if visible { "menu bar icon shown" } else { "menu bar icon hidden" });
    }
    STATUS.with_borrow(|s| {
        if let Some(s) = s {
            s.setVisible(visible);
        }
    });
}

// ---------------------------------------------------------------- window

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSSettingsWindow"]
    pub struct SettingsWindow;

    impl SettingsWindow {
        // An agent app has no menu bar, so Command-W and Escape are handled here.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.close_shortcut(event) || unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            if RECORDING.with_borrow(Option::is_none) {
                self.close();
            }
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            stop_recording(None);
        }
    }
);

impl SettingsWindow {
    fn close_shortcut(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
        let key = event.charactersIgnoringModifiers().map(|c| c.to_string());
        let close = flags == NSEventModifierFlags::Command && key.as_deref() == Some("w");
        if close {
            self.close();
        }
        close
    }
}

// ---------------------------------------------------------------- actions

define_class!(
    /// Target of every control in the Settings window.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSSettingsTarget"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(menuIconChanged:))]
        fn menu_icon_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            save("menu_bar_icon", || config::set_value("menu_bar_icon", if on { "true" } else { "false" }));
        }

        #[unsafe(method(loginChanged:))]
        fn login_changed(&self, sender: &NSButton) {
            set_login(sender.state() == NSControlStateValueOn);
            sync();
        }

        #[unsafe(method(recordShortcut:))]
        fn record_shortcut(&self, _sender: Option<&AnyObject>) {
            start_recording(None);
        }

        #[unsafe(method(recordClipboard:))]
        fn record_clipboard(&self, _sender: Option<&AnyObject>) {
            start_recording(Some(Mode::Clipboard));
        }

        #[unsafe(method(recordFiles:))]
        fn record_files(&self, _sender: Option<&AnyObject>) {
            start_recording(Some(Mode::Files));
        }

        #[unsafe(method(filesChanged:))]
        fn files_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            save("files ambient", || config::set_table_value("plugins.files", "ambient", if on { "true" } else { "false" }));
        }

        #[unsafe(method(openInputMonitoring:))]
        fn open_input_monitoring(&self, _sender: Option<&AnyObject>) {
            crate::keytap::open_settings();
        }

        #[unsafe(method(themeChanged:))]
        fn theme_changed(&self, sender: &NSPopUpButton) {
            let index = sender.indexOfSelectedItem().max(0) as usize;
            let value = match THEMES.get(index) {
                Some((_, "custom")) => {
                    // Show the colour picker straight away: the choice is the colour.
                    CONTROLS.with_borrow(|c| {
                        if let Some(w) = &c.colour {
                            w.setHidden(false);
                            w.activate(true);
                        }
                    });
                    colour_hex()
                }
                Some((_, value)) => value.to_string(),
                None => return,
            };
            save("theme", || config::set_value("theme", &config::quote(&value)));
            sync();
        }

        #[unsafe(method(colourChanged:))]
        fn colour_changed(&self, _sender: Option<&AnyObject>) {
            let value = colour_hex();
            save("theme", || config::set_value("theme", &config::quote(&value)));
            sync();
        }

        #[unsafe(method(strengthChanged:))]
        fn strength_changed(&self, sender: &NSSlider) {
            let value = format!("{:.2}", sender.doubleValue());
            save("theme_strength", || config::set_value("theme_strength", &config::quote(&value)));
            sync();
        }

        #[unsafe(method(animationsChanged:))]
        fn animations_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            save("animations", || config::set_value("animations", if on { "true" } else { "false" }));
        }

        #[unsafe(method(currencyChanged:))]
        fn currency_changed(&self, sender: &NSPopUpButton) {
            if let Some(title) = sender.titleOfSelectedItem() {
                let code = title.to_string().split_whitespace().next().unwrap_or("INR").to_string();
                save("default_currency", || config::set_value("default_currency", &config::quote(&code)));
            }
        }

        #[unsafe(method(askChanged:))]
        fn ask_changed(&self, sender: &NSPopUpButton) {
            if let Some(title) = sender.titleOfSelectedItem() {
                let tool = title.to_string().to_lowercase();
                save("ask default", || config::set_table_value("ask", "default", &config::quote(&tool)));
            }
        }

        #[unsafe(method(pluginChanged:))]
        fn plugin_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            if let Some(meta) = crate::plugins::all().nth(sender.tag() as usize) {
                let table = format!("plugins.{}", meta.id);
                save(meta.id, || config::set_table_value(&table, "enabled", if on { "true" } else { "false" }));
                event_log(&format!("plugin {}: {}", meta.id, if on { "on" } else { "off" }));
            }
        }

        #[unsafe(method(openConfigFile:))]
        fn open_config_file(&self, _sender: Option<&AnyObject>) {
            if let Some(path) = config::path() {
                NSWorkspace::sharedWorkspace().openURL(&NSURL::fileURLWithPath(&ns(&path.to_string_lossy())));
            }
        }

        #[unsafe(method(showLog:))]
        fn show_log(&self, _sender: Option<&AnyObject>) {
            let path = home().join("Library/Logs/rusty-spotlight/launcher.log");
            let urls = objc2_foundation::NSArray::from_retained_slice(&[NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))]);
            NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
        }

        #[unsafe(method(resetLearning:))]
        fn reset_learning(&self, _sender: Option<&AnyObject>) {
            let done = controller().map(|c| c.reset_learning());
            let text = match done {
                Some(Ok(())) => "Cleared. Results are ranked by name until you use them again.".to_string(),
                Some(Err(e)) => format!("Could not clear: {e}"),
                None => String::new(),
            };
            CONTROLS.with_borrow(|c| {
                if let Some(note) = &c.learning_note {
                    note.setStringValue(&ns(&text));
                }
            });
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

fn target() -> Retained<AnyObject> {
    TARGET.with_borrow(|t| Retained::into_super(Retained::into_super(t.clone().expect("installed"))))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

// ---------------------------------------------------------------- shortcut recording

fn mode_button(c: &Controls, mode: Mode) -> Option<&Retained<NSButton>> {
    match mode {
        Mode::Clipboard => c.clipboard.as_ref(),
        Mode::Files => c.files_key.as_ref(),
    }
}

fn mode_hint(mode: Mode) -> &'static str {
    match mode {
        Mode::Clipboard => CLIP_HINT,
        Mode::Files => FILES_HINT,
    }
}

fn start_recording(mode: Option<Mode>) {
    if RECORDING.with_borrow(Option::is_some) {
        return;
    }
    RECORD_MODE.set(mode);
    // Neither shortcut may open the panel while one is being replaced.
    crate::hotkey::pause(true);
    let hint = "Press the new shortcut, or Escape to keep the current one.";
    match mode {
        Some(mode) => note_mode(mode, hint, false),
        None => note_shortcut(hint, false),
    }
    CONTROLS.with_borrow(|c| {
        let button = match mode {
            Some(mode) => mode_button(c, mode),
            None => c.shortcut.as_ref(),
        };
        if let Some(button) = button {
            button.setTitle(&ns("Type shortcut…"));
        }
    });
    let block = block2::RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        let event = unsafe { event.as_ref() };
        let keycode = event.keyCode();
        if keycode == 53 {
            stop_recording(None);
            return std::ptr::null_mut();
        }
        let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
        match crate::hotkey::from_event(flags, keycode) {
            Ok(shortcut) => stop_recording(Some(shortcut)),
            Err(why) => match RECORD_MODE.get() {
                Some(mode) => note_mode(mode, &why, true),
                None => note_shortcut(&why, true),
            },
        }
        std::ptr::null_mut()
    });
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) };
    RECORDING.set(monitor);
}

/// End recording: apply `shortcut`, or with `None` keep the current one.
fn stop_recording(shortcut: Option<crate::hotkey::Shortcut>) {
    let Some(monitor) = RECORDING.take() else { return };
    unsafe { NSEvent::removeMonitor(&monitor) };
    let mode = RECORD_MODE.replace(None);
    crate::hotkey::pause(false);
    if let Some(mode) = mode {
        match shortcut {
            None => note_mode(mode, mode_hint(mode), false),
            Some(s) => match crate::hotkey::set_mode(mode, &s.text) {
                Ok(()) => {
                    let saved = config::set_table_value(&format!("plugins.{}", mode.id()), "hotkey", &config::quote(&s.text));
                    match saved {
                        Ok(()) => note_mode(mode, &format!("Saved. {} opens {}.", s.name, mode.opens()), false),
                        Err(e) => note_mode(mode, &format!("Working, but not saved: {e}"), true),
                    }
                }
                Err(e) => note_mode(mode, &format!("{e}. The previous shortcut is kept."), true),
            },
        }
    } else {
        match shortcut {
            None => note_shortcut(SHORTCUT_HINT, false),
            Some(s) => match crate::hotkey::change(&s) {
                Ok(()) => {
                    let saved = config::set_value("hotkey", &config::quote(&s.text));
                    match saved {
                        Ok(()) => note_shortcut(&format!("Saved. {} opens Rusty Spotlight now.", s.name), false),
                        Err(e) => note_shortcut(&format!("Working, but not saved: {e}"), true),
                    }
                }
                Err(e) => note_shortcut(&format!("{e}. The previous shortcut is kept."), true),
            },
        }
    }
    shortcut_changed();
}

const SHORTCUT_HINT: &str = "Click to record a new shortcut. For Command-Space, first turn off Spotlight’s own in System Settings › Keyboard › Keyboard Shortcuts.";

const CLIP_HINT: &str = "Needs the Clipboard history plugin. Click to record a shortcut that opens it; none is set until you do. For Shift-Command-Space, untick Ask Siri about active window under Keyboard Shortcuts › Screenshots.";

const FILES_HINT: &str = "Needs the Files plugin. Click to record a shortcut that opens straight into file search, with no “file” to type first.";

fn note_mode(mode: Mode, text: &str, warning: bool) {
    CONTROLS.with_borrow(|c| {
        let note = match mode {
            Mode::Clipboard => &c.clipboard_note,
            Mode::Files => &c.files_note,
        };
        if let Some(note) = note {
            note.setStringValue(&ns(text));
            let colour = if warning { NSColor::systemRedColor() } else { NSColor::secondaryLabelColor() };
            note.setTextColor(Some(&*colour));
        }
    });
}

fn note_shortcut(text: &str, warning: bool) {
    CONTROLS.with_borrow(|c| {
        if let Some(note) = &c.shortcut_note {
            note.setStringValue(&ns(text));
            let colour = if warning { NSColor::systemRedColor() } else { NSColor::secondaryLabelColor() };
            note.setTextColor(Some(&*colour));
        }
    });
}

// ---------------------------------------------------------------- start at login

const AGENT: &str = "local.rusty-spotlight";

unsafe extern "C" {
    fn getuid() -> u32;
}

fn agent_plist() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{AGENT}.plist"))
}

/// Whether the login item (scripts/install.sh's LaunchAgent) is on. `None`
/// when there is no LaunchAgent, e.g. when running from a build folder.
fn login_enabled() -> Option<bool> {
    if !agent_plist().exists() {
        return None;
    }
    let out = std::process::Command::new("/bin/launchctl")
        .args(["print-disabled", &format!("gui/{}", unsafe { getuid() })])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains(&format!("\"{AGENT}\"")));
    // Listed as "=> disabled" (or "=> true" on older systems) when switched off.
    Some(!line.is_some_and(|l| l.contains("disabled") || l.contains("true")))
}

fn set_login(on: bool) {
    let verb = if on { "enable" } else { "disable" };
    let ok = std::process::Command::new("/bin/launchctl")
        .args([verb, &format!("gui/{}/{AGENT}", unsafe { getuid() })])
        .status()
        .is_ok_and(|s| s.success());
    event_log(&format!("open at login: {}", if !ok { "could not change" } else if on { "on" } else { "off" }));
}

// ---------------------------------------------------------------- appearance helpers

/// Choices in the Glass colour menu and their `theme` values.
const THEMES: [(&str, &str); 5] = [
    ("Follow selection", "selection"),
    ("Aurora", "aurora"),
    ("Accent colour", "accent"),
    ("Custom colour", "custom"),
    ("None", "none"),
];

/// What each choice in THEMES does, shown under the menu.
const THEME_NOTES: [&str; 5] = [
    "Takes the selected app’s colour, or one per kind of result, and fades as you move.",
    "Drifts slowly through blues, violets and pinks while the panel is open.",
    "Your accent colour from System Settings › Appearance.",
    "One colour of your choice. Click the swatch to change it.",
    "Plain glass, as Spotlight has it.",
];

const CURRENCIES: &[&str] = &[
    "INR  Indian rupee", "USD  US dollar", "AED  UAE dirham", "AUD  Australian dollar", "NPR  Nepalese rupee",
    "EUR  Euro", "GBP  British pound", "JPY  Japanese yen", "CAD  Canadian dollar", "SGD  Singapore dollar",
    "CHF  Swiss franc", "CNY  Chinese yuan", "SAR  Saudi riyal", "QAR  Qatari riyal", "MYR  Malaysian ringgit",
    "THB  Thai baht", "BDT  Bangladeshi taka", "PKR  Pakistani rupee", "LKR  Sri Lankan rupee",
];

fn colour_hex() -> String {
    CONTROLS.with_borrow(|c| {
        let colour = c.colour.as_ref().map(|w| w.color());
        let rgb = colour
            .and_then(|c| c.colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace()))
            .map(|c| (c.redComponent(), c.greenComponent(), c.blueComponent()))
            .unwrap_or((0.49, 0.36, 1.0));
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", byte(rgb.0), byte(rgb.1), byte(rgb.2))
    })
}

fn strength_word(value: f64) -> &'static str {
    match value {
        v if v < 0.35 => "Subtle",
        v if v < 0.62 => "Mild",
        v if v < 0.87 => "Medium",
        _ => "Strong",
    }
}

// ---------------------------------------------------------------- refresh

/// The configuration was read: show it in every control.
pub fn config_changed(config: &config::Config) {
    set_icon_visible(config.menu_bar_icon);
    sync_from(config);
}

/// Show the Input Monitoring state. Called when the key tap starts.
pub fn refresh() {
    let on = crate::keytap::active();
    CONTROLS.with_borrow(|c| {
        if let Some(note) = &c.tap_note {
            note.setStringValue(&ns(if on {
                "On: caught ahead of other apps’ keyboard tools, so they can’t delay or swallow it."
            } else {
                "Allow Rusty Spotlight in Input Monitoring so other apps’ keyboard tools can’t delay or swallow the shortcut."
            }));
        }
        if let Some(b) = &c.tap_button {
            b.setHidden(on);
        }
    });
}

fn sync() {
    if let Some(c) = controller() {
        sync_from(&c.config());
    }
}

fn sync_from(config: &config::Config) {
    SYNCING.set(true);
    CONTROLS.with_borrow(|c| {
        let state = |on: bool| if on { NSControlStateValueOn } else { NSControlStateValueOff };
        if let Some(b) = &c.menu_icon {
            b.setState(state(config.menu_bar_icon));
        }
        if let Some(b) = &c.animations {
            b.setState(state(config.animations));
        }
        if let Some(b) = &c.search_files {
            b.setState(state(config.files_ambient()));
            b.setEnabled(crate::plugins::is_on(config, "files"));
        }
        for mode in Mode::ALL {
            if let Some(b) = mode_button(c, mode) {
                b.setEnabled(crate::plugins::is_on(config, mode.id()));
                let name = crate::hotkey::mode_current(mode).map(|s| s.name).unwrap_or_else(|| "None".into());
                b.setTitle(&ns(&name));
            }
        }
        let login = login_enabled();
        if let Some(b) = &c.login {
            b.setEnabled(login.is_some());
            b.setState(state(login.unwrap_or(false)));
        }
        if let Some(n) = &c.login_note {
            n.setHidden(login.is_some());
        }
        let parsed = theme::parse(&config.theme);
        let index = match &parsed {
            theme::Theme::Selection => 0,
            theme::Theme::Aurora => 1,
            theme::Theme::Accent => 2,
            theme::Theme::Fixed(_) => 3,
            theme::Theme::None => 4,
        };
        if let Some(p) = &c.theme {
            p.selectItemAtIndex(index);
        }
        if let (Some(w), theme::Theme::Fixed((r, g, b))) = (&c.colour, &parsed) {
            w.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(*r, *g, *b, 1.0));
        }
        if let Some(w) = &c.colour {
            w.setHidden(index != 3);
        }
        if let Some(n) = &c.theme_note {
            n.setStringValue(&ns(THEME_NOTES[index as usize]));
        }
        let strength = theme::strength(&config.theme_strength);
        if let Some(s) = &c.strength {
            s.setDoubleValue(strength);
            s.setEnabled(index != 4);
        }
        if let Some(v) = &c.strength_value {
            v.setStringValue(&ns(strength_word(strength)));
        }
        for (button, meta) in c.plugins.iter().zip(crate::plugins::all()) {
            button.setState(state(crate::plugins::is_on(config, meta.id)));
        }
        // A plugin's options stay visible but cannot be changed while it is off.
        if let Some(p) = &c.currency {
            p.setEnabled(crate::plugins::is_on(config, "currency"));
        }
        if let Some(p) = &c.ask {
            p.setEnabled(crate::plugins::is_on(config, "ask"));
        }
        if let Some(p) = &c.currency {
            if let Some(i) = CURRENCIES.iter().position(|t| t.starts_with(&config.default_currency)) {
                p.selectItemAtIndex(i as NSInteger);
            }
        }
        if let Some(p) = &c.ask {
            p.removeAllItems();
            for tool in config.ask.tools.keys() {
                p.addItemWithTitle(&ns(&crate::ui::display_name(tool)));
            }
            p.selectItemWithTitle(&ns(&crate::ui::display_name(&config.ask.default)));
        }
        PREVIEW.with_borrow(|t| {
            if let Some(t) = t {
                t.set_theme(parsed.clone(), strength);
                match &c.preview_icon {
                    Some(icon) => t.update(&theme::Subject::App(icon, "/Applications/Safari.app")),
                    None => t.update(&theme::Subject::Link),
                }
            }
        });
    });
    LISTS.with_borrow(|lists| {
        for list in lists {
            list.load(config);
        }
    });
    SYNCING.set(false);
    refresh();
    shortcut_changed();
}

// ---------------------------------------------------------------- layout

const WIDTH: f64 = 620.0;
/// Labels end here; controls start a little after.
const LABEL_W: f64 = 170.0;
const CONTROL_X: f64 = LABEL_W + 14.0;
const TOP: f64 = 26.0;

fn label(text: &str, size: f64, secondary: bool, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    if secondary {
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    l.setSelectable(false);
    l
}

/// Lays a tab out top-down: a right-aligned label, its control, and a note.
struct Form {
    view: Retained<NSView>,
    height: f64,
    y: f64,
    mtm: MainThreadMarker,
}

impl Form {
    fn new(height: f64, mtm: MainThreadMarker) -> Form {
        let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)));
        Form { view, height, y: TOP, mtm }
    }

    fn place(&self, v: &NSView, x: f64, y: f64, w: f64, h: f64) {
        v.setFrame(NSRect::new(NSPoint::new(x, self.height - y - h), NSSize::new(w, h)));
        self.view.addSubview(v);
    }

    /// The row's label, baseline-aligned with a control `h` tall.
    fn caption(&self, text: &str, h: f64) {
        if text.is_empty() {
            return;
        }
        let l = label(text, 13.0, false, self.mtm);
        l.setAlignment(NSTextAlignment::Right);
        self.place(&l, 0.0, self.y + (h - 17.0) / 2.0, LABEL_W, 17.0);
    }

    /// A control `w`×`h` beside `caption`; returns its x for neighbours.
    fn row(&mut self, caption: &str, control: &NSView, w: f64, h: f64) {
        self.caption(caption, h);
        self.place(control, CONTROL_X, self.y, w, h);
        self.y += h;
    }

    fn note(&mut self, text: &str) -> Retained<NSTextField> {
        let n = label(text, 11.0, true, self.mtm);
        let w = WIDTH - CONTROL_X - 28.0;
        let lines = (text.chars().count() as f64 * 6.2 / w).ceil().max(1.0);
        self.y += 3.0;
        self.place(&n, CONTROL_X + 2.0, self.y, w, 15.0 * lines);
        self.y += 15.0 * lines;
        n
    }

    fn gap(&mut self, h: f64) {
        self.y += h;
    }

    fn separator(&mut self) {
        self.y += 14.0;
        let line = NSBox::new(self.mtm);
        line.setBoxType(NSBoxType::Separator);
        self.place(&line, 28.0, self.y, WIDTH - 56.0, 1.0);
        self.y += 15.0;
    }

    fn controller(self, title: &str) -> Retained<NSViewController> {
        let vc = NSViewController::new(self.mtm);
        vc.setView(&self.view);
        vc.setTitle(Some(&ns(title)));
        vc.setPreferredContentSize(NSSize::new(WIDTH, self.height));
        vc
    }
}

fn checkbox(title: &str, action: Sel, mtm: MainThreadMarker) -> Retained<NSButton> {
    let t = target();
    unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), Some(&t), Some(action), mtm) }
}

fn button(title: &str, action: Sel, mtm: MainThreadMarker) -> Retained<NSButton> {
    let t = target();
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(&t), Some(action), mtm) };
    b.sizeToFit();
    b
}

fn popup(items: &[&str], action: Sel, mtm: MainThreadMarker) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    for item in items {
        p.addItemWithTitle(&ns(item));
    }
    let t = target();
    unsafe {
        p.setTarget(Some(&t));
        p.setAction(Some(action));
    }
    p
}

// ---------------------------------------------------------------- tabs

fn general(mtm: MainThreadMarker) -> Retained<NSViewController> {
    let mut f = Form::new(590.0, mtm);
    let shortcut = button("Command-Space", sel!(recordShortcut:), mtm);
    shortcut.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightMedium })));
    f.row("Shortcut:", &shortcut, 190.0, 28.0);
    let shortcut_note = f.note(SHORTCUT_HINT);
    f.gap(10.0);
    let tap_note = label("", 11.0, true, mtm);
    f.caption("Reliability:", 17.0);
    f.place(&tap_note, CONTROL_X + 2.0, f.y + 1.0, WIDTH - CONTROL_X - 28.0, 30.0);
    f.y += 32.0;
    let tap_button = button("Open Input Monitoring…", sel!(openInputMonitoring:), mtm);
    let w = tap_button.frame().size.width;
    f.place(&tap_button, CONTROL_X - 6.0, f.y, w, 30.0);
    f.y += 30.0;
    f.separator();

    let login = checkbox("Open Rusty Spotlight when you log in", sel!(loginChanged:), mtm);
    f.row("Start:", &login, 330.0, 20.0);
    let login_note = f.note("Install with scripts/install.sh to open at login.");
    f.gap(14.0);
    let menu_icon = checkbox("Show icon in menu bar", sel!(menuIconChanged:), mtm);
    f.row("Menu bar:", &menu_icon, 330.0, 20.0);
    f.note("When it is hidden, type “settings” in Rusty Spotlight, or open the app again in Finder, to come back here.");
    f.gap(14.0);

    let clipboard = button("None", sel!(recordClipboard:), mtm);
    clipboard.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightMedium })));
    f.row("Clipboard shortcut:", &clipboard, 190.0, 28.0);
    let clipboard_note = f.note(CLIP_HINT);
    f.gap(10.0);
    let files_key = button("None", sel!(recordFiles:), mtm);
    files_key.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightMedium })));
    f.row("Files shortcut:", &files_key, 190.0, 28.0);
    let files_note = f.note(FILES_HINT);
    f.gap(10.0);
    let search_files = checkbox("List file names under apps as you type", sel!(filesChanged:), mtm);
    f.row("Files:", &search_files, 330.0, 20.0);
    f.note("Needs the Files plugin. “file notes” and “folder notes” search without this; with it, names are also listed after two letters.");

    CONTROLS.with_borrow_mut(|c| {
        c.shortcut = Some(shortcut);
        c.shortcut_note = Some(shortcut_note);
        c.tap_note = Some(tap_note);
        c.tap_button = Some(tap_button);
        c.login = Some(login);
        c.login_note = Some(login_note);
        c.menu_icon = Some(menu_icon);
        c.clipboard = Some(clipboard);
        c.clipboard_note = Some(clipboard_note);
        c.files_key = Some(files_key);
        c.files_note = Some(files_note);
        c.search_files = Some(search_files);
    });
    f.controller("General")
}

fn appearance(mtm: MainThreadMarker) -> Retained<NSViewController> {
    let mut f = Form::new(372.0, mtm);

    // A small panel on glass, tinted as the launcher will be.
    let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), NSRect::ZERO);
    glass.setCornerRadius(26.0);
    let inside = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 56.0)));
    if let Some(tint) = theme::Tint::new(inside.bounds(), mtm) {
        inside.addSubview(tint.view());
        PREVIEW.set(Some(tint));
    }
    if let Some(glyph) = symbol("magnifyingglass", 18.0) {
        let v = NSImageView::imageViewWithImage(&glyph, mtm);
        v.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        v.setFrame(NSRect::new(NSPoint::new(18.0, 16.0), NSSize::new(24.0, 24.0)));
        inside.addSubview(&v);
    }
    let text = label("Safari", 20.0, false, mtm);
    text.setFrame(NSRect::new(NSPoint::new(52.0, 15.0), NSSize::new(200.0, 26.0)));
    inside.addSubview(&text);
    let safari = NSWorkspace::sharedWorkspace().iconForFile(&ns("/Applications/Safari.app"));
    let icon = NSImageView::imageViewWithImage(&safari, mtm);
    icon.setFrame(NSRect::new(NSPoint::new(352.0, 12.0), NSSize::new(32.0, 32.0)));
    inside.addSubview(&icon);
    glass.setContentView(Some(&inside));
    f.place(&glass, (WIDTH - 400.0) / 2.0, f.y, 400.0, 56.0);
    f.y += 56.0;
    let caption = label("Preview, with Safari selected", 11.0, true, mtm);
    caption.setAlignment(NSTextAlignment::Center);
    f.place(&caption, 0.0, f.y + 6.0, WIDTH, 15.0);
    f.y += 21.0;
    f.separator();

    let titles: Vec<&str> = THEMES.iter().map(|t| t.0).collect();
    let theme_menu = popup(&titles, sel!(themeChanged:), mtm);
    f.caption("Glass colour:", 26.0);
    f.place(&theme_menu, CONTROL_X, f.y, 190.0, 26.0);
    let colour = NSColorWell::initWithFrame(NSColorWell::alloc(mtm), NSRect::ZERO);
    let t = target();
    unsafe {
        colour.setTarget(Some(&t));
        colour.setAction(Some(sel!(colourChanged:)));
    }
    // A soft violet until one is picked, never the well's default white.
    colour.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(0.49, 0.36, 1.0, 1.0));
    f.place(&colour, CONTROL_X + 200.0, f.y + 1.0, 44.0, 24.0);
    f.y += 26.0;
    let theme_note = f.note(THEME_NOTES[0]);
    f.gap(12.0);

    let strength = unsafe { NSSlider::sliderWithValue_minValue_maxValue_target_action(0.5, 0.15, 1.0, Some(&t), Some(sel!(strengthChanged:)), mtm) };
    strength.setNumberOfTickMarks(4);
    f.caption("Strength:", 24.0);
    f.place(&strength, CONTROL_X, f.y, 240.0, 24.0);
    let value = label("Mild", 13.0, true, mtm);
    f.place(&value, CONTROL_X + 252.0, f.y + 3.0, 80.0, 18.0);
    f.y += 24.0;
    f.separator();

    let animations = checkbox("Animate opening and resizing", sel!(animationsChanged:), mtm);
    f.row("Motion:", &animations, 330.0, 20.0);
    f.note("Reduce Motion in System Settings › Accessibility also turns this off.");

    CONTROLS.with_borrow_mut(|c| {
        c.theme = Some(theme_menu);
        c.colour = Some(colour);
        c.theme_note = Some(theme_note);
        c.strength = Some(strength);
        c.strength_value = Some(value);
        c.animations = Some(animations);
        c.preview_icon = Some(safari);
    });
    f.controller("Appearance")
}

fn search(mtm: MainThreadMarker) -> Retained<NSViewController> {
    let mut f = Form::new(560.0, mtm);
    let currency = popup(CURRENCIES, sel!(currencyChanged:), mtm);
    f.row("Default currency:", &currency, 230.0, 26.0);
    f.note("Used when you name one currency, e.g. “100 usd”.");
    f.gap(12.0);
    let ask = popup(&[], sel!(askChanged:), mtm);
    f.row("Ask with:", &ask, 150.0, 26.0);
    f.note("“? question” goes to this tool; “?codex question” picks another.");
    f.separator();

    let aliases = PairList::new("aliases", mtm);
    f.caption("Aliases:", 17.0);
    aliases.build(&f, ("Type", "To open"), 130.0);
    f.y += 130.0 + 28.0;
    f.note("Typing the alias puts that app first, e.g. “ff” for Firefox.");
    f.gap(12.0);
    let links = PairList::new("links", mtm);
    f.caption("Website keywords:", 17.0);
    links.build(&f, ("Keyword", "Address"), 130.0);
    f.y += 130.0 + 28.0;
    f.note("“{query}” is replaced by what you type after the keyword: “gh rust-lang/rust”.");

    CONTROLS.with_borrow_mut(|c| {
        c.currency = Some(currency);
        c.ask = Some(ask);
    });
    LISTS.set(vec![aliases, links]);
    f.controller("Search")
}

/// One switch per feature, each with a line saying what it does when on.
fn plugins(mtm: MainThreadMarker) -> Retained<NSViewController> {
    const LEFT: f64 = 44.0;
    let width = WIDTH - LEFT * 2.0;
    let metas: Vec<_> = crate::plugins::all().collect();
    let mut f = Form::new(TOP + 44.0 + metas.len() as f64 * 62.0, mtm);
    let intro = label("Switch off what you don't use. A plugin that is off does no work at all: nothing is fetched, watched or kept in memory.", 12.0, true, mtm);
    f.place(&intro, LEFT, f.y, width, 32.0);
    f.y += 44.0;
    let mut buttons = Vec::new();
    for (i, meta) in metas.iter().enumerate() {
        let b = checkbox(meta.name, sel!(pluginChanged:), mtm);
        b.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightMedium })));
        b.setTag(i as NSInteger);
        f.place(&b, LEFT, f.y, width, 20.0);
        let blurb = label(meta.blurb, 11.0, true, mtm);
        f.place(&blurb, LEFT + 20.0, f.y + 22.0, width - 20.0, 30.0);
        f.y += 62.0;
        buttons.push(b);
    }
    CONTROLS.with_borrow_mut(|c| c.plugins = buttons);
    f.controller("Plugins")
}

fn advanced(mtm: MainThreadMarker) -> Retained<NSViewController> {
    let mut f = Form::new(300.0, mtm);
    let open = button("Open Configuration File", sel!(openConfigFile:), mtm);
    let w = open.frame().size.width;
    f.row("Configuration:", &open, w, 30.0);
    let shown = config::path().map(|p| p.to_string_lossy().replace(&home().to_string_lossy().to_string(), "~")).unwrap_or_default();
    f.note(&format!("{shown}. Everything here is saved there; hand edits apply the next time the panel opens."));
    f.gap(12.0);
    let log = button("Show Log in Finder", sel!(showLog:), mtm);
    let w = log.frame().size.width;
    f.row("Log:", &log, w, 30.0);
    f.note("Shortcut presses, shows and hides, and anything that went wrong.");
    f.gap(12.0);
    let reset = button("Reset Learned Ranking", sel!(resetLearning:), mtm);
    let w = reset.frame().size.width;
    f.row("Learning:", &reset, w, 30.0);
    let learning_note = f.note("Results you choose often rise to the top. This forgets that.");
    f.separator();
    let version = label(&format!("Rusty Spotlight {}", env!("CARGO_PKG_VERSION")), 13.0, true, mtm);
    f.row("Version:", &version, 300.0, 17.0);
    CONTROLS.with_borrow_mut(|c| c.learning_note = Some(learning_note));
    f.controller("Advanced")
}

fn build(mtm: MainThreadMarker) -> Retained<SettingsWindow> {
    let window: Retained<SettingsWindow> = unsafe {
        msg_send![
            SettingsWindow::alloc(mtm),
            initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 372.0)),
            styleMask: NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    unsafe { window.setReleasedWhenClosed(false) };
    // Open on the Space in use, even over a full-screen app.
    window.setCollectionBehavior(NSWindowCollectionBehavior::MoveToActiveSpace | NSWindowCollectionBehavior::FullScreenAuxiliary);
    window.setToolbarStyle(NSWindowToolbarStyle::Preference);

    let tabs = NSTabViewController::new(mtm);
    tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
    for (vc, image) in [
        (general(mtm), "gearshape"),
        (appearance(mtm), "paintpalette"),
        (search(mtm), "magnifyingglass"),
        (plugins(mtm), "puzzlepiece.extension"),
        (advanced(mtm), "wrench.and.screwdriver"),
    ] {
        let item = NSTabViewItem::tabViewItemWithViewController(&vc);
        item.setImage(NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(image), None).as_deref());
        tabs.addTabViewItem(&item);
    }
    window.setContentViewController(Some(&tabs));
    window.center();
    window
}

/// Bring the Settings window forward, building it on first use.
pub fn show(controller: &Controller) {
    let mtm = controller.mtm();
    let window = WINDOW.with_borrow(|w| w.clone()).unwrap_or_else(|| {
        let w = build(mtm);
        WINDOW.set(Some(w.clone()));
        w
    });
    sync_from(&controller.config());
    // An agent app has to be made active for its window to take the keyboard.
    NSApplication::sharedApplication(mtm).activate();
    window.makeKeyAndOrderFront(None);
}

// ---------------------------------------------------------------- key/value lists

#[derive(Default)]
struct PairIvars {
    /// `aliases` or `links`.
    table: RefCell<String>,
    rows: RefCell<Vec<(String, String)>>,
    view: RefCell<Option<Retained<NSTableView>>>,
    note: RefCell<Option<Retained<NSTextField>>>,
}

define_class!(
    /// An editable two-column list saved as one table of config.toml.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSPairList"]
    #[ivars = PairIvars]
    struct PairList;

    unsafe impl NSObjectProtocol for PairList {}

    unsafe impl NSTableViewDataSource for PairList {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }

        #[unsafe(method_id(tableView:objectValueForTableColumn:row:))]
        fn value(&self, _table: &NSTableView, column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<AnyObject>> {
            self.value_at(column, row)
        }

        #[unsafe(method(tableView:setObjectValue:forTableColumn:row:))]
        fn set_value(&self, _table: &NSTableView, value: Option<&AnyObject>, column: Option<&NSTableColumn>, row: NSInteger) {
            self.edited(value, column, row);
        }
    }

    impl PairList {
        #[unsafe(method(addRow:))]
        fn add_row(&self, _sender: Option<&AnyObject>) {
            self.add();
        }

        #[unsafe(method(removeRow:))]
        fn remove_row(&self, _sender: Option<&AnyObject>) {
            self.remove();
        }
    }
);

impl PairList {
    fn new(table: &str, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PairIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        *this.ivars().table.borrow_mut() = table.to_string();
        this
    }

    fn build(&self, f: &Form, titles: (&str, &str), height: f64) {
        let mtm = f.mtm;
        let w = WIDTH - CONTROL_X - 28.0;
        let table = NSTableView::new(mtm);
        for (id, title, width) in [("key", titles.0, 110.0), ("value", titles.1, w - 130.0)] {
            let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(id));
            column.setTitle(&ns(title));
            column.setWidth(width);
            column.setEditable(true);
            table.addTableColumn(&column);
        }
        table.setUsesAlternatingRowBackgroundColors(true);
        table.setColumnAutoresizingStyle(objc2_app_kit::NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
        unsafe {
            let _: () = msg_send![&*table, setDataSource: self];
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        f.place(&scroll, CONTROL_X, f.y, w, height);

        // + and − under the list, as macOS lists have them.
        let me: &AnyObject = self;
        for (i, (image, action)) in [("plus", sel!(addRow:)), ("minus", sel!(removeRow:))].into_iter().enumerate() {
            let b = NSButton::new(mtm);
            b.setBezelStyle(NSBezelStyle::SmallSquare);
            b.setImage(NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(image), None).as_deref());
            unsafe {
                b.setTarget(Some(me));
                b.setAction(Some(action));
            }
            f.place(&b, CONTROL_X + i as f64 * 25.0, f.y + height + 2.0, 24.0, 22.0);
        }
        let note = label("", 11.0, false, mtm);
        note.setTextColor(Some(&NSColor::systemRedColor()));
        f.place(&note, CONTROL_X + 60.0, f.y + height + 5.0, w - 60.0, 15.0);
        *self.ivars().view.borrow_mut() = Some(table);
        *self.ivars().note.borrow_mut() = Some(note);
    }

    /// Fill from the configuration, unless a half-typed row is being edited.
    fn load(&self, config: &config::Config) {
        let table = self.ivars().table.borrow().clone();
        let source = if table == "aliases" { &config.aliases } else { &config.links };
        let fresh: Vec<(String, String)> = source.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let pending = self.ivars().rows.borrow().iter().any(|(k, v)| k.is_empty() || v.is_empty());
        if pending {
            return;
        }
        *self.ivars().rows.borrow_mut() = fresh;
        if let Some(view) = self.ivars().view.borrow().as_ref() {
            view.reloadData();
        }
    }

    fn value_at(&self, column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<AnyObject>> {
        let rows = self.ivars().rows.borrow();
        let (k, v) = rows.get(row as usize)?;
        let text = if column.is_some_and(|c| c.identifier().to_string() == "key") { k } else { v };
        Some(Retained::into_super(Retained::into_super(ns(text))))
    }

    fn edited(&self, value: Option<&AnyObject>, column: Option<&NSTableColumn>, row: NSInteger) {
        let text = value.map(|v| unsafe { msg_send![v, description] }).map(|d: Retained<NSString>| d.to_string()).unwrap_or_default();
        let text = text.trim().to_string();
        let key = column.is_some_and(|c| c.identifier().to_string() == "key");
        if let Some(r) = self.ivars().rows.borrow_mut().get_mut(row as usize) {
            if key { r.0 = text } else { r.1 = text }
        }
        self.store();
    }

    fn add(&self) {
        let n = {
            let mut rows = self.ivars().rows.borrow_mut();
            rows.push((String::new(), String::new()));
            rows.len() as NSInteger
        };
        if let Some(view) = self.ivars().view.borrow().as_ref() {
            view.reloadData();
            view.selectRowIndexes_byExtendingSelection(&objc2_foundation::NSIndexSet::indexSetWithIndex((n - 1) as usize), false);
            view.editColumn_row_withEvent_select(0, n - 1, None, true);
        }
    }

    fn remove(&self) {
        let selected = self.ivars().view.borrow().as_ref().map(|v| v.selectedRow()).unwrap_or(-1);
        if selected < 0 {
            return;
        }
        {
            let mut rows = self.ivars().rows.borrow_mut();
            if (selected as usize) < rows.len() {
                rows.remove(selected as usize);
            }
        }
        if let Some(view) = self.ivars().view.borrow().as_ref() {
            view.reloadData();
        }
        self.store();
    }

    /// Save the complete rows; a half-filled one waits for its other half.
    fn store(&self) {
        let table = self.ivars().table.borrow().clone();
        let rows: Vec<(String, String)> = self.ivars().rows.borrow().iter().filter(|(k, v)| !k.is_empty() && !v.is_empty()).cloned().collect();
        let mut problem = None;
        let mut keys = std::collections::HashSet::new();
        for (k, v) in &rows {
            if !keys.insert(k.to_lowercase()) {
                problem = Some(format!("“{k}” is listed twice; only one is kept."));
            } else if table == "links" && !(v.starts_with("https://") || v.starts_with("http://")) {
                problem = Some(format!("“{k}” needs an address starting with https://"));
            }
        }
        let unique: Vec<(String, String)> = {
            let mut seen = std::collections::HashSet::new();
            rows.into_iter().filter(|(k, _)| seen.insert(k.to_lowercase())).collect()
        };
        if !save(&table, || config::set_table(&table, &unique)) && problem.is_none() && !SYNCING.get() {
            problem = Some("Could not save; see the log.".into());
        }
        if let Some(note) = self.ivars().note.borrow().as_ref() {
            note.setStringValue(&ns(problem.as_deref().unwrap_or("")));
        }
    }
}

// ---------------------------------------------------------------- snapshot

/// `rusty-spotlight --snapshot-settings DIR`: draw each Settings tab and the
/// menu bar icon to PNG files without showing anything, so the layout can be
/// checked from a script.
pub fn snapshot(controller: &Controller, dir: &str) {
    use objc2_app_kit::NSBitmapImageFileType;
    use objc2_foundation::NSDictionary;
    let mtm = controller.mtm();
    CONTROLLER.set(Some(controller.retain()));
    TARGET.set(Some(Target::new(mtm)));
    let save_png = |view: &NSView, name: &str| {
        let bounds = view.bounds();
        if let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) {
            view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) };
            if let Some(data) = data {
                let _ = std::fs::write(format!("{dir}/{name}"), data.to_vec());
            }
        }
    };
    let tabs = [general(mtm), appearance(mtm), search(mtm), plugins(mtm), advanced(mtm)];
    sync_from(&controller.config());
    for (vc, name) in tabs.iter().zip(["general", "appearance", "search", "plugins", "advanced"]) {
        save_png(&vc.view(), &format!("settings-{name}.png"));
    }
    if let Some(image) = symbol("magnifyingglass", 14.0) {
        image.setTemplate(true);
        let host = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(40.0, 24.0)));
        let view = NSImageView::imageViewWithImage(&image, mtm);
        view.setContentTintColor(Some(&NSColor::labelColor()));
        view.setFrame(host.bounds());
        host.addSubview(&view);
        save_png(&host, "icon.png");
    }
}
