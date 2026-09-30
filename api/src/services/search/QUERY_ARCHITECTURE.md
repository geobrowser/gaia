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
┌─────────────────┐
│ Sort + Collapse │ ◀── Total tiebreak order, one row per entity
└────────┬────────┘
         │
         ▼
    Search Results
```

---

## 0. Text Analysis

`name` and `description` are `search_as_you_type` fields using the custom
**`text_apostrophe_folded`** analyzer, defined in
`search-indexer-repository/src/opensearch/index_config.rs`:

```
char_filter: apostrophe_fold   (' ' ʼ ＇ → ')
tokenizer:   standard
filter:      lowercase
```

`name_raw` carries the matching **`apostrophe_folded_keyword`** normalizer — the same
fold, with no lowercase filter, because the exact-name clause is case-sensitive by design.

Two properties of this analyzer are load-bearing:

- **Apostrophes are folded, so spelling does not decide whether a search works.** The
  standard tokenizer keeps an apostrophe inside its token (`UAX #29` MidLetter), so without
  the fold `man's` and `man's` are unrelated terms. The corpus is genuinely mixed — of 294
  debate claims sampled on testnet, 66 use the ASCII apostrophe and 24 the typographic one
  — so before this fold, a query scored ~400 against one spelling and exactly 0.00 against
  the other (GEO-2904).
- **`name` and `description` are not stemmed, deliberately.** The prefix sub-fields of a
  `search_as_you_type` field index prefixes of the *indexed* term, so stemming here would
  break autocomplete on partially typed words: once "running" stems to "run", a user who
  has typed "runn" no longer matches it. Stemming lives on sibling fields instead (below).

### Stemmed siblings (index v7, GEO-3047)

`name_stemmed` and `description_stemmed` are plain `text` fields analyzed with
**`text_stemmed`**:

```
char_filter: apostrophe_fold
tokenizer:   standard
filter:      lowercase, possessive_english, stemmer_overrides (news => news), minimal_english
```

so `man's`, `man’s`, `mans` and `man` all index as `man`, and `pardons` as `pardon`.
`minimal_english` only strips plural endings; it leaves verb forms alone and does not
conflate unrelated words the way Porter does (`university` / `universe`).

They are filled by **`copy_to`** from `name` and `description`, not by a `fields`
multi-field: `search_as_you_type` accepts `fields` in the mapping and then silently drops
it (the same reason `name_raw` is top-level). Because copy_to runs at index time from
`_source`, a plain `_reindex` from a v6 index populates them, and the indexer needs no
change. Copied values are not added to `_source`, so responses are unchanged.

On an index without these fields (v6 and older) the stemmed clauses match nothing and do
not error, so the API change is a no-op until the alias points at v7.

Consequence worth knowing when reading scores: because folding merges two spellings into
one term, document frequency rises and IDF falls, so absolute scores for previously-split
terms drop slightly (e.g. `trump's tariffs` 354.2 → 338.8) while the ordering is unchanged.

---

## 1. UUID Fast Path

If the query matches a UUID pattern, it bypasses text search entirely and performs a direct `term` lookup on `entity_id`.

---

## 2. Base Text Query

Up to eleven parallel matching strategies run inside a `bool.should` clause with `minimum_should_match: 1`:

### Strategy Breakdown

| Strategy | Query Type | Fields | Boost | Purpose |
|----------|------------|--------|-------|---------|
| **Exact Raw Name** | `term` | `name_raw` | **10.0×** | Exact case-sensitive full-string match on unanalyzed name |
| **Raw Name (case-insensitive)** | `term` (case_insensitive) | `name_raw` | **5.0×** | Case-insensitive full-string match, still distinguishes punctuation |
| **Exact Name Token** | `match` | `name` | **8.0×** | Strong boost for exact analyzed token match in name |
| **Autocomplete** | `multi_match` (bool_prefix) | `name^1.5`, `name._2gram^1.5`, `name._3gram^1.5`, `description`, `description._2gram`, `description._3gram` | 1.5× on name fields | N-gram autocomplete matching |
| **Fuzzy** | `multi_match` | `name`, `description` | **0.6×** (reduced) | Typo tolerance with `AUTO:4,6` fuzziness; only for queries of up to 3 tokens |
| **Real-match floor** | `constant_score` over `bool_prefix` on `name`/`description` + `match` on the stemmed fields | — | **+100** flat | Only with the fuzzy clause. Every non-fuzzy match gets it, so fuzzy-only matches rank below all of them whatever their entity score (GEO-3048) |
| **Name coverage** | `constant_score` over `match` on `name_stemmed`, `minimum_should_match: 80%` of the content words | `name_stemmed` | **+50** flat | Only for queries with 3+ non-stopwords. Rewards names containing most of the query over short names matching fewer words (GEO-2640) |
| **Stemmed Name** | `match` | `name_stemmed` | **10.0×** | Plurals and possessives: `mans` / `man's` reach `man` (GEO-3047) |
| **Stemmed Desc** | `match` | `description_stemmed` | **3.0×** | The same for descriptions |
| **Name Prefix** | `match_phrase_prefix` | `name` | **5.0×** | Strong boost for "starts with" on name |
| **Desc Prefix** | `match_phrase_prefix` | `description` | **1.5×** | Moderate boost for "starts with" on description |

### Fuzziness Behavior (`AUTO:4,6`)

- 1-3 character words: 0 edits allowed (plain `AUTO` allows one from 3, which turned `nft` into `not`)
- 4-5 character words: 1 edit allowed
- 6+ character words: 2 edits allowed

`prefix_length` is 0, so a typo in the first letter (`vitcoin`) is still corrected.

---

## 3. Scope-Specific Behavior

| Scope | Filter | Score Field |
|-------|--------|-------------|
| `GLOBAL` | None | `entity_global_score` |
| `GLOBAL_BY_SPACE_SCORE` | None | `space_score` |
| `GLOBAL_BY_ENTITY_SPACE_SCORE` | None | `entity_space_score` |
| `SPACE_SINGLE` / `SPACE` | `space_id` term filter | `entity_space_score` |

---

## 4. Order and One Row Per Entity

`buildSearchBody` adds the same `sort` and `collapse` to every scope (GEO-2394).

**Sort:** `_score` desc, then `in_canonical_graph` desc, `entity_id` asc, `space_id` asc. Scores tie
exactly whenever several entities share a name, because the name clauses dominate the score. Without
a tiebreak OpenSearch orders tied rows by internal doc id, which changes whenever the indexer rewrites
a document, so `from`/`size` paging could repeat or skip rows. Canonical-first is the tiebreak that
carries meaning; the two ids make the order total, since there is one document per (entity, space).

**Collapse:** the index holds one document per (entity, space), so an entity in two spaces used to
fill two rows. `collapse` on `entity_id` keeps each entity's first document in sort order (best
score, canonical on a tie). `from`/`size` count collapsed entities, so pages stay full and nothing
is skipped. The entity's other matching documents come back through `inner_hits` and are returned
as `otherSpaces` (at most `MAX_OTHER_SPACES`). `total` still counts documents, so it is an upper
bound on the rows reachable by paging.

Each collapsed row runs one small inner query to fetch its `otherSpaces`. That is the main cost of
the collapse: locally about 0.4 ms per row, so under 10 ms for a 20-row page.

---

## Score Hierarchy

From highest to lowest impact:

1. **Exact name token match** — `match` on name (10.0×) — query terms exactly match analyzed tokens
2. **Exact name prefix match** — `match_phrase_prefix` on name (5.0×)
3. **Name n-gram matches** — `bool_prefix` with 1.5× field boost
4. **Description prefix match** — `match_phrase_prefix` on description (1.5×)
5. **Description n-gram matches** — no additional boost
6. **Fuzzy matches** — 0.6× penalty (deliberately reduced), and below every non-fuzzy match because of the real-match floor
7. **+ Score Fields** — additive boost from score fields via `function_score` with `script_score` (clamped, shifted, then 1.3× multiplied)

---

## Boost Constants

| Constant | Value | Usage |
|----------|-------|-------|
| `SCORE_BOOST` | 75.0 | Multiplier applied inside `script_score` logic after clamping and shifting score fields (see `buildScoreBoostFunction` in opensearch.ts) |
| `NAME_PREFIX_BOOST` | 5.0 | `match_phrase_prefix` on name |
| `DESCRIPTION_PREFIX_BOOST` | 1.5 | `match_phrase_prefix` on description |
| `NAME_FIELD_BOOST` | 1.5 | Field boost on name in `multi_match` |
| `FUZZY_REDUCTION_BOOST` | 0.6 | Penalty for fuzzy matches |
| `FUZZY_MIN_TERM_LENGTH` | 4 | Shortest term the fuzzy clause may edit |
| `FUZZY_PREFIX_LENGTH` | 0 | Leading characters a fuzzy edit may not touch |
| `REAL_MATCH_BOOST` | 100.0 | Flat bonus for any non-fuzzy match when the fuzzy clause is present; must exceed the entity-score spread (`SCORE_BOOST`) |
| `NAME_COVERAGE_BOOST` | 50.0 | Flat bonus for a name containing 80% of 3+ content words |

All of these, like the boosts above, can be overridden per request with the query parameter of
the same name in lowercase (`fuzzy_min_term_length`, `fuzzy_prefix_length`, `real_match_boost`,
`name_coverage_boost`); 0 turns a bonus off.

---

## Design Rationale

- **Name prioritization**: Users typically search by name, so name matches are weighted higher than description matches.
- **Prefix matching**: Strong prefix boosts indicate high user intent (typing the start of what they're looking for).
- **NAME_PREFIX_BOOST and BM25 field length normalization**: The `NAME_PREFIX_BOOST` (5.0) is intentionally much higher than `DESCRIPTION_PREFIX_BOOST` (1.5) — a 3.3× ratio. This is necessary because BM25 scoring includes a field length normalization factor (`dl/avgdl`) that can cause short description matches to outscore name matches. In production, when the average description length across the index is much longer than a given entity's description (e.g., average 50 tokens but description is 2 tokens), BM25 amplifies the description match score significantly. A smaller ratio (e.g., 2.0/1.5 = 1.33×) is insufficient to overcome this effect, leading to entities like "Rex" (description: "Researcher @Wonderland") outranking "Wonderland" (name match) for the query "Wonderland". If this becomes an issue again as index composition changes, consider wrapping `match_phrase_prefix` clauses in `constant_score` to bypass BM25 normalization entirely.
- **Fuzzy penalty**: Fuzzy matches are useful for typo tolerance but should rank below exact/prefix matches to prevent false positives.
- **Score field normalization**: Score fields use `float` type normalized to [0, 1] with 0.5 as average. Boosting is done via `function_score` with `script_score` (see `buildScoreBoostFunction` in opensearch.ts). The script applies:
  1. **Clamping**: Scores are clamped to [`MIN_SCORE_THRESHOLD`, `MAX_SCORE_THRESHOLD`] = [0.0, 1.0]. The upper clamp matters for `entity_global_score`, which scoring-service computes as a sum over the entity's spaces and which exceeds 1 for entities in many spaces (652 on testnet, up to 5.13). Without it the boost spread would exceed `REAL_MATCH_BOOST` and the fuzzy floor would stop holding
  2. **Shifting**: Scores are shifted by `SCORE_SHIFT` (1.0) to ensure all values are positive (OpenSearch requirement)
  3. **Multiplier**: The shifted score is multiplied by `SCORE_BOOST` (75.0) for the final boost value
  4. **Formula**: `(min(max(score, 0.0), 1.0) + 1.0) * 75.0`
  5. **Range**: score=0.0 → boost=75, score=0.5 → boost=112.5, score=1.0 → boost=150
- **Autocomplete support**: `search_as_you_type` field type with n-gram sub-fields enables smooth autocomplete UX.

---

## Output Score Fields

Each search result includes two computed score fields:

| Field | Description | Derivation |
|-------|-------------|------------|
| `relevanceScore` | Final score after all boosts | OpenSearch `_score` |
| `textMatchScore` | Text matching score without score field boosts | `relevanceScore - scoreBoost` (clamped to 0) |

The `scoreBoost` is computed via `script_fields` using the same Painless script as `buildScoreBoostFunction`. For empty queries (top-ranked), `textMatchScore` is 0 since `boost_mode: "replace"` means `_score` equals the boost. For UUID queries, `textMatchScore` equals `relevanceScore` since there is no score field boost. `textMatchScore` includes the real-match floor and name coverage bonuses, so for queries of up to 3 tokens every non-fuzzy match scores at least 100, and a value under 100 there means the match is fuzzy-only.

