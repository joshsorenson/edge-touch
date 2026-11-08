#!/bin/sh
# Build a download other people can open: universal (Apple silicon + Intel),
# signed with Developer ID, notarized by Apple, zipped, and published as a
# GitHub release.
#
#   scripts/release.sh            build, notarize, publish v<version>
#   scripts/release.sh --no-publish
#
# One-time setup (stores your Apple ID app-specific password in the keychain;
# make one at appleid.apple.com > Sign-In and Security > App-Specific Passwords):
#
#   xcrun notarytool store-credentials edge-touch --apple-id <you@example.com> --team-id <TEAMID>
#
# Settings (environment):
#   EDGE_TOUCH_RELEASE_SIGN  Developer ID identity (default: the first
#                            "Developer ID Application" in your keychain)
#   NOTARY_PROFILE           keychain profile from store-credentials (default: edge-touch)
set -eu
cd "$(dirname "$0")/.."

SIGN="${EDGE_TOUCH_RELEASE_SIGN:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
[ -n "$SIGN" ] || { echo "No Developer ID Application identity in the keychain." >&2; exit 1; }
PROFILE="${NOTARY_PROFILE:-edge-touch}"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/Edge Touch.app"
ZIP="dist/Edge-Touch-$VERSION.zip"

EDGE_TOUCH_SIGN="$SIGN" EDGE_TOUCH_UNIVERSAL=1 scripts/bundle.sh --no-install
codesign --verify --deep --strict "$APP"

# Notarize (Apple scans the app), then staple the ticket so it opens offline.
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
spctl --assess --type execute --verbose "$APP"
echo "Ready: $ZIP"

[ "${1:-}" = "--no-publish" ] && exit 0
gh release create "v$VERSION" "$ZIP" --title "Edge Touch $VERSION" --notes-file - <<EOF
Makes the Corsair XENEON Edge a real touchscreen on macOS.

**Install:** download Edge-Touch-$VERSION.zip, unzip, and drag Edge Touch to Applications. Open it, then allow Accessibility and Input Monitoring when macOS asks. A short tour on the Edge shows you the gestures.

Works on Apple silicon and Intel Macs, macOS 13 or later. Signed and notarized.
EOF
