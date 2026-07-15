# Lumin Spec-Driven Development

[`문서(한글)/SDD.ko.md`](문서(한글)/SDD.ko.md) is the canonical METHOD-000. This file is its English translation; when they differ, the Korean source wins.

## Definition

Lumin uses Spec-Driven Development (SDD). Define the final contract and destination architecture before writing code, then implement narrow production-grade vertical slices inside those boundaries.

## Workflow

1. **Route and define the problem:** Start at [WORKBOARD.md](WORKBOARD.md) and read only the owners for the current change. State the user, problem, scope, non-goals, and observable completion criteria; replace words such as `fast` or `safe` with measurable conditions.
2. **Design and freeze:** Product specifications own what and why; architecture owns how. Define boundaries, data ownership, failure states, concurrency, persistence, platform delivery, and verification, then freeze only a revision that has passed its required reviews.
3. **Implement a vertical slice:** Complete one user behavior with the final boundaries and types. Search for an existing owner before adding a helper, type, rule, or dependency; do not create crates without active behavior or fallback owners.
4. **Verify and close:** Derive expectations from the specification and test the core path, realistic edge cases, and hard stops. Remove duplication, ceremony, and dependency leaks only within the active slice, recheck acceptance, and update only the owner documents that changed.

## Minimal Specification

A substantial change needs only a one-line definition, current contract, non-goals, acceptance criteria, and verification commands. A small change may use only the definition, current contract, and acceptance. Create a separate document only when it owns a distinct durable fact; do not copy another document's contract.

## AI Judgment

A judgment states its checked source or evidence, scope, and uncertainty, and separates observed symptoms from inferred causes. Record architecture-changing decisions in the owner document; never approve an unchecked clean or absence claim.
