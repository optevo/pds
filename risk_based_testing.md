# Risk-Based Testing Register

<!-- ACTIVATION STATUS: NOT YET ACTIVATED
     This file is present as a project directive but has not been populated.
     To activate RBT for this project, explicitly request population of the
     register for the codebase. Until then this file is inert. -->

---

## Directive: Risk-Based Testing System (RBT)

This file defines and tracks the Risk-Based Testing model for this codebase.
It is not optional documentation — it is a living engineering artefact that
must be updated whenever code is added or changed.

---

## 1. Principles (Global Rules)

### 7.1 Fail Fast
No silent failures. Errors must surface immediately at the point of origin.

### 7.2 Explicit Edge Case Enumeration
Edge cases must be identified, not discovered accidentally.

### 7.3 Explicit Invariants
All units must define preconditions, postconditions, and invariants.

### 7.4 Determinism First
Outputs must be deterministic unless explicitly justified otherwise.

### 7.5 No Unverified Transformations
Any transformation must be validated by property tests, differential tests,
or enforced invariants.

### 7.6 Independence of Evidence
No single testing method may be the sole justification for correctness of
high-risk behaviour.

### 7.7 Explicit Input Space Modelling
Valid, invalid, boundary, and adversarial inputs must be defined for each unit.

### 7.8 Error States Are First-Class
Error behaviour must be specified and tested, not treated as secondary.

### 7.9 Complexity Budgeting
High-complexity units must be flagged as inherently higher risk and allocated
proportionally more testing effort.

### 7.10 Auditability
Behaviour should be explainable or traceable where applicable.

---

## 2. Code Units

<!-- Add a section per code unit when the register is activated.
     Template for each unit:

### 2.x <Unit Name>

#### Purpose
<What this unit does>

#### Failure Modes
- FM-1: <concrete failure description>
- FM-2: <concrete failure description>
(Generic labels like "bug" are not permitted.)

#### Risk Model

| Failure Mode | Severity (1–10) | Likelihood (1–10) | Initial Risk Score |
|--------------|-----------------|-------------------|--------------------|
| FM-1         |                 |                   |                    |
| FM-2         |                 |                   |                    |

#### Test & Evidence Matrix

| Failure Mode | Unit Tests | Property Tests | Fuzzing | Assertions | Differential/Oracle | Coverage |
|--------------|-----------|----------------|---------|------------|---------------------|----------|
| FM-1         |           |                |         |            |                     |          |
| FM-2         |           |                |         |            |                     |          |

Contribution strengths: None / Low / Medium / High / Very High

#### Residual Risk Assessment
Residual Risk = Severity × Likelihood × (1 − Detection Strength)
(Conceptual approximation — not required to be numerically exact.)

#### Applied Principles
<List which of 7.1–7.10 apply and how they are enforced for this unit>

#### Notes / Open Risks
<Any failure modes with no assigned test, or areas of known weakness>

-->

---

## 3. Update Rule

This file MUST be updated when:

- A new code unit is added
- A failure mode is discovered (including via bugs in production or testing)
- A test type is added or removed from a unit
- Architecture changes affect risk distribution
- A bug reveals a missing failure mode

---

## 4. Coverage Policy

Coverage is used only as a completeness indicator, never as correctness evidence.
A unit with 100% coverage and no failure mode analysis provides no risk assurance.

---

## 5. Quasi-Oracle Rule

If an external oracle is used for differential testing:

1. A Rust-native equivalent must be built.
2. It must pass equivalence tests against the oracle.
3. Only then may the external oracle be retired.

The external oracle must not be the sole long-term source of truth.
