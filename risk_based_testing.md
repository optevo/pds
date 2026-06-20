# Risk-Based Testing Register

<!-- ACTIVATION STATUS: NOT YET ACTIVATED
     This file is present as a project directive but the register (Section 2) has
     not been populated. To activate RBT for this project, explicitly request
     population of the register for the codebase.

     Always-active principles (7.1, 7.4, 7.5, 7.8, 7.9, 7.10) apply to all new
     code regardless of activation status — they govern how code is written.
     Documentary principles (7.2, 7.3, 7.6, 7.7) require the register to be
     populated before they can be enforced. -->

---

## Directive: Risk-Based Testing System (RBT)

This file defines and tracks the Risk-Based Testing model for this codebase.
It is not optional documentation — it is a living engineering artefact that
must be updated whenever code is added or changed.

---

## 1. Principles (Global Rules)

### Always-active principles

These apply to all new code regardless of whether the register has been populated.
They govern how code is written and tested, not what is documented.

#### 7.1 Fail Fast
No silent failures. Errors must surface immediately at the point of origin.

#### 7.4 Determinism First
Outputs must be deterministic unless explicitly justified otherwise.

#### 7.5 No Unverified Transformations
Any transformation must be validated by property tests, differential tests,
or enforced invariants.

#### 7.8 Error States Are First-Class
Error behaviour must be specified and tested, not treated as secondary.

#### 7.9 Complexity Budgeting
High-complexity units must be flagged as inherently higher risk and allocated
proportionally more testing effort.

#### 7.10 Auditability
Behaviour should be explainable or traceable where applicable.

### Register-required principles

These require the register (Section 2) to be populated before they can be
enforced — they govern documented artefacts, not just code behaviour.

#### 7.2 Explicit Edge Case Enumeration
Edge cases must be identified and recorded, not discovered accidentally.

#### 7.3 Explicit Invariants
All units must define preconditions, postconditions, and invariants.

#### 7.6 Independence of Evidence
No single testing method may be the sole justification for correctness of
high-risk behaviour. Multiple independent evidence sources are required.

#### 7.7 Explicit Input Space Modelling
Valid, invalid, boundary, and adversarial inputs must be defined for each unit.

---

## 2. Code Units

<!-- Add a section per code unit when the register is activated.
     See Section 3 for how to determine the right granularity level.
     Template for each unit:

### 2.x <Unit Name>

#### Purpose
<What this unit does>

#### Failure Modes
- FM-1: <concrete failure description>
- FM-2: <concrete failure description>
(Generic labels like "bug" are not permitted. Be specific: incorrect output,
panic on boundary input, silent state corruption, numerical instability, etc.)

#### Risk Model

| Failure Mode | Severity (1–10) | Likelihood (1–10) | Initial Risk Score |
|--------------|-----------------|-------------------|--------------------|
| FM-1         |                 |                   | S × L              |
| FM-2         |                 |                   | S × L              |

#### Test & Evidence Matrix

| Failure Mode | Unit Tests | Property Tests | Fuzzing | Assertions | Differential/Oracle | Coverage |
|--------------|-----------|----------------|---------|------------|---------------------|----------|
| FM-1         |           |                |         |            |                     |          |
| FM-2         |           |                |         |            |                     |          |

Contribution strengths: None / Low / Medium / High / Very High
(See Section 5 for numeric mapping used in residual risk calculation.)

#### Residual Risk Assessment
Residual Risk = S × L × (1 − D)
where D is the numeric detection strength from Section 5.
State the dominant failure modes and whether residual risk is acceptable.

#### Applied Principles
<List which of 7.1–7.10 apply and how they are enforced for this unit>

#### Notes / Open Risks
<Failure modes with no assigned test, or areas of known weakness>

-->

---

## 3. Code Unit Granularity

A "code unit" is a logical grouping at the level where failure modes are
meaningfully distinct from adjacent code.

**Default: module-level.** A module (`mod foo`) is the standard unit. Group
related functions and types into one register entry when they share the same
failure modes and risk profile.

**Escalate to function-level** when:
- A function is substantially complex (non-trivial algorithm, many branches)
- A function has failure modes that do not apply to the rest of the module
- A function is a high-severity hot path (inference, serialisation, query execution)

**Crate-level is acceptable** only for glue or passthrough crates with no
independent logic — crates whose entire purpose is to re-export or adapt
another interface. If a crate contains any algorithmic or stateful logic,
it must be broken into module-level units.

When in doubt, prefer coarser granularity and split later when distinct failure
modes emerge. Premature splitting creates maintenance overhead with no risk benefit.

---

## 4. Test & Evidence Types (Standard Taxonomy)

### 4.1 Unit Tests
Target known scenarios. Validate expected behaviour on specific inputs.

### 4.2 Property-Based Tests
Validate invariants over a sampled input space. Used for edge case coverage
and boundary conditions that are hard to enumerate by hand.

### 4.3 Fuzzing
Random and adversarial input testing. Primary for crash resistance and
parser robustness. Especially valuable for inputs crossing a trust boundary.

### 4.4 Assertions (Pre / Post / Invariants)
Enforced at runtime via `debug_assert!` or `assert!`. Used for internal
correctness guarantees; these are evidence even without a dedicated test.

### 4.5 Coverage
Used only as a **gap detector** — it identifies code that has no test at all.
It does not close gaps: executing a line provides no evidence the line is correct.
A unit with 100% coverage and no failure mode analysis provides no risk assurance.

### 4.6 Differential / Oracle Testing
Compare outputs against an external reference system or independent implementation.
Valuable as cross-validation. See Section 6 for the oracle lifecycle rule.

---

## 5. Detection Strength — Numeric Mapping

Use this table to convert qualitative contribution strengths to the numeric value
used in the residual risk formula (`Residual Risk = S × L × (1 − D)`).

| Label     | Numeric (D) | Meaning                                                          |
|-----------|-------------|------------------------------------------------------------------|
| None      | 0.00        | No test addresses this failure mode                              |
| Low       | 0.25        | A test exists but covers only part of the failure space          |
| Medium    | 0.50        | Reasonable coverage of the failure mode; edge cases may be missed|
| High      | 0.75        | Strong coverage with property tests or fuzzing across input space |
| Very High | 0.95        | Multiple independent methods; failure would be very hard to miss  |

When multiple test types address the same failure mode, D is the combined strength
from their joint evidence — not the maximum of any single type. Use judgment; the
formula is a conceptual model, not a precise calculator.

**Residual risk is acceptable** when it is proportionate to the severity of the
failure and the cost of further detection investment. Flag unacceptably high
residual risk in the Notes section.

---

## 6. Oracle Lifecycle Rule

External oracles (reference implementations, Python fixtures, third-party tools)
may be used for differential testing indefinitely — they do not need to be retired.

If a Rust-native equivalent is built:
1. It must pass equivalence tests against the oracle before the oracle is retired.
2. The oracle may remain as a permanent cross-check even after equivalence is confirmed.
3. The external oracle must not be the *sole* evidence of correctness for any
   high-severity failure mode — at least one independent Rust-side assertion or
   property test must also be in place.

There is no obligation to build a native equivalent. The obligation is only that
if one exists and the oracle is to be retired, equivalence must be demonstrated first.

---

## 7. Update Rule

This file MUST be updated when:

- A new code unit is added
- A failure mode is discovered (including via bugs found in testing or production)
- A test type is added or removed from a unit
- Architecture changes affect risk distribution across units
- A bug reveals a missing failure mode in an existing unit

---

## 8. Required Principles Summary

| # | Principle | Always Active | Register Required |
|---|-----------|:---:|:---:|
| 7.1 | Fail Fast | Yes | — |
| 7.2 | Explicit Edge Case Enumeration | — | Yes |
| 7.3 | Explicit Invariants | — | Yes |
| 7.4 | Determinism First | Yes | — |
| 7.5 | No Unverified Transformations | Yes | — |
| 7.6 | Independence of Evidence | — | Yes |
| 7.7 | Explicit Input Space Modelling | — | Yes |
| 7.8 | Error States Are First-Class | Yes | — |
| 7.9 | Complexity Budgeting | Yes | — |
| 7.10 | Auditability | Yes | — |
