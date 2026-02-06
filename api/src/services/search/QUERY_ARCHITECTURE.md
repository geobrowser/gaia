# OpenSearch Query Architecture

This document explains the order of operations, boost strategies, and scoring hierarchy for OpenSearch queries.

## Query Flow Overview

```
Query Input
    │
    ▼
┌─────────────────┐
│  UUID Check     │──────▶ Direct term lookup (fast path)
└────────┬────────┘
         │ (not UUID)
         ▼
┌─────────────────┐
│ Base Text Query │ ◀── 4 parallel matching strategies
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Scope Wrapper   │ ◀── Adds function_score boost + filters
└────────┬────────┘
         │
         ▼
    Search Results
```

---

## 1. UUID Fast Path

If the query matches a UUID pattern, it bypasses text search entirely and performs a direct `term` lookup on `entity_id`.

---

## 2. Base Text Query

Four parallel matching strategies run inside a `bool.should` clause with `minimum_should_match: 1`:

### Strategy Breakdown

| Strategy | Query Type | Fields | Boost | Purpose |
|----------|------------|--------|-------|---------|
| **Autocomplete** | `multi_match` (bool_prefix) | `name^1.5`, `name._2gram^1.5`, `name._3gram^1.5`, `description`, `description._2gram`, `description._3gram` | 1.5× on name fields | N-gram autocomplete matching |
| **Fuzzy** | `multi_match` | `name`, `description` | **0.6×** (reduced) | Typo tolerance with AUTO fuzziness |
| **Name Prefix** | `match_phrase_prefix` | `name` | **2.0×** | Strong boost for "starts with" on name |
| **Desc Prefix** | `match_phrase_prefix` | `description` | **1.5×** | Moderate boost for "starts with" on description |

### Fuzziness Behavior (AUTO)

- 1-2 character words: 0 edits allowed
- 3-4 character words: 1 edit allowed  
- 5+ character words: 2 edits allowed

---

## 3. Scope-Specific Behavior

| Scope | Filter | Score Field |
|-------|--------|-------------|
| `GLOBAL` | None | `entity_global_score` |
| `GLOBAL_BY_SPACE_SCORE` | None | `space_score` |
| `GLOBAL_BY_ENTITY_SPACE_SCORE` | None | `entity_space_score` |
| `SPACE_SINGLE` / `SPACE` | `space_id` term filter | `entity_space_score` |

---

## Score Hierarchy

From highest to lowest impact:

1. **Exact name prefix match** — `match_phrase_prefix` on name (2.0×)
2. **Name n-gram matches** — `bool_prefix` with 1.5× field boost
3. **Description prefix match** — `match_phrase_prefix` on description (1.5×)
4. **Description n-gram matches** — no additional boost
5. **Fuzzy matches** — 0.6× penalty (deliberately reduced)
6. **+ Score Fields** — additive boost from score fields via `function_score` with `script_score` (clamped, shifted, then 1.3× multiplied)

---

## Boost Constants

| Constant | Value | Usage |
|----------|-------|-------|
| `SCORE_BOOST` | 1.3 | Multiplier applied inside `script_score` logic after clamping and shifting score fields (see `buildScoreBoostFunction` in opensearch.ts) |
| `NAME_PREFIX_BOOST` | 2.0 | `match_phrase_prefix` on name |
| `DESCRIPTION_PREFIX_BOOST` | 1.5 | `match_phrase_prefix` on description |
| `NAME_FIELD_BOOST` | 1.5 | Field boost on name in `multi_match` |
| `FUZZY_REDUCTION_BOOST` | 0.6 | Penalty for fuzzy matches |

---

## Design Rationale

- **Name prioritization**: Users typically search by name, so name matches are weighted higher than description matches.
- **Prefix matching**: Strong prefix boosts indicate high user intent (typing the start of what they're looking for).
- **Fuzzy penalty**: Fuzzy matches are useful for typo tolerance but should rank below exact/prefix matches to prevent false positives.
- **Score field normalization**: Score fields use `float` type to support positive, zero, and negative values from z-score normalization. Boosting is done via `function_score` with `script_score` (see `buildScoreBoostFunction` in opensearch.ts). The script applies:
  1. **Clamping**: Scores below `MIN_SCORE_THRESHOLD` (-10.0) are clamped to -10.0 to prevent excessive penalties
  2. **Shifting**: Scores are shifted by `SCORE_SHIFT` (10.0) to ensure all values are positive (OpenSearch requirement)
  3. **Multiplier**: The shifted score is multiplied by `SCORE_BOOST` (1.3) for the final boost value
  4. **Formula**: `(max(score, -10.0) + 10.0) * 1.3`
- **Autocomplete support**: `search_as_you_type` field type with n-gram sub-fields enables smooth autocomplete UX.

