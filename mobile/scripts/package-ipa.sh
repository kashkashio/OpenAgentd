#!/usr/bin/env bash
#
# Package the release xcarchive into a sideloadable IPA.
#
# `cargo tauri ios build --archive-only` signs with the maintainer's
# development team, so the archived app carries an embedded provisioning
# profile (which lists the maintainer's device UDIDs) and development
# entitlements such as get-task-allow. It also contains the Rust static
# library `libapp.a` (~185 MB), which the generated project embeds even
# though nothing loads it at runtime.
#
# SideStore, AltStore, Sideloadly and TrollStore re-sign every IPA with the
# installing user's identity, so the IPA ships ad-hoc signed with no
# entitlements and none of the above. The SideStore source feed lists the
# same (empty) entitlements; a mismatch makes SideStore refuse the install.
#
# Output: dist/OpenAgentd_<version>_iOS.ipa and its .sha256.

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(../scripts/release_version.sh)"
APP_NAME="OpenAgentd.app"
ARCHIVE_APP="src-tauri/gen/apple/build/openagentd-mobile_iOS.xcarchive/Products/Applications/$APP_NAME"
DIST_DIR="dist"
IPA_NAME="OpenAgentd_${VERSION}_iOS.ipa"

die() {
  echo "package-ipa: error: $*" >&2
  exit 1
}

[ -d "$ARCHIVE_APP" ] || die "archived app not found at $ARCHIVE_APP (run 'make ios-ipa')"

built_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$ARCHIVE_APP/Info.plist")"
[ "$built_version" = "$VERSION" ] \
  || die "archived app is version $built_version, release version is $VERSION (stale archive?)"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
app="$stage/Payload/$APP_NAME"
mkdir -p "$stage/Payload"
ditto "$ARCHIVE_APP" "$app"

# Strip the developer's provisioning profile, the old signature and the
# never-loaded static library before signing.
rm -rf "$app/embedded.mobileprovision" "$app/_CodeSignature"
find "$app" -name '*.a' -type f -delete

# Sign nested code first, then the bundle. No --entitlements: ad-hoc, empty.
if [ -d "$app/Frameworks" ]; then
  find "$app/Frameworks" \( -name '*.framework' -o -name '*.dylib' \) -maxdepth 1 -print0 \
    | xargs -0 -I{} codesign --force --sign - --timestamp=none {}
fi
codesign --force --sign - --timestamp=none "$app"

codesign --verify --strict "$app" || die "signature verification failed"
entitlements="$(codesign -d --entitlements - --xml "$app" 2>/dev/null || true)"
[ -z "$entitlements" ] || die "signed app unexpectedly has entitlements: $entitlements"
[ ! -e "$app/embedded.mobileprovision" ] || die "embedded.mobileprovision still present"
[ -z "$(find "$app" -name '*.a' -type f -print -quit)" ] || die "static library still present"

mkdir -p "$DIST_DIR"
dist_abs="$(cd "$DIST_DIR" && pwd)"
rm -f "$DIST_DIR/$IPA_NAME" "$DIST_DIR/$IPA_NAME.sha256"
(cd "$stage" && zip -qry -X "$dist_abs/$IPA_NAME" Payload)
(cd "$DIST_DIR" && shasum -a 256 "$IPA_NAME" > "$IPA_NAME.sha256")

size="$(du -h "$DIST_DIR/$IPA_NAME" | cut -f1)"
echo "package-ipa: wrote mobile/$DIST_DIR/$IPA_NAME ($size)"
echo "package-ipa: wrote mobile/$DIST_DIR/$IPA_NAME.sha256"
