# OpenAgentd mobile

Remote-backend-only Tauri mobile shell for OpenAgentd.

The mobile app embeds the shared React Web UI from `../web/dist` and connects to an existing OpenAgentd API server. It does not bundle or start a backend.

## Local development

Run the backend and Web UI from the repo root or separate directories:

```bash
make run          # backend API on :8000
cd web && make dev # Vite Web UI on :5173
```

Then run the mobile shell:

```bash
cd mobile
make dev
```

## Icons

The mobile shell keeps `src-tauri/icons/icon.png` as the source icon. Generated icon outputs are ignored; regenerate them when the source changes:

```bash
cd mobile
make ios-icons
```

## iOS

Initialize iOS project files once. The public Apple development team
(`com.openagentd.mobile`) is configured in `src-tauri/tauri.conf.json`, so no
local team file is needed:

```bash
cd mobile
make ios-init
```

Run on simulator/device:

```bash
make ios-dev
```

Install a bundled debug build for faster iteration on a physical iPhone:

```bash
make ios-install-device-fast <device-name>
```

The fast target preserves Cargo and Xcode incremental build state and skips the Web build when `web/dist` is newer than its inputs. Before shipping, install a production build:

```bash
make ios-install-device <device-name>
```

For a physical iPhone, expose the dev servers on the LAN first:

```bash
cd ../web && bun dev --host 0.0.0.0
cd .. && OPENAGENTD_ACCESS_KEY=<choose-a-key> APP_ENV=development \
  cargo run --manifest-path appv3/Cargo.toml -p appv3-cli -- server serve --host 0.0.0.0 --port 8000
```

The server refuses to listen on the LAN without an access key; enter the same
key in the app's **Backend connection**. An installed CLI does the same with
`openagentd server start --host 0.0.0.0 --key`.

Then run the iOS dev app:

```bash
make ios-dev-device <device-name>
```

`ios-dev-device` uses the dev-only iOS bundle identifier `com.dev.openagentd.mobile` and display name `OpenAgentd Dev`, so it can be installed alongside the production app (`com.openagentd.mobile`). Override locally when needed:

```bash
make ios-dev-device <device-name> IOS_DEV_IDENTIFIER=com.example.openagentd.dev IOS_DEV_PRODUCT_NAME="OpenAgentd Dev"
```

For example, if `cargo tauri ios dev` detects a device named `OfficePhone`, run:

```bash
make ios-dev-device "OfficePhone"
```

If the generated Xcode project gets stale after changing signing or identifiers, clean and regenerate it:

```bash
make ios-clean
make ios-init
```

Build:

```bash
make ios-build
```

`ios-dev-device` uses Vite/dev servers. Use `ios-install-device` when you need to verify the built-in production UI from `../web/dist`.

### Release IPA

Each release ships a sideloadable IPA and a SideStore/AltStore source feed. The user guide is [INSTALL-IOS.md](INSTALL-IOS.md). Build and package locally on a Mac with Xcode and the signing team from `tauri.conf.json`:

```bash
make ios-ipa       # release archive -> dist/OpenAgentd_<version>_iOS.ipa + .sha256
DRY_RUN=1 make ios-release   # all checks + dist/source.json, prints the uploads
make ios-release   # upload the IPA to v<version> and source.json to latest-ios
```

`scripts/package-ipa.sh` removes the developer provisioning profile (it lists the maintainer's device UDIDs), the development signature, and the never-loaded `libapp.a` static library, then ad-hoc signs the app with no entitlements. SideStore and other sideloading tools re-sign it with the installing user's identity. `scripts/publish-ipa.sh` requires a clean tree with HEAD at the `v<version>` tag and an existing GitHub release. It generates the feed with `../scripts/make_sidestore_source.py` from the IPA's own `Info.plist` and keeps it on the rolling `latest-ios` pre-release, so the feed URL never changes and the release never becomes `releases/latest`.

If iOS blocks the first launch, trust the developer profile on the phone in **Settings → General → VPN & Device Management**.

Use **Backend connection** in the app to save/check a remote server. Simulator builds can usually reach the Mac with `http://localhost:8000`; physical devices normally need a LAN IP or HTTPS endpoint. The server has no built-in TLS: for access beyond a trusted LAN, put an HTTPS reverse proxy in front of it and connect to the proxy's URL.

For local developer builds, set a unique iOS bundle identifier in `src-tauri/tauri.conf.json` if `com.openagentd.mobile` is already registered to another Apple developer team.
