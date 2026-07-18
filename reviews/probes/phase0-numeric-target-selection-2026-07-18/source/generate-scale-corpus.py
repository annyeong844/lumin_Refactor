#!/usr/bin/env python3
"""Generate the implementation-independent Phase 1 scale corpus and truth."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


SCHEMA = "phase1-scale-findings.v1"
PACKAGE_COUNT = 8
LIVE_PER_PACKAGE = 59
AUTHORED_DEAD_PER_PACKAGE = 16
GENERATED_DEAD_PER_PACKAGE = 8
VENDORED_DEAD_PER_PACKAGE = 8
VUE_PER_PACKAGE = 4


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()


def payload_literal(seed: int) -> str:
    values = [str((seed + index) % 4096) for index in range(2048)]
    return ",".join(values)


def write_new(root: Path, relative: str, data: bytes, entries: list[dict[str, object]]) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        raise RuntimeError(f"refusing to overwrite corpus member: {relative}")
    path.write_bytes(data)
    entries.append(
        {
            "bytes": len(data),
            "path": relative,
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    )


def ts_module(export_name: str, seed: int, generated: bool = False) -> bytes:
    marker = "// @generated\n" if generated else ""
    source = (
        f"{marker}const payload = [{payload_literal(seed)}] as const;\n"
        f"export const {export_name} = payload[{seed % 2048}] + {seed};\n"
    )
    return source.encode()


def vue_module(package_index: int, view_index: int, live_index: int) -> bytes:
    seed = package_index * 1000 + 700 + view_index
    source = f"""<script setup lang="ts">
import {{ live{live_index:03d} }} from "../live/live-{live_index:03d}.js";
const payload = [{payload_literal(seed)}] as const;
const viewValue = live{live_index:03d} + payload[{seed % 2048}];
</script>
<template><div>{{{{ viewValue }}}}</div></template>
"""
    return source.encode()


def generate(output: Path) -> tuple[dict[str, object], dict[str, object]]:
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output must be absent or empty: {output}")
    output.mkdir(parents=True, exist_ok=True)

    entries: list[dict[str, object]] = []
    findings: list[dict[str, object]] = []
    package_names = [f"@lumin-scale/pkg-{index:02d}" for index in range(PACKAGE_COUNT)]
    entry_paths = ["src/main.ts"]

    write_new(
        output,
        "package.json",
        json_bytes(
            {
                "name": "lumin-phase1-scale-findings",
                "private": True,
                "type": "module",
                "workspaces": ["packages/*"],
            }
        ),
        entries,
    )
    write_new(
        output,
        "tsconfig.json",
        json_bytes(
            {
                "compilerOptions": {
                    "module": "esnext",
                    "moduleResolution": "bundler",
                    "target": "es2022",
                },
                "include": ["src/**/*", "packages/*/src/**/*"],
            }
        ),
        entries,
    )
    write_new(
        output,
        "lumin.json",
        json_bytes(
            {
                "entries": entry_paths,
                "scan": {
                    "include": ["packages/**"],
                    "exclude": [],
                    "roles": [
                        {"pattern": "packages/*/src/vendor/**", "role": "vendor"}
                    ],
                },
                "schemaVersion": "lumin-config.v1",
            }
        ),
        entries,
    )

    root_imports = [
        f'import {{ packageValue as package{index:02d} }} from "{name}";'
        for index, name in enumerate(package_names)
    ]
    root_source = "\n".join(root_imports) + "\n"
    root_source += "void [" + ", ".join(f"package{index:02d}" for index in range(PACKAGE_COUNT)) + "];\n"
    write_new(output, "src/main.ts", root_source.encode(), entries)

    for package_index, package_name in enumerate(package_names):
        package_root = f"packages/pkg-{package_index:02d}"
        write_new(
            output,
            f"{package_root}/package.json",
            json_bytes(
                {
                    "exports": {".": "./src/index.ts"},
                    "name": package_name,
                    "private": True,
                    "type": "module",
                }
            ),
            entries,
        )

        imports: list[str] = []
        live_names: list[str] = []
        for live_index in range(LIVE_PER_PACKAGE):
            name = f"live{live_index:03d}"
            imports.append(f'import {{ {name} }} from "./live/live-{live_index:03d}.js";')
            live_names.append(name)
            write_new(
                output,
                f"{package_root}/src/live/live-{live_index:03d}.ts",
                ts_module(name, package_index * 1000 + live_index),
                entries,
            )

        view_names: list[str] = []
        for view_index in range(VUE_PER_PACKAGE):
            name = f"View{view_index:02d}"
            view_names.append(name)
            imports.append(f'import {name} from "./views/view-{view_index:02d}.vue";')
            write_new(
                output,
                f"{package_root}/src/views/view-{view_index:02d}.vue",
                vue_module(package_index, view_index, view_index),
                entries,
            )

        entry_source = "\n".join(imports) + "\n"
        entry_source += f"void [{', '.join(view_names)}];\n"
        entry_source += f"export const packageValue = {' + '.join(live_names)};\n"
        write_new(output, f"{package_root}/src/index.ts", entry_source.encode(), entries)

        categories = (
            ("authored-dead", AUTHORED_DEAD_PER_PACKAGE, "ReviewCandidate", None, False),
            (
                "generated-dead",
                GENERATED_DEAD_PER_PACKAGE,
                "ReviewOnly",
                {
                    "classificationReason": "leading-comment-@generated-within-first-2KiB",
                    "classificationVersion": "source-classification.v1",
                    "role": "Generated",
                },
                True,
            ),
            (
                "vendor",
                VENDORED_DEAD_PER_PACKAGE,
                "ReviewOnly",
                {
                    "classificationReason": "explicit-vendor-role",
                    "classificationVersion": "source-classification.v1",
                    "role": "Vendored",
                },
                False,
            ),
        )
        for category, count, disposition, reason, generated in categories:
            for dead_index in range(count):
                category_token = category.replace("-", "_")
                export_name = f"dead_{category_token}_{dead_index:03d}"
                relative = f"{package_root}/src/{category}/dead-{dead_index:03d}.ts"
                seed = package_index * 1000 + 100 + len(findings)
                write_new(output, relative, ts_module(export_name, seed, generated), entries)
                findings.append(
                    {
                        "disposition": disposition,
                        "dispositionReason": reason,
                        "exportKind": "named-value",
                        "exportName": export_name,
                        "findingClass": "grounded-zero-fan-in-export",
                        "packageName": package_name,
                        "path": relative,
                    }
                )

    entries.sort(key=lambda row: str(row["path"]).encode())
    findings.sort(key=lambda row: (str(row["path"]).encode(), str(row["exportName"])))
    manifest_lines = "".join(f"{row['sha256']}  {row['path']}\n" for row in entries).encode()
    manifest = {
        "contentManifestSha256": hashlib.sha256(manifest_lines).hexdigest(),
        "entries": entries,
        "fileCount": len(entries),
        "schemaVersion": SCHEMA,
        "totalBytes": sum(int(row["bytes"]) for row in entries),
    }
    truth = {
        "breakdown": {
            "ReviewCandidate": AUTHORED_DEAD_PER_PACKAGE * PACKAGE_COUNT,
            "ReviewOnlyGenerated": GENERATED_DEAD_PER_PACKAGE * PACKAGE_COUNT,
            "ReviewOnlyVendored": VENDORED_DEAD_PER_PACKAGE * PACKAGE_COUNT,
        },
        "expectedFindings": findings,
        "filters": {},
        "findingIdRule": "The Phase 1 product must assign one stable unique FindingId to each authored semantic tuple; cold, warm, jobs=1, and default-jobs runs must return the same IDs.",
        "limitations": [],
        "matchedTotal": len(findings),
        "schemaVersion": SCHEMA,
        "scopeTotal": len(findings),
    }
    return manifest, truth


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--truth", required=True, type=Path)
    args = parser.parse_args()

    manifest, truth = generate(args.output.resolve())
    args.manifest.parent.mkdir(parents=True, exist_ok=True)
    args.truth.parent.mkdir(parents=True, exist_ok=True)
    args.manifest.write_bytes(json_bytes(manifest))
    args.truth.write_bytes(json_bytes(truth))


if __name__ == "__main__":
    main()
