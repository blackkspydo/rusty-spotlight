# macOS Sonoma compatibility

The next universal release targets macOS 14.0 and later on Intel and Apple silicon. Keep the existing bundle identifier, settings, keyboard shortcuts, and Homebrew cask name so users can upgrade in place.

Use one runtime material selection for the launcher and the Appearance preview. On Sonoma and Sequoia, use the launcher's existing AppKit HUD blur. On Tahoe and later, use Liquid Glass when the class is available. Preserve the existing blur override for testing. Resolve the glass class in a way that allows the app to load on systems where that class is absent. Both materials must size the same content and preserve the selected app's tint.

Build both architectures with a 14.0 deployment target. Check the resulting Mach-O load commands, imported symbols, and AppKit selectors against the minimum. Set the bundle minimum and release cask minimum to Sonoma. Keep the public install instructions accurate for the version currently available.

Before publishing a release that advertises Sonoma support, launch the signed app on Sonoma and exercise the global shortcut, app search and launch, Appearance preview, and login-item registration. On Tahoe or newer, check both the default glass and forced blur paths. Record the OS and architecture actually tested.
