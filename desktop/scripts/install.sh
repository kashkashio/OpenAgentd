#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# OpenAgentd installer for macOS + Linux.
#
# Windows users: run the bundled ``OpenAgentd-x.y.z-x64.msi`` installer
# instead. It already handles registration, Start Menu shortcut, and
# uninstall. (SmartScreen may warn on first launch — click "More info"
# → "Run anyway"; the binary is unsigned but not malicious.)
#
# ──────────────────────────────────────────────────────────────────────────────
# macOS branch
# ──────────────────────────────────────────────────────────────────────────────
# We don't ship a paid Apple Developer ID, so the bundle that comes
# out of CI is unsigned. macOS Gatekeeper rejects unsigned (or
# improperly re-signed) apps with:
#
#     "OpenAgentd.app" is damaged and can't be opened. You should
#      move it to the Trash.
#
# This is a lie — the bundle is fine, it's just unsigned. The fix
# is to sign it locally using your own machine as the signer.
# That's exactly what every open-source macOS app you compile from
# source already does; we're just doing it for you here.
#
# Steps:
#   1. Strip ``com.apple.quarantine`` xattr (set by the browser).
#   2. Strip any pre-existing invalid signature.
#   3. Sign recursively with your Apple Development identity, else a
#      local one kept in its own keychain (no password prompts), else
#      ad hoc (``-s -``), with the hardened runtime and the bundle's
#      ``entitlements.plist``.
#   4. Verify the result.
#   5. (Optional, with ``--install``) copy to /Applications.
#
# ──────────────────────────────────────────────────────────────────────────────
# Linux branch
# ──────────────────────────────────────────────────────────────────────────────
# Linux has no Gatekeeper-equivalent; signing is unnecessary. We
# instead:
#   1. ``chmod +x`` the AppImage / extracted binary.
#   2. Move it to ``~/.local/bin/openagentd`` (user-local, no sudo).
#   3. Drop a ``.desktop`` file under
#      ``~/.local/share/applications/`` so it appears in launchers.
#   4. Drop an icon under ``~/.local/share/icons/hicolor/.../apps/``.
#   5. Refresh the desktop database if ``update-desktop-database`` is
#      available.
#
# Both branches share the bundle-detection logic and the CLI surface.
#
# ──────────────────────────────────────────────────────────────────────────────
# Usage
# ──────────────────────────────────────────────────────────────────────────────
#     # Run with no args from the directory containing the bundle:
#     ./install.sh
#
#     # Point at a specific bundle / binary:
#     ./install.sh /path/to/OpenAgentd.app           # macOS
#     ./install.sh /path/to/OpenAgentd.AppImage      # Linux
#
#     # Also install into a system location (macOS → /Applications,
#     # Linux → ~/.local/bin + .desktop entry):
#     ./install.sh --install
#
#     # macOS only: skip the install copy but force resigning even if
#     # an existing signature is detected.
#     ./install.sh --force
#
# Exit codes:
#   0  success
#   1  cannot locate bundle / unsupported platform
#   2  signing failed (macOS) / chmod failed (Linux)
#   3  verification failed (macOS)
#   4  install copy failed
# ──────────────────────────────────────────────────────────────────────────────

set -euo pipefail

# ── ANSI colors (TTY only) ────────────────────────────────────────────────────
if [ -t 1 ]; then
  RED=$'\033[0;31m'
  YELLOW=$'\033[0;33m'
  GREEN=$'\033[0;32m'
  BLUE=$'\033[0;34m'
  BOLD=$'\033[1m'
  RESET=$'\033[0m'
else
  RED= YELLOW= GREEN= BLUE= BOLD= RESET=
fi

info() { printf '%s[oad]%s %s\n' "$BLUE"   "$RESET" "$*" >&2; }
warn() { printf '%s[oad]%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
ok()   { printf '%s[oad]%s %s\n' "$GREEN"  "$RESET" "$*" >&2; }
fail() { printf '%s[oad]%s %s\n' "$RED"    "$RESET" "$*" >&2; }

# ── Argument parsing ──────────────────────────────────────────────────────────
DO_INSTALL=0
FORCE_RESIGN=0
TARGET=""
for arg in "$@"; do
  case "$arg" in
    --install)
      DO_INSTALL=1
      ;;
    --force)
      FORCE_RESIGN=1
      ;;
    -h|--help)
      sed -n '2,74p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    --*)
      fail "Unknown option: $arg"
      exit 1
      ;;
    *)
      TARGET="$arg"
      ;;
  esac
done

# ── Detect platform ───────────────────────────────────────────────────────────
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
uname_s="$(uname -s)"

case "$uname_s" in
  Darwin) PLATFORM=macos ;;
  Linux)  PLATFORM=linux ;;
  *)
    fail "Unsupported platform: $uname_s"
    fail "Windows users: run OpenAgentd-*.msi instead."
    exit 1
    ;;
esac

info "Platform: ${BOLD}${PLATFORM}${RESET}"

# ════════════════════════════════════════════════════════════════════════════════
# macOS branch
# ════════════════════════════════════════════════════════════════════════════════
if [ "$PLATFORM" = "macos" ]; then

  # ── Locate the .app bundle ──────────────────────────────────────────────────
  BUNDLE="$TARGET"
  if [ -z "$BUNDLE" ]; then
    for candidate in \
      "$script_dir/OpenAgentd.app" \
      "$script_dir/../OpenAgentd.app" \
      "$PWD/OpenAgentd.app"; do
      if [ -d "$candidate" ]; then
        BUNDLE="$candidate"
        break
      fi
    done
  fi

  if [ -z "$BUNDLE" ] || [ ! -d "$BUNDLE" ] || [ "${BUNDLE##*.}" != "app" ]; then
    fail "Cannot find OpenAgentd.app. Pass the path explicitly:"
    fail "    $0 /path/to/OpenAgentd.app"
    exit 1
  fi

  BUNDLE="$(cd -- "$BUNDLE" && pwd)"
  info "Bundle: ${BOLD}${BUNDLE}${RESET}"

  # ── 1. Strip quarantine xattr ───────────────────────────────────────────────
  # Without this, even a freshly-signed bundle will trigger
  # Gatekeeper's first-launch translocation, which copies the app to
  # a read-only location and silently breaks the sidecar discovery.
  info "Stripping quarantine xattr…"
  if xattr -p com.apple.quarantine "$BUNDLE" &>/dev/null; then
    xattr -dr com.apple.quarantine "$BUNDLE" || warn "xattr -dr returned non-zero"
    ok "Quarantine xattr removed"
  else
    ok "No quarantine xattr present"
  fi
  xattr -cr "$BUNDLE" 2>/dev/null || true

  # ── 2. Refuse to overwrite a real signature ─────────────────────────────────
  existing_authority="$(codesign -dv --verbose=2 "$BUNDLE" 2>&1 \
    | awk -F= '/Authority=/ {print $2; exit}')" || true
  if [ "$FORCE_RESIGN" = "0" ] \
      && [ -n "${existing_authority:-}" ] \
      && [ "$existing_authority" != "(unknown)" ] \
      && [ "$existing_authority" != "-" ]; then
    warn "Bundle is already signed by: $existing_authority"
    warn "Refusing to overwrite. Re-run with ${BOLD}--force${RESET} to clobber it."
    exit 0
  fi

  # ── 3. Pick entitlements ────────────────────────────────────────────────────
  entitlements_file=""
  for candidate in \
    "$BUNDLE/Contents/Resources/entitlements.plist" \
    "$script_dir/entitlements.plist" \
    "$script_dir/../src-tauri/entitlements.plist"; do
    if [ -f "$candidate" ]; then
      entitlements_file="$candidate"
      break
    fi
  done

  if [ -z "$entitlements_file" ]; then
    warn "No entitlements.plist found; writing a minimal one to /tmp."
    entitlements_file="$(mktemp -t oad-ents).plist"
    cat > "$entitlements_file" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.network.client</key>
    <true/>
    <key>com.apple.security.network.server</key>
    <true/>
    <key>com.apple.security.files.user-selected.read-write</key>
    <true/>
</dict>
</plist>
PLIST
  fi
  info "Entitlements: ${entitlements_file}"

  bundle_id="com.openagentd.desktop"
  info "Bundle Identifier: ${bundle_id}"

  # ── 4. Sign the bundle with the local identity ──────────────────────────────
  # A persistent identity ("OpenAgentd Local Signer") keeps macOS TCC (Desktop
  # folder) and Keychain permissions across updates. It lives in a keychain of
  # its own, not the login keychain: with a login-keychain key, codesign showed
  # "codesign wants to access key …" for every signature unless the user picked
  # Always Allow. We hold this keychain's password, so it unlocks and lets
  # codesign in without a prompt. The password is not a secret: the cert is
  # trusted nowhere and the designated requirement below does not pin it, so
  # the key grants nothing an ad-hoc signature could not. The desktop updater
  # (src-tauri/src/updater.rs) and the Homebrew cask use the same values.
  local_cert_name="OpenAgentd Local Signer"
  signing_keychain="$HOME/Library/Keychains/openagentd-signing.keychain-db"
  signing_keychain_password="openagentd-local-signing"

  # Unlock the signing keychain, creating it and the identity on first use.
  ensure_local_signer() {
    if [ ! -f "$signing_keychain" ]; then
      security create-keychain -p "$signing_keychain_password" "$signing_keychain" >/dev/null 2>&1 || return 1
      # No auto-lock timeout; it is unlocked again before every use anyway.
      security set-keychain-settings "$signing_keychain" >/dev/null 2>&1 || true
    fi
    security unlock-keychain -p "$signing_keychain_password" "$signing_keychain" >/dev/null 2>&1 || return 1
    # Captured, not piped into ``grep -q``: under pipefail an early grep exit
    # can fail the pipe and mint a duplicate identity.
    local identities
    identities="$(security find-identity -p codesigning "$signing_keychain" 2>/dev/null || true)"
    case "$identities" in *"\"$local_cert_name\""*) return 0 ;; esac

    info "Creating the local signing identity…"
    local dir status=1
    dir="$(mktemp -d)"
    cat > "$dir/cert.cnf" <<'EOF'
[req]
distinguished_name = dn
prompt = no

[dn]
CN = OpenAgentd Local Signer
O = OpenAgentd Local

[v3_req]
basicConstraints = CA:FALSE
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
EOF
    # The system LibreSSL by absolute path: its default PKCS#12 encryption is
    # the one ``security import`` reads (OpenSSL 3 needs ``-legacy``, which
    # LibreSSL rejects). Without the partition list codesign still prompts.
    if /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -config "$dir/cert.cnf" -extensions v3_req \
        -keyout "$dir/key.pem" -out "$dir/cert.pem" &>/dev/null \
      && /usr/bin/openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" \
        -name "$local_cert_name" -out "$dir/openagentd-signing.p12" -passout pass:openagentd &>/dev/null \
      && security import "$dir/openagentd-signing.p12" -k "$signing_keychain" -P openagentd -T /usr/bin/codesign &>/dev/null \
      && security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
        -k "$signing_keychain_password" "$signing_keychain" &>/dev/null; then
      status=0
    fi
    rm -rf "$dir"
    return "$status"
  }

  # The explicit identifier-only designated requirement (-r=) is applied on
  # every signing path. Without it codesign derives a default requirement
  # that pins the signing certificate; local self-signed certs are not
  # stable across regenerations, so macOS keychain "Always Allow" and TCC
  # grants would be invalidated on the next update.
  sign_bundle() {
    local args=(--force --deep --options runtime
      "-r=designated => identifier \"$bundle_id\""
      --entitlements "$entitlements_file")
    if [ "$signing_identity" = "-" ]; then
      codesign "${args[@]}" --sign - --timestamp=none "$1"
    elif [ "$signing_identity" = "$local_cert_name" ]; then
      codesign "${args[@]}" --keychain "$signing_keychain" --sign "$signing_identity" "$1"
    else
      codesign "${args[@]}" --sign "$signing_identity" "$1"
    fi
  }

  # An Apple Development identity wins when there is one, as before: Xcode
  # made its key, and its Team ID scopes the app's own keychain items.
  signing_identity="-"
  login_identities="$(security find-identity -v -p codesigning 2>/dev/null || true)"
  apple_dev="$(printf '%s\n' "$login_identities" | awk -F'"' '/"Apple Development:/ && !n { print $2; n = 1 }')"
  if [ -n "$apple_dev" ]; then
    signing_identity="$apple_dev"
  elif ensure_local_signer; then
    signing_identity="$local_cert_name"
  else
    warn "Could not set up the local signing identity; signing ad hoc."
  fi

  info "Signing identity: ${signing_identity}"
  info "Signing the bundle (this can take a few seconds)…"
  if ! sign_bundle "$BUNDLE" 2>&1 | sed 's/^/  /'; then
    fail "codesign failed."
    exit 2
  fi
  ok "Signature applied"

  # ── 5. Verify ───────────────────────────────────────────────────────────────
  info "Verifying signature…"
  if ! codesign --verify --verbose=1 "$BUNDLE" 2>&1 | sed 's/^/  /'; then
    fail "Verification failed."
    exit 3
  fi
  ok "Signature verified"

  if spctl --assess --verbose=4 "$BUNDLE" &>/dev/null; then
    ok "Gatekeeper accepts the bundle"
  else
    warn "Gatekeeper still flags the bundle — this is expected without an Apple Developer ID."
    warn "${BOLD}Right-click the app → ${RESET}${BOLD}${YELLOW}Open${RESET}${BOLD} on first launch.${RESET}"
  fi

  # ── 6. Optional: install to /Applications ───────────────────────────────────
  if [ "$DO_INSTALL" = "1" ]; then
    bundle_name="$(basename "$BUNDLE")"
    dest="/Applications/$bundle_name"
    if [ -d "$dest" ]; then
      warn "Overwriting existing $dest"
      rm -rf "$dest"
    fi
    info "Copying to $dest …"
    if ! ditto "$BUNDLE" "$dest"; then
      fail "Copy failed."
      exit 4
    fi
    # ``ditto`` preserves signatures across volumes, but a different
    # filesystem can perturb xattrs — re-sign at destination to be
    # safe. Failure here is non-fatal.
    sign_bundle "$dest" >/dev/null 2>&1 || true
    ok "Installed to $dest"
  fi

  ok "${BOLD}Done.${RESET} You can launch OpenAgentd now."
  exit 0
fi

# ════════════════════════════════════════════════════════════════════════════════
# Linux branch
# ════════════════════════════════════════════════════════════════════════════════
if [ "$PLATFORM" = "linux" ]; then

  # ── Locate the binary / AppImage ────────────────────────────────────────────
  # Tauri produces several Linux artifacts; we accept any of them.
  # Preference order: AppImage (self-contained) → standalone binary
  # → .deb (defer to dpkg).
  BINARY="$TARGET"
  if [ -z "$BINARY" ]; then
    for candidate in \
      "$script_dir"/OpenAgentd*.AppImage \
      "$script_dir"/../OpenAgentd*.AppImage \
      "$PWD"/OpenAgentd*.AppImage \
      "$script_dir/openagentd" \
      "$script_dir/../openagentd"; do
      # Glob expansion can leave the literal pattern if there's no
      # match — guard with [ -f ].
      if [ -f "$candidate" ]; then
        BINARY="$candidate"
        break
      fi
    done
  fi

  if [ -z "$BINARY" ] || [ ! -f "$BINARY" ]; then
    fail "Cannot find OpenAgentd binary / AppImage. Pass the path explicitly:"
    fail "    $0 /path/to/OpenAgentd.AppImage"
    exit 1
  fi

  # If user handed us a .deb, defer to the package manager.
  case "$BINARY" in
    *.deb)
      info "Detected .deb package; deferring to dpkg."
      if ! sudo dpkg -i "$BINARY"; then
        fail "dpkg install failed. Try: sudo apt-get install -f"
        exit 4
      fi
      ok "${BOLD}Done.${RESET}"
      exit 0
      ;;
    *.rpm)
      info "Detected .rpm package; deferring to rpm."
      if ! sudo rpm -Uvh "$BINARY"; then
        fail "rpm install failed."
        exit 4
      fi
      ok "${BOLD}Done.${RESET}"
      exit 0
      ;;
  esac

  BINARY="$(cd -- "$(dirname -- "$BINARY")" && pwd)/$(basename -- "$BINARY")"
  info "Binary: ${BOLD}${BINARY}${RESET}"

  # ── 1. Make it executable ───────────────────────────────────────────────────
  if [ ! -x "$BINARY" ]; then
    info "Setting executable bit…"
    if ! chmod +x "$BINARY"; then
      fail "chmod failed."
      exit 2
    fi
  fi
  ok "Executable"

  if [ "$DO_INSTALL" = "0" ]; then
    ok "${BOLD}Done.${RESET} Launch with: ${BINARY}"
    ok "Pass ${BOLD}--install${RESET} to register a desktop launcher entry."
    exit 0
  fi

  # ── 2. Install to ~/.local/bin ──────────────────────────────────────────────
  # Per the XDG Base Directory spec, user-local executables go in
  # ``$XDG_BIN_HOME`` (with ``~/.local/bin`` as the conventional
  # fallback). This is on $PATH by default on Ubuntu 22.04+ /
  # Fedora 36+; older distros need the user to add it manually.
  bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
  mkdir -p "$bin_dir"
  dest_bin="$bin_dir/openagentd"
  info "Copying binary to $dest_bin …"
  if ! install -m 0755 "$BINARY" "$dest_bin"; then
    fail "install(1) failed."
    exit 4
  fi
  ok "Binary installed"

  # ── 3. Drop a .desktop entry ────────────────────────────────────────────────
  apps_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  mkdir -p "$apps_dir"
  desktop_file="$apps_dir/openagentd.desktop"

  # ``Exec=`` must be an absolute path or a basename on $PATH. We
  # use the absolute path to avoid surprises when ~/.local/bin
  # isn't on $PATH yet.
  cat > "$desktop_file" <<DESKTOP
[Desktop Entry]
Type=Application
Name=OpenAgentd
GenericName=AI Assistant
Comment=On-machine AI assistant
Exec=${dest_bin} %U
Icon=openagentd
Terminal=false
Categories=Development;Utility;Office;
StartupWMClass=OpenAgentd
StartupNotify=true
Keywords=AI;assistant;llm;chat;
DESKTOP
  chmod 0644 "$desktop_file"
  ok "Desktop entry: $desktop_file"

  # ── 4. Copy icon if present ─────────────────────────────────────────────────
  # Tauri ships icons under ``icons/`` next to the bundle; the
  # 512×512 PNG is the canonical one for the hicolor theme.
  icons_root="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
  for size in 32 64 128 256 512; do
    for src in \
      "$script_dir/icons/${size}x${size}.png" \
      "$script_dir/../icons/${size}x${size}.png" \
      "$script_dir/icons/${size}x${size}@2x.png"; do
      if [ -f "$src" ]; then
        target_dir="$icons_root/${size}x${size}/apps"
        mkdir -p "$target_dir"
        cp -f "$src" "$target_dir/openagentd.png"
      fi
    done
  done

  # ── 5. Refresh desktop / icon caches ────────────────────────────────────────
  # These commands are optional — distros without them still pick
  # up the .desktop file on next login. We swallow errors so a
  # missing tool doesn't fail the install.
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" >/dev/null 2>&1 || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$icons_root" >/dev/null 2>&1 || true
  fi

  # ── 6. PATH sanity check ────────────────────────────────────────────────────
  case ":$PATH:" in
    *":$bin_dir:"*)
      ok "${BOLD}Done.${RESET} Launch with: ${BOLD}openagentd${RESET}"
      ;;
    *)
      warn "${bin_dir} is not on \$PATH. Add this to your shell rc:"
      warn "    export PATH=\"${bin_dir}:\$PATH\""
      warn "Or launch via the desktop menu / file manager."
      ;;
  esac

  exit 0
fi
