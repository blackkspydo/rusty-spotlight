//! Shared native material for the launcher and Settings preview.
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::NSRect;

pub fn background(
    mtm: MainThreadMarker,
    frame: NSRect,
    radius: f64,
    content: &NSView,
) -> Retained<NSView> {
    if let Some(class) = AnyClass::get(c"NSGlassEffectView")
        .filter(|_| std::env::var("RS_MATERIAL").as_deref() != Ok("blur"))
    {
        // The lookup establishes the exact class without importing its Tahoe-only symbol.
        let allocated: Allocated<NSGlassEffectView> = unsafe { msg_send![class, alloc] };
        let glass = NSGlassEffectView::initWithFrame(allocated, frame);
        glass.setCornerRadius(radius);
        if std::env::var("RS_GLASS").as_deref() == Ok("clear") {
            glass.setStyle(NSGlassEffectViewStyle::Clear);
        }
        glass.setContentView(Some(content));
        return Retained::into_super(glass);
    }

    let blur = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
    blur.setMaterial(NSVisualEffectMaterial::HUDWindow);
    blur.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    blur.setState(NSVisualEffectState::Active);
    blur.setWantsLayer(true);
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![&*blur, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: radius];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    content.setFrame(blur.bounds());
    content.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    blur.addSubview(content);
    Retained::into_super(blur)
}
