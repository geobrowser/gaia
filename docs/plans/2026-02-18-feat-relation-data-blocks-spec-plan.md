---
title: feat: Specify relation data block filter schema
type: feat
date: 2026-02-18
---

# feat: Specify relation data block filter schema

## Overview

Define a schema-only specification for relation data blocks. The spec defines the persisted JSON shape stored on the block entity and the filter language constraints for relation-oriented filtering.

This RFC does not define execution contracts. Clients may interpret the schema, execute queries via different APIs, and render results in different UI shapes.

## Problem Statement / Motivation

Current filtering semantics are documented in RFC 0004, but relation-block-specific persisted schema constraints are not yet explicit. Without a dedicated schema contract, teams can store incompatible relation block configurations.

This plan turns the approved brainstorm direction into an implementation-ready spec with explicit boundaries and acceptance criteria.

## Proposed Solution

Create an RFC/spec update that defines a dedicated relation-block schema profile with:

- Required top-level source anchor (`sourceEntityId`).
- RFC 0004 filter grammar/operators reused unchanged.
- Relation type constraints expressed in `filter` only.
- Recursive `_relation` filters allowed in schema.
- Explicit conflict rule when `sourceEntityId` and `_relation.fromEntity` both imply source constraints.

Reuse RFC 0004 filter grammar and validation style wherever possible, and explicitly document inheritance plus relation-block-specific constraints.

## Technical Considerations

- **Schema boundary**: this spec defines persisted block JSON only, not API request/response contracts.
- **Grammar inheritance**: operators and logical semantics should match RFC 0004.
- **Determinism**: anchor semantics must be unambiguous even when overlapping constraints appear.
- **Interpretation**: clients should follow canonical interpretation rules without requiring centralized validation infrastructure.
- **Discriminator model**: presence of `sourceEntityId` is the relation-block discriminator for this RFC.

## Acceptance Criteria

- [ ] Relation block payload requires a top-level `sourceEntityId` (always required for relation blocks).
- [ ] Presence of `sourceEntityId` is the discriminator for relation blocks in this RFC.
- [ ] Canonical top-level keys are `version`, `spaceId`, `properties`, `filter`, and `sourceEntityId`.
- [ ] Unknown top-level keys are ignored by clients.
- [ ] `filter` remains required and follows RFC 0004 grammar/operators.
- [ ] Relation type constraints are specified in `filter` only (no top-level relation type field).
- [ ] Recursive `_relation` filters are valid in schema.
- [ ] Conflict rule is explicit: when `sourceEntityId` and `_relation.fromEntity` both impose source constraints, clients must treat `sourceEntityId` as canonical and ignore conflicting `_relation.fromEntity` constraints.
- [ ] Interpretation guidance is deterministic even where clients do not run explicit validation.
- [ ] Spec includes at least one valid and one invalid JSON example for each relation-block-specific rule.
- [ ] Out-of-scope section explicitly excludes result shape, pagination, authorization semantics, and UI rendering behavior.
- [ ] Out-of-scope section notes that explicit block-type metadata (for example, `dataSourceType=relations`) is a future enhancement.

## Success Metrics

- Spec reviewers can independently validate stored relation block payloads without clarification loops.
- No unresolved ambiguity remains for anchor semantics or top-level schema shape.
- Different clients can execute/render from the same stored schema while remaining conformant.

## Dependencies & Risks

### Dependencies

- RFC 0004 filter grammar and validation model as canonical base.
- Knowledge graph ontology semantics for relation direction and block modeling.

### Risks

- **RFC drift**: relation-block schema diverges from RFC 0004 grammar/operators.
- **Anchor ambiguity**: `sourceEntityId` versus `_relation.fromEntity` conflicts are interpreted inconsistently.
- **Schema sprawl**: tolerating unknown top-level keys creates incompatible client behavior.
- **Scope creep**: execution semantics leak into this schema RFC and dilute clarity.

## Implementation Notes (Planning Scope)

- This plan is spec-first. It defines what must be true before implementation begins.
- Decision set is now narrowed to schema concerns only:
  - Canonical top-level shape and required keys.
  - RFC 0004 inheritance boundaries.
  - Source-anchor conflict semantics.
  - Relation-block validation rules and examples.

## References & Research

### Internal References

- `docs/brainstorms/2026-02-18-relation-data-blocks-brainstorm.md:1` - Chosen direction and open questions.
- `docs/rfcs/0004-data-block-filter-spec.md:25` - Canonical filter envelope/grammar.
- `docs/rfcs/0004-data-block-filter-spec.md:100` - `_relation` recursive scope semantics.
- `docs/rfcs/0004-data-block-filter-spec.md:172` - Validation style baseline.
- `docs/rfcs/0004-data-block-filter-spec.md:181` - Versioning pattern baseline.
- `docs/protocol/knowledge-graph-ontology.md:80` - Relation direction semantics.
- `docs/protocol/knowledge-graph-ontology.md:90` - Relation `position` semantics.
- `docs/specs/versioned-diffing.md:347` - Example of keeping schema and execution contracts separate by scope.

### Institutional Learnings

- No `docs/solutions/` entries were found for this topic; nearest applicable guidance comes from RFC/spec docs listed above.

### External Research

- Skipped intentionally: strong local protocol/RFC context and clear brainstorm decisions made external research low-value for this planning pass.
