#!/bin/sh
# Build the launcher, install it as an app in /Applications, and set it to
# start at login. Run again after any code change to update the installed copy.
set -eu
cd "$(dirname "$0")/.."

APP="/Applications/Rusty Spotlight.app"
LABEL="local.rusty-spotlight"
AGENT="$HOME/Library/LaunchAgents/$LABEL.plist"
LOGS="$HOME/Library/Logs/rusty-spotlight"

cargo build --release

# Stop whatever copy is running, whether started by hand or at login.
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
pkill -x rusty-spotlight 2>/dev/null || true

mkdir -p "$LOGS"
scripts/bundle.sh "$APP" target/release/rusty-spotlight "$LABEL"
# Signed for this Mac only (ad hoc); enough to run here, not to give to anyone else.
# The requirement names the bundle identifier instead of this build's hash, so
# a permission granted once (Input Monitoring) still applies after a rebuild.
codesign --force --sign - -r="designated => identifier \"$LABEL\"" "$APP"
# Have Finder and the Dock pick up the icon of the new copy.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" 2>/dev/null || true

mkdir -p "$(dirname "$AGENT")"
cat > "$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$APP/Contents/MacOS/rusty-spotlight</string></array>
  <key>RunAtLoad</key><true/>
  <!-- Restart after a crash, but stay quit when quit from the menu bar. -->
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardErrorPath</key><string>$LOGS/launcher.log</string>
  <key>StandardOutPath</key><string>$LOGS/launcher.log</string>
</dict>
</plist>
PLIST
# "Open at login" switched off in Settings disables the login item; macOS then
# refuses to load it, so start the app directly instead.
if launchctl print-disabled "gui/$(id -u)" | grep -q "\"$LABEL\" => \(disabled\|true\)"; then
  open "$APP"
  WHEN="Open at login is off (Settings > General)."
else
  launchctl bootstrap "gui/$(id -u)" "$AGENT"
  WHEN="Starts at login."
fi

echo "Installed $APP"
echo "$WHEN Log: $LOGS/launcher.log"
echo "To remove: scripts/uninstall.sh"
