# Lumin Spec-Driven Development

Document role: permanent development method

Status: active

Revision: 2026-07-15

## Definition

Lumin uses spec-anchored development: define the final contract and architecture before code, then implement narrow production-grade vertical slices without temporary architecture.

## Major Principles

1. **Route before reading.** Start at `WORKBOARD.md` and load only the owner documents for the current change.
2. **Separate what from how.** Product specs own behavior, constraints, non-goals, and acceptance. Architecture owns language, boundaries, dependency direction, execution, storage, and delivery.
3. **Design broadly, implement narrowly.** Freeze the destination architecture as a whole; implement one complete user path at a time.
4. **One fact, one owner.** Counts, statuses, rules, schemas, and policies have one canonical owner. Every other form is a projection.
5. **Make boundaries physical.** Cargo edges, visibility, project-owned types, lints, and architecture checks enforce dependency direction.
6. **Derive truth from specs.** Expected test results come from accepted behavior and hand-authored corpus truth, not from the current implementation or legacy output.
7. **Preserve uncertainty.** Incomplete, unsupported, stale, opaque, and truncated are product states; none may become empty success.
8. **Refactor from evidence.** Remove observed duplication, leaks, oversized owners, and ceremony. Do not invent abstractions for hypothetical reuse.

## Workflow

### 1. Route and Specify

- Read the Workboard and the active owners only.
- State the user, problem, boundary, non-goals, and observable completion criteria.
- Replace words such as fast or safe with measurable behavior.

### 2. Design, Review, and Freeze

- Define the destination crate DAG, forbidden edges, data ownership, failure states, concurrency, persistence, platform delivery, and verification.
- Require one design review and one independent adversarial review for architecture changes.
- Resolve findings or record explicit accepted risks before freezing the revision.

### 3. Implement One Vertical Slice

- Use final boundaries, types, scheduler, storage, packaging, and failure semantics.
- Search before creating helpers, types, rules, or dependencies.
- Add only crates containing real behavior required by the active slice.
- Keep unsupported scope explicit rather than adding a fallback owner.

### 4. Verify, Refactor, and Close

- Run focused behavior tests, realistic edge and hard-stop cases, architecture checks, corpus/determinism checks, and required package execution.
- Refactor only the active slice: remove duplication and ceremony, close dependency leaks, and narrow public APIs.
- Recheck every acceptance criterion, update the owning spec and Workboard, then commit the coherent result.

## Do Not

- Do not build an MVP on temporary architecture.
- Do not create horizontal scaffolding, empty future crates, placeholder modules, or a second engine.
- Do not copy legacy module boundaries before proving their new owner.
- Do not pass domain facts through JSON or duplicate truth in projections.
- Do not weaken tests, delete cases, add arbitrary caps/timeouts, swallow failures, or mark checks skipped to make code pass.
- Do not create generic `utils`, one-use traits, invariant-free wrappers, or owner/contract/policy stacks.
- Do not claim clean or absent without checked evidence, scope, and limitations.
- Do not load every specification into model context.

## Minimal Spec Shape

Every substantial spec needs only:

1. one-line definition;
2. current contract;
3. non-goals;
4. acceptance criteria;
5. verification commands.

Small changes may use only definition, contract, and acceptance. Add a document only when it owns a distinct durable fact.

## AI Judgment

AI reviewers make the repository's contextual judgments. A valid judgment cites checked source or evidence, states uncertainty, separates observed symptom from inferred cause, and records architecture-changing decisions in the owning spec. Confidence without checked evidence is not approval.
