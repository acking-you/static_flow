# Outline Template for Mechanism-First Deep-Dive

Use this template as a default structure.

## 1. Background and Goals
- State business/engineering context.
- State what cannot be solved by current observability/docs.
- Define scope and non-goals.

## 2. Model and Terminology
- Define entities and event objects.
- Define invariants and constraints.
- Define key terms once.

## 3. End-to-End Architecture
- Provide one global flow diagram.
- Explain component boundaries and responsibilities.
- Explain where data changes shape.

## 4. Core Mechanisms (repeat per mechanism)
### 4.x Mechanism Name
- Problem addressed
- Decision rationale
- Runtime execution path
- Failure modes and mitigation
- Verification signals

## 5. Query/Storage/Runtime Behavior
- Explain persistence model and write path.
- Explain query and aggregation path.
- Explain runtime controls and guardrails.

## 6. UI/Ops Integration
- Explain how users/operators consume the outputs.
- Explain drill-down workflow from overview to sample.

## 7. Trade-offs and Boundaries
- Provide at least one comparison table.
- State what the design optimizes for.
- State what the design does not solve.

## 8. Operations Playbook
- Add at least two concrete incident scenarios.
- For each scenario: symptoms -> filters -> correlation -> next action.

## 9. Code Index Appendix
- Group by subsystem.
- Keep compact; avoid turning appendix into the main body.

## 10. Summary
- Restate capability gains.
- Restate key limits.
- Suggest next evolution steps.
