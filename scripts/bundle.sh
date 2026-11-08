#!/bin/sh
# Build Edge Touch.app and install it in /Applications.
#
#   scripts/bundle.sh            build, sign, install, relaunch
#   scripts/bundle.sh --no-install
#
# Signing keeps macOS permissions (Input Monitoring, Accessibility) across
# rebuilds. Set EDGE_TOUCH_SIGN to your "Apple Development: ..." identity
# (see `security find-identity -v -p codesigning`), or "-" for ad-hoc, which
# makes macOS ask for permissions again after every build.
set -eu
cd "$(dirname "$0")/.."

SIGN="${EDGE_TOUCH_SIGN:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)}"
SIGN="${SIGN:--}"
APP="dist/Edge Touch.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

taskpolicy -c utility cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/edge-touch "$APP/Contents/MacOS/edge-touch"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.thesqd.edgetouch</string>
  <key>CFBundleName</key><string>Edge Touch</string>
  <key>CFBundleDisplayName</key><string>Edge Touch</string>
  <key>CFBundleExecutable</key><string>edge-touch</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
codesign --force --options runtime --sign "$SIGN" "$APP"
echo "Built $APP (signed with: $SIGN)"

[ "${1:-}" = "--no-install" ] && exit 0
pkill -x edge-touch 2>/dev/null && sleep 1 || true
rm -rf "/Applications/Edge Touch.app"
ditto "$APP" "/Applications/Edge Touch.app"
open "/Applications/Edge Touch.app"
echo "Installed and opened /Applications/Edge Touch.app"
