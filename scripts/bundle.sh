#!/bin/sh
# Build Edge Touch.app and install it in /Applications.
#
#   scripts/bundle.sh                build, sign, install, relaunch
#   scripts/bundle.sh --no-install   just build dist/Edge Touch.app
#
# Settings (environment):
#   EDGE_TOUCH_SIGN       signing identity. Default: your "Apple Development"
#                         identity, so macOS permissions survive rebuilds.
#                         "-" signs ad-hoc (macOS then re-asks for permissions
#                         after every build).
#   EDGE_TOUCH_UNIVERSAL  1 builds for Apple silicon and Intel in one app.
#
# For a build other people can download, use scripts/release.sh.
set -eu
cd "$(dirname "$0")/.."

SIGN="${EDGE_TOUCH_SIGN:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)}"
SIGN="${SIGN:--}"
APP="dist/Edge Touch.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ "${EDGE_TOUCH_UNIVERSAL:-0}" = "1" ]; then
  taskpolicy -c utility cargo build --release --target aarch64-apple-darwin
  taskpolicy -c utility cargo build --release --target x86_64-apple-darwin
  mkdir -p target/universal
  lipo -create -output target/universal/edge-touch \
    target/aarch64-apple-darwin/release/edge-touch target/x86_64-apple-darwin/release/edge-touch
  BIN=target/universal/edge-touch
else
  taskpolicy -c utility cargo build --release
  BIN=target/release/edge-touch
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/edge-touch"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.thesqd.edgetouch</string>
  <key>CFBundleName</key><string>Edge Touch</string>
  <key>CFBundleDisplayName</key><string>Edge Touch</string>
  <key>CFBundleExecutable</key><string>edge-touch</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>Made by Josh Sorenson. joshsorenson.com</string>
</dict>
</plist>
EOF
# Hardened runtime + secure timestamp: both required for notarization.
TS="--timestamp"
[ "$SIGN" = "-" ] && TS=""
codesign --force --options runtime $TS --sign "$SIGN" "$APP"
echo "Built $APP (signed with: $SIGN)"

[ "${1:-}" = "--no-install" ] && exit 0
pkill -x edge-touch 2>/dev/null && sleep 1 || true
ditto "$APP" "/Applications/Edge Touch.app"
open "/Applications/Edge Touch.app"
echo "Installed and opened /Applications/Edge Touch.app"
