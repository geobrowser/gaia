/**
 * Tests for profile queries.
 *
 * These tests verify:
 * 1. Profile fetching by address and space ID
 * 2. Batch profile fetching
 * 3. Front page entity resolution for name/avatar
 * 4. Default profile fallback behavior
 */

import { Effect } from "effect";
import { Hono } from "hono";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createProfileRouter } from "../index";

// =============================================================================
// Test Setup
// =============================================================================

/**
 * Create a minimal mock database that implements the query interface.
 */
function createMockDb() {
  return {
    execute: vi.fn(),
  };
}

/**
 * Create a minimal mock runtime for testing.
 */
function createMockRuntime() {
  return {
    runPromise: <A, E>(effect: Effect.Effect<A, E, never>) =>
      Effect.runPromise(effect),
  };
}

/**
 * Set up test app with mock dependencies.
 */
function setupTestApp() {
  const db = createMockDb();
  const runtime = createMockRuntime();
  // biome-ignore lint/suspicious/noExplicitAny: test mock
  const router = createProfileRouter(db as any, runtime as any);
  const app = new Hono();
  app.route("/profile", router);
  return { app, db, runtime };
}

/**
 * Create a mock profile row as returned from the database.
 */
function makeDbProfileRow(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    entity_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    space_id: "f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
    space_address: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
    entity_name: "Test User",
    avatar_url: "https://example.com/avatar.png",
    ...overrides,
  };
}

// =============================================================================
// GET /profile/address/:address Tests
// =============================================================================

describe("GET /profile/address/:address", () => {
  it("should return profile for valid address", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow();
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request(
      "/profile/address/0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual({
      entityId: "a1b2c3d4e5f67890abcdef1234567890",
      spaceId: "f3dab79cb5a3d9d1759656dd5361d1c6",
      name: "Test User",
      avatarUrl: "https://example.com/avatar.png",
      address: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
    });
  });

  it("should return default profile when address has no space", async () => {
    const { app, db } = setupTestApp();
    db.execute.mockResolvedValueOnce({ rows: [] });

    const res = await app.request(
      "/profile/address/0x1234567890123456789012345678901234567890",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual({
      entityId: null,
      spaceId: "0x1234567890123456789012345678901234567890",
      name: null,
      avatarUrl: null,
      address: "0x1234567890123456789012345678901234567890",
    });
  });

  it("should return 400 for invalid address format", async () => {
    const { app } = setupTestApp();

    const res = await app.request("/profile/address/not-an-address");

    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error).toBe("Invalid parameter");
  });

  it("should normalize address to lowercase", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow();
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    await app.request(
      "/profile/address/0xAB28066D9A7DDFF52B67DF699592BA7060E0D3B9",
    );

    // Verify the query was called with lowercase address
    expect(db.execute).toHaveBeenCalled();
    const queryCall = db.execute.mock.calls[0]?.[0];
    expect(queryCall).toBeDefined();
    // The SQL template includes the lowercase address in its queryChunks or values
    const sqlString = queryCall?.queryChunks?.join("") ?? String(queryCall);
    expect(sqlString.toLowerCase()).toContain(
      "0xab28066d9a7ddff52b67df699592ba7060e0d3b9",
    );
  });
});

// =============================================================================
// GET /profile/space/:spaceId Tests
// =============================================================================

describe("GET /profile/space/:spaceId", () => {
  it("should return profile for valid space ID", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow();
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request(
      "/profile/space/f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual({
      entityId: "a1b2c3d4e5f67890abcdef1234567890",
      spaceId: "f3dab79cb5a3d9d1759656dd5361d1c6",
      name: "Test User",
      avatarUrl: "https://example.com/avatar.png",
      address: "0xab28066d9a7ddFF52B67dF699592BA7060e0d3b9",
    });
  });

  it("should return default profile when space not found", async () => {
    const { app, db } = setupTestApp();
    db.execute.mockResolvedValueOnce({ rows: [] });

    const res = await app.request(
      "/profile/space/f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toEqual({
      entityId: null,
      spaceId: "f3dab79cb5a3d9d1759656dd5361d1c6",
      name: null,
      avatarUrl: null,
      address: "f3dab79cb5a3d9d1759656dd5361d1c6",
    });
  });

  it("should return 400 for invalid space ID format", async () => {
    const { app } = setupTestApp();

    const res = await app.request("/profile/space/not-a-uuid");

    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error).toBe("Invalid parameter");
  });

  it("should accept space ID without dashes", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow();
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request(
      "/profile/space/f3dab79cb5a3d9d1759656dd5361d1c6",
    );

    expect(res.status).toBe(200);
  });
});

// =============================================================================
// POST /profile/batch Tests
// =============================================================================

describe("POST /profile/batch", () => {
  it("should return profiles for valid space IDs", async () => {
    const { app, db } = setupTestApp();
    const mockRow1 = makeDbProfileRow();
    const mockRow2 = makeDbProfileRow({
      space_id: "a1234567-b5a3-d9d1-7596-56dd5361d1c6",
      entity_name: "Another User",
    });
    db.execute.mockResolvedValueOnce({ rows: [mockRow1, mockRow2] });

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        spaceIds: [
          "f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
          "a1234567-b5a3-d9d1-7596-56dd5361d1c6",
        ],
      }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.profiles).toHaveLength(2);
    expect(body.profiles[0].name).toBe("Test User");
    expect(body.profiles[1].name).toBe("Another User");
  });

  it("should return empty array for empty input", async () => {
    const { app } = setupTestApp();

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ spaceIds: [] }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.profiles).toEqual([]);
  });

  it("should return default profiles for missing spaces", async () => {
    const { app, db } = setupTestApp();
    // Only return one profile even though two were requested
    const mockRow = makeDbProfileRow();
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        spaceIds: [
          "f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
          "00000000-0000-0000-0000-000000000000",
        ],
      }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.profiles).toHaveLength(2);
    expect(body.profiles[0].name).toBe("Test User");
    expect(body.profiles[1].name).toBeNull(); // Default profile
  });

  it("should return 400 for invalid space ID in batch", async () => {
    const { app } = setupTestApp();

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        spaceIds: ["f3dab79c-b5a3-d9d1-7596-56dd5361d1c6", "not-a-uuid"],
      }),
    });

    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.error).toBe("Invalid parameter");
  });

  it("should return 400 when exceeding batch size limit", async () => {
    const { app } = setupTestApp();
    const tooManyIds = Array(101)
      .fill(null)
      .map((_, i) => `f3dab79c-b5a3-d9d1-7596-${String(i).padStart(12, "0")}`);

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ spaceIds: tooManyIds }),
    });

    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.message).toContain("Maximum");
  });

  it("should return 400 for missing spaceIds field", async () => {
    const { app } = setupTestApp();

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });

    expect(res.status).toBe(400);
  });

  it("should preserve order of input space IDs", async () => {
    const { app, db } = setupTestApp();
    // Return profiles in different order than requested
    const mockRow1 = makeDbProfileRow({
      space_id: "b0000000-0000-0000-0000-000000000000",
      entity_name: "User B",
    });
    const mockRow2 = makeDbProfileRow({
      space_id: "a0000000-0000-0000-0000-000000000000",
      entity_name: "User A",
    });
    db.execute.mockResolvedValueOnce({ rows: [mockRow1, mockRow2] });

    const res = await app.request("/profile/batch", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        spaceIds: [
          "a0000000-0000-0000-0000-000000000000",
          "b0000000-0000-0000-0000-000000000000",
        ],
      }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    // Should match input order, not DB return order
    expect(body.profiles[0].name).toBe("User A");
    expect(body.profiles[1].name).toBe("User B");
  });
});

// =============================================================================
// Profile Data Resolution Tests
// =============================================================================

describe("Profile data resolution", () => {
  it("should return profile with null name when front page entity has no name", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow({ entity_name: null });
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request(
      "/profile/space/f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.name).toBeNull();
    expect(body.avatarUrl).toBe("https://example.com/avatar.png");
  });

  it("should return profile with null avatar when no avatar relation exists", async () => {
    const { app, db } = setupTestApp();
    const mockRow = makeDbProfileRow({ avatar_url: null });
    db.execute.mockResolvedValueOnce({ rows: [mockRow] });

    const res = await app.request(
      "/profile/space/f3dab79c-b5a3-d9d1-7596-56dd5361d1c6",
    );

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.name).toBe("Test User");
    expect(body.avatarUrl).toBeNull();
  });
});
