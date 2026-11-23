//! The panel and its controller. Everything here runs on the main thread.
//!
//! Ranking is synchronous, so what is in `State::rows` is exactly what the
//! table shows after `present()` returns. File results fill in on a later
//! turn and keep the selected row where it was. Return, click and completion
//! all read that one presentation (FR-14).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use launcher_core::rank::completion;
use launcher_core::text::fold;
use launcher_core::{Item, Ranker, Usage};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::AnyThread;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackgroundStyle, NSBackingStoreType, NSBezierPath, NSBox, NSBoxType, NSColor, NSControl,
    NSControlTextEditingDelegate, NSEvent, NSEventModifierFlags, NSFloatingWindowLevel, NSFocusRingType, NSFont,
    NSFontWeightMedium, NSGlassEffectView, NSImage, NSImageScaling, NSImageView,
    NSModalPanelWindowLevel, NSPanel, NSPasteboard, NSPasteboardTypeString, NSPopUpMenuWindowLevel, NSScreen, NSScreenSaverWindowLevel,
    NSScrollView, NSStatusWindowLevel, NSTableColumn, NSTableRowView, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewSelectionHighlightStyle,
    NSTableViewStyle, NSTextField, NSTextFieldDelegate, NSTextInputClient, NSTextInputTraitType, NSTextView, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
    NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSDate, NSIndexSet, NSInteger, NSNotification, NSNumber, NSObject, NSTimer, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimeZone, NSURL,
};

use crate::{apps, ask, clipboard, config, files, plugins, store};

// Sizes in points, measured from Spotlight (docs/REQUIREMENTS.md, section 5.5).
const PANEL_W: f64 = 640.0;
const FIELD_H: f64 = 56.0;
const ROW_H: f64 = 56.0;
const RADIUS: f64 = 28.0;
const TEXT_X: f64 = 61.0;
const MAX_ROWS: usize = 7;
const LIST_PAD_TOP: f64 = 6.0;
const LIST_PAD_BOTTOM: f64 = 8.0;
/// Transparent border of the window around the visible panel, so the panel
/// can be drawn larger than its resting size while it animates in.
const MARGIN: f64 = 48.0;
/// Height of the panel with every row showing. The window is always this tall
/// (plus margins); only the visible surface inside it changes size.
const MAX_H: f64 = FIELD_H + LIST_PAD_TOP + ROW_H * MAX_ROWS as f64 + LIST_PAD_BOTTOM;

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// One line per summon and per dismissal, with the app in front, so a report
/// of "it did not open" can be matched to what actually happened. Always on:
/// a few lines per use, written to the log file the login item points at.
pub fn event_log(what: &str) {
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let secs = now() as u64 % 86_400;
    eprintln!("[rs] {:02}:{:02}:{:02} UTC {what} | front={front}", secs / 3600, secs / 60 % 60, secs % 60);
}

fn debug() -> bool {
    std::env::var_os("RS_DEBUG").is_some()
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

thread_local! {
    /// Until when a loss of key status is attributed to an app we just launched.
    static LAUNCH_GRACE: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    /// The app that was in front when the panel was summoned.
    static SHOWN_OVER: Cell<i32> = const { Cell::new(-1) };
    /// How many times the panel has taken the keyboard back since it was shown.
    static RETAKES: Cell<u32> = const { Cell::new(0) };
}

/// A window that keeps taking the keyboard is not fought forever.
const MAX_RETAKES: u32 = 4;

fn frontmost_pid() -> i32 {
    NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier()).unwrap_or(-1)
}

fn front_name() -> String {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- panel

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSLauncherPanel"]
    pub struct LauncherPanel;

    impl LauncherPanel {
        // A borderless panel refuses key status unless this says yes.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }

        // An agent app has no Edit menu, so the standard editing shortcuts
        // would go nowhere. Route them to the field editor here.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.edit_shortcut(event) || unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        // Losing key status usually means a click elsewhere, but an input-method
        // palette or a menu can take it for a moment. Decide on the next turn of
        // the run loop, and never order out from inside resignKey itself.
        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            let _: () = unsafe {
                msg_send![
                    self,
                    performSelector: sel!(rsHideIfNotKey),
                    withObject: Option::<&AnyObject>::None,
                    afterDelay: 0.0f64,
                ]
            };
        }

        #[unsafe(method(rsHideIfNotKey))]
        fn hide_if_not_key(&self) {
            if !self.isVisible() || self.isKeyWindow() {
                return;
            }
            // A mouse button being down means a click elsewhere: the user left.
            if NSEvent::pressedMouseButtons() != 0 {
                event_log("hidden: clicked elsewhere");
                self.orderOut(None);
                return;
            }
            // No click. Either the user switched apps from the keyboard, or an
            // app grabbed the keyboard by itself (T3 Code refocusing its input,
            // an app this launcher just opened finishing its start). Wait a
            // moment so the frontmost app is current, then decide.
            let _: () = unsafe {
                msg_send![
                    self,
                    performSelector: sel!(rsRetakeOrHide),
                    withObject: Option::<&AnyObject>::None,
                    afterDelay: 0.12f64,
                ]
            };
        }

        #[unsafe(method(rsRetakeOrHide))]
        fn retake_or_hide(&self) {
            if !self.isVisible() || self.isKeyWindow() {
                return;
            }
            let in_grace = LAUNCH_GRACE.get().is_some_and(|until| std::time::Instant::now() < until);
            let same_app = frontmost_pid() == SHOWN_OVER.get();
            let clicked = NSEvent::pressedMouseButtons() != 0;
            if !clicked && (in_grace || same_app) && RETAKES.get() < MAX_RETAKES {
                RETAKES.set(RETAKES.get() + 1);
                event_log(&format!("kept: keyboard taken back from {}", front_name()));
                self.makeKeyWindow();
                return;
            }
            event_log("hidden: another app took the keyboard");
            self.orderOut(None);
        }
    }
);

impl LauncherPanel {
    fn edit_shortcut(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        let command_only = flags.contains(NSEventModifierFlags::Command)
            && !flags.contains(NSEventModifierFlags::Control)
            && !flags.contains(NSEventModifierFlags::Option);
        if !command_only {
            return false;
        }
        let key = event.charactersIgnoringModifiers().map(|c| c.to_string().to_lowercase());
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let action = match (key.as_deref(), shift) {
            (Some("a"), false) => sel!(selectAll:),
            (Some("c"), false) => sel!(copy:),
            (Some("v"), false) => sel!(paste:),
            (Some("x"), false) => sel!(cut:),
            (Some("z"), false) => sel!(undo:),
            (Some("z"), true) => sel!(redo:),
            (Some("k"), false) => sel!(rsShowActions:),
            _ => return false,
        };
        let app = objc2_app_kit::NSApplication::sharedApplication(self.mtm());
        unsafe { app.sendAction_to_from(action, None, Some(self)) }
    }
}

// ---------------------------------------------------------------- row view

#[derive(Default)]
struct RowIvars {
    card: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSTableRowView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSRowView"]
    #[ivars = RowIvars]
    struct RowView;

    impl RowView {
        // Spotlight's selection: a neutral translucent fill, inset, rounded (DS-3b).
        #[unsafe(method(drawSelectionInRect:))]
        fn draw_selection(&self, _dirty: NSRect) {
            if self.ivars().card.get() {
                return;
            }
            let b = self.bounds();
            let rect = NSRect::new(
                NSPoint::new(b.origin.x + 9.0, b.origin.y + 0.5),
                NSSize::new(b.size.width - 18.0, b.size.height - 1.0),
            );
            NSColor::labelColor().colorWithAlphaComponent(0.16).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 18.0, 18.0).fill();
        }

        // Keep label colours as they are on a selected row; the fill is neutral, not accent.
        #[unsafe(method(interiorBackgroundStyle))]
        fn interior_background_style(&self) -> NSBackgroundStyle {
            NSBackgroundStyle::Normal
        }

        #[unsafe(method(isEmphasized))]
        fn is_emphasized(&self) -> bool {
            false
        }
    }
);

// ---------------------------------------------------------------- controller

/// What Return does with a presented row.
#[derive(Clone)]
enum Kind {
    App { running: bool },
    /// Open a web address in the default browser.
    Link { url: String },
    /// A calculation or conversion: Return copies `copy`.
    Value { expression: String, copy: String, note: Option<String> },
    /// A question for an assistant tool: Return sends it (or retries).
    Ask { tool: String, question: String, pending: bool },
    /// Something the user should know (a broken configuration file): Return opens `path`.
    Notice { path: String },
    /// The tool's answer: Return copies it.
    Answer { tool: String, text: String, height: f64, truncated: bool },
    /// A clipboard item. Return puts it back on the clipboard.
    Clip { id: String, pinned: bool, image: bool, icon_path: Option<String> },
    /// A file or folder from Spotlight. Return opens it.
    File { path: String },
    /// A line of explanation. Return does nothing.
    Hint,
    /// One of the selected row's actions, listed by Command-K (FR-40).
    Action { act: Act, symbol: &'static str },
}

/// What a Command-K action does.
#[derive(Clone)]
enum Act {
    /// What Return does on the row the actions are for.
    Primary,
    Reveal(String),
    Copy(String),
    Quit { path: String, force: bool },
    /// Ask the same question with another tool.
    AskWith { tool: String, question: String },
    PinClip(String),
    DeleteClip(String),
    ClearClips,
}

/// One presented row. Return, click and the completion all act on this, never
/// on a fresh ranking or a newer rate (FR-14, FR-19).
#[derive(Clone)]
struct Row {
    id: String,
    title: String,
    subtitle: Option<String>,
    kind: Kind,
}

impl Row {
    fn action(&self) -> &'static str {
        match self.kind {
            Kind::App { running: true } => "Switch",
            Kind::App { running: false } | Kind::Link { .. } => "Open",
            Kind::Value { .. } | Kind::Answer { .. } | Kind::Clip { .. } => "Copy",
            Kind::Ask { pending: true, .. } => "",
            Kind::Ask { .. } => "Ask",
            Kind::Notice { .. } | Kind::File { .. } => "Open",
            Kind::Hint => "",
            Kind::Action { .. } => "Run",
        }
    }

    fn height(&self) -> f64 {
        match &self.kind {
            Kind::Value { note: Some(_), .. } => 94.0,
            Kind::Value { note: None, .. } => 72.0,
            Kind::Answer { height, .. } => *height,
            _ => ROW_H,
        }
    }
}

/// One question sent to an assistant tool, and what came back.
struct AskRun {
    /// The whole typed query it belongs to; a different query abandons it.
    query: String,
    request: Option<ask::Request>,
    outcome: Option<Result<String, String>>,
}

/// A file search the current query is waiting on.
#[derive(Clone)]
struct FileWant {
    key: String,
    term: String,
    folders_only: bool,
}

pub struct State {
    panel: Retained<LauncherPanel>,
    /// The visible rounded panel (glass or blur), inset by MARGIN in the window.
    surface: Retained<NSView>,
    /// Sits behind the surface and carries the panel's drop shadow.
    shadow: Retained<NSView>,
    #[allow(dead_code)] // kept alive here; laid out once at build
    content: Retained<NSView>,
    field: Retained<NSTextField>,
    completion: Retained<NSTextField>,
    /// The selected result's icon at the trailing edge of the field, as Spotlight shows.
    field_icon: Retained<NSImageView>,
    measure: Retained<NSTextField>,
    separator: Retained<NSBox>,
    scroll: Retained<NSScrollView>,
    table: Retained<NSTableView>,
    items: RefCell<Vec<Item>>,
    config: RefCell<config::Config>,
    /// The features that are switched on.
    plugins: RefCell<plugins::Registry>,
    config_modified: Cell<Option<std::time::SystemTime>>,
    config_error: RefCell<Option<String>>,
    usage: RefCell<Usage>,
    ranker: RefCell<Ranker>,
    rows: RefCell<Vec<Row>>,
    /// While Command-K's actions are listed: the rows and selection they replaced.
    actions: RefCell<Option<(Vec<Row>, usize)>>,
    query: RefCell<String>,
    selected: Cell<usize>,
    running: RefCell<HashSet<String>>,
    icons: RefCell<HashMap<String, Retained<NSImage>>>,
    /// Colour on the glass (`theme` in config.toml).
    tint: Option<crate::theme::Tint>,
    /// Rows the surface is currently sized for.
    shown_height: Cell<f64>,
    ask: RefCell<Option<AskRun>>,
    ask_timer: RefCell<Option<Retained<NSTimer>>>,
    /// Clipboard history is what the field filters, instead of apps.
    clip_mode: Cell<bool>,
    clip_timer: RefCell<Option<Retained<NSTimer>>>,
    file_want: RefCell<Option<FileWant>>,
    file_want_at: Cell<std::time::Instant>,
    file_run: RefCell<Option<files::Running>>,
    file_cache: RefCell<HashMap<String, Vec<files::Hit>>>,
    file_gen: Cell<u64>,
    file_timer: RefCell<Option<Retained<NSTimer>>>,
    thumbs: RefCell<HashMap<String, Retained<NSImage>>>,
    /// Height animation in flight: (from, to, start time), stepped by `spring_timer`.
    spring: Cell<Option<(f64, f64, std::time::Instant)>>,
    spring_timer: RefCell<Option<Retained<NSTimer>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RSController"]
    #[ivars = State]
    pub struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    // Opening the app again from Finder while it runs (the only way back when
    // the menu bar icon is hidden) shows the settings.
    unsafe impl NSApplicationDelegate for Controller {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            crate::settings::show(self);
            false
        }
    }

    unsafe impl NSControlTextEditingDelegate for Controller {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _n: &NSNotification) {
            let t = std::time::Instant::now();
            let text = self.ivars().field.stringValue().to_string();
            *self.ivars().query.borrow_mut() = text;
            self.present();
            if debug() {
                eprintln!("[rs] keystroke to results laid out: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
            }
        }

        // Keys the field editor did not consume as text. While an input method
        // is composing, the editor gets first refusal on everything (FR-7).
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, text_view: &NSTextView, command: Sel) -> bool {
            self.handle_command(text_view, command)
        }
    }

    unsafe impl NSTextFieldDelegate for Controller {}

    unsafe impl NSTableViewDataSource for Controller {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSTableViewDelegate for Controller {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.cell_for(row as usize)
        }

        #[unsafe(method_id(tableView:rowViewForRow:))]
        fn row_view(&self, _table: &NSTableView, row: NSInteger) -> Option<Retained<NSTableRowView>> {
            let view = RowView::alloc(self.mtm()).set_ivars(RowIvars::default());
            let view: Retained<RowView> = unsafe { msg_send![super(view), init] };
            // A result card is its own highlight; it gets no selection fill.
            let card = matches!(
                self.ivars().rows.borrow().get(row as usize).map(|r| &r.kind),
                Some(Kind::Value { .. } | Kind::Answer { .. })
            );
            view.ivars().card.set(card);
            Some(Retained::into_super(view))
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of_row(&self, _table: &NSTableView, row: NSInteger) -> f64 {
            self.ivars().rows.borrow().get(row as usize).map_or(ROW_H, Row::height)
        }
    }

    impl Controller {
        #[unsafe(method(rowClicked:))]
        fn row_clicked(&self, _sender: Option<&AnyObject>) {
            let row = self.ivars().table.clickedRow();
            if row >= 0 && (row as usize) < self.ivars().rows.borrow().len() {
                self.ivars().selected.set(row as usize);
                self.activate();
            }
        }

        #[unsafe(method(askTick:))]
        fn ask_tick(&self, _timer: Option<&AnyObject>) {
            self.poll_ask();
        }

        #[unsafe(method(clipTick:))]
        fn clip_tick(&self, _timer: Option<&AnyObject>) {
            if clipboard::poll() && self.ivars().clip_mode.get() && self.ivars().panel.isVisible() {
                self.present_inner(false);
            }
        }

        #[unsafe(method(fileTick:))]
        fn file_tick(&self, _timer: Option<&AnyObject>) {
            self.poll_files();
        }

        #[unsafe(method(springTick:))]
        fn spring_tick(&self, _timer: Option<&AnyObject>) {
            self.step_spring();
        }

        #[unsafe(method(showPanel:))]
        fn show_panel(&self, _sender: Option<&AnyObject>) {
            self.show();
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            self.dismiss();
            crate::settings::show(self);
        }

        // Command-K, sent up the responder chain from the panel.
        #[unsafe(method(rsShowActions:))]
        fn show_actions(&self, _sender: Option<&AnyObject>) {
            self.toggle_actions();
        }

        #[unsafe(method(rsAfterShow))]
        fn rs_after_show(&self) {
            self.after_show();
        }

        #[unsafe(method(rsCheckOnScreen))]
        fn rs_check_on_screen(&self) {
            self.check_on_screen();
        }

    }
);

impl Controller {
    // Keys the field editor did not consume as text. While an input method is
    // composing, the editor gets first refusal on everything (FR-7).
    fn handle_command(&self, text_view: &NSTextView, command: Sel) -> bool {
        if text_view.hasMarkedText() {
            return false;
        }
        if command == sel!(moveDown:) {
            self.move_selection(1);
        } else if command == sel!(moveUp:) {
            self.move_selection(-1);
        } else if command == sel!(insertNewline:) {
            self.activate();
        } else if command == sel!(cancelOperation:) {
            if !self.leave_actions() {
                self.dismiss();
            }
        } else if command == sel!(moveRight:) || command == sel!(insertTab:) {
            return self.accept_completion(text_view);
        } else {
            return false;
        }
        true
    }

    fn cell_for(&self, row: usize) -> Option<Retained<NSView>> {
        let row_data = self.ivars().rows.borrow().get(row).cloned()?;
        Some(self.cell(&row_data, row == self.ivars().selected.get()))
    }

    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_W, FIELD_H));
        let full = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_W, MAX_H));
        let window_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(PANEL_W + 2.0 * MARGIN, MAX_H + 2.0 * MARGIN),
        );
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel: Retained<LauncherPanel> = unsafe {
            msg_send![
                LauncherPanel::alloc(mtm),
                initWithContentRect: window_rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        // The level and Space behaviour stay switchable until the fullscreen,
        // Stage Manager and second-display checks of spike S1 have been done by hand.
        let level = match env("RS_LEVEL", "popup").as_str() {
            "floating" => NSFloatingWindowLevel,
            "modal" => NSModalPanelWindowLevel,
            "status" => NSStatusWindowLevel,
            "screensaver" => NSScreenSaverWindowLevel,
            _ => NSPopUpMenuWindowLevel,
        };
        let behavior = match env("RS_BEHAVIOR", "a").as_str() {
            "b" => {
                NSWindowCollectionBehavior::CanJoinAllApplications
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle
            }
            "c" => {
                NSWindowCollectionBehavior::MoveToActiveSpace
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle
            }
            _ => {
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle
            }
        };
        // Order matters: setFloatingPanel(true) forces the floating level, so the
        // level has to be set after it.
        panel.setFloatingPanel(true);
        panel.setLevel(level);
        panel.setCollectionBehavior(behavior);
        if debug() {
            eprintln!("[rs] panel level={} behavior={:#x}", panel.level(), behavior.0);
        }
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        // The window shadow is rectangular; the rounded glass draws its own edge.
        panel.setHasShadow(false);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        unsafe { panel.setReleasedWhenClosed(false) };

        // `content` is always full height and pinned to the top of `clip`, which
        // is what the glass shows. Growing the panel only reveals more of it, so
        // nothing inside has to move while the surface animates.
        let content = NSView::initWithFrame(NSView::alloc(mtm), full);
        content.setFrameOrigin(NSPoint::new(0.0, FIELD_H - MAX_H));
        content.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin);
        let clip = NSView::initWithFrame(NSView::alloc(mtm), rect);
        // The tint sits under everything else inside the glass, so it rides
        // the same springs as the surface.
        let tint = crate::theme::Tint::new(rect, mtm);
        if let Some(tint) = &tint {
            clip.addSubview(tint.view());
        }
        clip.addSubview(&content);

        let field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(TEXT_X, MAX_H - FIELD_H + 11.0), NSSize::new(PANEL_W - TEXT_X - 60.0, 34.0)),
        );
        field.setBordered(false);
        field.setBezeled(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        // One line that scrolls sideways, never wraps.
        field.setUsesSingleLineMode(true);
        field.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByClipping);
        if let Some(cell) = field.cell() {
            cell.setScrollable(true);
            cell.setWraps(false);
        }
        field.setFont(Some(&NSFont::systemFontOfSize(25.0)));
        field.setPlaceholderString(Some(&ns("Search")));
        field.setTextColor(Some(&strong_label(mtm)));
        // No system suggestion popups under the field: this panel draws its own results.
        field.setAutomaticTextCompletionEnabled(false);
        field.setAllowsWritingTools(false);
        field.setAllowsWritingToolsAffordance(false);
        content.addSubview(&field);

        let glyph = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("magnifyingglass"), None)
            .and_then(|g| {
                let config = objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    24.0,
                    unsafe { NSFontWeightMedium },
                );
                g.imageWithSymbolConfiguration(&config)
            });
        if let Some(glyph) = glyph {
            let view = NSImageView::imageViewWithImage(&glyph, mtm);
            view.setContentTintColor(Some(&NSColor::labelColor().colorWithAlphaComponent(0.62)));
            view.setImageScaling(NSImageScaling::ScaleNone);
            view.setFrame(NSRect::new(NSPoint::new(17.0, MAX_H - FIELD_H + (FIELD_H - 32.0) / 2.0), NSSize::new(32.0, 32.0)));
            content.addSubview(&view);
        }

        // Inline completion (DS-3a): drawn after the typed text, not editable.
        let completion = NSTextField::labelWithString(&ns(""), mtm);
        completion.setFont(Some(&NSFont::systemFontOfSize(25.0)));
        completion.setTextColor(Some(&NSColor::labelColor().colorWithAlphaComponent(0.72)));
        completion.setDrawsBackground(true);
        completion.setBackgroundColor(Some(&NSColor::labelColor().colorWithAlphaComponent(0.17)));
        completion.setWantsLayer(true);
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![&*completion, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setCornerRadius: 8.0f64];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
            }
        }
        completion.setHidden(true);
        content.addSubview(&completion);

        let field_icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(
                NSPoint::new(PANEL_W - 19.0 - 32.0, MAX_H - FIELD_H + (FIELD_H - 32.0) / 2.0),
                NSSize::new(32.0, 32.0),
            ),
        );
        field_icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        field_icon.setHidden(true);
        content.addSubview(&field_icon);

        // Never shown: used to measure the width of the typed text in the field's font.
        let measure = NSTextField::labelWithString(&ns(""), mtm);
        measure.setFont(Some(&NSFont::systemFontOfSize(25.0)));

        let separator = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(NSPoint::new(19.0, MAX_H - FIELD_H - 1.0), NSSize::new(PANEL_W - 38.0, 1.0)),
        );
        separator.setBoxType(NSBoxType::Separator);
        content.addSubview(&separator);

        let table = NSTableView::initWithFrame(NSTableView::alloc(mtm), rect);
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns("main"));
        column.setWidth(PANEL_W);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setRowHeight(ROW_H);
        table.setIntercellSpacing(NSSize::new(0.0, 0.0));
        table.setBackgroundColor(&NSColor::clearColor());
        table.setStyle(NSTableViewStyle::FullWidth);
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::Regular);
        table.setRefusesFirstResponder(true);
        table.setAllowsEmptySelection(true);

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, LIST_PAD_BOTTOM), NSSize::new(PANEL_W, ROW_H * MAX_ROWS as f64)),
        );
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(false);
        scroll.setDocumentView(Some(&table));
        content.addSubview(&scroll);

        let root = NSView::initWithFrame(NSView::alloc(mtm), window_rect);
        root.setWantsLayer(true);
        let surface: Retained<NSView> = if env("RS_MATERIAL", "glass") == "blur" {
            let blur = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), rect);
            blur.setMaterial(NSVisualEffectMaterial::HUDWindow);
            blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            blur.setState(NSVisualEffectState::Active);
            blur.setWantsLayer(true);
            unsafe {
                let layer: Option<Retained<AnyObject>> = msg_send![&*blur, layer];
                if let Some(layer) = layer {
                    let _: () = msg_send![&*layer, setCornerRadius: RADIUS];
                    let _: () = msg_send![&*layer, setMasksToBounds: true];
                }
            }
            clip.setAutoresizingMask(
                objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                    | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            blur.addSubview(&clip);
            Retained::into_super(blur)
        } else {
            let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), rect);
            glass.setCornerRadius(RADIUS);
            if env("RS_GLASS", "regular") == "clear" {
                glass.setStyle(objc2_app_kit::NSGlassEffectViewStyle::Clear);
            }
            glass.setContentView(Some(&clip));
            Retained::into_super(glass)
        };
        surface.setWantsLayer(true);
        // The drop shadow is a rounded shadow path on a layer of its own; the
        // window's own shadow is rectangular and stays off.
        let shadow = NSView::initWithFrame(NSView::alloc(mtm), rect);
        shadow.setWantsLayer(true);
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![&*shadow, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setMasksToBounds: false];
            }
        }
        // AppKit owns a layer-backed view's shadow settings, so they are given
        // to the view; only the path is set on the layer directly.
        let drop = objc2_app_kit::NSShadow::new();
        drop.setShadowColor(Some(&NSColor::blackColor().colorWithAlphaComponent(0.5)));
        drop.setShadowBlurRadius(26.0);
        drop.setShadowOffset(NSSize::new(0.0, -12.0));
        shadow.setShadow(Some(&drop));
        root.addSubview(&shadow);
        root.addSubview(&surface);
        panel.setContentView(Some(&root));

        let state = State {
            panel,
            surface,
            tint,
            shadow,
            content,
            field,
            completion,
            field_icon,
            measure,
            separator,
            scroll,
            table,
            items: RefCell::new(Vec::new()),
            config: RefCell::new(config::Config::default()),
            plugins: RefCell::new(plugins::Registry::default()),
            config_modified: Cell::new(None),
            config_error: RefCell::new(None),
            usage: RefCell::new(store::load()),
            ranker: RefCell::new(Ranker::default()),
            rows: RefCell::new(Vec::new()),
            actions: RefCell::new(None),
            query: RefCell::new(String::new()),
            selected: Cell::new(0),
            running: RefCell::new(HashSet::new()),
            icons: RefCell::new(HashMap::new()),
            shown_height: Cell::new(0.0),
            ask: RefCell::new(None),
            ask_timer: RefCell::new(None),
            clip_mode: Cell::new(false),
            clip_timer: RefCell::new(None),
            file_want: RefCell::new(None),
            file_want_at: Cell::new(std::time::Instant::now()),
            file_run: RefCell::new(None),
            file_cache: RefCell::new(HashMap::new()),
            file_gen: Cell::new(0),
            file_timer: RefCell::new(None),
            thumbs: RefCell::new(HashMap::new()),
            spring: Cell::new(None),
            spring_timer: RefCell::new(None),
        };
        let this = Self::alloc(mtm).set_ivars(state);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        let s = this.ivars();
        unsafe {
            let delegate: &AnyObject = &this;
            let _: () = msg_send![&*s.field, setDelegate: delegate];
            let _: () = msg_send![&*s.table, setDelegate: delegate];
            let _: () = msg_send![&*s.table, setDataSource: delegate];
            let _: () = msg_send![&*s.table, setTarget: delegate];
            s.table.setAction(Some(sel!(rowClicked:)));
        }
        this.reload_config(true);
        this.layout(0.0, false);
        this
    }

    // ------------------------------------------------------------ show / hide

    pub fn toggle(&self) {
        if self.ivars().panel.isVisible() {
            self.dismiss();
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
        self.open(false);
    }

    /// The clipboard shortcut: the field filters clipboard history.
    pub fn show_clipboard(&self) {
        let s = self.ivars();
        if !s.plugins.borrow().on("clipboard") {
            return;
        }
        if s.panel.isVisible() && s.clip_mode.get() {
            self.dismiss();
            return;
        }
        self.open(true);
    }

    fn open(&self, clips: bool) {
        let s = self.ivars();
        s.clip_mode.set(clips);
        let t = std::time::Instant::now();
        if config::changed_since(s.config_modified.get()) {
            self.reload_config(false);
        }
        SHOWN_OVER.set(frontmost_pid());
        RETAKES.set(0);
        s.field.setStringValue(&ns(""));
        s.query.borrow_mut().clear();
        self.place();
        self.present();
        self.order_front_here();
        s.panel.makeKeyWindow();
        // The field keeps the keyboard between summons; only a first summon,
        // or one after another view took it, needs the (slow) hand-over.
        let editor = s.field.currentEditor();
        let has_focus = editor.as_ref().is_some_and(|e| {
            s.panel.firstResponder().is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<u8>(), Retained::as_ptr(e).cast::<u8>()))
        });
        let focused = has_focus || s.panel.makeFirstResponder(Some(&s.field));
        if !has_focus {
            // The shared field editor carries its own typing aids; turn them off too.
            if let Some(editor) = s.field.currentEditor() {
                if let Ok(editor) = editor.downcast::<NSTextView>() {
                    editor.setInlinePredictionType(NSTextInputTraitType::No);
                    editor.setMathExpressionCompletionType(NSTextInputTraitType::No);
                    editor.setAutomaticTextCompletionEnabled(false);
                    editor.setContinuousSpellCheckingEnabled(false);
                }
            }
        }
        self.animate_in();
        let shown_ms = t.elapsed().as_secs_f64() * 1000.0;
        event_log(&format!("shown in {shown_ms:.1} ms (key={} focus={focused})", s.panel.isKeyWindow()));
        // Which apps are running, and the app folders, are refreshed only
        // after this frame is on screen: together they took 20–25 ms, and the
        // empty panel needs neither.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(rsAfterShow), withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.02f64]
        };
    }

    /// The part of a summon that can wait until the panel is on screen.
    fn after_show(&self) {
        let s = self.ivars();
        let t = std::time::Instant::now();
        *s.running.borrow_mut() = if s.plugins.borrow().on("apps") { running_bundle_paths() } else { HashSet::new() };
        let fresh = self.scan_apps();
        let changed = {
            let items = s.items.borrow();
            fresh.len() != items.len() || fresh.iter().zip(items.iter()).any(|(a, b)| a.id != b.id)
        };
        if changed {
            *s.items.borrow_mut() = fresh;
        }
        // Anything typed in the meantime was ranked without these; rank again.
        if !s.query.borrow().is_empty() {
            self.present();
        }
        if debug() {
            eprintln!("[rs] after show: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Show the panel on the Space in use.
    ///
    /// A hidden panel belongs to no Space, and when shown the window server
    /// puts it in every Space ("all Spaces" + full-screen auxiliary). A
    /// long-running panel was found listed in the desktop Space only, and
    /// from then on it was shown only there: over a full-screen app it was
    /// ordered in but off screen. AppKit cannot see or undo that, so the
    /// window server is asked directly (see spaces.rs) and the Space in use
    /// is added when it is missing, before showing and again after.
    fn order_front_here(&self) {
        let panel = &self.ivars().panel;
        let window = panel.windowNumber();
        // Healthy, the panel is listed in no Space (never shown) or in all of
        // them. Listed in some but not the one in use, it would open off
        // screen: add the Space in use first.
        if let (Some(listed), Some(active)) = (crate::spaces::of_window(window), crate::spaces::active()) {
            if !listed.is_empty() && !listed.contains(&active) {
                crate::spaces::add(window, active);
                event_log(&format!("panel was held in Space {listed:?} only; added the Space in use ({active})"));
            }
        }
        panel.setLevel(panel.level());
        panel.setCollectionBehavior(panel.collectionBehavior());
        panel.orderFrontRegardless();
        // The window server applies the order a moment later, so whether the
        // panel landed in the Space in use is checked on a later turn.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(rsCheckOnScreen), withObject: std::ptr::null::<AnyObject>(), afterDelay: 0.15f64]
        };
    }

    /// 0.15 s after showing: if the panel is open but still not on screen,
    /// add it to the Space in use.
    fn check_on_screen(&self) {
        let panel = &self.ivars().panel;
        let window = panel.windowNumber();
        if !panel.isVisible() || on_screen(window) {
            return;
        }
        let spaces = crate::spaces::of_window(window).unwrap_or_default();
        let Some(active) = crate::spaces::active() else { return };
        crate::spaces::add(window, active);
        event_log(&format!(
            "panel was open but not on screen (in Space {spaces:?}, Space in use {active}); added it there: {}",
            if crate::spaces::of_window(window).unwrap_or_default().contains(&active) { "done" } else { "failed" }
        ));
    }

    /// Spotlight's opening animation, measured from a 60 fps recording over a
    /// black card on macOS 27.2: the field is first visible at about 110% of
    /// its width and 96% of its height, and reaches its resting shape in
    /// about 125 ms with almost no overshoot (0.2%), fading in over about
    /// 75 ms. Purely visual: the field already has focus.
    /// Skipped under Reduce Motion or with RS_ANIM=off.
    fn animate_in(&self) {
        if !self.animate() {
            return;
        }
        let s = self.ivars();
        let layer: Option<Retained<AnyObject>> = unsafe { msg_send![&*s.surface, layer] };
        let Some(layer) = layer else { return };
        let size = s.surface.frame().size;
        const FROM_X: f64 = 1.12;
        const FROM_Y: f64 = 0.95;

        // The layer's anchor is its bottom-left corner, so each scale is paired
        // with a translation on the same curve to keep the centre fixed.
        let spring = |key: &str, from: f64, to: f64| -> Option<Retained<AnyObject>> {
            let class = AnyClass::get(c"CASpringAnimation")?;
            unsafe {
                let a: Retained<AnyObject> = msg_send![class, animationWithKeyPath: &*ns(key)];
                // Damping ratio about 0.76 at about 45 rad/s.
                let _: () = msg_send![&*a, setMass: 1.0f64];
                let _: () = msg_send![&*a, setStiffness: 2000.0f64];
                let _: () = msg_send![&*a, setDamping: 68.0f64];
                let _: () = msg_send![&*a, setFromValue: &*NSNumber::new_f64(from)];
                let _: () = msg_send![&*a, setToValue: &*NSNumber::new_f64(to)];
                let settle: f64 = msg_send![&*a, settlingDuration];
                let _: () = msg_send![&*a, setDuration: settle];
                Some(a)
            }
        };
        let animations = [
            ("rs.sx", spring("transform.scale.x", FROM_X, 1.0)),
            ("rs.sy", spring("transform.scale.y", FROM_Y, 1.0)),
            ("rs.tx", spring("transform.translation.x", size.width * (1.0 - FROM_X) / 2.0, 0.0)),
            ("rs.ty", spring("transform.translation.y", size.height * (1.0 - FROM_Y) / 2.0, 0.0)),
        ];
        for (key, animation) in animations {
            if let Some(animation) = animation {
                let _: () = unsafe { msg_send![&*layer, addAnimation: &*animation, forKey: &*ns(key)] };
            }
        }
        if let Some(class) = AnyClass::get(c"CABasicAnimation") {
            unsafe {
                let fade: Retained<AnyObject> = msg_send![class, animationWithKeyPath: &*ns("opacity")];
                let _: () = msg_send![&*fade, setFromValue: &*NSNumber::new_f64(0.0)];
                let _: () = msg_send![&*fade, setToValue: &*NSNumber::new_f64(1.0)];
                let _: () = msg_send![&*fade, setDuration: 0.075f64];
                let _: () = msg_send![&*layer, addAnimation: &*fade, forKey: &*ns("rs.fade")];
            }
        }
    }

    /// A newer exchange rate arrived: redraw, if the panel is showing a conversion.
    pub fn rates_updated(&self) {
        let s = self.ivars();
        let showing = s.rows.borrow().iter().any(|r| r.id == "convert");
        if s.panel.isVisible() && showing {
            let selected = s.selected.get();
            self.present();
            let len = s.rows.borrow().len();
            if selected < len && selected != 0 {
                s.selected.set(selected);
                self.reload();
                self.update_completion();
            }
        }
    }

    pub fn dismiss(&self) {
        self.cancel_ask();
        self.cancel_files();
        let s = self.ivars();
        s.clip_mode.set(false);
        let panel = s.panel.clone();
        if panel.isVisible() {
            event_log("hidden: dismissed");
            panel.orderOut(None);

        }
        if let Some(tint) = &self.ivars().tint {
            tint.stop();
        }
    }

    /// Pick the screen under the pointer and fix the panel's top edge on it.
    fn place(&self) {
        let mtm = self.mtm();
        let mouse = NSEvent::mouseLocation();
        let mut chosen = None;
        for screen in NSScreen::screens(mtm).iter() {
            let f = screen.frame();
            if mouse.x >= f.origin.x
                && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y
                && mouse.y < f.origin.y + f.size.height
            {
                chosen = Some(screen);
                break;
            }
        }
        let Some(screen) = chosen.or_else(|| NSScreen::mainScreen(mtm)) else { return };
        let f = screen.frame();
        let v = screen.visibleFrame();
        // Spotlight: centred, top edge about 28% down the screen; kept inside the visible area.
        let top = (f.origin.y + f.size.height * (1.0 - 0.28)).min(v.origin.y + v.size.height - 8.0);
        let x = f.origin.x + (f.size.width - PANEL_W) / 2.0 - MARGIN;
        let s = self.ivars();
        let size = s.panel.frame().size;
        s.panel.setFrame_display(
            NSRect::new(NSPoint::new(x.round(), top.round() + MARGIN - size.height), size),
            false,
        );
    }

    /// Size the visible surface for `rows` result rows. Its top edge never
    /// moves; it grows downward inside the fixed window. With `animate`, the
    /// height follows Spotlight's own spring (see `spring_progress`).
    fn layout(&self, rows_height: f64, animate: bool) {
        let s = self.ivars();
        let rows = rows_height > 0.0;
        let list_h = if rows { LIST_PAD_TOP + rows_height + LIST_PAD_BOTTOM } else { 0.0 };
        let h = FIELD_H + list_h;
        s.separator.setHidden(!rows);
        s.scroll.setHidden(!rows);
        s.shown_height.set(rows_height);

        let animate = animate && self.animate();
        if !animate {
            self.stop_spring();
            self.set_height(h);
            return;
        }
        let from = s.surface.frame().size.height;
        s.spring.set(Some((from, h, std::time::Instant::now())));
        if s.spring_timer.borrow().is_none() {
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    1.0 / 120.0,
                    self,
                    sel!(springTick:),
                    None,
                    true,
                )
            };
            *s.spring_timer.borrow_mut() = Some(timer);
        }
    }

    fn set_height(&self, h: f64) {
        let s = self.ivars();
        // The spring gives fractional heights. Unrounded, the glass and the
        // rows pinned to its top round to different pixels on alternate
        // frames, and the rows shiver by a pixel while the panel grows. Snap
        // to the display's pixel grid so every edge moves in whole pixels.
        let scale = s.panel.backingScaleFactor().max(1.0);
        let h = (h * scale).round() / scale;
        let frame = NSRect::new(NSPoint::new(MARGIN, MARGIN + MAX_H - h), NSSize::new(PANEL_W, h));
        s.shadow.setFrame(frame);
        update_shadow(&s.shadow, PANEL_W, h);
        s.surface.setFrame(frame);
    }

    fn stop_spring(&self) {
        let s = self.ivars();
        s.spring.set(None);
        if let Some(timer) = s.spring_timer.borrow_mut().take() {
            timer.invalidate();
        }
    }

    fn step_spring(&self) {
        let Some((from, to, start)) = self.ivars().spring.get() else {
            self.stop_spring();
            return;
        };
        let t = start.elapsed().as_secs_f64();
        if t >= SPRING_SETTLE {
            self.stop_spring();
            self.set_height(to);
            return;
        }
        let h = from + (to - from) * spring_progress(t);
        self.set_height(h.max(FIELD_H * 0.8));
    }

    // ------------------------------------------------------------ results

    /// Read the configuration and rebuild the catalog with its aliases.
    /// Settings saved a change: read the file now rather than on the next summon.
    pub fn apply_settings(&self) {
        self.reload_config(false);
    }

    /// The configuration in use, for Settings to show.
    pub fn config(&self) -> config::Config {
        self.ivars().config.borrow().clone()
    }

    /// Forget what was learned from use (FR-12).
    pub fn reset_learning(&self) -> std::io::Result<()> {
        let mut usage = self.ivars().usage.borrow_mut();
        *usage = Usage::default();
        store::save(&usage)
    }

    /// The opening and resizing springs: off with `animations = false`,
    /// RS_ANIM=off or Reduce Motion.
    fn animate(&self) -> bool {
        self.ivars().config.borrow().animations
            && env("RS_ANIM", "on") != "off"
            && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }

    fn reload_config(&self, first: bool) {
        let s = self.ivars();
        let previous = (!first).then(|| s.config.borrow().clone());
        let loaded = config::load(previous.as_ref());
        *s.config.borrow_mut() = loaded.config;
        s.config_modified.set(loaded.modified);
        if let Some(error) = &loaded.error {
            eprintln!("[rs] config.toml: {error}");
        }
        *s.config_error.borrow_mut() = loaded.error;
        s.plugins.borrow_mut().configure(&s.config.borrow());
        if !s.plugins.borrow().on("ask") {
            self.cancel_ask(); // a tool still answering is stopped with its plugin
        }
        crate::settings::config_changed(&s.config.borrow());
        if let Some(tint) = &s.tint {
            let config = s.config.borrow();
            tint.set_theme(crate::theme::parse(&config.theme), crate::theme::strength(&config.theme_strength));
        }
        let items = self.scan_apps();
        *s.items.borrow_mut() = items;
        self.sync_optional_plugins();
    }

    /// The clipboard shortcut in force: none while the plugin is off.
    pub fn clipboard_hotkey(&self) -> String {
        let s = self.ivars();
        if s.plugins.borrow().on("clipboard") { s.config.borrow().clipboard_hotkey() } else { String::new() }
    }

    /// Start or stop what the files and clipboard plugins keep running. Off
    /// leaves no timer, no search, no shortcut and nothing in memory.
    fn sync_optional_plugins(&self) {
        let s = self.ivars();
        let (files_on, clips_on) = {
            let plugins = s.plugins.borrow();
            (plugins.on("files"), plugins.on("clipboard"))
        };
        if !files_on {
            self.cancel_files();
            s.file_cache.borrow_mut().clear();
        }
        if clips_on {
            let config = s.config.borrow();
            clipboard::start(config.clipboard_keep_days(), config.clipboard_ignored_apps());
            if s.clip_timer.borrow().is_none() {
                let timer = unsafe {
                    NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(0.8, self, sel!(clipTick:), None, true)
                };
                // Lets the system fire it together with other wake-ups.
                timer.setTolerance(0.3);
                *s.clip_timer.borrow_mut() = Some(timer);
                eprintln!("[rs] plugin clipboard: recording");
            }
        } else {
            if let Some(timer) = s.clip_timer.borrow_mut().take() {
                timer.invalidate();
                eprintln!("[rs] plugin clipboard: off");
            }
            clipboard::stop();
            s.clip_mode.set(false);
        }
        if let Err(error) = crate::hotkey::set_clipboard(&self.clipboard_hotkey()) {
            eprintln!("[rs] {error}");
        }
    }

    fn scan_apps(&self) -> Vec<Item> {
        let s = self.ivars();
        let config = s.config.borrow();
        // With the apps plugin off no folder is read; only Settings is listed.
        let mut items = if s.plugins.borrow().on("apps") { apps::scan(&config.aliases) } else { Vec::new() };
        // This app's own entry opens its settings; launching it again would
        // only start a second copy. It is the host's, so it is always there.
        if let Some(own) = own_bundle() {
            let settings = Item::new(own.clone(), "Rusty Spotlight Settings", launcher_core::Kind::App, vec!["preferences".to_string()]);
            match items.iter_mut().find(|i| i.id == own) {
                Some(item) => *item = settings,
                None => items.push(settings),
            }
        }
        items
    }

    /// The calculation, conversion, question or web address the query asks
    /// for, if a plugin that is switched on wants it.
    fn special_row(&self, query: &str) -> Option<Row> {
        let trimmed = query.trim();
        let asked = plugins::Query { text: trimmed, now: now(), zone: &zone_offset };
        let answer = self.ivars().plugins.borrow_mut().special(&asked)?;
        let kind = match answer.effect {
            plugins::Effect::Ask { tool, question } => return Some(self.ask_row(trimmed, &tool, &question)),
            plugins::Effect::Copy { expression, copy, note } => Kind::Value { expression, copy, note },
            plugins::Effect::Open { url } => Kind::Link { url },
        };
        Some(Row { id: answer.id, title: answer.title, subtitle: answer.subtitle, kind })
    }

    /// Rank for the current query and show the result. After this returns,
    /// `rows` and `selected` describe exactly what is on screen.
    fn present(&self) {
        self.present_inner(true);
    }

    /// `reset` puts the selection on the first row. A late file or clipboard
    /// update passes false and keeps the row that was selected.
    fn present_inner(&self, reset: bool) {
        let s = self.ivars();
        let query = s.query.borrow().clone();
        let abandoned = s.ask.borrow().as_ref().is_some_and(|run| run.query != query.trim());
        if abandoned {
            self.cancel_ask();
        }
        let kept = if reset {
            None
        } else {
            let selected = s.selected.get();
            s.rows.borrow().get(selected).map(|row| (selected, row.id.clone()))
        };
        let (search_files, clips) = {
            let plugins = s.plugins.borrow();
            (plugins.on("files"), plugins.on("clipboard"))
        };
        let rows = if clips && (s.clip_mode.get() || clipboard::keyword_filter(&query).is_some()) {
            self.cancel_files();
            self.clipboard_rows(&query)
        } else if search_files {
            if let Some((term, folders_only)) = files::keyword(&query) {
                self.exclusive_files(&term, folders_only)
            } else {
                self.app_rows(&query)
            }
        } else {
            self.cancel_files();
            self.app_rows(&query)
        };
        let rows = within_budget(rows);
        let len = rows.len();
        *s.rows.borrow_mut() = rows;
        if let Some((previous, id)) = kept {
            let index = s.rows.borrow().iter().position(|row| row.id == id).unwrap_or(previous);
            s.selected.set(if len == 0 { 0 } else { index.min(len - 1) });
        } else {
            s.selected.set(0);
        }
        s.actions.borrow_mut().take();
        self.show_rows();
    }

    fn app_rows(&self, query: &str) -> Vec<Row> {
        let s = self.ivars();
        let special = if query.trim().is_empty() { None } else { self.special_row(query) };
        let mut rows: Vec<Row> = {
            let items = s.items.borrow();
            let usage = s.usage.borrow();
            let running = s.running.borrow();
            let ranked = s.ranker.borrow_mut().rank(query, &items, &usage, now(), MAX_ROWS);
            ranked
                .iter()
                .map(|r| {
                    let item = &items[r.index];
                    Row {
                        id: item.id.clone(),
                        title: item.title.clone(),
                        subtitle: None,
                        kind: Kind::App { running: running.contains(&item.id) },
                    }
                })
                .collect()
        };
        // The launcher's own entry (Settings) never outranks a real app.
        if let Some(own) = own_bundle() {
            let (mut others, ours): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|r| r.id != own);
            others.extend(ours);
            rows = others;
        }
        // An empty query shows only the field, as Spotlight does (Q16), unless
        // the configuration file is broken: that is said once, up front (FR-50).
        if query.is_empty() {
            rows.clear();
            if let (Some(error), Some(path)) = (s.config_error.borrow().as_ref(), config::path()) {
                rows.push(Row {
                    id: "notice:config".into(),
                    title: "config.toml has an error; using the last good settings".into(),
                    subtitle: Some(error.lines().next().unwrap_or("").to_string()),
                    kind: Kind::Notice { path: path.to_string_lossy().into_owned() },
                });
            }
        }
        // The special row leads unless an app is named exactly (FR-22).
        if let Some(special) = special {
            let exact = rows.first().is_some_and(|r| fold(&r.title) == fold(query));
            rows.insert(if exact { 1 } else { 0 }, special);
        }
        let trimmed = query.trim();
        // File names sit under the apps, and only when nothing else claimed the query.
        let ambient = s.plugins.borrow().on("files")
            && s.config.borrow().files_ambient()
            && trimmed.chars().count() >= 2
            && rows.iter().all(|row| matches!(row.kind, Kind::App { .. }));
        if ambient {
            self.want_files(trimmed, false);
            let key = files::cache_key(trimmed, false);
            if let Some(hits) = s.file_cache.borrow().get(&key).filter(|hits| !hits.is_empty()).cloned() {
                let files: Vec<Row> = hits.iter().take(3).map(file_row).collect();
                let room = MAX_ROWS.saturating_sub(files.len());
                rows.truncate(room);
                rows.extend(files);
            }
        } else {
            self.cancel_files();
        }
        rows
    }

    fn clipboard_rows(&self, query: &str) -> Vec<Row> {
        let filter = if self.ivars().clip_mode.get() {
            query.trim().to_string()
        } else {
            clipboard::keyword_filter(query).unwrap_or_default()
        };
        let entries = clipboard::search(&filter);
        if entries.is_empty() {
            let title = if filter.is_empty() { "Clipboard history is empty" } else { "No matches" };
            return vec![hint("clip:none", title)];
        }
        entries.iter().take(MAX_ROWS).map(clip_row).collect()
    }

    fn exclusive_files(&self, term: &str, folders_only: bool) -> Vec<Row> {
        if files::spotlight_query(term, folders_only).is_none() {
            self.cancel_files();
            return vec![hint("file:more", "Type a bit more to search files")];
        }
        self.want_files(term, folders_only);
        let key = files::cache_key(term, folders_only);
        match self.ivars().file_cache.borrow().get(&key) {
            None => vec![hint("file:wait", "Searching…")],
            Some(hits) if hits.is_empty() => {
                vec![hint("file:none", if folders_only { "No matching folders" } else { "No matching files" })]
            }
            Some(hits) => hits.iter().map(file_row).collect(),
        }
    }

    fn want_files(&self, term: &str, folders_only: bool) {
        let key = files::cache_key(term, folders_only);
        let s = self.ivars();
        let same = s.file_want.borrow().as_ref().is_some_and(|want| want.key == key);
        if !same {
            *s.file_want.borrow_mut() = Some(FileWant { key, term: term.to_string(), folders_only });
            s.file_want_at.set(std::time::Instant::now());
            s.file_run.borrow_mut().take();
        }
        self.arm_file_timer();
    }

    fn arm_file_timer(&self) {
        let s = self.ivars();
        if s.file_timer.borrow().is_some() {
            return;
        }
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(0.05, self, sel!(fileTick:), None, true)
        };
        *s.file_timer.borrow_mut() = Some(timer);
    }

    fn disarm_file_timer(&self) {
        if let Some(timer) = self.ivars().file_timer.borrow_mut().take() {
            timer.invalidate();
        }
    }

    fn cancel_files(&self) {
        let s = self.ivars();
        s.file_want.borrow_mut().take();
        s.file_run.borrow_mut().take();
        self.disarm_file_timer();
    }

    /// Called by the file timer. Starts a search after a short pause, then
    /// paints the rows when `mdfind` answers.
    fn poll_files(&self) {
        let s = self.ivars();
        let Some(want) = s.file_want.borrow().clone() else {
            self.disarm_file_timer();
            return;
        };
        if s.file_run.borrow().as_ref().is_some_and(|run| run.key != want.key) {
            s.file_run.borrow_mut().take();
        }
        let ready = s.file_run.borrow().as_ref().and_then(|run| run.poll().map(|hits| (run.generation, hits)));
        if let Some((generation, hits)) = ready {
            s.file_run.borrow_mut().take();
            if generation == s.file_gen.get() {
                self.remember_files(&want.key, hits);
                if s.panel.isVisible() && s.actions.borrow().is_none() {
                    self.present_inner(false);
                }
            }
            return;
        }
        if s.file_run.borrow().is_some() {
            return;
        }
        if s.file_cache.borrow().contains_key(&want.key) {
            self.disarm_file_timer();
            return;
        }
        if s.file_want_at.get().elapsed() < std::time::Duration::from_millis(120) {
            return;
        }
        let generation = s.file_gen.get().wrapping_add(1);
        s.file_gen.set(generation);
        match files::start(&want.term, want.folders_only, generation) {
            Some(run) => *s.file_run.borrow_mut() = Some(run),
            None => self.disarm_file_timer(),
        }
    }

    fn remember_files(&self, key: &str, hits: Vec<files::Hit>) {
        let mut cache = self.ivars().file_cache.borrow_mut();
        if cache.len() >= 8 && !cache.contains_key(key) {
            if let Some(old) = cache.keys().next().cloned() {
                cache.remove(&old);
            }
        }
        cache.insert(key.to_string(), hits);
    }

    /// Size the panel for `rows` and draw them.
    fn show_rows(&self) {
        let s = self.ivars();
        let height: f64 = s.rows.borrow().iter().map(Row::height).sum();
        // No borrow is held past this point: the table calls back into the data source.
        let animate = s.panel.isVisible() && (height - s.shown_height.get()).abs() > 0.5;
        self.layout(height, animate);
        self.reload();
        self.update_completion();
    }

    // ------------------------------------------------------------ Command-K

    /// List the selected row's actions in place of the results, or go back.
    fn toggle_actions(&self) {
        if self.leave_actions() {
            return;
        }
        let s = self.ivars();
        let Some(row) = s.rows.borrow().get(s.selected.get()).cloned() else { return };
        let actions = self.actions_for(&row);
        if actions.len() < 2 {
            return; // nothing beyond what Return already does
        }
        let rows = s.rows.replace(actions);
        let selected = s.selected.replace(0);
        *s.actions.borrow_mut() = Some((rows, selected));
        self.show_rows();
    }

    /// Back from the action list to the results. False if it was not shown.
    fn leave_actions(&self) -> bool {
        let s = self.ivars();
        let saved = s.actions.borrow_mut().take();
        let Some((rows, selected)) = saved else { return false };
        *s.rows.borrow_mut() = rows;
        s.selected.set(selected);
        self.show_rows();
        true
    }

    fn actions_for(&self, row: &Row) -> Vec<Row> {
        let s = self.ivars();
        let action = |n: usize, title: String, act: Act, symbol: &'static str| Row {
            id: format!("action:{n}"),
            title,
            subtitle: None,
            kind: Kind::Action { act, symbol },
        };
        let primary = match &row.kind {
            Kind::App { .. } if own_bundle().as_deref() == Some(row.id.as_str()) => "Open Settings".to_string(),
            Kind::App { running: true } => format!("Switch to {}", row.title),
            Kind::App { running: false } => format!("Open {}", row.title),
            Kind::Link { .. } | Kind::Ask { .. } => row.title.clone(),
            Kind::Value { .. } => format!("Copy {}", row.title),
            Kind::Answer { .. } => "Copy Answer".to_string(),
            Kind::Notice { path } => format!("Open {}", path.rsplit('/').next().unwrap_or(path)),
            Kind::Clip { .. } => "Copy".to_string(),
            Kind::File { .. } => format!("Open {}", row.title),
            Kind::Hint | Kind::Action { .. } => return Vec::new(),
        };
        let mut list = vec![action(0, primary, Act::Primary, "return")];
        let mut add = |title: String, act: Act, symbol: &'static str| {
            let n = list.len();
            list.push(action(n, title, act, symbol));
        };
        match &row.kind {
            Kind::App { running } if own_bundle().as_deref() != Some(row.id.as_str()) => {
                add("Show in Finder".into(), Act::Reveal(row.id.clone()), "folder");
                add("Copy Path".into(), Act::Copy(row.id.clone()), "doc.on.doc");
                if *running {
                    add(format!("Quit {}", row.title), Act::Quit { path: row.id.clone(), force: false }, "xmark.circle");
                    add(format!("Force Quit {}", row.title), Act::Quit { path: row.id.clone(), force: true }, "exclamationmark.octagon");
                }
            }
            Kind::Link { url } => add("Copy Link".into(), Act::Copy(url.clone()), "link"),
            Kind::Value { copy, expression, .. } if !copy.is_empty() => {
                let joined = format!("{expression}{} {}", if row.id == "time" { ":" } else { " =" }, row.title);
                add(format!("Copy “{joined}”"), Act::Copy(joined), "doc.on.doc");
            }
            Kind::Ask { pending: false, tool, .. } | Kind::Answer { tool, .. } => {
                let config = s.config.borrow();
                let query = s.query.borrow();
                if let Some((_, question)) = config::ask_query(&config, &query) {
                    for other in config.ask.tools.keys().filter(|t| *t != tool) {
                        let act = Act::AskWith { tool: other.clone(), question: question.to_string() };
                        add(format!("Ask {} Instead", display_name(other)), act, "sparkles");
                    }
                }
            }
            Kind::Notice { path } => add("Show in Finder".into(), Act::Reveal(path.clone()), "folder"),
            Kind::Clip { id, pinned, .. } => {
                add((if *pinned { "Unpin" } else { "Pin" }).into(), Act::PinClip(id.clone()), "pin");
                add("Delete".into(), Act::DeleteClip(id.clone()), "trash");
                add("Clear Unpinned".into(), Act::ClearClips, "trash");
            }
            Kind::File { path, .. } => {
                add("Show in Finder".into(), Act::Reveal(path.clone()), "folder");
                add("Copy Path".into(), Act::Copy(path.clone()), "doc.on.doc");
            }
            _ => {}
        }
        list
    }

    fn run_action(&self, act: &Act) {
        let s = self.ivars();
        match act {
            Act::Primary => {
                self.leave_actions();
                self.activate();
            }
            Act::Reveal(path) => {
                self.dismiss();
                let urls = objc2_foundation::NSArray::from_retained_slice(&[NSURL::fileURLWithPath(&ns(path))]);
                NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
            }
            Act::Copy(text) => {
                copy_text(text);
                self.dismiss();
            }
            Act::Quit { path, force } => {
                self.dismiss();
                for app in NSWorkspace::sharedWorkspace().runningApplications().iter() {
                    if app.bundleURL().and_then(|u| u.path()).is_some_and(|p| p.to_string() == *path) {
                        event_log(&format!("{} {path}", if *force { "force quit" } else { "quit" }));
                        if *force { app.forceTerminate() } else { app.terminate() };
                    }
                }
            }
            Act::AskWith { tool, question } => {
                self.leave_actions();
                let text = format!("?{tool} {question}");
                s.field.setStringValue(&ns(&text));
                if let Some(editor) = s.field.currentEditor() {
                    editor.setSelectedRange(objc2_foundation::NSRange::new(ns(&text).length(), 0));
                }
                *s.query.borrow_mut() = text;
                self.start_ask(tool, question);
            }
            Act::PinClip(id) => {
                clipboard::pin(id);
                self.present_inner(false);
            }
            Act::DeleteClip(id) => {
                clipboard::delete(id);
                self.present_inner(false);
            }
            Act::ClearClips => {
                clipboard::clear_unpinned();
                self.present_inner(false);
            }
        }
    }

    fn reload(&self) {
        let s = self.ivars();
        s.table.reloadData();
        if !s.rows.borrow().is_empty() {
            let index = NSIndexSet::indexSetWithIndex(s.selected.get());
            s.table.selectRowIndexes_byExtendingSelection(&index, false);
            s.table.scrollRowToVisible(s.selected.get() as NSInteger);
        }
    }

    fn move_selection(&self, delta: isize) {
        let s = self.ivars();
        let len = s.rows.borrow().len();
        if len == 0 {
            return;
        }
        let next = (s.selected.get() as isize + delta).clamp(0, len as isize - 1) as usize;
        if next != s.selected.get() {
            s.selected.set(next);
            self.reload();
            self.update_completion();
        }
    }

    /// The selected row's title continues what was typed: show the rest and the action.
    fn update_completion(&self) {
        let s = self.ivars();
        let query = s.query.borrow().clone();
        let row = s.rows.borrow().get(s.selected.get()).cloned();
        let text = match row {
            Some(row) if !query.is_empty() => Some(match &row.kind {
                Kind::Value { copy, .. } if copy.is_empty() => String::new(),
                Kind::Value { .. } => format!(" = {}", row.title),
                Kind::Link { .. } => format!(" — {}", row.title),
                Kind::Ask { pending: false, .. } => format!(" — {}", row.title),
                Kind::Ask { .. } | Kind::Answer { .. } | Kind::Notice { .. } | Kind::Action { .. } | Kind::Hint => String::new(),
                Kind::Clip { .. } | Kind::File { .. } => format!(" — {}", row.action()),
                Kind::App { .. } => match completion(&query, &row.title) {
                    Some(rest) => format!("{rest} — {}", row.action()),
                    None => format!(" — {}", row.title),
                },
            }),
            _ => None,
        }
        .filter(|t| !t.is_empty());
        let selected = s.rows.borrow().get(s.selected.get()).cloned();
        self.update_tint(selected.as_ref());
        match &selected {
            Some(row) if !query.is_empty() && matches!(row.kind, Kind::App { .. } | Kind::File { .. }) => {
                let path = match &row.kind {
                    Kind::File { path, .. } => path,
                    _ => &row.id,
                };
                s.field_icon.setImage(Some(&self.icon(path)));
                s.field_icon.setHidden(false);
            }
            _ => s.field_icon.setHidden(true),
        }
        let Some(text) = text else {
            s.completion.setHidden(true);
            return;
        };
        s.measure.setStringValue(&ns(&query));
        s.measure.sizeToFit();
        let typed_w = s.measure.frame().size.width;
        s.completion.setStringValue(&ns(&text));
        s.completion.sizeToFit();
        let field = s.field.frame();
        let size = s.completion.frame().size;
        let x = field.origin.x + typed_w - 2.0;
        let max_w = (field.origin.x + field.size.width - x).max(0.0);
        // Same origin and height as the field: both are single-line text cells
        // in the same font, so their baselines coincide.
        s.completion.setFrame(NSRect::new(
            NSPoint::new(x, field.origin.y),
            NSSize::new(size.width.min(max_w), field.size.height),
        ));
        s.completion.setHidden(max_w < 40.0);
    }

    /// Colour the glass for the selected row. An action list keeps the colour
    /// of the row it belongs to.
    fn update_tint(&self, row: Option<&Row>) {
        use crate::theme::Subject;
        let Some(tint) = &self.ivars().tint else { return };
        let icon;
        let subject = match row {
            None => Subject::Nothing,
            Some(row) => match &row.kind {
                Kind::Action { .. } => return,
                Kind::App { .. } => {
                    icon = self.icon(&row.id);
                    Subject::App(&icon, &row.id)
                }
                Kind::Value { .. } if row.id == "time" => Subject::Time,
                Kind::Value { .. } if row.id == "units" => Subject::Units,
                Kind::Value { .. } => Subject::Calc,
                Kind::Link { .. } => Subject::Link,
                Kind::Ask { .. } | Kind::Answer { .. } => Subject::Ask,
                Kind::Notice { .. } => Subject::Notice,
                Kind::Clip { .. } => Subject::Clip,
                Kind::File { .. } => Subject::File,
                Kind::Hint => Subject::Nothing,
            },
        };
        tint.update(&subject);
    }

    /// Right arrow or Tab at the end of the text: take the completed title.
    fn accept_completion(&self, text_view: &NSTextView) -> bool {
        let s = self.ivars();
        let query = s.query.borrow().clone();
        let at_end = {
            let range = text_view.selectedRange();
            range.length == 0 && range.location == ns(&query).length()
        };
        let row = s.rows.borrow().get(s.selected.get()).cloned();
        let Some(row) = row else { return false };
        if !matches!(row.kind, Kind::App { .. }) {
            return false;
        }
        if !at_end || query.is_empty() || fold(&query) == fold(&row.title) {
            return false;
        }
        if completion(&query, &row.title).is_none() {
            return false;
        }
        s.field.setStringValue(&ns(&row.title));
        if let Some(editor) = s.field.currentEditor() {
            let end = ns(&row.title).length();
            editor.setSelectedRange(objc2_foundation::NSRange::new(end, 0));
        }
        *s.query.borrow_mut() = row.title.clone();
        self.present();
        true
    }

    // ------------------------------------------------------------ action

    /// Return or click: run the presented, selected row (FR-14, FR-20a).
    fn activate(&self) {
        let s = self.ivars();
        let row = s.rows.borrow().get(s.selected.get()).cloned();
        let Some(row) = row else { return };
        let query = fold(&s.query.borrow());

        match &row.kind {
            Kind::App { .. } if own_bundle().as_deref() == Some(row.id.as_str()) => {
                self.dismiss();
                crate::settings::show(self);
            }
            Kind::App { .. } => {
                // Hide first, then ask the system to open; never wait for the app.
                self.dismiss();
                let url = NSURL::fileURLWithPath(&ns(&row.id));
                let config = NSWorkspaceOpenConfiguration::configuration();
                NSWorkspace::sharedWorkspace()
                    .openApplicationAtURL_configuration_completionHandler(&url, &config, None);
                LAUNCH_GRACE.set(Some(std::time::Instant::now() + std::time::Duration::from_secs(4)));
            }
            Kind::Link { url } => {
                self.dismiss();
                if let Some(url) = NSURL::URLWithString(&ns(url)) {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
                return; // a typed address is not something to learn a ranking from
            }
            Kind::Notice { path } => {
                self.dismiss();
                NSWorkspace::sharedWorkspace().openURL(&NSURL::fileURLWithPath(&ns(path)));
                return;
            }
            Kind::Action { act, .. } => {
                self.run_action(act);
                return;
            }
            Kind::Ask { pending: true, .. } => return, // already asked; a second Return does nothing
            Kind::Ask { tool, question, .. } => {
                self.start_ask(tool, question);
                return;
            }
            Kind::Answer { text, .. } => {
                copy_text(text);
                self.dismiss();
                return;
            }
            Kind::Value { copy, .. } => {
                if copy.is_empty() {
                    return; // no rate yet: nothing was shown, so nothing is copied
                }
                copy_text(copy);
                self.dismiss();
                return;
            }
            Kind::Clip { id, .. } => {
                if let Err(error) = clipboard::copy_entry(id) {
                    eprintln!("[rs] clipboard: {error}");
                    return;
                }
                self.dismiss();
                return;
            }
            Kind::File { path, .. } => {
                self.dismiss();
                NSWorkspace::sharedWorkspace().openURL(&NSURL::fileURLWithPath(&ns(path)));
                return;
            }
            Kind::Hint => return,
        }

        let mut usage = s.usage.borrow_mut();
        usage.record(&query, &row.id, now());
        if let Err(e) = store::save(&usage) {
            eprintln!("[rs] could not save usage: {e}");
        }
    }

    // ------------------------------------------------------------ cells

    fn icon(&self, path: &str) -> Retained<NSImage> {
        if let Some(icon) = self.ivars().icons.borrow().get(path) {
            return icon.clone();
        }
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&ns(path));
        self.ivars().icons.borrow_mut().insert(path.to_string(), icon.clone());
        icon
    }

    /// A clipboard image's thumbnail, decoded once. The full image stays on disk.
    fn thumb(&self, id: &str) -> Option<Retained<NSImage>> {
        if let Some(image) = self.ivars().thumbs.borrow().get(id) {
            return Some(image.clone());
        }
        let path = clipboard::thumb_path(id)?;
        let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &ns(&path.to_string_lossy()))?;
        let mut thumbs = self.ivars().thumbs.borrow_mut();
        if thumbs.len() >= 48 {
            thumbs.clear();
        }
        thumbs.insert(id.to_string(), image.clone());
        Some(image)
    }

    /// The row's icon, and whether it is a picture that should fill its frame
    /// (an app, a file, or a clipboard image) rather than a symbol.
    fn row_image(&self, row: &Row) -> (Retained<NSImageView>, bool) {
        let mtm = self.mtm();
        let picture = |image: Retained<NSImage>| (NSImageView::imageViewWithImage(&image, mtm), true);
        match &row.kind {
            Kind::App { .. } => picture(self.icon(&row.id)),
            Kind::File { path, .. } => picture(self.icon(path)),
            Kind::Clip { icon_path: Some(path), .. } => picture(self.icon(path)),
            Kind::Clip { image: true, id, .. } => {
                if let Some(image) = self.thumb(id) {
                    picture(image)
                } else {
                    self.symbol_view("photo")
                }
            }
            kind => {
                let symbol = match kind {
                    Kind::Action { symbol, .. } => symbol,
                    Kind::Ask { .. } => "sparkles",
                    Kind::Notice { .. } => "exclamationmark.triangle",
                    Kind::Clip { .. } => "doc.on.clipboard",
                    Kind::Hint => "info.circle",
                    _ => "globe",
                };
                self.symbol_view(symbol)
            }
        }
    }

    fn symbol_view(&self, name: &str) -> (Retained<NSImageView>, bool) {
        let mtm = self.mtm();
        let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None).and_then(|image| {
            image.imageWithSymbolConfiguration(&objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                24.0,
                unsafe { NSFontWeightMedium },
            ))
        });
        view.setImage(symbol.as_deref());
        view.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        (view, false)
    }

    // ------------------------------------------------------------ ask

    fn ask_row(&self, query: &str, tool: &str, question: &str) -> Row {
        let name = display_name(tool);
        let run = self.ivars().ask.borrow();
        let run = run.as_ref().filter(|run| run.query == query);
        let (title, subtitle, kind) = match run.map(|r| (&r.outcome, r.request.is_some())) {
            Some((Some(Ok(text)), _)) => {
                let (height, truncated) = self.answer_height(text);
                return Row {
                    id: "answer".into(),
                    title: text.lines().next().unwrap_or("").to_string(),
                    subtitle: None,
                    kind: Kind::Answer { tool: name, text: text.clone(), height, truncated },
                };
            }
            Some((Some(Err(error)), _)) => (
                format!("{name} could not answer"),
                error.clone(),
                Kind::Ask { tool: tool.to_string(), question: question.to_string(), pending: false },
            ),
            Some((None, true)) => (
                format!("Asking {name}…"),
                question.to_string(),
                Kind::Ask { tool: tool.to_string(), question: question.to_string(), pending: true },
            ),
            _ => (
                format!("Ask {name}"),
                question.to_string(),
                Kind::Ask { tool: tool.to_string(), question: question.to_string(), pending: false },
            ),
        };
        Row { id: format!("ask:{tool}"), title, subtitle: Some(subtitle), kind }
    }

    fn start_ask(&self, tool: &str, question: &str) {
        let s = self.ivars();
        let query = s.query.borrow().trim().to_string();
        let command = s.config.borrow().ask.tools.get(tool).cloned().unwrap_or_default();
        self.cancel_ask();
        let run = match ask::start(&command, question) {
            Ok(request) => AskRun { query, request: Some(request), outcome: None },
            Err(error) => AskRun { query, request: None, outcome: Some(Err(error)) },
        };
        let pending = run.request.is_some();
        *s.ask.borrow_mut() = Some(run);
        if pending {
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    0.1,
                    self,
                    sel!(askTick:),
                    None,
                    true,
                )
            };
            *s.ask_timer.borrow_mut() = Some(timer);
        }
        self.present();
    }

    fn poll_ask(&self) {
        let s = self.ivars();
        let finished = {
            let mut run = s.ask.borrow_mut();
            let Some(run) = run.as_mut() else { return };
            let Some(request) = run.request.as_ref() else { return };
            let outcome = request.poll().or_else(|| {
                (request.started.elapsed() > ask::DEADLINE).then(|| Err("no answer in time".to_string()))
            });
            match outcome {
                Some(outcome) => {
                    run.request = None; // dropping the request stops the tool if it is still running
                    run.outcome = Some(outcome);
                    true
                }
                None => false,
            }
        };
        if finished {
            if let Some(timer) = s.ask_timer.borrow_mut().take() {
                timer.invalidate();
            }
            if s.panel.isVisible() {
                self.present();
            }
        }
    }

    fn cancel_ask(&self) {
        let s = self.ivars();
        if let Some(timer) = s.ask_timer.borrow_mut().take() {
            timer.invalidate();
        }
        s.ask.borrow_mut().take(); // dropping a pending request kills the tool
    }

    const ANSWER_WIDTH: f64 = PANEL_W - 36.0 - 34.0;

    fn answer_label(&self, text: &str) -> Retained<NSTextField> {
        let label = NSTextField::wrappingLabelWithString(&ns(text), self.mtm());
        label.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        label.setTextColor(Some(&strong_label(self.mtm())));
        label.setSelectable(false);
        label
    }

    /// Row height for an answer: its text, a heading line and padding, kept
    /// inside the panel. Longer answers are cut off on screen but copied whole.
    fn answer_height(&self, text: &str) -> (f64, bool) {
        let label = self.answer_label(text);
        let size = label
            .cell()
            .map(|c| c.cellSizeForBounds(NSRect::new(NSPoint::ZERO, NSSize::new(Self::ANSWER_WIDTH, 10_000.0))))
            .unwrap_or(NSSize::new(0.0, 20.0));
        let (wanted, limit) = (size.height + 54.0, ROW_H * MAX_ROWS as f64);
        (wanted.clamp(72.0, limit), wanted > limit)
    }

    fn answer_card(&self, row: &Row, tool: &str, text: &str, truncated: bool) -> Retained<NSView> {
        let mtm = self.mtm();
        const CELL_INSET: f64 = 5.5;
        let h = row.height();
        let cell = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, NSSize::new(PANEL_W, h)));
        let frame = NSRect::new(NSPoint::new(18.0 - CELL_INSET, 4.0), NSSize::new(PANEL_W - 36.0, h - 8.0));
        let card = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(objc2_app_kit::NSTitlePosition::NoTitle);
        card.setCornerRadius(20.0);
        card.setBorderWidth(1.0);
        card.setBorderColor(&NSColor::labelColor().colorWithAlphaComponent(0.2));
        card.setFillColor(&NSColor::labelColor().colorWithAlphaComponent(0.12));
        cell.addSubview(&card);

        let top = frame.origin.y + frame.size.height;
        let note = if truncated {
            "too long to show in full · return copies all of it"
        } else {
            "return copies the answer"
        };
        let heading = NSTextField::labelWithString(&ns(&format!("{tool} · {note}")), mtm);
        heading.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        heading.setTextColor(Some(&NSColor::secondaryLabelColor()));
        heading.setFrame(NSRect::new(NSPoint::new(frame.origin.x + 17.0, top - 26.0), NSSize::new(Self::ANSWER_WIDTH, 15.0)));
        cell.addSubview(&heading);

        let body = self.answer_label(text);
        let body_h = h - 8.0 - 46.0;
        if truncated {
            body.setMaximumNumberOfLines((body_h / 17.0).floor() as isize);
            body.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        }
        body.setFrame(NSRect::new(
            NSPoint::new(frame.origin.x + 17.0, frame.origin.y + 12.0),
            NSSize::new(Self::ANSWER_WIDTH, body_h),
        ));
        cell.addSubview(&body);
        cell
    }

    /// Spotlight's result card (DS-6): the expression small, the result under
    /// it, a copy hint at the right, and for a conversion the rate and source.
    fn card(&self, row: &Row, expression: &str, note: Option<&str>) -> Retained<NSView> {
        let mtm = self.mtm();
        const CELL_INSET: f64 = 5.5;
        let h = row.height();
        let cell = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, NSSize::new(PANEL_W, h)));

        let frame = NSRect::new(NSPoint::new(18.0 - CELL_INSET, 4.0), NSSize::new(PANEL_W - 36.0, h - 8.0));
        let card = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(objc2_app_kit::NSTitlePosition::NoTitle);
        card.setCornerRadius(20.0);
        card.setBorderWidth(1.0);
        card.setBorderColor(&NSColor::labelColor().colorWithAlphaComponent(0.2));
        card.setFillColor(&NSColor::labelColor().colorWithAlphaComponent(0.12));
        card.setContentViewMargins(NSSize::new(0.0, 0.0));
        cell.addSubview(&card);

        let width = frame.size.width - 34.0 - 70.0;
        let top = frame.origin.y + frame.size.height;
        let label = |text: &str, size: f64, color: &NSColor, y: f64, height: f64| {
            let l = NSTextField::labelWithString(&ns(text), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize(size)));
            l.setTextColor(Some(color));
            l.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            l.setFrame(NSRect::new(NSPoint::new(frame.origin.x + 17.0, y), NSSize::new(width, height)));
            cell.addSubview(&l);
        };
        // A time reads as a place or a conversion, not a sum.
        let head = if row.id == "time" { expression.to_string() } else { format!("{expression} =") };
        label(&head, 13.0, &NSColor::secondaryLabelColor(), top - 30.0, 18.0);
        label(&row.title, 17.0, &strong_label(mtm), top - 54.0, 22.0);
        if let Some(note) = note {
            label(note, 11.0, &NSColor::secondaryLabelColor(), frame.origin.y + 9.0, 15.0);
        }

        let key = NSTextField::labelWithString(&ns("return"), mtm);
        key.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        key.setTextColor(Some(&NSColor::labelColor()));
        key.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        key.setDrawsBackground(true);
        key.setBackgroundColor(Some(&NSColor::labelColor().colorWithAlphaComponent(0.2)));
        key.setWantsLayer(true);
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![&*key, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setCornerRadius: 5.0f64];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
            }
        }
        key.sizeToFit();
        let size = NSSize::new(key.frame().size.width + 8.0, 18.0);
        let x = frame.origin.x + frame.size.width - 14.0 - size.width;
        key.setFrame(NSRect::new(NSPoint::new(x, top - 38.0), size));
        cell.addSubview(&key);

        let hint = NSTextField::labelWithString(&ns("Copy"), mtm);
        hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        hint.sizeToFit();
        let hint_size = hint.frame().size;
        hint.setFrame(NSRect::new(NSPoint::new(x - 8.0 - hint_size.width, top - 36.0), hint_size));
        cell.addSubview(&hint);
        cell
    }

    fn cell(&self, row: &Row, selected: bool) -> Retained<NSView> {
        if let Kind::Value { expression, note, .. } = &row.kind {
            return self.card(row, expression, note.as_deref());
        }
        if let Kind::Answer { tool, text, truncated, .. } = &row.kind {
            return self.answer_card(row, tool, text, *truncated);
        }
        let mtm = self.mtm();
        let cell = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(PANEL_W, ROW_H)),
        );

        // The table insets its cells by CELL_INSET; positions below are from
        // the panel's edge as measured on Spotlight, so that inset is taken off.
        const CELL_INSET: f64 = 5.5;
        // App icon artwork fills about 80% of its canvas: a 44 pt view shows a 36 pt icon.
        let (image, picture) = self.row_image(row);
        image.setImageScaling(if picture { NSImageScaling::ScaleProportionallyUpOrDown } else { NSImageScaling::ScaleNone });
        image.setFrame(NSRect::new(NSPoint::new(13.0 - CELL_INSET, 6.0), NSSize::new(44.0, 44.0)));
        cell.addSubview(&image);

        // Trailing: the action and its key on the selected row; a dot for a running app.
        let mut trailing = PANEL_W - 17.0 - CELL_INSET;
        if selected && !row.action().is_empty() {
            let key = NSTextField::labelWithString(&ns("return"), mtm);
            key.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            key.setTextColor(Some(&NSColor::labelColor()));
            key.setAlignment(objc2_app_kit::NSTextAlignment::Center);
            key.setDrawsBackground(true);
            key.setBackgroundColor(Some(&NSColor::labelColor().colorWithAlphaComponent(0.2)));
            key.setWantsLayer(true);
            unsafe {
                let layer: Option<Retained<AnyObject>> = msg_send![&*key, layer];
                if let Some(layer) = layer {
                    let _: () = msg_send![&*layer, setCornerRadius: 5.0f64];
                    let _: () = msg_send![&*layer, setMasksToBounds: true];
                }
            }
            key.sizeToFit();
            let size = NSSize::new(key.frame().size.width + 8.0, 18.0);
            trailing -= size.width;
            key.setFrame(NSRect::new(NSPoint::new(trailing, (ROW_H - size.height) / 2.0), size));
            cell.addSubview(&key);
            trailing -= 8.0;

            let hint = NSTextField::labelWithString(&ns(row.action()), mtm);
            hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
            hint.sizeToFit();
            let size = hint.frame().size;
            trailing -= size.width;
            hint.setFrame(NSRect::new(NSPoint::new(trailing, (ROW_H - size.height) / 2.0), size));
            cell.addSubview(&hint);
            trailing -= 10.0;
        }
        if matches!(row.kind, Kind::App { running: true }) {
            let dot = NSTextField::labelWithString(&ns("•"), mtm);
            dot.setFont(Some(&NSFont::systemFontOfSize(13.0)));
            dot.setTextColor(Some(&NSColor::tertiaryLabelColor()));
            dot.sizeToFit();
            let size = dot.frame().size;
            trailing -= size.width;
            dot.setFrame(NSRect::new(NSPoint::new(trailing, (ROW_H - size.height) / 2.0), size));
            cell.addSubview(&dot);
            trailing -= 10.0;
        }

        let title = NSTextField::labelWithString(&ns(&row.title), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize(17.0)));
        title.setTextColor(Some(&strong_label(mtm)));
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        let title_x = 68.5 - CELL_INSET;
        let title_y = if row.subtitle.is_some() { 27.0 } else { 17.0 };
        title.setFrame(NSRect::new(NSPoint::new(title_x, title_y), NSSize::new(trailing - title_x, 22.0)));
        if let Some(subtitle) = &row.subtitle {
            let sub = NSTextField::labelWithString(&ns(subtitle), mtm);
            sub.setFont(Some(&NSFont::systemFontOfSize(15.0)));
            sub.setTextColor(Some(&NSColor::secondaryLabelColor()));
            sub.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            sub.setFrame(NSRect::new(NSPoint::new(title_x, 7.0), NSSize::new(trailing - title_x, 20.0)));
            cell.addSubview(&sub);
        }
        cell.addSubview(&title);
        cell
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut AnyObject;
}

/// Whether the window server has this window on screen now, in the Space in use.
fn on_screen(window: NSInteger) -> bool {
    const ON_SCREEN_ONLY: u32 = 1;
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0);
        if list.is_null() {
            return true; // cannot tell; do not move anything
        }
        let list = Retained::from_raw(list).expect("non-null");
        let list: &objc2_foundation::NSArray<objc2_foundation::NSDictionary<NSString, AnyObject>> = &*(Retained::as_ptr(&list) as *const _);
        let key = ns("kCGWindowNumber");
        list.iter().any(|info| {
            info.objectForKey(&key)
                .and_then(|n| n.downcast::<NSNumber>().ok())
                .is_some_and(|n| n.integerValue() == window)
        })
    }
}

fn hint(id: &str, title: &str) -> Row {
    Row { id: id.into(), title: title.into(), subtitle: None, kind: Kind::Hint }
}

fn clip_row(entry: &clipboard::Entry) -> Row {
    let subtitle = match (entry.detail.is_empty(), entry.pinned) {
        (true, false) => None,
        (true, true) => Some("Pinned".into()),
        (false, false) => Some(entry.detail.clone()),
        (false, true) => Some(format!("{} · Pinned", entry.detail)),
    };
    let icon_path = (entry.kind == clipboard::ClipKind::Files).then(|| entry.files.first().cloned()).flatten();
    Row {
        id: format!("clip:{}", entry.id),
        title: if entry.preview.is_empty() { "Clipboard item".into() } else { entry.preview.clone() },
        subtitle,
        kind: Kind::Clip { id: entry.id.clone(), pinned: entry.pinned, image: entry.kind == clipboard::ClipKind::Image, icon_path },
    }
}

fn file_row(hit: &files::Hit) -> Row {
    Row {
        id: format!("file:{}", hit.path),
        title: hit.name.clone(),
        subtitle: Some(clipboard::pretty_parent(&hit.path)),
        kind: Kind::File { path: hit.path.clone() },
    }
}

/// Keep the list inside the fixed window.
fn within_budget(mut rows: Vec<Row>) -> Vec<Row> {
    let budget = ROW_H * MAX_ROWS as f64;
    let mut height = 0.0;
    rows.retain(|row| {
        height += row.height();
        height <= budget
    });
    rows
}

fn copy_text(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(&ns(text), unsafe { NSPasteboardTypeString });
}

/// A zone's offset from UTC, in seconds, at `t` seconds since 1970. `local`
/// is this Mac's zone. Comes from the system's time zone database.
pub fn zone_offset(id: &str, t: i64) -> Option<i32> {
    let zone = if id == "local" { NSTimeZone::localTimeZone() } else { NSTimeZone::timeZoneWithName(&ns(id))? };
    let date = NSDate::dateWithTimeIntervalSince1970(t as f64);
    Some(zone.secondsFromGMTForDate(&date) as i32)
}

/// This app's bundle, when it runs from one.
fn own_bundle() -> Option<String> {
    let path = objc2_foundation::NSBundle::mainBundle().bundlePath().to_string();
    path.ends_with(".app").then_some(path)
}

/// `claude` → `Claude`.
pub fn display_name(tool: &str) -> String {
    let mut chars = tool.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Spotlight draws titles and typed text at full strength, where the system
/// label colour is slightly translucent. Follows the current appearance.
fn strong_label(mtm: MainThreadMarker) -> Retained<NSColor> {
    let dark = objc2_app_kit::NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .name()
        .to_string()
        .contains("Dark");
    if dark { NSColor::whiteColor() } else { NSColor::blackColor() }
}

fn running_bundle_paths() -> HashSet<String> {
    let mut set = HashSet::new();
    for app in NSWorkspace::sharedWorkspace().runningApplications().iter() {
        if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
            continue; // helpers and agents are not apps the user switches to
        }
        if let Some(path) = app.bundleURL().and_then(|u| u.path()) {
            set.insert(path.to_string());
        }
    }
    set
}

// ---------------------------------------------------------------- shadow

// The shadow is drawn by a layer behind the glass. Left alone it would also
// darken the glass from underneath, so a mask removes everything inside the
// panel's outline and only the part that spills outside remains.

type CGPath = *mut std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPathCreateWithRoundedRect(rect: NSRect, cw: f64, ch: f64, transform: *const std::ffi::c_void) -> CGPath;
    fn CGPathCreateMutable() -> CGPath;
    fn CGPathAddRect(path: CGPath, transform: *const std::ffi::c_void, rect: NSRect);
    fn CGPathAddRoundedRect(path: CGPath, transform: *const std::ffi::c_void, rect: NSRect, cw: f64, ch: f64);
    fn CGPathRelease(path: CGPath);
}

/// How far outside the panel the shadow is allowed to reach.
const SHADOW_REACH: f64 = 90.0;

type PathSetter = unsafe extern "C-unwind" fn(&AnyObject, Sel, CGPath);

fn send_path(target: &AnyObject, selector: Sel, path: CGPath) {
    // CGPathRef arguments are sent directly: msg_send! has no type for them.
    unsafe {
        let send: PathSetter = std::mem::transmute(objc2::ffi::objc_msgSend as unsafe extern "C-unwind" fn());
        send(target, selector, path);
    }
}

/// The panel's outline, `dy` above the layer's origin.
fn outline(w: f64, h: f64, dy: f64) -> CGPath {
    let r = RADIUS.min(h / 2.0);
    unsafe {
        CGPathCreateWithRoundedRect(NSRect::new(NSPoint::new(0.0, dy), NSSize::new(w, h)), r, r, std::ptr::null())
    }
}

/// Everything within SHADOW_REACH of the outline except the outline's interior
/// (used with the even-odd fill rule).
fn ring(w: f64, h: f64, dy: f64) -> CGPath {
    let r = RADIUS.min(h / 2.0);
    unsafe {
        let path = CGPathCreateMutable();
        CGPathAddRect(
            path,
            std::ptr::null(),
            NSRect::new(
                NSPoint::new(-SHADOW_REACH, dy - SHADOW_REACH),
                NSSize::new(w + 2.0 * SHADOW_REACH, h + 2.0 * SHADOW_REACH),
            ),
        );
        CGPathAddRoundedRect(path, std::ptr::null(), NSRect::new(NSPoint::new(0.0, dy), NSSize::new(w, h)), r, r);
        path
    }
}

fn shadow_layers(view: &NSView) -> Option<(Retained<AnyObject>, Retained<AnyObject>)> {
    let layer: Option<Retained<AnyObject>> = unsafe { msg_send![view, layer] };
    let layer = layer?;
    let mask: Option<Retained<AnyObject>> = unsafe { msg_send![&*layer, mask] };
    let mask = match mask {
        Some(mask) => mask,
        None => {
            let class = AnyClass::get(c"CAShapeLayer")?;
            let mask: Retained<AnyObject> = unsafe { msg_send![class, layer] };
            unsafe {
                let _: () = msg_send![&*mask, setFillRule: &*ns("even-odd")];
                let _: () = msg_send![&*layer, setMask: &*mask];
            }
            mask
        }
    };
    Some((layer, mask))
}

/// Size the shadow and its mask for a `w` × `h` panel.
fn update_shadow(view: &NSView, w: f64, h: f64) {
    let Some((layer, mask)) = shadow_layers(view) else { return };
    let (shape, hole) = (outline(w, h, 0.0), ring(w, h, 0.0));
    // Set without implicit animation: the caller steps the size every frame.
    if let Some(class) = AnyClass::get(c"CATransaction") {
        unsafe {
            let _: () = msg_send![class, begin];
            let _: () = msg_send![class, setDisableActions: true];
        }
        send_path(&layer, sel!(setShadowPath:), shape);
        send_path(&mask, sel!(setPath:), hole);
        unsafe {
            let _: () = msg_send![class, commit];
        }
    }
    unsafe {
        CGPathRelease(shape);
        CGPathRelease(hole);
    }
}

// ---------------------------------------------------------------- spring

// Spotlight's resize spring, fitted to a 60 fps recording on macOS 27.2 of
// its field retracting to make room for its buttons: 499 px of travel, target
// first reached after about 165 ms, 8.6% overshoot at about 240 ms, a small
// rebound past the target at about 475 ms, at rest by about 650 ms.
// That is a damping ratio of about 0.62 at about 17 rad/s.
const SPRING_DAMPING_RATIO: f64 = 0.62;
const SPRING_FREQUENCY: f64 = 17.0;
const SPRING_SETTLE: f64 = 0.75;

/// Position of that spring at time `t`, from 0 towards 1, overshooting.
fn spring_progress(t: f64) -> f64 {
    let (z, w) = (SPRING_DAMPING_RATIO, SPRING_FREQUENCY);
    let wd = w * (1.0 - z * z).sqrt();
    1.0 - (-z * w * t).exp() * ((wd * t).cos() + (z * w / wd) * (wd * t).sin())
}

#[cfg(test)]
mod tests {
    use super::zone_offset;

    #[test]
    fn every_zone_is_in_the_system_database() {
        for zone in launcher_core::time::zones() {
            assert!(zone_offset(zone.id, 0).is_some(), "{}", zone.id);
        }
        assert_eq!(zone_offset("Asia/Kathmandu", 1_795_321_800), Some(20_700));
        // Sydney: summer time in January, not in July (2026).
        assert_eq!(zone_offset("Australia/Sydney", 1_768_000_000), Some(39_600));
        assert_eq!(zone_offset("Australia/Sydney", 1_783_000_000), Some(36_000));
    }
}
