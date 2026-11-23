//! System Settings, searchable from the panel: every pane on this Mac and the
//! sections each pane lists in its own search terms, opened by deep link.
//!
//! Panes are app extensions under /System/Library/ExtensionKit/Extensions.
//! Each says in its Info.plist whether it takes `x-apple.systempreferences:`
//! links and which `.searchTerms` file holds its sections; the file maps a
//! section's link anchor to its title and search words, in every language.
//! The list is read once, on a background thread, when the plugin starts
//! (about 50 files), and dropped when it is switched off.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use launcher_core::settings::Setting;
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSData, NSJSONSerialization, NSJSONWritingOptions, NSLocale, NSPropertyListMutabilityOptions, NSPropertyListSerialization};
use serde_json::Value;

const EXTENSIONS: &str = "/System/Library/ExtensionKit/Extensions";
const SETTINGS_POINT: &str = "com.apple.Settings.extension.ui";

/// Panes whose bundles carry no readable name.
const NAMES: &[(&str, &str)] = &[
    ("com.apple.BluetoothSettings", "Bluetooth"),
    ("com.apple.Battery-Settings.extension", "Battery"),
    ("com.apple.systempreferences.AppleIDSettings", "Apple Account"),
    ("com.apple.HeadphoneSettings", "Headphones"),
    ("com.apple.settings.Storage", "Storage"),
    ("com.apple.Touch-ID-Settings.extension", "Touch ID & Password"),
    ("com.apple.Wallpaper-Settings.extension", "Wallpaper"),
    ("com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension", "VPN"),
];

/// Panes that are not shown in System Settings' sidebar.
const HIDDEN: &[&str] = &["com.apple.FollowUpSettings.FollowUpSettingsExtension", "com.apple.SecurityImprovements-Settings.extension"];

/// Panes with hundreds of narrow sections, ranked below the rest.
const NARROW: &[&str] = &["com.apple.Accessibility-Settings.extension"];

/// Words people type for panes that list few or none of their own.
const WORDS: &[(&str, &str)] = &[
    ("com.apple.wifi-settings-extension", "wifi, wireless, internet, network, ssid, hotspot"),
    ("com.apple.Desktop-Settings.extension", "dock, mission control, hot corners, stage manager, windows, widgets, spaces, desktop"),
    ("com.apple.Notifications-Settings.extension", "alerts, banners, badges, notification centre, notification center"),
    ("com.apple.Trackpad-Settings.extension", "tap to click, scroll, gestures, force click, tracking speed, natural scrolling"),
    ("com.apple.Mouse-Settings.extension", "scroll, tracking speed, natural scrolling, secondary click"),
    ("com.apple.ControlCenter-Settings.extension", "control centre, control center, menu bar icons, status items"),
    ("com.apple.Focus-Settings.extension", "do not disturb, dnd, focus modes"),
    ("com.apple.Software-Update-Settings.extension", "update, upgrade, macos, automatic updates"),
    ("com.apple.settings.Storage", "disk, space, free up, storage"),
    ("com.apple.Time-Machine-Settings.extension", "backup, restore"),
    ("com.apple.Touch-ID-Settings.extension", "fingerprint, password, touch id"),
    ("com.apple.Wallpaper-Settings.extension", "background, desktop picture"),
    ("com.apple.Startup-Disk-Settings.extension", "boot, startup"),
    ("com.apple.Battery-Settings.extension", "energy, power, low power mode, battery health"),
    ("com.apple.Lock-Screen-Settings.extension", "screen saver, screensaver, lock"),
    ("com.apple.systempreferences.GeneralSettings", "about, storage, sharing, login items"),
    ("com.apple.NetworkExtensionSettingsUI.NESettingsUIExtension", "vpn, tunnel"),
];

/// A property list (XML or binary) as JSON. `None` for anything else.
fn read_plist(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    autoreleasepool(|_| {
        let data = NSData::with_bytes(&bytes);
        let object = unsafe {
            NSPropertyListSerialization::propertyListWithData_options_format_error(&data, NSPropertyListMutabilityOptions(0), std::ptr::null_mut())
        }
        .ok()?;
        let json = unsafe { NSJSONSerialization::dataWithJSONObject_options_error(&object, NSJSONWritingOptions(0)) }.ok()?;
        serde_json::from_slice(&json.to_vec()).ok()
    })
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(value, |v, key| v.get(key))?.as_str()
}

/// The languages to look for, best first: `en-IN` gives `en_IN`, `en`, then English.
pub fn languages() -> Vec<String> {
    let mut out = Vec::new();
    for lang in NSLocale::preferredLanguages().iter().take(3) {
        let lang = lang.to_string().replace('-', "_");
        out.push(lang.clone());
        if let Some((base, _)) = lang.split_once('_') {
            out.push(base.to_string());
        }
    }
    out.push("en".into());
    out.dedup();
    out
}

/// A pane's name in the user's language.
fn pane_name(bundle: &Path, info: &Value, id: &str, languages: &[String]) -> Option<String> {
    if let Some((_, name)) = NAMES.iter().find(|(i, _)| *i == id) {
        return Some(name.to_string());
    }
    if let Some(table) = read_plist(&bundle.join("Contents/Resources/InfoPlist.loctable")) {
        for lang in languages {
            if let Some(name) = text(&table, &[lang, "CFBundleDisplayName"]).or_else(|| text(&table, &[lang, "CFBundleName"])) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    // Info.plist names are often internal ("PowerPreferences"); only a plain one will do.
    let name = text(info, &["CFBundleDisplayName"])?;
    (!name.contains("Extension") && !name.contains("Settings") && !name.contains("Preferences")).then(|| name.to_string())
}

fn link(id: &str, anchor: Option<&str>) -> String {
    match anchor {
        Some(anchor) if !matches!(anchor, "Main" | "primary") => format!("x-apple.systempreferences:{id}?{}", anchor.replace(' ', "%20")),
        _ => format!("x-apple.systempreferences:{id}"),
    }
}

fn split(words: &str) -> Vec<String> {
    words.split(',').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect()
}

/// Every pane and section on this Mac.
pub fn build(languages: &[String]) -> Vec<Setting> {
    build_from(Path::new(EXTENSIONS), languages)
}

fn build_from(root: &Path, languages: &[String]) -> Vec<Setting> {
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "appex")).collect())
        .unwrap_or_default();
    bundles.sort();
    let mut out = Vec::new();
    for bundle in bundles {
        let Some(info) = read_plist(&bundle.join("Contents/Info.plist")) else { continue };
        let attributes = &info["EXAppExtensionAttributes"];
        if text(attributes, &["EXExtensionPointIdentifier"]) != Some(SETTINGS_POINT) {
            continue;
        }
        let settings = &attributes["SettingsExtensionAttributes"];
        // A pane that does not take links cannot be opened at a section, or at all.
        if settings["allowsXAppleSystemPreferencesURLScheme"].as_bool() != Some(true) {
            continue;
        }
        let Some(id) = text(&info, &["CFBundleIdentifier"]).map(str::to_string) else { continue };
        if HIDDEN.contains(&id.as_str()) {
            continue;
        }
        let Some(pane) = pane_name(&bundle, &info, &id, languages) else { continue };
        let words = WORDS.iter().find(|(i, _)| *i == id).map(|(_, w)| split(w)).unwrap_or_default();
        let icon = text(&info, &["CFBundleIcons", "ISGraphicIconConfiguration", "ISTypeIdentifier"]).unwrap_or("").to_string();
        out.push(Setting::new(&pane, &pane, &link(&id, None), &words).with_icon(&icon));

        let Some(file) = text(settings, &["searchTermsFileName"]) else { continue };
        let resources = bundle.join("Contents/Resources");
        let terms = languages
            .iter()
            .map(|l| resources.join(format!("{l}.lproj/{file}.searchTerms")))
            .chain([resources.join(format!("Base.lproj/{file}.searchTerms")), resources.join(format!("{file}.searchTerms"))])
            .find_map(|p| read_plist(&p));
        let Some(Value::Object(sections)) = terms else { continue };
        for (anchor, section) in &sections {
            let Some(entries) = section["localizableStrings"].as_array() else { continue };
            for entry in entries {
                let Some(title) = entry["title"].as_str().filter(|t| !t.trim().is_empty()) else { continue };
                let words = entry["index"].as_str().map(split).unwrap_or_default();
                let setting = Setting::new(title.trim(), &pane, &link(&id, Some(anchor)), &words).with_icon(&icon);
                out.push(if NARROW.contains(&id.as_str()) { setting.demoted() } else { setting });
            }
        }
    }
    out
}

/// Read the list on a background thread; the receiver gets it once.
pub fn build_in_background() -> Receiver<Vec<Setting>> {
    let languages = languages();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        let list = build(&languages);
        eprintln!("[rs] settings: {} panes and sections read in {:.0} ms", list.len(), t.elapsed().as_secs_f64() * 1000.0);
        let _ = tx.send(list);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_drop_the_main_anchor_and_encode_spaces() {
        assert_eq!(link("com.apple.BluetoothSettings", Some("Main")), "x-apple.systempreferences:com.apple.BluetoothSettings");
        assert_eq!(
            link("com.apple.Displays-Settings.extension", Some("nightShiftSection")),
            "x-apple.systempreferences:com.apple.Displays-Settings.extension?nightShiftSection"
        );
        assert_eq!(link("x", Some("Advanced Ethernet")), "x-apple.systempreferences:x?Advanced%20Ethernet");
    }

    /// Reads this Mac's real panes; skipped where there are none.
    #[test]
    fn the_panes_on_this_mac_are_read() {
        if !Path::new(EXTENSIONS).is_dir() {
            return;
        }
        let list = build(&["en".to_string()]);
        let find = |title: &str| list.iter().find(|s| s.title == title);
        let bluetooth = find("Bluetooth").expect("Bluetooth pane");
        assert!(bluetooth.is_pane());
        assert_eq!(bluetooth.url, "x-apple.systempreferences:com.apple.BluetoothSettings");
        let night = find("Night Shift options").expect("Night Shift section");
        assert_eq!(night.pane, "Displays");
        assert!(night.url.ends_with("?nightShiftSection"), "{}", night.url);
        assert!(list.iter().all(|s| s.url.starts_with("x-apple.systempreferences:com.apple.")));
        assert!(list.len() > 200, "{}", list.len());
    }
}
