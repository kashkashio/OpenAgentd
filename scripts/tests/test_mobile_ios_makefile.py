from __future__ import annotations

import json
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
MOBILE_MAKEFILE = REPO_ROOT / "mobile" / "Makefile"
TAURI_CONFIG = REPO_ROOT / "mobile" / "src-tauri" / "tauri.conf.json"
PACKAGE_IPA = REPO_ROOT / "mobile" / "scripts" / "package-ipa.sh"
PUBLISH_IPA = REPO_ROOT / "mobile" / "scripts" / "publish-ipa.sh"


def _target_recipe(makefile: str, target: str) -> str:
    lines = makefile.splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith(f"{target}:"))
    recipe = []
    for line in lines[start + 1 :]:
        if not line.startswith("\t"):
            break
        recipe.append(line.strip())
    return "\n".join(recipe)


def test_fast_device_install_uses_debug_archive_and_existing_installer_path():
    makefile = MOBILE_MAKEFILE.read_text()

    assert "ios-install-device-fast:" in makefile
    assert "cargo tauri ios build --debug --archive-only" in makefile
    assert "Products/Applications/OpenAgentd.app" in makefile


def test_ios_web_build_is_skipped_when_dist_is_fresh():
    makefile = MOBILE_MAKEFILE.read_text()
    config = json.loads(TAURI_CONFIG.read_text())

    assert "ios-web:" in makefile
    assert "find $(WEB_DIR)/src $(WEB_DIR)/public" in makefile
    assert "Web bundle is current; skipping build" in makefile
    assert config["build"]["beforeBuildCommand"] == {
        "cwd": "../..",
        "script": "make -C mobile ios-web",
    }


def test_ios_ipa_packages_a_release_archive():
    makefile = MOBILE_MAKEFILE.read_text()
    recipe = _target_recipe(makefile, "ios-ipa")

    assert "cargo tauri ios build --archive-only" in recipe
    assert "--debug" not in recipe
    assert "./scripts/package-ipa.sh" in recipe
    assert makefile.index("ios-ipa:") < makefile.index("%:")


def test_ios_release_builds_then_publishes():
    makefile = MOBILE_MAKEFILE.read_text()

    assert any(line.startswith("ios-release: ios-ipa") for line in makefile.splitlines())
    assert "./scripts/publish-ipa.sh" in _target_recipe(makefile, "ios-release")
    assert makefile.index("ios-release:") < makefile.index("%:")


def test_ipa_packaging_strips_developer_signing_and_static_library():
    script = PACKAGE_IPA.read_text()

    assert "embedded.mobileprovision" in script
    assert "-name '*.a' -type f -delete" in script
    assert "codesign --force --sign - --timestamp=none" in script
    assert "--entitlements -" in script
    assert "CFBundleShortVersionString" in script


def test_ipa_publish_keeps_rolling_source_off_releases_latest():
    script = PUBLISH_IPA.read_text()

    assert 'ROLLING_TAG="latest-ios"' in script
    assert "--prerelease --latest=false" in script
    assert "--clobber" in script
    assert "make_sidestore_source.py" in script
    assert 'DRY_RUN="${DRY_RUN:-0}"' in script
