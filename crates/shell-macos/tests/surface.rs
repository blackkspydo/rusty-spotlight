// AppKit views must be constructed on the main thread.
#[path = "../src/surface.rs"]
mod surface;

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::runtime::AnyClass;
use objc2_app_kit::{NSApplication, NSView};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

fn main() {
    let mtm = MainThreadMarker::new().unwrap();
    let _app = NSApplication::sharedApplication(mtm);
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 56.0));
    let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
    let panel = surface::background(mtm, frame, 26.0, &content);
    assert!(content.isDescendantOf(&panel));
    assert_eq!(panel.frame(), frame);
    let glass = AnyClass::get(c"NSGlassEffectView");
    if let Some(class) = glass.filter(|_| std::env::var("RS_MATERIAL").as_deref() != Ok("blur")) {
        assert!(panel.isKindOfClass(class));
    } else {
        assert!(panel.isKindOfClass(objc2::class!(NSVisualEffectView)));
        assert_eq!(content.frame(), panel.bounds());
    }
}
