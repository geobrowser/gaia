---
status: complete
priority: p2
issue_id: "002"
tags: [code-review, safety, base58, tigerstyle]
dependencies: []
---

# Add assertions and input validation to Base58 codec

## Problem Statement

The Base58 codec (`base58.ts`) and UUID utilities (`uuid.ts`) lack runtime assertions at function boundaries. While the code is correct for current callers, exported functions can be misused. The TigerStyle review identified 4 missing assertion sites and 1 silent failure path.

**Impact:** Harder to debug misuse; error messages from `BigInt` constructor are cryptic. The `toUuid()` try/catch swallows the specific overflow error from `decodeBase58`.

## Findings

- **`encodeBase58(dashlessHex)`** (`base58.ts:27`): No validation that input is 32 lowercase hex chars. Passing non-hex produces a cryptic `BigInt` SyntaxError.
- **`decodeBase58(encoded)`** (`base58.ts:72`): No postcondition asserting output is exactly 32 hex chars.
- **`toUuid()` try/catch** (`uuid.ts:64-71`): Catches `decodeBase58` overflow error and replaces it with generic "Invalid UUID" message. Loses diagnostic context.
- **`toBase58(uuid)`** (`uuid.ts:105-109`): Trusts the branded type at runtime. If a raw string is cast to `Uuid`, `replaceAll("-", "")` could produce non-32-char string.
- **`hexToBigInt(hex)`** (`base58.ts:87-89`): Internal function with no guard on empty string.
- **Dash-insertion logic duplicated** at `uuid.ts:60` and `uuid.ts:67` — two places to get wrong, no postcondition.

## Proposed Solutions

### Option 1: Add assertions at all function boundaries

**Approach:** Add precondition checks to `encodeBase58`, postcondition to `decodeBase58`, runtime check in `toBase58`, and extract `insertDashes` helper.

**Pros:**
- Catches misuse immediately with clear error messages
- Postconditions act as safety nets for internal logic bugs
- Eliminates duplicated dash-insertion

**Cons:**
- Small runtime cost per call (regex test on hot path)
- ~20 lines of added code

**Effort:** 1 hour

**Risk:** Low

---

### Option 2: Remove try/catch in `toUuid()` Base58 branch only

**Approach:** If `isBase58()` returns true, let `decodeBase58` throw naturally (only overflow can cause this). Don't swallow the error.

**Pros:**
- Preserves specific error message for overflow
- Minimal change (remove try/catch, keep the rest)

**Cons:**
- Doesn't address the other assertion gaps

**Effort:** 15 minutes

**Risk:** Low

---

### Option 3: Full TigerStyle treatment (Options 1 + 2 combined)

**Approach:** All assertions + remove try/catch + extract `insertDashes`.

**Effort:** 1.5 hours

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts:27,72,87` — `encodeBase58`, `decodeBase58`, `hexToBigInt`
- `api/src/utils/uuid.ts:55-73,105-109` — `toUuid`, `toBase58`

## Acceptance Criteria

- [ ] `encodeBase58` throws descriptive error on non-32-char or non-hex input
- [ ] `decodeBase58` asserts output is exactly 32 hex chars
- [ ] `toUuid()` does not swallow overflow error from `decodeBase58`
- [ ] `toBase58()` has runtime length check on dashless string
- [ ] Dash-insertion extracted to single helper with postcondition
- [ ] All existing tests still pass
- [ ] New tests for invalid `encodeBase58` input

## Work Log

### 2026-02-09 - TigerStyle Review Discovery

**By:** Claude Code (tigerstyle-reviewer agent)

**Actions:**
- Identified 6 missing assertion sites across base58.ts and uuid.ts
- Verified that `isBase58("zzzzzzzzzzzzzzzzzzzzzz")` returns true but `decodeBase58` throws overflow — error is swallowed by toUuid's catch
- Confirmed dash-insertion logic is duplicated at two sites
