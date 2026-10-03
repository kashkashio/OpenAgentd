from __future__ import annotations

import importlib.util
import json
import plistlib
import zipfile
from pathlib import Path

import pytest


def _load_module():
    path = Path(__file__).resolve().parents[2] / "scripts" / "make_sidestore_source.py"
    spec = importlib.util.spec_from_file_location("make_sidestore_source", path)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


INFO = {
    "CFBundleIdentifier": "com.openagentd.mobile",
    "CFBundleShortVersionString": "3.7.0",
    "CFBundleVersion": "3.7.0",
    "MinimumOSVersion": "15.0",
    "NSMicrophoneUsageDescription": "Mic for voice input.",
    "NSLocalNetworkUsageDescription": "Local servers.",
    "NSAppTransportSecurity": {"NSAllowsLocalNetworking": True},
}


def _make_ipa(tmp_path: Path, info: dict | None = None, name: str = "app.ipa") -> Path:
    ipa = tmp_path / name
    with zipfile.ZipFile(ipa, "w") as archive:
        if info is not None:
            archive.writestr(
                "Payload/OpenAgentd.app/Info.plist",
                plistlib.dumps(info, fmt=plistlib.FMT_BINARY),
            )
        archive.writestr("Payload/OpenAgentd.app/OpenAgentd", b"binary")
    return ipa


def _run(module, tmp_path: Path, ipa: Path, *extra: str, version: str = "3.7.0") -> dict:
    notes = tmp_path / "notes.md"
    notes.write_text("Release notes")
    out = tmp_path / "source.json"
    code = module.main(
        [
            "--ipa",
            str(ipa),
            "--version",
            version,
            "--download-url",
            f"https://github.com/lthoangg/openagentd/releases/download/v{version}/x.ipa",
            "--date",
            "2026-10-05",
            "--notes-file",
            str(notes),
            "--out",
            str(out),
            *extra,
        ]
    )
    assert code == 0
    return json.loads(out.read_text())


def test_reads_app_metadata_and_permissions_from_ipa(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, INFO)

    source = _run(module, tmp_path, ipa)

    assert source["name"] == "OpenAgentd"
    [app] = source["apps"]
    assert app["bundleIdentifier"] == "com.openagentd.mobile"
    assert app["appPermissions"] == {
        "entitlements": [],
        "privacy": {
            "NSLocalNetworkUsageDescription": "Local servers.",
            "NSMicrophoneUsageDescription": "Mic for voice input.",
        },
    }
    [version] = app["versions"]
    assert version == {
        "version": "3.7.0",
        "buildVersion": "3.7.0",
        "date": "2026-10-05",
        "localizedDescription": "Release notes",
        "downloadURL": "https://github.com/lthoangg/openagentd/releases/download/v3.7.0/x.ipa",
        "size": ipa.stat().st_size,
        "minOSVersion": "15.0",
    }


def test_entitlements_are_listed_when_given(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, INFO)

    source = _run(module, tmp_path, ipa, "--entitlement", "b.ent", "--entitlement", "a.ent")

    assert source["apps"][0]["appPermissions"]["entitlements"] == ["a.ent", "b.ent"]


def test_merges_previous_feed_newest_first_and_replaces_same_version(tmp_path):
    module = _load_module()
    previous = tmp_path / "previous.json"
    old_versions = [{"version": "3.7.0", "date": "stale"}] + [
        {"version": f"3.{minor}.0", "date": "2026-01-01"} for minor in range(6, -6, -1)
    ]
    previous.write_text(json.dumps({"apps": [{"versions": old_versions}]}))
    ipa = _make_ipa(tmp_path, INFO)

    source = _run(module, tmp_path, ipa, "--previous", str(previous))

    versions = [entry["version"] for entry in source["apps"][0]["versions"]]
    assert versions[0] == "3.7.0"
    assert versions.count("3.7.0") == 1
    assert versions[1:3] == ["3.6.0", "3.5.0"]
    assert len(versions) == module.MAX_VERSIONS
    assert source["apps"][0]["versions"][0]["date"] == "2026-10-05"


@pytest.mark.parametrize("content", [None, "", "not json"])
def test_missing_or_unreadable_previous_feed_starts_fresh(tmp_path, content):
    module = _load_module()
    previous = tmp_path / "previous.json"
    if content is not None:
        previous.write_text(content)
    ipa = _make_ipa(tmp_path, INFO)

    source = _run(module, tmp_path, ipa, "--previous", str(previous))

    assert [v["version"] for v in source["apps"][0]["versions"]] == ["3.7.0"]


def test_output_is_stable_across_runs(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, INFO)

    _run(module, tmp_path, ipa)
    first = (tmp_path / "source.json").read_text()
    _run(module, tmp_path, ipa)

    assert (tmp_path / "source.json").read_text() == first


def test_rejects_non_https_download_url(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, INFO)

    with pytest.raises(SystemExit, match="https"):
        module.main(
            [
                "--ipa", str(ipa), "--version", "3.7.0",
                "--download-url", "http://example.com/x.ipa",
                "--out", str(tmp_path / "s.json"),
            ]
        )


def test_rejects_ipa_without_info_plist(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, None)

    with pytest.raises(SystemExit, match="Info.plist"):
        _run(module, tmp_path, ipa)


def test_rejects_version_mismatch(tmp_path):
    module = _load_module()
    ipa = _make_ipa(tmp_path, {**INFO, "CFBundleShortVersionString": "3.3.1"})

    with pytest.raises(SystemExit, match="3.3.1"):
        _run(module, tmp_path, ipa)
