---
status: complete
priority: p2
issue_id: "003"
tags: [code-review, edge-case, base58, security]
dependencies: []
---

# Zero UUID encodes to empty Base58 string (roundtrip broken)

## Problem Statement

`encodeBase58("00000000000000000000000000000000")` returns `""`. `decodeBase58("")` throws "empty string". This means the nil UUID (`00000000-0000-0000-0000-000000000000`) cannot roundtrip through encode/decode. If a nil UUID appears in the database, the API returns an empty string ID.

**Impact:** Clients receiving `""` for an ID field may treat it as null/missing. The roundtrip test conspicuously excludes the zero UUID. This matches Rust parity, so it's a known design trade-off.

## Findings

- `encodeBase58` line 29: returns `""` for zero value (matches Rust `encode_uuid_to_base58`)
- `decodeBase58` line 50: throws on empty string input
- Roundtrip test (`base58.test.ts:62-79`): starts at `"...0001"`, excludes zero UUID
- `toBase58(toUuid("00000000-0000-0000-0000-000000000000"))` → `""` in production
- Found by: security-sentinel (medium), code-reviewer (low), tigerstyle-reviewer (low)

## Proposed Solutions

### Option 1: Document the behavior and add a guard comment

**Approach:** Add a prominent warning comment on `toBase58` and `encodeBase58`. Add a test that explicitly documents the zero UUID edge case.

**Pros:**
- No behavior change
- Maintains Rust parity
- Zero UUIDs are extremely rare in practice

**Cons:**
- Doesn't fix the roundtrip gap
- Clients still get `""` if it happens

**Effort:** 15 minutes

**Risk:** Low

---

### Option 2: Return a sentinel value for zero UUID

**Approach:** Encode zero UUID as `"1"` (single digit, maps to 0x1... wait, `1` maps to index 0 in Base58, which is value 0... this doesn't cleanly map). Alternative: return `"11111111111111111111111111111111"` (Base58Check style leading-1 padding).

**Pros:**
- Roundtrip works for zero UUID

**Cons:**
- Breaks Rust parity
- Adds special-case logic

**Effort:** 30 minutes

**Risk:** Medium (parity break)

---

### Option 3: Assert zero UUID cannot reach `toBase58`

**Approach:** Add a guard in `toBase58` that throws if the UUID is all zeros.

**Pros:**
- Fails fast instead of returning confusing empty string
- Documents that zero UUIDs should not exist in the data

**Cons:**
- Throws on valid (if unlikely) data

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts:27-29` — encodeBase58 zero check
- `api/src/utils/uuid.ts:105-109` — toBase58
- `api/src/utils/base58.test.ts:17,62-79` — zero UUID test, roundtrip exclusion

## Acceptance Criteria

- [ ] Zero UUID behavior is either documented, guarded, or handled
- [ ] Roundtrip test includes zero UUID (documenting expected behavior)
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Multi-agent Review Discovery

**By:** Claude Code (security-sentinel + tigerstyle-reviewer)

**Actions:**
- Identified roundtrip gap: encode("000...") → "" → decode("") throws
- Verified this matches Rust implementation behavior
- Confirmed zero UUID is valid per RFC 4122 but extremely unlikely in production data
