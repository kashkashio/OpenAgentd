"""Regression tests for security-patched dependency floors.

These tests guard the lockfiles against accidentally reintroducing versions that
are known to be vulnerable or incompatible with the current build stack.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _version_tuple(version: str) -> tuple[int, ...]:
    """Return numeric release components for simple pinned dependency versions."""
    return tuple(int(part) for part in version.split(".") if part.isdigit())


def test_web_lock_pins_patched_mcp_sdk_without_reintroducing_react_plugin_break() -> (
    None
):
    """Any MCP SDK v1 copy is patched; Vite React plugin stays on the compatible major.

    The web app moved to ``@modelcontextprotocol/client``/``core`` v2 and no
    longer depends on ``@modelcontextprotocol/sdk``. A direct dependency or a
    transitive copy that comes back must still be at least 1.26.0.
    """
    package_json = json.loads((ROOT / "web/package.json").read_text())
    direct = package_json["dependencies"].get("@modelcontextprotocol/sdk")
    if direct is not None:
        assert _version_tuple(direct.lstrip("^~")) >= _version_tuple("1.26.0")
    assert _version_tuple(
        package_json["devDependencies"]["@vitejs/plugin-react"]
    ) >= _version_tuple("6.0.3")

    lock_text = (ROOT / "web/bun.lock").read_text()
    locked = re.findall(r'"@modelcontextprotocol/sdk@([0-9][^"]*)"', lock_text)
    for version in locked:
        assert _version_tuple(version) >= _version_tuple("1.26.0"), version


def test_desktop_lock_keeps_patched_tar_version() -> None:
    """Desktop updater archive handling uses the patched tar release."""
    lock_text = (ROOT / "desktop/src-tauri/Cargo.lock").read_text()
    match = re.search(r'name = "tar"\nversion = "(?P<version>[^"]+)"', lock_text)

    assert match is not None
    assert _version_tuple(match.group("version")) >= _version_tuple("0.4.46")
