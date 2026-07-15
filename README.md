# Lumin v2

Lumin v2 is an architecture-first rewrite of the Lumin repository-audit and write-gate product. It remains a native CLI and Codex/Claude Code skill, but replaces the legacy multi-producer artifact pipeline with one deterministic Rust engine, queryable canonical evidence, and durable pre-write/post-write transactions.

Status: Phase 0 architecture draft. This line is a projection of `WORKBOARD.md`; no implementation scaffold has been accepted.

## Read the Blueprint

Start with [`WORKBOARD.md`](WORKBOARD.md). It routes each kind of work to the smallest owner document.

The architecture review packet is:

1. [`specs/000-product-contract.md`](specs/000-product-contract.md)
2. [`architecture/000-system-blueprint.md`](architecture/000-system-blueprint.md)
3. [`architecture/001-execution-and-ownership.md`](architecture/001-execution-and-ownership.md)
4. [`architecture/002-evidence-and-write-gate.md`](architecture/002-evidence-and-write-gate.md)
5. [`specs/001-foundation-slice.md`](specs/001-foundation-slice.md)

The current freeze ledger is [`REVIEW-002`](reviews/architecture-v1-independent-verification-2026-07-15.md); [`REVIEW-001`](reviews/architecture-v1-adversarial-2026-07-15.md) preserves the first review history.

[`SDD.md`](SDD.md) defines the permanent development method. [`AGENTS.md`](AGENTS.md) is the canonical English repository-agent contract, and [`AGENTS.ko.md`](AGENTS.ko.md) is its Korean translation.

## Current Decision

The destination architecture is designed as a whole. Implementation will then proceed through a narrow production-grade slice covering native JS/TS/Vue extraction, resolution, export-level dead evidence, bounded queries, write-gate transactions, and prebuilt Windows/Linux delivery.

Legacy Lumin remains a compatibility and defect corpus. Its internal boundaries are not migration targets.
