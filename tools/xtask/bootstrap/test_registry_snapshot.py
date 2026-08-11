"""Tests for the isolated registry archive/tree verifier."""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("registry_snapshot.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_registry_snapshot", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
SNAPSHOT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SNAPSHOT
SPEC.loader.exec_module(SNAPSHOT)


def _octal(value: int, width: int) -> bytes:
    return f"{value:0{width - 1}o}\0".encode("ascii")


def _header(
    name: str | bytes,
    content: bytes,
    *,
    mode: int = 0o644,
    type_flag: bytes = b"0",
) -> bytes:
    encoded = name.encode("ascii") if isinstance(name, str) else name
    if len(encoded) > 100:
        raise ValueError("test member name is too long")
    header = bytearray(512)
    header[0 : len(encoded)] = encoded
    header[100:108] = _octal(mode, 8)
    header[108:116] = _octal(0, 8)
    header[116:124] = _octal(0, 8)
    header[124:136] = _octal(len(content), 12)
    header[136:148] = _octal(0, 12)
    header[148:156] = b" " * 8
    header[156:157] = type_flag
    header[257:263] = b"ustar\0"
    header[263:265] = b"00"
    checksum = sum(header)
    header[148:156] = f"{checksum:06o}\0 ".encode("ascii")
    return bytes(header)


def _archive_bytes(
    members: list[tuple[str | bytes, bytes, int, bytes]],
    *,
    trailing: bytes = b"",
) -> bytes:
    payload = bytearray()
    for name, content, mode, type_flag in members:
        payload.extend(_header(name, content, mode=mode, type_flag=type_flag))
        payload.extend(content)
        payload.extend(bytes((-len(content)) % 512))
    payload.extend(bytes(1024))
    payload.extend(trailing)
    output = bytearray()
    with tempfile.TemporaryFile() as buffer:
        with gzip.GzipFile(fileobj=buffer, mode="wb", mtime=0) as compressed:
            compressed.write(payload)
        buffer.seek(0)
        output.extend(buffer.read())
    return bytes(output)


class Fixture:
    name = "fixture"
    version = "1.2.3"
    registry_key = "index.crates.io-test"

    def __init__(self, temporary: Path) -> None:
        self.repository = temporary / "repository"
        self.cargo_home = temporary / "cargo-home"
        self.package = (
            self.cargo_home
            / "registry"
            / "src"
            / self.registry_key
            / f"{self.name}-{self.version}"
        )
        self.archive = (
            self.cargo_home
            / "registry"
            / "cache"
            / self.registry_key
            / f"{self.name}-{self.version}.crate"
        )
        self.repository.mkdir()
        self.package.mkdir(parents=True)
        self.archive.parent.mkdir(parents=True)
        self.files = {
            "Cargo.toml": b'[package]\nname = "fixture"\nversion = "1.2.3"\n',
            "src/lib.rs": b"pub fn answer() -> u8 { 42 }\n",
        }
        self.write_tree()
        self.write_archive(self.default_members())

    def default_members(self) -> list[tuple[str | bytes, bytes, int, bytes]]:
        prefix = f"{self.name}-{self.version}"
        return [
            (f"{prefix}/{relative}", content, 0o644, b"0")
            for relative, content in self.files.items()
        ]

    def write_tree(self) -> None:
        for relative, content in self.files.items():
            path = self.package / Path(relative)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        (self.package / ".cargo-ok").write_bytes(b"ok\n")

    def write_archive(
        self,
        members: list[tuple[str | bytes, bytes, int, bytes]],
        *,
        trailing: bytes = b"",
    ) -> None:
        self.archive.write_bytes(_archive_bytes(members, trailing=trailing))

    def envelope(self) -> dict[str, object]:
        return {
            "schemaVersion": 1,
            "repositoryRoot": str(self.repository),
            "cargoHome": str(self.cargo_home),
            "packages": [
                {
                    "name": self.name,
                    "version": self.version,
                    "source": SNAPSHOT.CRATES_IO_SOURCE,
                    "checksum": hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                    "manifestPath": str(self.package / "Cargo.toml"),
                }
            ],
        }


class RegistrySnapshotTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Fixture]:
        temporary = tempfile.TemporaryDirectory()
        return temporary, Fixture(Path(temporary.name))

    def test_archive_and_tree_match(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            self.assertEqual(
                SNAPSHOT.validate_snapshot(fixture.envelope()),
                {"schemaVersion": 1, "packageCount": 1},
            )

    def test_archive_checksum_is_bound_before_parsing(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            envelope = fixture.envelope()
            fixture.archive.write_bytes(fixture.archive.read_bytes() + b"tamper")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "checksum mismatch"):
                SNAPSHOT.validate_snapshot(envelope)

    def test_archive_and_tree_content_divergence_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            (fixture.package / "src" / "lib.rs").write_bytes(b"substituted\n")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "content differs"):
                SNAPSHOT.validate_snapshot(fixture.envelope())

            (fixture.package / "src" / "lib.rs").write_bytes(fixture.files["src/lib.rs"])
            changed = fixture.default_members()
            changed[-1] = (changed[-1][0], b"changed archive\n", 0o644, b"0")
            fixture.write_archive(changed)
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "content differs"):
                SNAPSHOT.validate_snapshot(fixture.envelope())

    def test_extra_missing_and_hardlinked_tree_paths_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            extra = fixture.package / "src" / "extra.rs"
            extra.write_text("extra\n", encoding="utf-8")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "extra registry source"):
                SNAPSHOT.validate_snapshot(fixture.envelope())
            extra.unlink()

            missing = fixture.package / "src" / "lib.rs"
            missing.unlink()
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "missing registry source"):
                SNAPSHOT.validate_snapshot(fixture.envelope())
            missing.write_bytes(fixture.files["src/lib.rs"])

            alias = fixture.package / "src" / "alias.rs"
            try:
                os.link(missing, alias)
            except OSError as error:
                self.skipTest(f"hard links are unavailable: {error}")
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "hard-linked"):
                SNAPSHOT.validate_snapshot(fixture.envelope())

    def test_nonportable_and_colliding_archive_names_are_rejected(self) -> None:
        prefix = "fixture-1.2.3"
        cases: tuple[tuple[str, list[tuple[str | bytes, bytes, int, bytes]], str], ...] = (
            (
                "device",
                [(f"{prefix}/src/CON.txt", b"x", 0o644, b"0")],
                "device component",
            ),
            (
                "case collision",
                [
                    (f"{prefix}/src/lib.rs", b"x", 0o644, b"0"),
                    (f"{prefix}/SRC/LIB.RS", b"y", 0o644, b"0"),
                ],
                "case-colliding",
            ),
            (
                "non-ASCII",
                [(f"{prefix}/src/".encode() + b"\xff.rs", b"x", 0o644, b"0")],
                "not ASCII",
            ),
            (
                "directory record",
                [(f"{prefix}/src", b"", 0o755, b"5")],
                "member type",
            ),
        )
        for label, members, message in cases:
            with self.subTest(label=label):
                temporary, fixture = self.fixture()
                with temporary:
                    fixture.write_archive(members)
                    with self.assertRaisesRegex(SNAPSHOT.SnapshotError, message):
                        SNAPSHOT.validate_snapshot(fixture.envelope())

    def test_trailing_decompressed_payload_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.write_archive(fixture.default_members(), trailing=bytes(512))
            with self.assertRaisesRegex(SNAPSHOT.SnapshotError, "trailing"):
                SNAPSHOT.validate_snapshot(fixture.envelope())

    def test_isolated_cli_emits_one_verdict(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            completed = subprocess.run(
                [sys.executable, "-I", "-S", str(SCRIPT)],
                input=json.dumps(fixture.envelope()),
                text=True,
                capture_output=True,
                check=False,
                cwd=fixture.repository,
                env={"SYSTEMROOT": os.environ.get("SYSTEMROOT", "")},
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(
                json.loads(completed.stdout),
                {"schemaVersion": 1, "packageCount": 1},
            )


if __name__ == "__main__":
    unittest.main()
