# Lumin Spec-Driven Development

Document role: permanent development method

Status: active

Revision: 2026-07-15

## 0. One-Line Definition

Lumin uses spec-anchored development: the final architecture is designed broadly before implementation, while code advances through narrow, production-grade vertical slices whose acceptance criteria are proven by behavior.

## 1. Non-Negotiable Principles

### 1.1 Start From the Workboard

Read `WORKBOARD.md` first. Follow its routing rules and load only the owner documents for the current change. Reading every specification is a context failure, not diligence.

### 1.2 Separate Product Contract From Technical Design

A product specification defines what and why:

- intended behavior;
- user and boundary;
- constraints;
- non-goals;
- acceptance criteria.

Architecture and technical plans define how:

- language and runtime;
- crate and module boundaries;
- dependency direction;
- execution and ownership;
- storage and transport;
- implementation order.

Implementation details do not silently rewrite the product contract.

### 1.3 Design Broadly, Implement Narrowly

The destination architecture is settled before code. Implementation then proceeds through one narrow vertical slice that crosses every permanent layer at production quality.

Allowed:

```text
product surface -> application -> domain -> infrastructure -> verified result
```

Rejected:

- an MVP that depends on temporary architecture;
- horizontal empty-crate scaffolding;
- placeholder modules created only so tests can import them;
- a second fallback engine intended to be removed later;
- copying legacy boundaries before deciding their owner.

### 1.4 One Fact, One Owner

Every rule, count, status, protocol shape, and policy has one canonical owner. Other views are projections or links. If two modules can independently decide the same fact, the design is not ready.

### 1.5 Make Boundaries Physical

Dependency direction must be visible in the repository and enforced by Cargo metadata, crate APIs, visibility, lints, and architecture checks. A prose-only boundary is a wish.

### 1.6 Make Acceptance Executable

Every implemented acceptance criterion maps one-to-one to:

- an externally observable behavior;
- a realistic fixture or corpus case when repository semantics are involved;
- a stable verification command;
- evidence that distinguishes pass, incomplete, unsupported, and failure.

Expectations are derived from the specification, not copied from the current implementation.

### 1.7 Preserve Uncertainty

Unavailable, degraded, stale, truncated, and unsupported are product states. They are never converted to empty success. AI judgment must cite checked evidence, scan scope, and limitations; otherwise the result is unknown.

### 1.8 Refactor From Evidence

Refactor when a real boundary leak, repeated implementation, shared shape drift, overgrown owner, or ceremony stack is observed. Do not invent an abstraction for a hypothetical second use.

Keep the smallest structure that enforces a real invariant. Delete owner/contract/policy layers that only forward values and protect nothing.

### 1.9 Use Git as the Safety Boundary

- Keep specifications and code in the same repository.
- Commit an approved architecture baseline before implementation.
- Land small coherent slices after behavior is verified.
- Do not combine unrelated cleanup with a feature or migration slice.
- Never weaken a specification or test merely to make a change mergeable.

### 1.10 Keep Specifications Thin

Workboard is an index, not a handbook. Detailed facts live behind links. A new document requires a distinct owner role; otherwise update the existing owner.

## 2. Required Workflow

### Step 0: Route

1. Read `WORKBOARD.md`.
2. Identify the active specification and architecture owners.
3. Read only those documents and relevant repository rules.
4. Confirm whether the requested behavior is inside the active phase.

### Step 1: Define the Problem

Record:

- who needs the behavior;
- what observable problem exists;
- the implementation boundary;
- why it matters;
- what proves completion;
- what is explicitly excluded.

Replace adjectives such as fast, safe, or robust with measurable behavior.

### Step 2: Confirm Language and Runtime

Choose the language and runtime from product constraints, deployment, safety, ecosystem, team operation, and performance evidence. Once Architecture v1 fixes this decision, a slice cannot introduce a second runtime without an architecture amendment.

### Step 3: Fix the Destination Architecture

Before implementation, define:

- final component and crate owners;
- dependency DAG;
- forbidden dependencies;
- data ownership and lifecycle;
- failure and uncertainty taxonomy;
- concurrency and determinism model;
- persistence and product transport;
- distribution boundary.

Large architecture changes require two independent reviews: one design review and one adversarial review. Resolve findings or record explicit accepted risks before freezing the revision.

### Step 4: Design Dependency Direction

For every boundary, state:

- which owner defines the project type;
- which direction the dependency points;
- what outside dependency is isolated;
- what must never cross the boundary;
- why a private module is insufficient if a crate is proposed.

Do not add a trait merely to imitate dependency injection. Use a trait only when the boundary needs multiple real implementations, test substitution at a physical boundary, or inversion that cannot be expressed more simply.

### Step 5: Design Repository Structure

Make responsibility and dependency direction visible in paths. Avoid flat crate warehouses, generic `utils`, and tiny files with no independent owner. Do not create future crates before a vertical slice contains their real behavior.

### Step 6: Define Conventions and Machine Enforcement

Use ecosystem naming and formatting. Put enforceable rules in formatter, compiler, Clippy, dependency-edge checks, tests, and package probes. If the same review comment recurs, explain why automation did not catch it and close that enforcement gap.

### Step 7: Activate a Vertical-Slice Spec

The slice must define:

- one complete user-visible capability;
- participating final owners;
- supported and unsupported scope;
- real corpus truth;
- failure behavior;
- platform and distribution behavior;
- performance evidence required;
- acceptance criteria and commands.

Do not start code while required architecture decisions remain implicit.

### Step 8: Implement Narrowly and Deeply

Implement only the active slice, but implement its path at final quality. Use final types, boundaries, scheduler, persistence, packaging, and error semantics. Avoid compatibility shortcuts that the next slice must remove.

Search before creating helpers, types, rules, and dependencies. Reuse only when ownership and semantics actually match.

### Step 9: Verify Behavior

Verification order:

1. focused behavior tests;
2. realistic edge and hard-stop cases;
3. crate and architecture checks;
4. full affected-workspace tests;
5. corpus and determinism runs;
6. platform package execution;
7. dogfood on real repositories;
8. acceptance-criteria audit against the specification.

Tests must verify public behavior and failure contracts. File existence, helper exports, prose wrapping, or static strings are not product behavior unless they are the actual public protocol.

Never respond to a failing test by deleting the case, weakening the assertion, adding an arbitrary timeout or cap, swallowing the failure, or marking it skipped.

### Step 10: Refactor the Slice

After behavior passes:

- remove duplicated implementations;
- collapse pass-through ceremony;
- inspect nontrivial clones and shared mutable state;
- check crate API visibility and dependency leakage;
- split overgrown owners by cohesive responsibility;
- remove unused future hooks;
- keep tests close to the contract owner.

Do not mix unrelated repository-wide cleanup into the slice.

### Step 11: Close Out

Before completion:

1. Run every verification command owned by the slice.
2. Confirm canonical source and packaged products were built from one source tree.
3. Check that no generated build output or temporary transport is tracked.
4. Update the active spec and Workboard status.
5. Record unsupported scope and accepted risks.
6. Commit the coherent verified result.

## 3. Continuous Checks

Every slice and review checks:

- dependency direction and third-party type leakage;
- hidden shared mutable state;
- clone-to-compile and unnecessary allocation on hot paths;
- swallowed errors and fallback that changes semantics;
- one owner for every count and status;
- deterministic results across worker counts;
- bounded and explicit query truncation;
- behavior coverage for realistic edge cases;
- unused abstractions and ceremony stacks;
- source, binary, protocol, and skill drift;
- artifact-visible incomplete and unsupported states;
- performance and memory against approved corpus budgets.

## 4. Anti-Patterns

- **Bottom-up contract accumulation:** adding another guard or result shape instead of repairing ownership.
- **Horizontal scaffolding:** creating all layers as empty shells before one capability works.
- **Temporary MVP architecture:** shipping a shortcut expected to become permanent by accident.
- **Mirror ownership:** copying source or hard-coded contracts into package surfaces.
- **Artifact warehouse:** requiring agents to discover and read a large file set.
- **String contract test:** treating prose wrapping or internal labels as behavior.
- **Projection as truth:** deriving totals from a bounded example list.
- **Fallback owner switch:** silently running a different engine when the required owner fails.
- **Helper zoo:** extracting one-use helpers without increasing cohesion.
- **Type for type's sake:** wrappers, traits, or policies that enforce no invariant.
- **Ghost citation:** making a clean or absence claim without evidence value, scope, and limitation.

## 5. Workboard Contract

`WORKBOARD.md` contains only:

- one-line project purpose;
- source-of-truth order;
- spec registry;
- current active work;
- routing rules;
- phase ledger;
- context loading rule.

When the Workboard grows into a specification, move detail to the owning document and leave a one-line route.

## 6. Minimal Spec Template

```text
# <ID>: <Title>

Document role: <product contract | architecture owner | implementation spec | retired record>
Status: <draft | active | closed | retired>
Revision: <date or revision>
Scope: <phase and boundary>
Parent: <higher owner>

## 0. One-Line Definition
The single fact this document owns.

## 1. Background or Rejected Design (only when useful)
Why the current contract exists and which mistake must not recur.

## 2. Contract
Observable behavior and constraints.

## 3. Non-Goals
What this document does not introduce.

## 4. Acceptance Criteria
Concrete pass/fail statements.

## 5. Verification Commands
Commands that prove those statements.
```

Small changes may use only sections 0, 2, and 4. Do not fill sections to satisfy ceremony.

## 7. AI Judgment Rule

Lumin is developed and reviewed by AI agents under human direction. Wherever a traditional process says "a person must judge," this repository requires an evidence-grounded model judgment instead:

1. cite the checked source, artifact value, corpus result, or command output;
2. state uncertainty and unsupported scope;
3. distinguish observed symptom from inferred cause;
4. explain what would break if a proposed contract were removed;
5. record the decision in the owning spec when it changes architecture or product behavior.

Model confidence without checked evidence is not approval.
