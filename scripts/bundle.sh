#!/bin/sh
# Assemble Rusty Spotlight.app from a built binary. Used by install.sh (this
# Mac's own copy) and release.sh (the copy given to others).
#   scripts/bundle.sh <app folder> <binary> <bundle identifier>
set -eu
cd "$(dirname "$0")/.."
APP="$1"; BINARY="$2"; ID="$3"
# One version for everything: the workspace's, in Cargo.toml.
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/rusty-spotlight"
# Drawn by scripts/make-icon.swift.
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>Rusty Spotlight</string>
  <key>CFBundleDisplayName</key><string>Rusty Spotlight</string>
  <key>CFBundleExecutable</key><string>rusty-spotlight</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
