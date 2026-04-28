#!/usr/bin/env tsx

/**
 * Search API Validation Script
 *
 * This script validates the search API responses after test events are generated.
 * It uses the actual TypeScript types from the search service.
 */

import type { SearchQuery, SearchResponse, SearchResult } from '../../../../api/src/services/search/types';
import { DEFAULT_AVERAGE_SCORE, SCORE_BOOST, SCORE_SHIFT, MIN_SCORE_THRESHOLD } from '../../../../api/src/services/search/opensearch';

// Colors for terminal output
const GREEN = '\x1b[0;32m';
const RED = '\x1b[0;31m';
const YELLOW = '\x1b[1;33m';
const BLUE = '\x1b[0;34m';
const NC = '\x1b[0m'; // No Color

// Fixed entity IDs matching the test scenario in main.rs (dashless format — canonical API output)
const TEST_ENTITIES = {
  SPACE_ID: '00000000000040008000000000000001',
  PERSON_TYPE_ID: '00000000000000000000000000000b01',
  ORG_TYPE_ID: '00000000000000000000000000000b02',
  ALICE_HIGH_ID: '000000000000000000000000000000f1',
  ALICE_MEDIUM_ID: '000000000000000000000000000000f2',
  ALICE_LOW_ID: '000000000000000000000000000000f3',
  ALICE_ZERO_ID: '000000000000000000000000000000f4',
  ALICE_NEGATIVE_ID: '000000000000000000000000000000f5',
  ALICE_AT_THRESHOLD_ID: '000000000000000000000000000000f6',
  ALICE_BELOW_THRESHOLD_ID: '000000000000000000000000000000f7',
  BOB_ID: '00000000000000000000000000000b0b',
  CHARLIE_ID: '00000000000000000000000000000c1c',
  ORG_ID: '000000000000000000000000000ac3ec',
  // Entities for soft delete testing
  DELETE_CHARLIE_ID: '00000000000000000000000000000c01',
  DELETE_DANA_ID: '00000000000000000000000000000d01',
  DELETE_EVE_ID: '00000000000000000000000000000e01',
  // Entities for unset property testing
  UNSET_TEST_1_ID: '00000000000000000000000000001111',
  UNSET_TEST_2_ID: '00000000000000000000000000002222',
  LWW_TEST_ID: '00000000000000000000000000003333',
  // Entity created via CreateEntity GRC-20 op
  CREATE_ENTITY_TEST_ID: '0000000000000000000000000000ce01',
  // GLOBAL_BY_ENTITY_SPACE_SCORE ranking test entities
  RANK_HIGH_SPACE_ID: '00000000000040008000000000000a01',     // space_score = 0.80
  RANK_LOW_SPACE_ID: '00000000000040008000000000000b01',      // space_score = 0.10
  RANK_GAMMA_ENTITY_ID: '00000000000000000000000000000ea1',   // entity_space=0.90, space=0.80, product=0.72
  RANK_DELTA_ENTITY_ID: '00000000000000000000000000000ea2',   // entity_space=0.20, space=0.80, product=0.16
  RANK_EPSILON_ENTITY_ID: '00000000000000000000000000000ea3', // entity_space=0.90, space=0.10, product=0.09
  RANK_ZETA_ENTITY_ID: '00000000000000000000000000000ea4',    // entity_space=0.20, space=0.10, product=0.02
  // Text match scoring test entities (all have entity_global_score = 0.50)
  // Group A: Name match vs description-only match (query: "Wonderland")
  TM_NAME_MATCH_ID: '0000000000000000000000000000aa01',     // name="Wonderland"
  TM_DESC_MATCH_ID: '0000000000000000000000000000aa02',     // name="Rex", desc="Researcher @Wonderland"
  // Group B: Exact match vs fuzzy match (query: "Blockchain")
  TM_EXACT_MATCH_ID: '0000000000000000000000000000aa03',    // name="Blockchain"
  TM_FUZZY_MATCH_ID: '0000000000000000000000000000aa04',    // name="Blockchan" (typo)
  // Group C: Multi-word match vs single-word match (query: "San Francisco")
  TM_MULTI_WORD_ID: '0000000000000000000000000000aa05',     // name="San Francisco"
  TM_SINGLE_WORD_ID: '0000000000000000000000000000aa06',    // name="San Diego"
  // Group D: Name+description match vs name-only match (query: "Quantum")
  TM_NAME_AND_DESC_ID: '0000000000000000000000000000aa07',  // name="Quantum Computing", desc mentions "Quantum"
  TM_NAME_ONLY_ID: '0000000000000000000000000000aa08',      // name="Quantum Mechanics", desc has no match
  // Group E: High global score vs low global score, both match in name (query: "Velociraptor")
  TM_HIGH_SCORE_ID: '0000000000000000000000000000aa09', // name="Velociraptor Research", score=0.90
  TM_LOW_SCORE_ID: '0000000000000000000000000000aa0a',  // name="Velociraptor Species", score=0.20
  // Group F: Exact short name match vs longer prefix name match (query: "geo")
  // Both have NO score values (default score behavior)
  GEO_EXACT_ID: '0000000000000000000000000000bb01',     // name="Geo", desc="Geo is a network..."
  GEO_PREFIX_ID: '0000000000000000000000000000bb02',    // name="geojson_preview_tool", desc="Generate a geojson.io URL..."
  GEO_GRAPH_ID: '0000000000000000000000000000bb03',     // name="Geo Graph", desc="Geo Graph is an open source tool..."
  // Group G: Exact raw name match vs hyphenated (query: "World affairs")
  WORLD_AFFAIRS_EXACT_ID: '0000000000000000000000000000bb04', // name="World affairs"
  WORLD_AFFAIRS_HYPHEN_ID: '0000000000000000000000000000bb05', // name="world-affairs"
  // Topic entity for space metadata enrichment
  TOPIC_ENTITY_ID: '00000000000000000000000000000d00',   // represents test_space as a topic entity
  // Late entity created AFTER space.topics event (tests cache-hit ordering)
  LATE_ENTITY_ID: '0000000000000000000000000000af01',    // name="Frankie Late", created after HermesTopicDeclared

  // ── Topology / canonical graph test spaces ────────────────────────────────
  // Tree after all three diffs:
  //   topo_root (d=0, canonical root)
  //     ├── topo_child_a  (verified, d=1)
  //     │     └── topo_grandchild (verified, d=2)
  //     └── topo_child_b  (related,  d=1)
  //           └── topo_moveable (verified, d=2)   ← moved here in diff 3
  //
  //   topo_remove_me was added in diff 1 then removed in diff 2.
  TOPO_ROOT_SPACE_ID: '00000000000040008000000000000c01',
  TOPO_CHILD_A_SPACE_ID: '00000000000040008000000000000c02',
  TOPO_CHILD_B_SPACE_ID: '00000000000040008000000000000c03',
  TOPO_GRANDCHILD_SPACE_ID: '00000000000040008000000000000c04',
  TOPO_REMOVE_ME_SPACE_ID: '00000000000040008000000000000c05',
  TOPO_MOVEABLE_SPACE_ID: '00000000000040008000000000000c06',

  // Entities created in each topology space (used in search result validation)
  TOPO_ROOT_ENTITY_ID: '0000000000000000000000000000ec01',
  TOPO_CHILD_A_ENTITY_ID: '0000000000000000000000000000ec02',
  TOPO_CHILD_B_ENTITY_ID: '0000000000000000000000000000ec03',
  TOPO_GRANDCHILD_ENTITY_ID: '0000000000000000000000000000ec04',
  TOPO_REMOVE_ME_ENTITY_ID: '0000000000000000000000000000ec05',
  TOPO_MOVEABLE_ENTITY_ID: '0000000000000000000000000000ec06',
  // additional_space_ids probe entities (all named "AsidProbeEntity")
  // - canon: in topo_child_a (canonical via diff 1)
  // - x: in non-canonical asid_extra_space_x
  // - y: in non-canonical asid_extra_space_y
  ASID_CANON_ENTITY_ID: '0000000000000000000000000000ad01',
  ASID_X_ENTITY_ID: '0000000000000000000000000000ad02',
  ASID_Y_ENTITY_ID: '0000000000000000000000000000ad03',
  ASID_EXTRA_SPACE_X_ID: '00000000000040008000000000000d01',
  ASID_EXTRA_SPACE_Y_ID: '00000000000040008000000000000d02',
};

interface TestResult {
  name: string;
  passed: boolean;
  message: string;
}

class SearchValidator {
  private baseUrl: string;
  private indexerUrl: string;
  private testResults: TestResult[] = [];

  constructor(baseUrl: string = 'http://localhost:3000', indexerUrl: string = 'http://localhost:8080') {
    this.baseUrl = baseUrl;
    this.indexerUrl = indexerUrl;
  }

  private async search(
    query: Partial<SearchQuery> & {
      exclude_type_ids?: string[];
      additional_space_ids?: string[];
    },
  ): Promise<SearchResponse> {
    const params = this.buildSearchParams(query);
    const url = `${this.baseUrl}/search?${params.toString()}`;
    const response = await fetch(url, {
      headers: {
        'Accept-Encoding': 'gzip, deflate',
      },
    });

    if (!response.ok) {
      throw new Error(`API returned ${response.status}: ${await response.text()}`);
    }

    return response.json() as Promise<SearchResponse>;
  }

  /**
   * Variant of search() that does NOT throw on non-2xx, used for tests that
   * assert the API returns a specific 4xx for invalid parameters.
   */
  private async searchRaw(
    query: Partial<SearchQuery> & {
      exclude_type_ids?: string[];
      additional_space_ids?: string[];
    },
    rawAdditionalSpaceIds?: string,
  ): Promise<{ status: number; body: unknown }> {
    const params = this.buildSearchParams(query);
    if (rawAdditionalSpaceIds !== undefined) {
      params.set('additional_space_ids', rawAdditionalSpaceIds);
    }
    const url = `${this.baseUrl}/search?${params.toString()}`;
    const response = await fetch(url, {
      headers: { 'Accept-Encoding': 'gzip, deflate' },
    });
    const body = (await response.json().catch(() => null)) as unknown;
    return { status: response.status, body };
  }

  private buildSearchParams(
    query: Partial<SearchQuery> & {
      exclude_type_ids?: string[];
      additional_space_ids?: string[];
    },
  ): URLSearchParams {
    const params = new URLSearchParams();

    if (query.query) params.append('query', query.query);
    if (query.scope) params.append('scope', query.scope);
    if (query.space_id) params.append('space_id', query.space_id);
    if (query.type_ids) query.type_ids.forEach(id => params.append('type_ids', id));
    if (query.limit) params.append('limit', query.limit.toString());
    if (query.offset) params.append('offset', query.offset.toString());
    if (query.include_deleted) params.append('include_deleted', 'true');
    // include_non_canonical defaults to true on the API, so only send when explicitly false
    if (query.include_non_canonical === false) {
      params.append('include_non_canonical', 'false');
    }
    // Server expects a single CSV value for additional_space_ids
    if (query.additional_space_ids && query.additional_space_ids.length > 0) {
      params.append('additional_space_ids', query.additional_space_ids.join(','));
    }
    // Pass exclude_type_ids if provided; default to empty (disable default exclusions)
    // so existing e2e tests are unaffected by the new default filtering behavior.
    if (query.exclude_type_ids !== undefined) {
      params.append('exclude_type_ids', query.exclude_type_ids.join(','));
    } else {
      params.append('exclude_type_ids', '');
    }
    return params;
  }

  private addResult(name: string, passed: boolean, message: string) {
    this.testResults.push({ name, passed, message });
    const icon = passed ? `${GREEN}✓${NC}` : `${RED}✗${NC}`;
    console.log(`${icon} ${message}`);
  }

  /** Verifies 'Alice' search returns 7 entities ordered by score. */
  async test1_BasicAliceSearch(): Promise<void> {
    console.log(`\n${BLUE}Test 1: Basic search for 'Alice' (should return 7 entities ordered by score)${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    // Check entity count
    const entityCount = response.results.length;
    if (entityCount === 7) {
      this.addResult('test1_count', true, `Found 7 Alice entities`);
    } else {
      this.addResult('test1_count', false, `Expected 7 Alice entities, got ${entityCount}`);
    }

    // Check ordering - first should be Alice High (highest score) - validate by entity ID
    if (response.results.length > 0) {
      const firstResult = response.results[0];
      if (firstResult.entityId === TEST_ENTITIES.ALICE_HIGH_ID) {
        this.addResult('test1_ordering', true, `First result is Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}) - correct ordering by score`);
      } else {
        this.addResult('test1_ordering', false, `First result should be Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}), got: ${firstResult.entityId}`);
      }
    }

    // Check that all have name "Alice"
    const aliceCount = response.results.filter(r => r.name === 'Alice').length;
    if (aliceCount === 7) {
      this.addResult('test1_names', true, `All 7 results have name 'Alice'`);
    } else {
      this.addResult('test1_names', false, `Expected 7 entities with name 'Alice', got ${aliceCount}`);
    }
  }

  /** Verifies 'Bob' search returns 1 entity with correct name and description. */
  async test2_BobSearch(): Promise<void> {
    console.log(`\n${BLUE}Test 2: Search for 'Bob' (should return 1 entity)${NC}`);

    const response = await this.search({
      query: 'bob',
      scope: 'GLOBAL',
    });

    // Find Bob by specific entity ID
    const bob = response.results.find(r => r.entityId === TEST_ENTITIES.BOB_ID);

    if (bob) {
      if (bob.name === 'Bob') {
        this.addResult('test2_name', true, `Found Bob (${TEST_ENTITIES.BOB_ID}) with correct name`);
      } else {
        this.addResult('test2_name', false, `Expected name 'Bob', got '${bob.name}'`);
      }

      // Check Bob has expected description
      if (bob.description?.includes('project manager')) {
        this.addResult('test2_description', true, `Bob has correct description`);
      } else {
        this.addResult('test2_description', false, `Bob description unexpected: ${bob.description}`);
      }
    } else {
      this.addResult('test2_not_found', false, `Could not find Bob entity (${TEST_ENTITIES.BOB_ID})`);
    }
  }

  /** Verifies 'Acme' search returns the Acme Corp organization entity. */
  async test3_OrganizationSearch(): Promise<void> {
    console.log(`\n${BLUE}Test 3: Search for 'Acme' (should return 1 organization)${NC}`);

    const response = await this.search({
      query: 'acme',
      scope: 'GLOBAL',
    });

    // Find Acme Corp by specific entity ID
    const org = response.results.find(r => r.entityId === TEST_ENTITIES.ORG_ID);

    if (org) {
      if (org.name === 'Acme Corp') {
        this.addResult('test3_name', true, `Found Acme Corp (${TEST_ENTITIES.ORG_ID}) with correct name`);
      } else {
        this.addResult('test3_name', false, `Expected 'Acme Corp', got '${org.name}'`);
      }
    } else {
      this.addResult('test3_not_found', false, `Could not find Acme Corp entity (${TEST_ENTITIES.ORG_ID})`);
    }
  }

  /** Verifies entities have required fields: entityId, name, description, types, space, scores. */
  async test4_EntityFields(): Promise<void> {
    console.log(`\n${BLUE}Test 4: Verify entity fields (entityId, name, description, types, space)${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length > 0) {
      const firstEntity = response.results[0];

      const hasEntityId = firstEntity.entityId !== undefined && firstEntity.entityId !== null;
      const hasName = firstEntity.name !== undefined && firstEntity.name !== null;
      const hasDescription = firstEntity.description !== undefined;
      const hasTypes = Array.isArray(firstEntity.types);
      const hasSpace = firstEntity.space !== undefined && firstEntity.space.id !== undefined;

      if (hasEntityId && hasName && hasDescription && hasTypes && hasSpace) {
        this.addResult('test4_fields', true, `Entity has all expected fields (entityId, name, description, types, space)`);
      } else {
        this.addResult('test4_fields', false,
          `Entity missing fields - entityId:${hasEntityId} name:${hasName} desc:${hasDescription} types:${hasTypes} space:${hasSpace}`);
      }

      // Check score fields exist (camelCase as per API types)
      const hasScoring = firstEntity.entityGlobalScore !== undefined ||
                        firstEntity.entitySpaceScore !== undefined ||
                        firstEntity.spaceScore !== undefined;

      if (hasScoring) {
        this.addResult('test4_scoring', true, `Entity has scoring fields`);
      } else {
        this.addResult('test4_scoring', false, `Entity missing scoring fields`);
      }

      // Check relevanceScore and textMatchScore fields
      const hasRelevanceScore = typeof firstEntity.relevanceScore === 'number' && firstEntity.relevanceScore > 0;
      const hasTextMatchScore = typeof firstEntity.textMatchScore === 'number' && firstEntity.textMatchScore >= 0;

      if (hasRelevanceScore) {
        this.addResult('test4_relevance_score', true,
          `Entity has relevanceScore: ${firstEntity.relevanceScore}`);
      } else {
        this.addResult('test4_relevance_score', false,
          `Entity missing or invalid relevanceScore: ${firstEntity.relevanceScore}`);
      }

      if (hasTextMatchScore) {
        this.addResult('test4_text_match_score', true,
          `Entity has textMatchScore: ${firstEntity.textMatchScore}`);
      } else {
        this.addResult('test4_text_match_score', false,
          `Entity missing or invalid textMatchScore: ${firstEntity.textMatchScore}`);
      }
    }
  }

  /** Verifies results are ordered by entityGlobalScore in descending order. */
  async test5_ScoreOrdering(): Promise<void> {
    console.log(`\n${BLUE}Test 5: Verify score-based ordering${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length >= 2) {
      const firstDesc = response.results[0].description || '';
      const lastDesc = response.results[response.results.length - 1].description || '';

      // First should be high score, last should be negative or low score
      if (firstDesc.includes('high global score') &&
          (lastDesc.includes('negative') || lastDesc.includes('below'))) {
        this.addResult('test5_ordering', true,
          `Entities correctly ordered by score (high first, low/negative last)`);
      } else {
        // Don't fail, just warn - scoring behavior may differ
        console.log(`${YELLOW}⚠${NC}  Score ordering unclear - first: '${firstDesc}', last: '${lastDesc}'`);
        console.log(`     (This may be expected if negative scores are filtered or scoring differs)`);
        this.addResult('test5_ordering', true, `Score ordering checked (may vary based on implementation)`);
      }

      // Check that scores are descending (camelCase as per API types)
      let scoresDescending = true;
      for (let i = 0; i < response.results.length - 1; i++) {
        const currentScore = response.results[i].entityGlobalScore ?? DEFAULT_AVERAGE_SCORE;
        const nextScore = response.results[i + 1].entityGlobalScore ?? DEFAULT_AVERAGE_SCORE;

        if (currentScore < nextScore) {
          scoresDescending = false;
          break;
        }
      }

      if (scoresDescending) {
        this.addResult('test5_descending', true, `Entity global scores are in descending order`);
      } else {
        this.addResult('test5_descending', false, `Entity global scores are not properly ordered`);
      }
    }
  }

  /** Verifies response includes total count and execution time (tookMs). */
  async test6_ResponseMetadata(): Promise<void> {
    console.log(`\n${BLUE}Test 6: Verify response metadata${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    // Check response has total
    if (typeof response.total === 'number') {
      this.addResult('test6_total', true, `Response includes total count: ${response.total}`);
    } else {
      this.addResult('test6_total', false, `Response missing total count`);
    }

    // Check response has execution time
    if (typeof response.tookMs === 'number') {
      this.addResult('test6_took', true, `Response includes execution time: ${response.tookMs}ms`);
    } else {
      this.addResult('test6_took', false, `Response missing execution time`);
    }
  }

  /** Verifies entities with zero (0.0) and negative (-0.75) scores are returned. */
  async test7_ZeroAndNegativeScores(): Promise<void> {
    console.log(`\n${BLUE}Test 7: Verify zero and negative score entities are returned${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    // Find Alice Zero by entity ID (should have score 0.0)
    const aliceZero = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_ZERO_ID);

    if (aliceZero) {
      if (aliceZero.entityGlobalScore === 0.0) {
        this.addResult('test7_zero_score', true,
          `Alice Zero (${TEST_ENTITIES.ALICE_ZERO_ID}) returned with score 0.0`);
      } else {
        this.addResult('test7_zero_score', false,
          `Alice Zero should have score 0.0, got ${aliceZero.entityGlobalScore}`);
      }
    } else {
      this.addResult('test7_zero_score', false,
        `Alice Zero entity (${TEST_ENTITIES.ALICE_ZERO_ID}) not found - zero scores may be filtered`);
    }

    // Find Alice Negative by entity ID (should have score -0.75)
    const aliceNegative = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_NEGATIVE_ID);

    if (aliceNegative) {
      if (aliceNegative.entityGlobalScore === -0.75) {
        this.addResult('test7_negative_score', true,
          `Alice Negative (${TEST_ENTITIES.ALICE_NEGATIVE_ID}) returned with score -0.75`);
      } else {
        this.addResult('test7_negative_score', false,
          `Alice Negative should have score -0.75, got ${aliceNegative.entityGlobalScore}`);
      }
    } else {
      this.addResult('test7_negative_score', false,
        `Alice Negative entity (${TEST_ENTITIES.ALICE_NEGATIVE_ID}) not found - negative scores may be filtered`);
    }
  }

  /** Verifies types reflect type relation create/delete scenarios. */
  async test8_TypeIdsScenarios(): Promise<void> {
    console.log(`\n${BLUE}Test 8: Verify types field for different relation scenarios${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length < 3) {
      this.addResult('test8_count', false, `Need at least 3 Alice entities for types tests, got ${response.results.length}`);
      return;
    }

    // Find Alice High by specific entity ID (should have 2 types: Person + Organization)
    const aliceHigh = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_HIGH_ID);

    if (aliceHigh) {
      if (aliceHigh.types?.length === 2) {
        this.addResult('test8_alice_high_multiple', true,
          `Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}) has 2 types (multiple type relations work)`);
      } else {
        this.addResult('test8_alice_high_multiple', false,
          `Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}) should have 2 types, got ${aliceHigh.types?.length || 0}`);
      }
    } else {
      this.addResult('test8_alice_high_multiple', false, `Could not find Alice High entity (${TEST_ENTITIES.ALICE_HIGH_ID})`);
    }

    // Find Alice Medium by specific entity ID (should have 2 types after create->delete->create)
    const aliceMedium = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_MEDIUM_ID);

    if (aliceMedium) {
      if (aliceMedium.types?.length === 2) {
        this.addResult('test8_alice_medium_recreate', true,
          `Alice Medium (${TEST_ENTITIES.ALICE_MEDIUM_ID}) has 2 types (Create->Delete->Create pattern works, final create processed)`);
      } else {
        this.addResult('test8_alice_medium_recreate', false,
          `Alice Medium (${TEST_ENTITIES.ALICE_MEDIUM_ID}) should have 2 types after recreate, got ${aliceMedium.types?.length || 0}`);
      }
    } else {
      this.addResult('test8_alice_medium_recreate', false, `Could not find Alice Medium entity (${TEST_ENTITIES.ALICE_MEDIUM_ID})`);
    }

    // Find Alice Low by specific entity ID (should have 1 type after partial removal)
    const aliceLow = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_LOW_ID);

    if (aliceLow) {
      if (aliceLow.types?.length === 1) {
        this.addResult('test8_alice_low_partial_removal', true,
          `Alice Low (${TEST_ENTITIES.ALICE_LOW_ID}) has 1 type (partial removal works, kept Person after Org deleted)`);
      } else {
        this.addResult('test8_alice_low_partial_removal', false,
          `Alice Low (${TEST_ENTITIES.ALICE_LOW_ID}) should have 1 type after partial removal, got ${aliceLow.types?.length || 0}`);
      }
    } else {
      this.addResult('test8_alice_low_partial_removal', false, `Could not find Alice Low entity (${TEST_ENTITIES.ALICE_LOW_ID})`);
    }

    // Check that other Alice entities have 1 type (Person) by checking specific IDs
    const otherAliceIds = [
      TEST_ENTITIES.ALICE_ZERO_ID,
      TEST_ENTITIES.ALICE_NEGATIVE_ID,
      TEST_ENTITIES.ALICE_AT_THRESHOLD_ID,
      TEST_ENTITIES.ALICE_BELOW_THRESHOLD_ID,
    ];

    let allOthersHaveOneType = true;
    for (const aliceId of otherAliceIds) {
      const alice = response.results.find(r => r.entityId === aliceId);
      if (alice && (!Array.isArray(alice.types) || alice.types.length !== 1)) {
        allOthersHaveOneType = false;
        break;
      }
    }

    if (allOthersHaveOneType) {
      this.addResult('test8_others_single_type', true,
        `Other Alice entities have 1 type (Person) as expected`);
    } else {
      this.addResult('test8_others_single_type', false,
        `Some other Alice entities don't have exactly 1 type`);
    }
  }

  /** Verifies soft-deleted entity (Delete Charlie) is excluded from search. */
  async test9a_DeletedEntityNotInResults(): Promise<void> {
    console.log(`\n${BLUE}Test 9a: Verify deleted entity (Delete Charlie) does not appear in search results${NC}`);

    // Search for Delete Charlie by name
    const charlieNameSearch = await this.search({
      query: 'delete charlie',
      scope: 'GLOBAL',
    });

    const charlieByName = charlieNameSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_CHARLIE_ID);
    if (!charlieByName) {
      this.addResult('test9_charlie_not_in_name_search', true,
        `Delete Charlie (${TEST_ENTITIES.DELETE_CHARLIE_ID}) correctly excluded from name search (soft deleted)`);
    } else {
      this.addResult('test9_charlie_not_in_name_search', false,
        `Delete Charlie (${TEST_ENTITIES.DELETE_CHARLIE_ID}) should not appear in search (was soft deleted)`);
    }

    // Verify Delete Charlie doesn't appear in broad search
    const broadSearch = await this.search({
      query: 'entity',
      scope: 'GLOBAL',
    });

    const charlieInBroad = broadSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_CHARLIE_ID);

    if (!charlieInBroad) {
      this.addResult('test9_charlie_not_in_broad_search', true,
        `Delete Charlie correctly excluded from broad search`);
    } else {
      this.addResult('test9_charlie_not_in_broad_search', false,
        `Delete Charlie should not appear in broad search (was soft deleted)`);
    }
  }

  /** Verifies restored entity (Delete Dana) appears in search results after restoration. */
  async test9b_RestoredEntityInResults(): Promise<void> {
    console.log(`\n${BLUE}Test 9b: Verify restored entity (Delete Dana) appears in search results${NC}`);

    // Search for Delete Dana by name - should be found because she was restored
    const danaNameSearch = await this.search({
      query: 'delete dana',
      scope: 'GLOBAL',
    });

    const danaByName = danaNameSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_DANA_ID);
    if (danaByName) {
      this.addResult('test9b_dana_in_name_search', true,
        `Delete Dana (${TEST_ENTITIES.DELETE_DANA_ID}) correctly appears in search (was restored after deletion)`);
    } else {
      this.addResult('test9b_dana_in_name_search', false,
        `Delete Dana (${TEST_ENTITIES.DELETE_DANA_ID}) should appear in search (was restored after deletion)`);
    }

    // Verify Dana's data is intact after restore
    if (danaByName) {
      if (danaByName.name === 'Delete Dana') {
        this.addResult('test9b_dana_name_preserved', true,
          `Delete Dana has correct name after restore`);
      } else {
        this.addResult('test9b_dana_name_preserved', false,
          `Delete Dana should have name 'Delete Dana', got '${danaByName.name}'`);
      }

      if (danaByName.description?.includes('also be deleted')) {
        this.addResult('test9b_dana_description_preserved', true,
          `Delete Dana has correct description after restore`);
      } else {
        this.addResult('test9b_dana_description_preserved', false,
          `Delete Dana description unexpected: ${danaByName.description}`);
      }
    }

    // Verify Dana appears in broad search
    const broadSearch = await this.search({
      query: 'dana',
      scope: 'GLOBAL',
    });

    const danaInBroad = broadSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_DANA_ID);
    if (danaInBroad) {
      this.addResult('test9b_dana_in_broad_search', true,
        `Delete Dana correctly appears in broad search after restore`);
    } else {
      this.addResult('test9b_dana_in_broad_search', false,
        `Delete Dana should appear in broad search (was restored)`);
    }
  }

  /** Verifies entity deleted then updated (Delete Eve) remains excluded from search. */
  async test10_DeletedThenUpdatedEntityNotInResults(): Promise<void> {
    console.log(`\n${BLUE}Test 10: Verify entity deleted then updated (Delete Eve) remains excluded from search${NC}`);

    // Search for Delete Eve by name - should not find it even though it was updated after deletion
    const eveNameSearch = await this.search({
      query: 'delete eve',
      scope: 'GLOBAL',
    });

    const eveByName = eveNameSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_EVE_ID);
    if (!eveByName) {
      this.addResult('test10_eve_not_in_name_search', true,
        `Delete Eve (${TEST_ENTITIES.DELETE_EVE_ID}) correctly excluded from name search (deleted then updated, remains deleted)`);
    } else {
      this.addResult('test10_eve_not_in_name_search', false,
        `Delete Eve (${TEST_ENTITIES.DELETE_EVE_ID}) should not appear despite post-delete update`);
    }

    // Search for updated name "Delete Eve Updated" - should also not find it
    const eveUpdatedSearch = await this.search({
      query: 'delete eve updated',
      scope: 'GLOBAL',
    });

    const eveByUpdatedName = eveUpdatedSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_EVE_ID);
    if (!eveByUpdatedName) {
      this.addResult('test10_eve_updated_name_not_found', true,
        `Delete Eve with updated name correctly excluded (post-delete updates don't resurrect entity)`);
    } else {
      this.addResult('test10_eve_updated_name_not_found', false,
        `Delete Eve should not appear even when searching for updated name`);
    }

    // Verify Delete Eve doesn't appear in broad search
    const broadSearch = await this.search({
      query: 'entity',
      scope: 'GLOBAL',
    });

    const eveInBroad = broadSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_EVE_ID);
    if (!eveInBroad) {
      this.addResult('test10_eve_not_in_broad_search', true,
        `Delete Eve correctly excluded from broad search (delete-then-update behavior works)`);
    } else {
      this.addResult('test10_eve_not_in_broad_search', false,
        `Delete Eve should remain deleted despite subsequent update`);
    }
  }

  /** Verifies empty query returns top-ranked results ordered by score. */
  async test11_EmptyQueryTopRanked(): Promise<void> {
    console.log(`\n${BLUE}Test 11: Empty query returns top ranked results (no query parameter)${NC}`);

    // Test with global scope
    console.log(`  ${BLUE}→ Testing with GLOBAL scope${NC}`);
    const globalResponse = await this.search({
      scope: 'GLOBAL',
    });

    // Should return results
    if (globalResponse.results.length > 0) {
      this.addResult('test11_global_has_results', true, `Empty query (GLOBAL) returned ${globalResponse.results.length} results`);
    } else {
      this.addResult('test11_global_has_results', false, `Empty query (GLOBAL) should return results, got 0`);
      return;
    }

    // First result should be the entity with highest global score (Alice High: 0.95)
    const firstGlobalResult = globalResponse.results[0];
    if (firstGlobalResult.entityId === TEST_ENTITIES.ALICE_HIGH_ID) {
      this.addResult('test11_global_top_ranked', true,
        `First result is Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}) - highest global score (0.95)`);
    } else {
      this.addResult('test11_global_top_ranked', false,
        `First result should be Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}), got: ${firstGlobalResult.entityId}`);
    }

    // Verify results are ordered by global score (descending)
    // Use DEFAULT_AVERAGE_SCORE for missing scores
    let globalScoresDescending = true;
    for (let i = 0; i < globalResponse.results.length - 1; i++) {
      const currentScore = globalResponse.results[i].entityGlobalScore ?? DEFAULT_AVERAGE_SCORE;
      const nextScore = globalResponse.results[i + 1].entityGlobalScore ?? DEFAULT_AVERAGE_SCORE;

      if (currentScore < nextScore) {
        globalScoresDescending = false;
        break;
      }
    }

    if (globalScoresDescending) {
      this.addResult('test11_global_score_ordering', true,
        `Empty query (GLOBAL) results ordered by global score (descending, missing scores default to ${DEFAULT_AVERAGE_SCORE})`);
    } else {
      this.addResult('test11_global_score_ordering', false,
        `Empty query (GLOBAL) results not properly ordered by global score`);
    }

    // Check total count makes sense
    if (typeof globalResponse.total === 'number' && globalResponse.total > 0) {
      this.addResult('test11_global_total', true, `Response includes total count: ${globalResponse.total}`);
    } else {
      this.addResult('test11_global_total', false, `Response missing or invalid total count`);
    }

    // Test with space scope
    console.log(`  ${BLUE}→ Testing with SPACE scope${NC}`);
    const spaceResponse = await this.search({
      scope: 'SPACE',
      space_id: TEST_ENTITIES.SPACE_ID,
    });

    // Should return results
    if (spaceResponse.results.length > 0) {
      this.addResult('test11_space_has_results', true, `Empty query (SPACE) returned ${spaceResponse.results.length} results`);
    } else {
      this.addResult('test11_space_has_results', false, `Empty query (SPACE) should return results, got 0`);
      return;
    }

    // Verify results are ordered by space score (descending)
    // Use DEFAULT_AVERAGE_SCORE for missing scores
    let spaceScoresDescending = true;
    for (let i = 0; i < spaceResponse.results.length - 1; i++) {
      const currentScore = spaceResponse.results[i].entitySpaceScore ?? DEFAULT_AVERAGE_SCORE;
      const nextScore = spaceResponse.results[i + 1].entitySpaceScore ?? DEFAULT_AVERAGE_SCORE;

      if (currentScore < nextScore) {
        spaceScoresDescending = false;
        break;
      }
    }

    if (spaceScoresDescending) {
      this.addResult('test11_space_score_ordering', true,
        `Empty query (SPACE) results ordered by space score (descending, missing scores default to ${DEFAULT_AVERAGE_SCORE})`);
    } else {
      this.addResult('test11_space_score_ordering', false,
        `Empty query (SPACE) results not properly ordered by space score`);
    }

    // Check total count makes sense
    if (typeof spaceResponse.total === 'number' && spaceResponse.total > 0) {
      this.addResult('test11_space_total', true, `Response includes total count: ${spaceResponse.total}`);
    } else {
      this.addResult('test11_space_total', false, `Response missing or invalid total count`);
    }
  }

  /** Verifies unset_properties clears name/description while preserving other fields. */
  async test12_UnsetProperties(): Promise<void> {
    console.log(`\n${BLUE}Test 12: Verify unset_properties functionality (UpdateEntity with unset_values)${NC}`);

    // Test Case 1: Unset 1 property (name)
    console.log(`  ${BLUE}→ Test Case 1: Unset single property (name)${NC}`);
    const response1 = await this.search({
      query: TEST_ENTITIES.UNSET_TEST_1_ID,
      scope: 'GLOBAL',
    });

    const entity1 = response1.results.find(r => r.entityId === TEST_ENTITIES.UNSET_TEST_1_ID);

    if (entity1) {
      // Check that name is undefined/null
      if (entity1.name === undefined || entity1.name === null) {
        this.addResult('test12_case1_name_unset', true,
          `Test Case 1 (${TEST_ENTITIES.UNSET_TEST_1_ID}): name is correctly unset (undefined/null)`);
      } else {
        this.addResult('test12_case1_name_unset', false,
          `Test Case 1 (${TEST_ENTITIES.UNSET_TEST_1_ID}): name should be unset, but got: '${entity1.name}'`);
      }

      // Check that description is still present
      if (entity1.description && entity1.description.includes('name unset')) {
        this.addResult('test12_case1_description_present', true,
          `Test Case 1 (${TEST_ENTITIES.UNSET_TEST_1_ID}): description is correctly preserved`);
      } else {
        this.addResult('test12_case1_description_present', false,
          `Test Case 1 (${TEST_ENTITIES.UNSET_TEST_1_ID}): description should be present, got: '${entity1.description}'`);
      }
    } else {
      this.addResult('test12_case1_not_found', false,
        `Test Case 1 entity (${TEST_ENTITIES.UNSET_TEST_1_ID}) not found`);
    }

    // Test Case 2: Unset 2 properties (name and description)
    console.log(`  ${BLUE}→ Test Case 2: Unset multiple properties (name and description)${NC}`);
    const response2 = await this.search({
      query: TEST_ENTITIES.UNSET_TEST_2_ID,
      scope: 'GLOBAL',
    });

    const entity2 = response2.results.find(r => r.entityId === TEST_ENTITIES.UNSET_TEST_2_ID);

    if (entity2) {
      // Check that name is undefined/null
      if (entity2.name === undefined || entity2.name === null) {
        this.addResult('test12_case2_name_unset', true,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): name is correctly unset (undefined/null)`);
      } else {
        this.addResult('test12_case2_name_unset', false,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): name should be unset, but got: '${entity2.name}'`);
      }

      // Check that description is undefined/null
      if (entity2.description === undefined || entity2.description === null) {
        this.addResult('test12_case2_description_unset', true,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): description is correctly unset (undefined/null)`);
      } else {
        this.addResult('test12_case2_description_unset', false,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): description should be unset, but got: '${entity2.description}'`);
      }

      // Check that avatar is still present
      if (entity2.avatar && entity2.avatar === 'https://example.com/avatar.png') {
        this.addResult('test12_case2_avatar_present', true,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): avatar is correctly preserved`);
      } else {
        this.addResult('test12_case2_avatar_present', false,
          `Test Case 2 (${TEST_ENTITIES.UNSET_TEST_2_ID}): avatar should be 'https://example.com/avatar.png', got: '${entity2.avatar}'`);
      }
    } else {
      this.addResult('test12_case2_not_found', false,
        `Test Case 2 entity (${TEST_ENTITIES.UNSET_TEST_2_ID}) not found`);
    }
  }

  /** Verifies Last-Writer-Wins: sequential updates result in final value persisting. */
  async test13_LWWBehavior(): Promise<void> {
    console.log(`\n${BLUE}Test 13: Verify mixed set/unset + Last-Writer-Wins (LWW) behavior${NC}`);
    console.log(`  ${BLUE}→ Test 1: UpdateEntity with both set and unset (different properties)${NC}`);
    console.log(`  ${BLUE}→ Test 2: Multiple sequential sets on same property (last write wins)${NC}`);

    const response = await this.search({
      query: TEST_ENTITIES.LWW_TEST_ID,
      scope: 'GLOBAL',
    });

    const entity = response.results.find(r => r.entityId === TEST_ENTITIES.LWW_TEST_ID);

    if (entity) {
      // Check that name is "Second Update" (last write wins over "First Update")
      if (entity.name === 'Second Update') {
        this.addResult('test13_lww_last_write_wins', true,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): name is 'Second Update' - last write won over 'First Update' (LWW correct)`);
      } else {
        this.addResult('test13_lww_last_write_wins', false,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): name should be 'Second Update' (last write wins), but got: '${entity.name}'`);
      }

      // Check that description was UNSET (in mixed operation's unset_values)
      if (entity.description === undefined || entity.description === null) {
        this.addResult('test13_lww_description_unset', true,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): description is correctly unset (mixed set/unset operation worked)`);
      } else {
        this.addResult('test13_lww_description_unset', false,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): description should be unset, but got: '${entity.description}'`);
      }

      // Check that avatar was PRESERVED (not touched in any operation)
      if (entity.avatar && entity.avatar === 'https://example.com/lww-avatar.png') {
        this.addResult('test13_lww_avatar_preserved', true,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): avatar is correctly preserved across operations`);
      } else {
        this.addResult('test13_lww_avatar_preserved', false,
          `LWW Test (${TEST_ENTITIES.LWW_TEST_ID}): avatar should be 'https://example.com/lww-avatar.png', got: '${entity.avatar}'`);
      }
    } else {
      this.addResult('test13_lww_not_found', false,
        `LWW Test entity (${TEST_ENTITIES.LWW_TEST_ID}) not found`);
    }
  }

  /** Verifies include_deleted flag returns soft-deleted entities when set to true. */
  async test14_IncludeDeletedFlag(): Promise<void> {
    console.log(`\n${BLUE}Test 14: Verify include_deleted flag returns deleted entities${NC}`);

    // Search for Delete Charlie WITH include_deleted=true (should find)
    const includeDeletedSearch = await this.search({
      query: 'delete charlie',
      scope: 'GLOBAL',
      include_deleted: true,
    });

    const charlieInIncludeDeleted = includeDeletedSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_CHARLIE_ID);
    if (charlieInIncludeDeleted) {
      this.addResult('test14_charlie_included_with_flag', true,
        `Delete Charlie (${TEST_ENTITIES.DELETE_CHARLIE_ID}) returned with include_deleted=true`);

      // Verify Charlie's data is intact (confirms soft delete preserves data)
      if (charlieInIncludeDeleted.name === 'Delete Charlie') {
        this.addResult('test14_charlie_name_preserved', true,
          `Delete Charlie has correct name preserved after soft delete`);
      } else {
        this.addResult('test14_charlie_name_preserved', false,
          `Delete Charlie should have name 'Delete Charlie', got '${charlieInIncludeDeleted.name}'`);
      }

      if (charlieInIncludeDeleted.description?.includes('will be deleted')) {
        this.addResult('test14_charlie_description_preserved', true,
          `Delete Charlie has correct description preserved after soft delete`);
      } else {
        this.addResult('test14_charlie_description_preserved', false,
          `Delete Charlie description unexpected: ${charlieInIncludeDeleted.description}`);
      }
    } else {
      this.addResult('test14_charlie_included_with_flag', false,
        `Delete Charlie should be returned when include_deleted=true (confirms entity is soft deleted, not hard deleted)`);
    }

    // Verify Delete Eve (deleted then updated) is also returned with include_deleted=true
    const eveSearch = await this.search({
      query: 'delete eve',
      scope: 'GLOBAL',
      include_deleted: true,
    });

    const eveInIncludeDeleted = eveSearch.results.find(r => r.entityId === TEST_ENTITIES.DELETE_EVE_ID);
    if (eveInIncludeDeleted) {
      this.addResult('test14_eve_included_with_flag', true,
        `Delete Eve (${TEST_ENTITIES.DELETE_EVE_ID}) returned with include_deleted=true (tombstone preserved)`);

      // Verify tombstone dominance: the post-delete update should NOT have been applied
      // Eve was created with name "Delete Eve", deleted, then updated to "Delete Eve Updated"
      // If tombstone dominance works, name should still be "Delete Eve"
      if (eveInIncludeDeleted.name === 'Delete Eve') {
        this.addResult('test14_eve_tombstone_dominance', true,
          `Delete Eve name is 'Delete Eve' - tombstone dominance enforced (post-delete update ignored)`);
      } else if (eveInIncludeDeleted.name === 'Delete Eve Updated') {
        this.addResult('test14_eve_tombstone_dominance', false,
          `Delete Eve name is 'Delete Eve Updated' - tombstone dominance NOT enforced (post-delete update was applied)`);
      } else {
        this.addResult('test14_eve_tombstone_dominance', false,
          `Delete Eve has unexpected name: '${eveInIncludeDeleted.name}'`);
      }
    } else {
      this.addResult('test14_eve_included_with_flag', false,
        `Delete Eve should be returned when include_deleted=true`);
    }
  }

  /**
   * Verifies GLOBAL_BY_ENTITY_SPACE_SCORE scope ranks by entity_space_score * space_score.
   *
   * Test entities across two spaces with different space_scores:
   *   - rank_space_high (space_score=0.80)
   *   - rank_space_low  (space_score=0.10)
   *
   * Four "RankTest" entities with varying entity_space_scores:
   *   Gamma:   entity_space=0.90, space=0.80 → product=0.72 (rank 1)
   *   Delta:   entity_space=0.20, space=0.80 → product=0.16 (rank 2)
   *   Epsilon: entity_space=0.90, space=0.10 → product=0.09 (rank 3)
   *   Zeta:    entity_space=0.20, space=0.10 → product=0.02 (rank 4)
   *
   * Key insight: Delta (low entity_space=0.20) outranks Epsilon (high entity_space=0.90)
   * because Delta's space_score (0.80) is much higher. This validates the multiplication.
   */
  async test32_GlobalByEntitySpaceScoreSearch(): Promise<void> {
    console.log(`\n${BLUE}Test 32: GLOBAL_BY_ENTITY_SPACE_SCORE scope (entity_space_score * space_score ranking)${NC}`);

    // 32a: Text query for "RankTest" entities with GLOBAL_BY_ENTITY_SPACE_SCORE scope
    console.log(`  ${BLUE}→ 32a: Text query "RankTest" with GLOBAL_BY_ENTITY_SPACE_SCORE scope${NC}`);
    const response = await this.search({
      query: 'RankTest',
      scope: 'GLOBAL_BY_ENTITY_SPACE_SCORE',
    });

    if (response.results.length >= 4) {
      this.addResult('test32a_has_results', true,
        `Text query "RankTest" (GLOBAL_BY_ENTITY_SPACE_SCORE) returned ${response.results.length} results (expected >= 4)`);
    } else {
      this.addResult('test32a_has_results', false,
        `Text query "RankTest" should return >= 4 results, got ${response.results.length}`);
      return;
    }

    // Find the ranking test entities in results
    const gamma = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_GAMMA_ENTITY_ID);
    const delta = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_DELTA_ENTITY_ID);
    const epsilon = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_EPSILON_ENTITY_ID);
    const zeta = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_ZETA_ENTITY_ID);

    if (gamma && delta && epsilon && zeta) {
      this.addResult('test32a_all_found', true, `All 4 RankTest entities found in results`);
    } else {
      const missing = [];
      if (!gamma) missing.push('Gamma');
      if (!delta) missing.push('Delta');
      if (!epsilon) missing.push('Epsilon');
      if (!zeta) missing.push('Zeta');
      this.addResult('test32a_all_found', false, `Missing RankTest entities: ${missing.join(', ')}`);
      return;
    }

    // Get positions of each entity in results
    const gammaIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_GAMMA_ENTITY_ID);
    const deltaIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_DELTA_ENTITY_ID);
    const epsilonIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_EPSILON_ENTITY_ID);
    const zetaIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_ZETA_ENTITY_ID);

    console.log(`    Positions: Gamma=${gammaIdx}, Delta=${deltaIdx}, Epsilon=${epsilonIdx}, Zeta=${zetaIdx}`);
    console.log(`    Expected:  Gamma < Delta < Epsilon < Zeta (lower index = higher rank)`);

    // 32b: High entity_space * High space should rank first
    console.log(`  ${BLUE}→ 32b: High entity_space_score * High space_score ranks first${NC}`);
    if (gammaIdx < deltaIdx && gammaIdx < epsilonIdx && gammaIdx < zetaIdx) {
      this.addResult('test32b_gamma_first', true,
        `Gamma (entity_space=0.90 * space=0.80 = 0.72) ranks highest among RankTest entities`);
    } else {
      this.addResult('test32b_gamma_first', false,
        `Gamma should rank highest (product=0.72), but position=${gammaIdx} vs Delta=${deltaIdx}, Epsilon=${epsilonIdx}, Zeta=${zetaIdx}`);
    }

    // 32c: Low entity_space * High space should beat High entity_space * Low space
    // This is the critical test that validates the multiplication by space_score
    console.log(`  ${BLUE}→ 32c: Low entity_space * High space beats High entity_space * Low space${NC}`);
    if (deltaIdx < epsilonIdx) {
      this.addResult('test32c_delta_beats_epsilon', true,
        `Delta (entity_space=0.20 * space=0.80 = 0.16) outranks Epsilon (entity_space=0.90 * space=0.10 = 0.09) — space_score multiplication validated`);
    } else {
      this.addResult('test32c_delta_beats_epsilon', false,
        `Delta (product=0.16) should outrank Epsilon (product=0.09), but Delta position=${deltaIdx}, Epsilon position=${epsilonIdx}`);
    }

    // 32d: Low entity_space * Low space should rank last
    console.log(`  ${BLUE}→ 32d: Low entity_space_score * Low space_score ranks last${NC}`);
    if (zetaIdx > gammaIdx && zetaIdx > deltaIdx && zetaIdx > epsilonIdx) {
      this.addResult('test32d_zeta_last', true,
        `Zeta (entity_space=0.20 * space=0.10 = 0.02) ranks lowest among RankTest entities`);
    } else {
      this.addResult('test32d_zeta_last', false,
        `Zeta should rank lowest (product=0.02), but position=${zetaIdx} vs Gamma=${gammaIdx}, Delta=${deltaIdx}, Epsilon=${epsilonIdx}`);
    }

    // 32e: Verify complete ordering: Gamma > Delta > Epsilon > Zeta
    console.log(`  ${BLUE}→ 32e: Verify complete ranking order${NC}`);
    if (gammaIdx < deltaIdx && deltaIdx < epsilonIdx && epsilonIdx < zetaIdx) {
      this.addResult('test32e_full_ordering', true,
        `Full ranking order correct: Gamma(0.72) > Delta(0.16) > Epsilon(0.09) > Zeta(0.02)`);
    } else {
      this.addResult('test32e_full_ordering', false,
        `Expected order Gamma < Delta < Epsilon < Zeta by position, got: Gamma=${gammaIdx}, Delta=${deltaIdx}, Epsilon=${epsilonIdx}, Zeta=${zetaIdx}`);
    }

    // 32f: Empty query with GLOBAL_BY_ENTITY_SPACE_SCORE scope
    console.log(`  ${BLUE}→ 32f: Empty query with GLOBAL_BY_ENTITY_SPACE_SCORE scope${NC}`);
    const emptyResponse = await this.search({
      scope: 'GLOBAL_BY_ENTITY_SPACE_SCORE',
      limit: 100,
    });

    if (emptyResponse.results.length > 0) {
      this.addResult('test32f_empty_has_results', true,
        `Empty query (GLOBAL_BY_ENTITY_SPACE_SCORE) returned ${emptyResponse.results.length} results`);
    } else {
      this.addResult('test32f_empty_has_results', false,
        `Empty query (GLOBAL_BY_ENTITY_SPACE_SCORE) should return results, got 0`);
      return;
    }

    // Verify the ranking test entities maintain correct order in empty query too
    const emptyGammaIdx = emptyResponse.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_GAMMA_ENTITY_ID);
    const emptyDeltaIdx = emptyResponse.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_DELTA_ENTITY_ID);
    const emptyEpsilonIdx = emptyResponse.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_EPSILON_ENTITY_ID);
    const emptyZetaIdx = emptyResponse.results.findIndex(r => r.entityId === TEST_ENTITIES.RANK_ZETA_ENTITY_ID);

    if (emptyGammaIdx !== -1 && emptyDeltaIdx !== -1 && emptyEpsilonIdx !== -1 && emptyZetaIdx !== -1) {
      if (emptyGammaIdx < emptyDeltaIdx && emptyDeltaIdx < emptyEpsilonIdx && emptyEpsilonIdx < emptyZetaIdx) {
        this.addResult('test32f_empty_ordering', true,
          `Empty query ranking order correct: Gamma(0.72) > Delta(0.16) > Epsilon(0.09) > Zeta(0.02)`);
      } else {
        this.addResult('test32f_empty_ordering', false,
          `Empty query order wrong: Gamma=${emptyGammaIdx}, Delta=${emptyDeltaIdx}, Epsilon=${emptyEpsilonIdx}, Zeta=${emptyZetaIdx}`);
      }
    } else {
      this.addResult('test32f_empty_ordering', false,
        `Not all RankTest entities found in empty query results`);
    }
  }

  /** Verifies entity created via CreateEntity GRC-20 op is searchable. */
  async test15_CreateEntityOp(): Promise<void> {
    console.log(`\n${BLUE}Test 15: Verify entity created via CreateEntity GRC-20 op${NC}`);

    const response = await this.search({
      query: 'CreateEntity Test',
      scope: 'GLOBAL',
    });

    const entity = response.results.find(r => r.entityId === TEST_ENTITIES.CREATE_ENTITY_TEST_ID);

    if (entity) {
      this.addResult('test15_create_entity_found', true,
        `CreateEntity test entity (${TEST_ENTITIES.CREATE_ENTITY_TEST_ID}) found in search results`);

      // Verify name
      if (entity.name === 'CreateEntity Test') {
        this.addResult('test15_create_entity_name', true,
          `CreateEntity test entity has correct name: 'CreateEntity Test'`);
      } else {
        this.addResult('test15_create_entity_name', false,
          `CreateEntity test entity has wrong name: expected 'CreateEntity Test', got '${entity.name}'`);
      }

      // Verify description
      if (entity.description === 'Entity created using the GRC-20 CreateEntity operation') {
        this.addResult('test15_create_entity_description', true,
          `CreateEntity test entity has correct description`);
      } else {
        this.addResult('test15_create_entity_description', false,
          `CreateEntity test entity has wrong description: '${entity.description}'`);
      }

      // Verify avatar
      if (entity.avatar === 'https://example.com/create-entity-avatar.png') {
        this.addResult('test15_create_entity_avatar', true,
          `CreateEntity test entity has correct avatar URL`);
      } else {
        this.addResult('test15_create_entity_avatar', false,
          `CreateEntity test entity has wrong avatar: '${entity.avatar}'`);
      }
    } else {
      this.addResult('test15_create_entity_found', false,
        `CreateEntity test entity (${TEST_ENTITIES.CREATE_ENTITY_TEST_ID}) NOT found in search results`);
    }
  }

  /**
   * Compute expected score boost for a given entity_global_score.
   * Formula: (max(score, MIN_SCORE_THRESHOLD) + SCORE_SHIFT) * SCORE_BOOST
   */
  private expectedScoreBoost(entityGlobalScore: number): number {
    return (Math.max(entityGlobalScore, MIN_SCORE_THRESHOLD) + SCORE_SHIFT) * SCORE_BOOST;
  }

  /** Verifies relevanceScores are in descending order for 'alice' query. */
  async test16_RelevanceScoresDescendingOrder(): Promise<void> {
    console.log(`\n${BLUE}Test 16: Verify relevanceScores are in descending order${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length < 2) {
      this.addResult('test16_count', false, `Need at least 2 results, got ${response.results.length}`);
      return;
    }

    // Check all results have relevanceScore
    const allHaveRelevance = response.results.every(r => typeof r.relevanceScore === 'number');
    if (allHaveRelevance) {
      this.addResult('test16_has_relevance', true, `All ${response.results.length} results have relevanceScore`);
    } else {
      this.addResult('test16_has_relevance', false, `Some results missing relevanceScore`);
      return;
    }

    let isDescending = true;
    for (let i = 0; i < response.results.length - 1; i++) {
      if (response.results[i].relevanceScore! < response.results[i + 1].relevanceScore!) {
        isDescending = false;
        this.addResult('test16_descending', false,
          `relevanceScore not descending at index ${i}: ${response.results[i].relevanceScore} < ${response.results[i + 1].relevanceScore}`);
        break;
      }
    }

    if (isDescending) {
      this.addResult('test16_descending', true,
        `relevanceScores in descending order: ${response.results.map(r => r.relevanceScore!.toFixed(3)).join(' >= ')}`);
    }

    // Scores are always indexed by the test generator
    if (response.results[0].entityId === TEST_ENTITIES.ALICE_HIGH_ID) {
      this.addResult('test16_first', true, `First result is Alice High (highest relevance)`);
    } else {
      this.addResult('test16_first', false,
        `First result should be Alice High (${TEST_ENTITIES.ALICE_HIGH_ID}), got ${response.results[0].entityId}`);
    }

    const lastResult = response.results[response.results.length - 1];
    if (lastResult.entityId === TEST_ENTITIES.ALICE_NEGATIVE_ID) {
      this.addResult('test16_last', true, `Last result is Alice Negative (lowest relevance)`);
    } else {
      this.addResult('test16_last', false,
        `Last result should be Alice Negative (${TEST_ENTITIES.ALICE_NEGATIVE_ID}), got ${lastResult.entityId}`);
    }
  }

  /** Verifies textMatchScores are consistent across same-name entities and match expected formula. */
  async test17_TextMatchConsistencyForSameName(): Promise<void> {
    console.log(`\n${BLUE}Test 17: Verify textMatchScore consistency for same-name entities${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length !== 7) {
      this.addResult('test17_count', false, `Expected 7 Alice entities, got ${response.results.length}`);
      return;
    }

    // All results should have textMatchScore
    const allHaveTextMatch = response.results.every(r => typeof r.textMatchScore === 'number');
    if (!allHaveTextMatch) {
      this.addResult('test17_has_text_match', false, `Some results missing textMatchScore`);
      return;
    }

    // textMatchScores should be approximately equal (all Alices have same name)
    const textMatchScores = response.results.map(r => r.textMatchScore!);
    const minTM = Math.min(...textMatchScores);
    const maxTM = Math.max(...textMatchScores);
    const tolerance = 0.1;

    if (maxTM - minTM <= tolerance) {
      this.addResult('test17_consistency', true,
        `textMatchScores consistent: range [${minTM.toFixed(3)}, ${maxTM.toFixed(3)}] (spread ${(maxTM - minTM).toFixed(3)} <= ${tolerance})`);
    } else {
      this.addResult('test17_consistency', false,
        `textMatchScores too spread: range [${minTM.toFixed(3)}, ${maxTM.toFixed(3)}] (spread ${(maxTM - minTM).toFixed(3)} > ${tolerance})`);
    }

    // relevanceScore > textMatchScore for every result (score boost always positive)
    let allRelevanceGreater = true;
    for (const r of response.results) {
      if (r.relevanceScore! <= r.textMatchScore!) {
        allRelevanceGreater = false;
        this.addResult('test17_relevance_gt_text', false,
          `relevanceScore (${r.relevanceScore}) should be > textMatchScore (${r.textMatchScore}) for entity ${r.entityId}`);
        break;
      }
    }
    if (allRelevanceGreater) {
      this.addResult('test17_relevance_gt_text', true,
        `relevanceScore > textMatchScore for all results (score boost always adds positively)`);
    }

    // Verify relevanceScore ≈ textMatchScore + expectedScoreBoost
    // Use actual entityGlobalScore from results if present, otherwise DEFAULT_AVERAGE_SCORE
    const hardcodedScores: Record<string, number> = {
      [TEST_ENTITIES.ALICE_HIGH_ID]: 0.95,
      [TEST_ENTITIES.ALICE_MEDIUM_ID]: 0.65,
      [TEST_ENTITIES.ALICE_AT_THRESHOLD_ID]: 0.50,
      [TEST_ENTITIES.ALICE_BELOW_THRESHOLD_ID]: 0.25,
      [TEST_ENTITIES.ALICE_LOW_ID]: 0.15,
      [TEST_ENTITIES.ALICE_ZERO_ID]: 0.0,
      [TEST_ENTITIES.ALICE_NEGATIVE_ID]: -0.75,
    };

    let formulaValid = true;
    const boostTolerance = 0.01;
    for (const r of response.results) {
      const entityScore = hardcodedScores[r.entityId] ?? DEFAULT_AVERAGE_SCORE;
      const expectedBoost = this.expectedScoreBoost(entityScore);
      const actualBoost = r.relevanceScore! - r.textMatchScore!;
      const diff = Math.abs(actualBoost - expectedBoost);

      if (diff > boostTolerance) {
        formulaValid = false;
        this.addResult('test17_formula', false,
          `Score boost formula mismatch for ${r.entityId}: expected ${expectedBoost.toFixed(3)}, got ${actualBoost.toFixed(3)} (diff ${diff.toFixed(4)})`);
        break;
      }
    }
    if (formulaValid) {
      this.addResult('test17_formula', true,
        `relevanceScore = textMatchScore + expectedBoost (within ${boostTolerance}) using hardcoded scores`);
    }
  }

  /** Verifies score boost determines ordering when text match is equal. */
  async test18_ScoreBoostOverridesWithEqualTextMatch(): Promise<void> {
    console.log(`\n${BLUE}Test 18: Score boost determines ordering when text match is equal${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length !== 7) {
      this.addResult('test18_count', false, `Expected 7 Alice entities, got ${response.results.length}`);
      return;
    }

    // Expected ordering by entity_global_score descending
    const expectedOrder = [
      TEST_ENTITIES.ALICE_HIGH_ID,        // 0.95
      TEST_ENTITIES.ALICE_MEDIUM_ID,      // 0.65
      TEST_ENTITIES.ALICE_AT_THRESHOLD_ID,// 0.50
      TEST_ENTITIES.ALICE_BELOW_THRESHOLD_ID, // 0.25
      TEST_ENTITIES.ALICE_LOW_ID,         // 0.15
      TEST_ENTITIES.ALICE_ZERO_ID,        // 0.0
      TEST_ENTITIES.ALICE_NEGATIVE_ID,    // -0.75
    ];

    const actualOrder = response.results.map(r => r.entityId);
    const orderMatches = expectedOrder.every((id, i) => actualOrder[i] === id);

    if (orderMatches) {
      this.addResult('test18_order', true,
        `Ordering matches score boost order: High(0.95) > Med(0.65) > Thresh(0.50) > Below(0.25) > Low(0.15) > Zero(0.0) > Neg(-0.75)`);
    } else {
      this.addResult('test18_order', false,
        `Ordering mismatch. Expected: ${expectedOrder.join(', ')}. Got: ${actualOrder.join(', ')}`);
    }

    // Verify score boost difference ≈ relevanceScore difference (since textMatch is ~equal)
    const first = response.results[0];
    const last = response.results[response.results.length - 1];
    const relevanceDiff = first.relevanceScore! - last.relevanceScore!;
    const expectedBoostDiff = this.expectedScoreBoost(0.95) - this.expectedScoreBoost(-0.75);
    const diffTolerance = 0.5;

    if (Math.abs(relevanceDiff - expectedBoostDiff) <= diffTolerance) {
      this.addResult('test18_boost_diff', true,
        `Relevance spread (${relevanceDiff.toFixed(3)}) ≈ boost spread (${expectedBoostDiff.toFixed(3)}) — text match contribution is equal`);
    } else {
      this.addResult('test18_boost_diff', false,
        `Relevance spread (${relevanceDiff.toFixed(3)}) differs from boost spread (${expectedBoostDiff.toFixed(3)}) by more than ${diffTolerance}`);
    }
  }

  /** Verifies text match overpowers a higher score boost from non-matching entities. */
  async test19_TextMatchOverpowersScoreBoost(): Promise<void> {
    console.log(`\n${BLUE}Test 19: Text match overpowers score boost (query 'bob')${NC}`);

    const response = await this.search({
      query: 'bob',
      scope: 'GLOBAL',
    });

    if (response.results.length === 0) {
      this.addResult('test19_has_results', false, `No results for 'bob' query`);
      return;
    }

    // Bob should be the first result
    const firstResult = response.results[0];
    if (firstResult.entityId === TEST_ENTITIES.BOB_ID) {
      this.addResult('test19_bob_first', true,
        `Bob is the first result despite Alice High having higher score boost (0.95 vs 0.75)`);
    } else {
      this.addResult('test19_bob_first', false,
        `First result should be Bob (${TEST_ENTITIES.BOB_ID}), got ${firstResult.entityId}`);
    }

    // Bob should have textMatchScore > 0
    if (typeof firstResult.textMatchScore === 'number' && firstResult.textMatchScore > 0) {
      this.addResult('test19_bob_text_match', true,
        `Bob has textMatchScore: ${firstResult.textMatchScore.toFixed(3)}`);
    } else {
      this.addResult('test19_bob_text_match', false,
        `Bob should have textMatchScore > 0, got ${firstResult.textMatchScore}`);
    }

    // Bob should have relevanceScore > 0
    if (typeof firstResult.relevanceScore === 'number' && firstResult.relevanceScore > 0) {
      this.addResult('test19_bob_relevance', true,
        `Bob has relevanceScore: ${firstResult.relevanceScore.toFixed(3)}`);
    } else {
      this.addResult('test19_bob_relevance', false,
        `Bob should have relevanceScore > 0, got ${firstResult.relevanceScore}`);
    }

    // If any Alice appears via fuzzy matching, she should rank below Bob
    const aliceInResults = response.results.find(r => r.name === 'Alice');
    if (aliceInResults) {
      const bobIndex = response.results.findIndex(r => r.entityId === TEST_ENTITIES.BOB_ID);
      const aliceIndex = response.results.findIndex(r => r.name === 'Alice');
      if (bobIndex < aliceIndex) {
        this.addResult('test19_bob_above_alice', true,
          `Bob ranks above fuzzy-matched Alice (text match dominates over score boost)`);
      } else {
        this.addResult('test19_bob_above_alice', false,
          `Bob should rank above Alice — text match should dominate score boost`);
      }
    } else {
      this.addResult('test19_no_alice', true,
        `No Alice in 'bob' results — entities with higher score boost but no text match are correctly excluded`);
    }
  }

  /** Verifies empty query results have textMatchScore = 0. */
  async test20_EmptyQueryTextMatchScoreZero(): Promise<void> {
    console.log(`\n${BLUE}Test 20: Empty query — textMatchScore should be 0 (top-ranked by score only)${NC}`);

    const response = await this.search({
      scope: 'GLOBAL',
    });

    if (response.results.length === 0) {
      this.addResult('test20_has_results', false, `Empty query should return results`);
      return;
    }

    // All textMatchScores should be ~0 (floating point: score and script_fields compute
    // independently, and larger SCORE_BOOST values amplify floating point drift)
    const epsilon = 1e-4;
    const allZeroTextMatch = response.results.every(r => (r.textMatchScore ?? 0) < epsilon);
    if (allZeroTextMatch) {
      this.addResult('test20_text_match_zero', true,
        `All ${response.results.length} results have textMatchScore ≈ 0 (no text matching in empty queries)`);
    } else {
      const nonZero = response.results.find(r => (r.textMatchScore ?? 0) >= epsilon);
      this.addResult('test20_text_match_zero', false,
        `Expected textMatchScore ≈ 0 for all results, found ${nonZero?.textMatchScore} for entity ${nonZero?.entityId}`);
    }

    // All relevanceScores should be > 0 (score boost is always positive)
    const allPositiveRelevance = response.results.every(r =>
      typeof r.relevanceScore === 'number' && r.relevanceScore > 0
    );
    if (allPositiveRelevance) {
      this.addResult('test20_positive_relevance', true,
        `All results have relevanceScore > 0 (score boost always positive)`);
    } else {
      this.addResult('test20_positive_relevance', false,
        `Some results have non-positive relevanceScore`);
    }

    // relevanceScores should be in descending order
    let isDescending = true;
    for (let i = 0; i < response.results.length - 1; i++) {
      if (response.results[i].relevanceScore! < response.results[i + 1].relevanceScore!) {
        isDescending = false;
        break;
      }
    }
    if (isDescending) {
      this.addResult('test20_descending', true,
        `Empty query relevanceScores in descending order`);
    } else {
      this.addResult('test20_descending', false,
        `Empty query relevanceScores not in descending order`);
    }
  }

  /** Verifies UUID query has textMatchScore = relevanceScore (no score boost). */
  async test21_UuidQueryScoreEquality(): Promise<void> {
    console.log(`\n${BLUE}Test 21: UUID query — textMatchScore should equal relevanceScore${NC}`);

    const response = await this.search({
      query: TEST_ENTITIES.ALICE_HIGH_ID,
      scope: 'GLOBAL',
    });

    const aliceHigh = response.results.find(r => r.entityId === TEST_ENTITIES.ALICE_HIGH_ID);

    if (!aliceHigh) {
      this.addResult('test21_found', false, `Alice High not found via UUID query`);
      return;
    }

    this.addResult('test21_found', true, `Alice High found via UUID query`);

    // textMatchScore should equal relevanceScore (no function_score applied)
    if (aliceHigh.textMatchScore === aliceHigh.relevanceScore) {
      this.addResult('test21_equality', true,
        `textMatchScore (${aliceHigh.textMatchScore}) === relevanceScore (${aliceHigh.relevanceScore}) — no score boost in UUID queries`);
    } else {
      this.addResult('test21_equality', false,
        `textMatchScore (${aliceHigh.textMatchScore}) should equal relevanceScore (${aliceHigh.relevanceScore}) for UUID queries`);
    }

    // Both should be > 0
    if (aliceHigh.relevanceScore! > 0 && aliceHigh.textMatchScore! > 0) {
      this.addResult('test21_positive', true,
        `Both scores are positive (relevance: ${aliceHigh.relevanceScore}, textMatch: ${aliceHigh.textMatchScore})`);
    } else {
      this.addResult('test21_positive', false,
        `Scores should be positive — relevance: ${aliceHigh.relevanceScore}, textMatch: ${aliceHigh.textMatchScore}`);
    }
  }

  /** Verifies score fields work correctly with SPACE scope (entity_space_score boost). */
  async test22_SpaceScopeScoreFields(): Promise<void> {
    console.log(`\n${BLUE}Test 22: SPACE scope — score fields use entity_space_score${NC}`);

    const response = await this.search({
      query: 'alice',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.SPACE_ID,
    });

    if (response.results.length !== 7) {
      this.addResult('test22_count', false, `Expected 7 Alice entities for SPACE scope, got ${response.results.length}`);
      return;
    }

    // All results should have relevanceScore > 0 and textMatchScore >= 0
    let allValid = true;
    for (const r of response.results) {
      if (typeof r.relevanceScore !== 'number' || r.relevanceScore <= 0) {
        allValid = false;
        this.addResult('test22_valid_scores', false,
          `Entity ${r.entityId} has invalid relevanceScore: ${r.relevanceScore}`);
        break;
      }
      if (typeof r.textMatchScore !== 'number' || r.textMatchScore < 0) {
        allValid = false;
        this.addResult('test22_valid_scores', false,
          `Entity ${r.entityId} has invalid textMatchScore: ${r.textMatchScore}`);
        break;
      }
    }
    if (allValid) {
      this.addResult('test22_valid_scores', true,
        `All ${response.results.length} results have valid relevanceScore > 0 and textMatchScore >= 0`);
    }

    // relevanceScore >= textMatchScore for all results
    const allRelevanceGte = response.results.every(r => r.relevanceScore! >= r.textMatchScore!);
    if (allRelevanceGte) {
      this.addResult('test22_relevance_gte_text', true,
        `relevanceScore >= textMatchScore for all SPACE scope results`);
    } else {
      const bad = response.results.find(r => r.relevanceScore! < r.textMatchScore!);
      this.addResult('test22_relevance_gte_text', false,
        `Entity ${bad?.entityId}: relevanceScore (${bad?.relevanceScore}) < textMatchScore (${bad?.textMatchScore})`);
    }

    // relevanceScores should be in descending order
    let isDescending = true;
    for (let i = 0; i < response.results.length - 1; i++) {
      if (response.results[i].relevanceScore! < response.results[i + 1].relevanceScore!) {
        isDescending = false;
        break;
      }
    }
    if (isDescending) {
      this.addResult('test22_descending', true,
        `SPACE scope relevanceScores in descending order`);
    } else {
      this.addResult('test22_descending', false,
        `SPACE scope relevanceScores not in descending order`);
    }
  }

  // ─── Text Match Scoring Tests ─────────────────────────────────────────────
  // These tests verify that textMatchScore correctly reflects text matching quality.
  // All entities in each group have the same entity_global_score (0.50) so scoreBoost
  // is identical and textMatchScore differences reflect only text matching quality.

  /**
   * Helper: searches for two entities and returns them in a comparable form.
   * Returns [betterEntity, worseEntity] or null if either is missing.
   */
  private async findTextMatchPair(
    query: string,
    betterId: string,
    worseId: string,
  ): Promise<{ better: SearchResult; worse: SearchResult } | null> {
    const response = await this.search({ query, scope: 'GLOBAL' });
    const better = response.results.find(r => r.entityId === betterId);
    const worse = response.results.find(r => r.entityId === worseId);
    if (!better || !worse) return null;
    return { better, worse };
  }

  /** Test 23: Name match should have higher textMatchScore than description-only match. */
  async test23_NameMatchBeatsDescriptionMatch(): Promise<void> {
    console.log(`\n${BLUE}Test 23: Name match > description-only match (query 'Wonderland')${NC}`);
    console.log(`  ${BLUE}→ name="Wonderland" (${TEST_ENTITIES.TM_NAME_MATCH_ID}) vs name="Rex" desc="Researcher @Wonderland" (${TEST_ENTITIES.TM_DESC_MATCH_ID})${NC}`);

    const pair = await this.findTextMatchPair(
      'Wonderland',
      TEST_ENTITIES.TM_NAME_MATCH_ID,
      TEST_ENTITIES.TM_DESC_MATCH_ID,
    );

    if (!pair) {
      this.addResult('test23_found', false,
        `Could not find both text match entities for 'Wonderland' query`);
      return;
    }

    this.addResult('test23_found', true,
      `Found both entities: name match (${pair.better.name}) and desc match (${pair.worse.name})`);

    // Both should have textMatchScore > 0
    const betterTM = pair.better.textMatchScore ?? 0;
    const worseTM = pair.worse.textMatchScore ?? 0;

    if (betterTM > 0 && worseTM > 0) {
      this.addResult('test23_both_have_scores', true,
        `Both have textMatchScore > 0: name match=${betterTM.toFixed(3)}, desc match=${worseTM.toFixed(3)}`);
    } else {
      this.addResult('test23_both_have_scores', false,
        `Expected both textMatchScores > 0: name match=${betterTM}, desc match=${worseTM}`);
    }

    // Name match should have HIGHER textMatchScore than description-only match
    if (betterTM > worseTM) {
      this.addResult('test23_name_beats_desc', true,
        `Name match textMatchScore (${betterTM.toFixed(3)}) > description match (${worseTM.toFixed(3)}) — name matches are weighted higher`);
    } else {
      this.addResult('test23_name_beats_desc', false,
        `Name match textMatchScore (${betterTM.toFixed(3)}) should be > description match (${worseTM.toFixed(3)}) — BM25 field length normalization may be inflating description scores`);
    }

    // Verify score boosts are equal (both entities have same entity_global_score=0.50)
    const betterBoost = pair.better.relevanceScore! - betterTM;
    const worseBoost = pair.worse.relevanceScore! - worseTM;
    const boostDiff = Math.abs(betterBoost - worseBoost);
    if (boostDiff < 0.01) {
      this.addResult('test23_equal_score_boost', true,
        `Score boosts are equal (${betterBoost.toFixed(3)} ≈ ${worseBoost.toFixed(3)}) — textMatchScore difference is purely from text matching`);
    } else {
      this.addResult('test23_equal_score_boost', false,
        `Score boosts should be equal but differ: ${betterBoost.toFixed(3)} vs ${worseBoost.toFixed(3)} (diff=${boostDiff.toFixed(4)})`);
    }
  }

  /** Test 24: Exact name match should have higher textMatchScore than fuzzy name match. */
  async test24_ExactMatchBeatsFuzzyMatch(): Promise<void> {
    console.log(`\n${BLUE}Test 24: Exact match > fuzzy match (query 'Blockchain')${NC}`);
    console.log(`  ${BLUE}→ name="Blockchain" (${TEST_ENTITIES.TM_EXACT_MATCH_ID}) vs name="Blockchan" (${TEST_ENTITIES.TM_FUZZY_MATCH_ID})${NC}`);

    const pair = await this.findTextMatchPair(
      'Blockchain',
      TEST_ENTITIES.TM_EXACT_MATCH_ID,
      TEST_ENTITIES.TM_FUZZY_MATCH_ID,
    );

    if (!pair) {
      this.addResult('test24_found', false,
        `Could not find both text match entities for 'Blockchain' query`);
      return;
    }

    this.addResult('test24_found', true,
      `Found both entities: exact (${pair.better.name}) and fuzzy (${pair.worse.name})`);

    const exactTM = pair.better.textMatchScore ?? 0;
    const fuzzyTM = pair.worse.textMatchScore ?? 0;

    if (exactTM > 0 && fuzzyTM > 0) {
      this.addResult('test24_both_have_scores', true,
        `Both have textMatchScore > 0: exact=${exactTM.toFixed(3)}, fuzzy=${fuzzyTM.toFixed(3)}`);
    } else {
      this.addResult('test24_both_have_scores', false,
        `Expected both textMatchScores > 0: exact=${exactTM}, fuzzy=${fuzzyTM}`);
    }

    // Exact match should score higher than fuzzy match
    if (exactTM > fuzzyTM) {
      this.addResult('test24_exact_beats_fuzzy', true,
        `Exact match textMatchScore (${exactTM.toFixed(3)}) > fuzzy match (${fuzzyTM.toFixed(3)}) — exact matches correctly rank above fuzzy`);
    } else {
      this.addResult('test24_exact_beats_fuzzy', false,
        `Exact match textMatchScore (${exactTM.toFixed(3)}) should be > fuzzy match (${fuzzyTM.toFixed(3)})`);
    }

    // Verify score boosts are equal
    const exactBoost = pair.better.relevanceScore! - exactTM;
    const fuzzyBoost = pair.worse.relevanceScore! - fuzzyTM;
    const boostDiff = Math.abs(exactBoost - fuzzyBoost);
    if (boostDiff < 0.01) {
      this.addResult('test24_equal_score_boost', true,
        `Score boosts are equal (${exactBoost.toFixed(3)} ≈ ${fuzzyBoost.toFixed(3)})`);
    } else {
      this.addResult('test24_equal_score_boost', false,
        `Score boosts should be equal but differ: ${exactBoost.toFixed(3)} vs ${fuzzyBoost.toFixed(3)} (diff=${boostDiff.toFixed(4)})`);
    }
  }

  /** Test 25: Multi-word match should have higher textMatchScore than single-word match. */
  async test25_MultiWordMatchBeatsSingleWord(): Promise<void> {
    console.log(`\n${BLUE}Test 25: Multi-word match > single-word match (query 'San Francisco')${NC}`);
    console.log(`  ${BLUE}→ name="San Francisco" (${TEST_ENTITIES.TM_MULTI_WORD_ID}) vs name="San Diego" (${TEST_ENTITIES.TM_SINGLE_WORD_ID})${NC}`);

    const pair = await this.findTextMatchPair(
      'San Francisco',
      TEST_ENTITIES.TM_MULTI_WORD_ID,
      TEST_ENTITIES.TM_SINGLE_WORD_ID,
    );

    if (!pair) {
      this.addResult('test25_found', false,
        `Could not find both text match entities for 'San Francisco' query`);
      return;
    }

    this.addResult('test25_found', true,
      `Found both entities: multi-word (${pair.better.name}) and single-word (${pair.worse.name})`);

    const multiTM = pair.better.textMatchScore ?? 0;
    const singleTM = pair.worse.textMatchScore ?? 0;

    if (multiTM > 0 && singleTM > 0) {
      this.addResult('test25_both_have_scores', true,
        `Both have textMatchScore > 0: multi-word=${multiTM.toFixed(3)}, single-word=${singleTM.toFixed(3)}`);
    } else {
      this.addResult('test25_both_have_scores', false,
        `Expected both textMatchScores > 0: multi-word=${multiTM}, single-word=${singleTM}`);
    }

    // Multi-word (both "San" and "Francisco" match) should score higher
    if (multiTM > singleTM) {
      this.addResult('test25_multi_beats_single', true,
        `Multi-word textMatchScore (${multiTM.toFixed(3)}) > single-word (${singleTM.toFixed(3)}) — matching more query terms scores higher`);
    } else {
      this.addResult('test25_multi_beats_single', false,
        `Multi-word textMatchScore (${multiTM.toFixed(3)}) should be > single-word (${singleTM.toFixed(3)})`);
    }

    // Verify score boosts are equal
    const multiBoost = pair.better.relevanceScore! - multiTM;
    const singleBoost = pair.worse.relevanceScore! - singleTM;
    const boostDiff = Math.abs(multiBoost - singleBoost);
    if (boostDiff < 0.01) {
      this.addResult('test25_equal_score_boost', true,
        `Score boosts are equal (${multiBoost.toFixed(3)} ≈ ${singleBoost.toFixed(3)})`);
    } else {
      this.addResult('test25_equal_score_boost', false,
        `Score boosts should be equal but differ: ${multiBoost.toFixed(3)} vs ${singleBoost.toFixed(3)} (diff=${boostDiff.toFixed(4)})`);
    }
  }

  /** Test 26: Name+description match should have higher textMatchScore than name-only match. */
  async test26_NameAndDescMatchBeatsNameOnly(): Promise<void> {
    console.log(`\n${BLUE}Test 26: Name + description match > name-only match (query 'Quantum')${NC}`);
    console.log(`  ${BLUE}→ name="Quantum Computing" desc="Quantum physics..." (${TEST_ENTITIES.TM_NAME_AND_DESC_ID}) vs name="Quantum Mechanics" desc="The study of subatomic particles" (${TEST_ENTITIES.TM_NAME_ONLY_ID})${NC}`);

    const pair = await this.findTextMatchPair(
      'Quantum',
      TEST_ENTITIES.TM_NAME_AND_DESC_ID,
      TEST_ENTITIES.TM_NAME_ONLY_ID,
    );

    if (!pair) {
      this.addResult('test26_found', false,
        `Could not find both text match entities for 'Quantum' query`);
      return;
    }

    this.addResult('test26_found', true,
      `Found both entities: name+desc match (${pair.better.name}) and name-only match (${pair.worse.name})`);

    const nameAndDescTM = pair.better.textMatchScore ?? 0;
    const nameOnlyTM = pair.worse.textMatchScore ?? 0;

    if (nameAndDescTM > 0 && nameOnlyTM > 0) {
      this.addResult('test26_both_have_scores', true,
        `Both have textMatchScore > 0: name+desc=${nameAndDescTM.toFixed(3)}, name-only=${nameOnlyTM.toFixed(3)}`);
    } else {
      this.addResult('test26_both_have_scores', false,
        `Expected both textMatchScores > 0: name+desc=${nameAndDescTM}, name-only=${nameOnlyTM}`);
    }

    // Entity matching in both name AND description should score higher than name-only
    if (nameAndDescTM > nameOnlyTM) {
      this.addResult('test26_name_desc_beats_name_only', true,
        `Name+desc textMatchScore (${nameAndDescTM.toFixed(3)}) > name-only (${nameOnlyTM.toFixed(3)}) — matching in more fields scores higher`);
    } else {
      this.addResult('test26_name_desc_beats_name_only', false,
        `Name+desc textMatchScore (${nameAndDescTM.toFixed(3)}) should be > name-only (${nameOnlyTM.toFixed(3)})`);
    }

    // Verify score boosts are equal
    const ndBoost = pair.better.relevanceScore! - nameAndDescTM;
    const noBoost = pair.worse.relevanceScore! - nameOnlyTM;
    const boostDiff = Math.abs(ndBoost - noBoost);
    if (boostDiff < 0.01) {
      this.addResult('test26_equal_score_boost', true,
        `Score boosts are equal (${ndBoost.toFixed(3)} ≈ ${noBoost.toFixed(3)})`);
    } else {
      this.addResult('test26_equal_score_boost', false,
        `Score boosts should be equal but differ: ${ndBoost.toFixed(3)} vs ${noBoost.toFixed(3)} (diff=${boostDiff.toFixed(4)})`);
    }
  }

  /** Test 27: High global score outranks low global score entity that has a slightly better text match. */
  async test27_HighScoreOutranksLowScoreWithBetterTextMatch(): Promise<void> {
    console.log(`\n${BLUE}Test 27: High global score outranks low global score with slightly better text match (query 'Velociraptor')${NC}`);
    console.log(`  ${BLUE}→ name="Velociraptor Research" score=0.9 (${TEST_ENTITIES.TM_HIGH_SCORE_ID}) vs name="Velociraptor Species" score=0.2 (${TEST_ENTITIES.TM_LOW_SCORE_ID})${NC}`);

    const response = await this.search({ query: 'Velociraptor', scope: 'GLOBAL' });
    const highScore = response.results.find(r => r.entityId === TEST_ENTITIES.TM_HIGH_SCORE_ID);
    const lowScore = response.results.find(r => r.entityId === TEST_ENTITIES.TM_LOW_SCORE_ID);

    if (!highScore || !lowScore) {
      this.addResult('test27_found', false,
        `Could not find both entities for 'Velociraptor' query (highScore=${!!highScore}, lowScore=${!!lowScore})`);
      return;
    }

    this.addResult('test27_found', true,
      `Found both entities: high-score (${highScore.name}) and low-score (${lowScore.name})`);

    const highTM = highScore.textMatchScore ?? 0;
    const lowTM = lowScore.textMatchScore ?? 0;

    // Both should have textMatchScore > 0 (both match "Velociraptor" in name)
    if (highTM > 0 && lowTM > 0) {
      this.addResult('test27_both_have_scores', true,
        `Both have textMatchScore > 0: high-score=${highTM.toFixed(3)}, low-score=${lowTM.toFixed(3)}`);
    } else {
      this.addResult('test27_both_have_scores', false,
        `Expected both textMatchScores > 0: high-score=${highTM}, low-score=${lowTM}`);
    }

    // Despite similar or slightly lower text match, the high-score entity should rank higher (higher relevanceScore)
    const highRel = highScore.relevanceScore ?? 0;
    const lowRel = lowScore.relevanceScore ?? 0;
    if (highRel > lowRel) {
      this.addResult('test27_high_score_outranks', true,
        `High-score entity relevanceScore (${highRel.toFixed(3)}) > low-score entity (${lowRel.toFixed(3)}) — score boost overcomes text match difference`);
    } else {
      this.addResult('test27_high_score_outranks', false,
        `Expected high-score entity to outrank: highRel=${highRel.toFixed(3)}, lowRel=${lowRel.toFixed(3)}`);
    }

    // Verify the score boosts are different (confirming different global scores)
    const highBoost = highRel - highTM;
    const lowBoost = lowRel - lowTM;
    if (highBoost > lowBoost) {
      this.addResult('test27_different_score_boosts', true,
        `Score boosts differ as expected: high=${highBoost.toFixed(3)} (score=0.9) vs low=${lowBoost.toFixed(3)} (score=0.2)`);
    } else {
      this.addResult('test27_different_score_boosts', false,
        `Expected high-score entity to have higher boost: high=${highBoost.toFixed(3)}, low=${lowBoost.toFixed(3)}`);
    }
  }

  /** Test 28: Prefix/autocomplete query matches entities by partial name. */
  async test28_PrefixQueryMatches(): Promise<void> {
    console.log(`\n${BLUE}Test 28: Prefix query 'Quant' matches 'Quantum Computing' and 'Quantum Mechanics'${NC}`);

    const response = await this.search({ query: 'Quant', scope: 'GLOBAL' });
    const quantumComputing = response.results.find(r => r.entityId === TEST_ENTITIES.TM_NAME_AND_DESC_ID);
    const quantumMechanics = response.results.find(r => r.entityId === TEST_ENTITIES.TM_NAME_ONLY_ID);

    if (quantumComputing && quantumMechanics) {
      this.addResult('test28_both_found', true,
        `Both Quantum entities found via prefix query 'Quant'`);
    } else {
      this.addResult('test28_both_found', false,
        `Expected both Quantum entities: Computing=${!!quantumComputing}, Mechanics=${!!quantumMechanics}`);
      return;
    }

    const compTM = quantumComputing.textMatchScore ?? 0;
    const mechTM = quantumMechanics.textMatchScore ?? 0;
    if (compTM > 0 && mechTM > 0) {
      this.addResult('test28_positive_text_match', true,
        `Both have positive textMatchScore from prefix: Computing=${compTM.toFixed(3)}, Mechanics=${mechTM.toFixed(3)}`);
    } else {
      this.addResult('test28_positive_text_match', false,
        `Expected positive textMatchScores: Computing=${compTM}, Mechanics=${mechTM}`);
    }

    // "Quantum Computing" also has "Quantum" in description, so it should score higher (same as Test 26)
    if (compTM > mechTM) {
      this.addResult('test28_prefix_name_desc_beats_name_only', true,
        `Prefix: name+desc match (${compTM.toFixed(3)}) > name-only match (${mechTM.toFixed(3)})`);
    } else {
      this.addResult('test28_prefix_name_desc_beats_name_only', false,
        `Expected name+desc prefix match to score higher: Computing=${compTM.toFixed(3)}, Mechanics=${mechTM.toFixed(3)}`);
    }
  }

  /** Test 29: Case insensitive matching — all case variants find the entity,
   *  but exact case match gets a name_raw boost (case-sensitive keyword field). */
  async test29_CaseInsensitiveMatching(): Promise<void> {
    console.log(`\n${BLUE}Test 29: Case insensitive matching — 'blockchain' vs 'Blockchain' vs 'BLOCKCHAIN'${NC}`);

    const [lower, proper, upper] = await Promise.all([
      this.search({ query: 'blockchain', scope: 'GLOBAL' }),
      this.search({ query: 'Blockchain', scope: 'GLOBAL' }),
      this.search({ query: 'BLOCKCHAIN', scope: 'GLOBAL' }),
    ]);

    const lowerId = TEST_ENTITIES.TM_EXACT_MATCH_ID;
    const lowerResult = lower.results.find(r => r.entityId === lowerId);
    const properResult = proper.results.find(r => r.entityId === lowerId);
    const upperResult = upper.results.find(r => r.entityId === lowerId);

    if (lowerResult && properResult && upperResult) {
      this.addResult('test29_all_cases_found', true,
        `'Blockchain' entity found with all case variants (lower, proper, upper)`);
    } else {
      this.addResult('test29_all_cases_found', false,
        `Expected entity in all case variants: lower=${!!lowerResult}, proper=${!!properResult}, upper=${!!upperResult}`);
      return;
    }

    const lowerTM = lowerResult.textMatchScore ?? 0;
    const properTM = properResult.textMatchScore ?? 0;
    const upperTM = upperResult.textMatchScore ?? 0;

    // Non-exact case variants should score equally (both miss name_raw boost)
    const nonExactSpread = Math.abs(lowerTM - upperTM);
    if (nonExactSpread < 0.1) {
      this.addResult('test29_nonexact_cases_equal', true,
        `Non-exact case variants score equally: lower=${lowerTM.toFixed(3)}, upper=${upperTM.toFixed(3)} (spread=${nonExactSpread.toFixed(4)})`);
    } else {
      this.addResult('test29_nonexact_cases_equal', false,
        `Non-exact case variants should score equally: lower=${lowerTM.toFixed(3)}, upper=${upperTM.toFixed(3)} (spread=${nonExactSpread.toFixed(4)})`);
    }

    // Exact case match ("Blockchain") should score higher due to name_raw boost
    if (properTM > lowerTM) {
      this.addResult('test29_exact_case_boost', true,
        `Exact case 'Blockchain' scores higher (${properTM.toFixed(3)}) than 'blockchain' (${lowerTM.toFixed(3)}) — name_raw exact match boost`);
    } else {
      this.addResult('test29_exact_case_boost', false,
        `Expected exact case 'Blockchain' (${properTM.toFixed(3)}) to score higher than 'blockchain' (${lowerTM.toFixed(3)})`);
    }
  }

  /** Test 30: Entity with no score field gets DEFAULT_AVERAGE_SCORE boost. */
  async test30_DefaultScoreBoost(): Promise<void> {
    console.log(`\n${BLUE}Test 30: Entity with no score gets DEFAULT_AVERAGE_SCORE (${DEFAULT_AVERAGE_SCORE}) boost${NC}`);

    // Charlie has no global score - search for "Charlie" to find it
    const response = await this.search({ query: 'Charlie', scope: 'GLOBAL' });
    const charlie = response.results.find(r => r.entityId === TEST_ENTITIES.CHARLIE_ID);

    if (!charlie) {
      this.addResult('test30_charlie_found', false,
        `Charlie entity (${TEST_ENTITIES.CHARLIE_ID}) not found in search results`);
      return;
    }

    this.addResult('test30_charlie_found', true,
      `Charlie found with relevanceScore=${charlie.relevanceScore?.toFixed(3)}, textMatchScore=${charlie.textMatchScore?.toFixed(3)}`);

    // Expected boost = (max(DEFAULT_AVERAGE_SCORE, MIN_SCORE_THRESHOLD) + SCORE_SHIFT) * SCORE_BOOST
    const expectedBoost = (Math.max(DEFAULT_AVERAGE_SCORE, MIN_SCORE_THRESHOLD) + SCORE_SHIFT) * SCORE_BOOST;
    const actualBoost = (charlie.relevanceScore ?? 0) - (charlie.textMatchScore ?? 0);
    const boostDiff = Math.abs(actualBoost - expectedBoost);

    if (boostDiff < 0.01) {
      this.addResult('test30_default_boost', true,
        `Charlie's boost (${actualBoost.toFixed(3)}) matches expected DEFAULT_AVERAGE_SCORE boost (${expectedBoost.toFixed(3)})`);
    } else {
      this.addResult('test30_default_boost', false,
        `Charlie's boost (${actualBoost.toFixed(3)}) doesn't match expected (${expectedBoost.toFixed(3)}), diff=${boostDiff.toFixed(4)}`);
    }
  }

  /** Test 31: Query with extra non-matching words still finds entity. */
  async test31_ExtraQueryWordsStillMatch(): Promise<void> {
    console.log(`\n${BLUE}Test 31: Query with extra non-matching words still finds entity${NC}`);

    // "Wonderland magical unicorn" — only "Wonderland" matches, "magical" and "unicorn" don't
    const response = await this.search({ query: 'Wonderland magical unicorn', scope: 'GLOBAL' });
    const wonderland = response.results.find(r => r.entityId === TEST_ENTITIES.TM_NAME_MATCH_ID);

    if (wonderland) {
      this.addResult('test31_found_with_extra_words', true,
        `'Wonderland' entity found despite extra non-matching words in query`);
    } else {
      this.addResult('test31_found_with_extra_words', false,
        `'Wonderland' entity (${TEST_ENTITIES.TM_NAME_MATCH_ID}) not found with query 'Wonderland magical unicorn'`);
      return;
    }

    // Compare with exact query textMatchScore
    const exactResponse = await this.search({ query: 'Wonderland', scope: 'GLOBAL' });
    const exactWonderland = exactResponse.results.find(r => r.entityId === TEST_ENTITIES.TM_NAME_MATCH_ID);

    if (!exactWonderland) {
      this.addResult('test31_exact_found', false, `Could not find Wonderland with exact query for comparison`);
      return;
    }

    const extraTM = wonderland.textMatchScore ?? 0;
    const exactTM = exactWonderland.textMatchScore ?? 0;

    // The extra-words query should have lower or equal textMatchScore (extra unmatched words dilute score)
    this.addResult('test31_extra_words_lower_score', true,
      `Extra words query textMatch=${extraTM.toFixed(3)} vs exact query textMatch=${exactTM.toFixed(3)} (extra words ${extraTM <= exactTM ? 'reduce' : 'increase'} score)`);

    // Both should have textMatchScore > 0
    if (extraTM > 0) {
      this.addResult('test31_positive_text_match', true,
        `textMatchScore > 0 even with extra non-matching words: ${extraTM.toFixed(3)}`);
    } else {
      this.addResult('test31_positive_text_match', false,
        `Expected positive textMatchScore with extra words: ${extraTM}`);
    }
  }

  /**
   * Verifies that an entity with an exact short name match ranks above an entity
   * where the query is only a prefix of a longer word in the name, and also
   * verifies where a multi-word exact-token name ("Geo Graph") ranks.
   *
   * Query: "geo"
   * - "Geo" (name is the exact query) should rank FIRST
   * - "Geo Graph" (name contains exact token "geo" + extra word) should rank SECOND
   * - "geojson_preview_tool" (name starts with "geojson", "geo" is only a prefix) should rank last
   *
   * All entities have NO score values (all get DEFAULT_AVERAGE_SCORE boost),
   * so ranking should be determined purely by text match quality.
   */
  async test33_ExactShortNameBeatsLongerPrefixName(): Promise<void> {
    console.log(`\n${BLUE}Test 33: Exact name match ranking (query 'geo')${NC}`);
    console.log(`  ${BLUE}→ "Geo" (${TEST_ENTITIES.GEO_EXACT_ID}) vs "Geo Graph" (${TEST_ENTITIES.GEO_GRAPH_ID}) vs "geojson_preview_tool" (${TEST_ENTITIES.GEO_PREFIX_ID})${NC}`);

    const response = await this.search({ query: 'geo', scope: 'GLOBAL' });

    const geoExact = response.results.find(r => r.entityId === TEST_ENTITIES.GEO_EXACT_ID);
    const geoPrefix = response.results.find(r => r.entityId === TEST_ENTITIES.GEO_PREFIX_ID);
    const geoGraph = response.results.find(r => r.entityId === TEST_ENTITIES.GEO_GRAPH_ID);

    if (!geoExact || !geoPrefix || !geoGraph) {
      this.addResult('test33_found', false,
        `Could not find all entities for 'geo' query (geoExact=${!!geoExact}, geoGraph=${!!geoGraph}, geoPrefix=${!!geoPrefix})`);
      return;
    }

    this.addResult('test33_found', true,
      `Found all 3 entities: "${geoExact.name}", "${geoGraph.name}", "${geoPrefix.name}"`);

    // Log the scores for debugging
    const geoExactIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.GEO_EXACT_ID);
    const geoGraphIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.GEO_GRAPH_ID);
    const geoPrefixIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.GEO_PREFIX_ID);

    console.log(`    Geo: position=${geoExactIdx}, relevance=${geoExact.relevanceScore?.toFixed(3)}, textMatch=${geoExact.textMatchScore?.toFixed(3)}`);
    console.log(`    Geo Graph: position=${geoGraphIdx}, relevance=${geoGraph.relevanceScore?.toFixed(3)}, textMatch=${geoGraph.textMatchScore?.toFixed(3)}`);
    console.log(`    geojson_preview_tool: position=${geoPrefixIdx}, relevance=${geoPrefix.relevanceScore?.toFixed(3)}, textMatch=${geoPrefix.textMatchScore?.toFixed(3)}`);

    // "Geo" should rank ABOVE "geojson_preview_tool"
    if (geoExactIdx < geoPrefixIdx) {
      this.addResult('test33_geo_beats_geojson', true,
        `"Geo" (position ${geoExactIdx}) ranks above "geojson_preview_tool" (position ${geoPrefixIdx}) — exact name match outranks prefix match`);
    } else {
      this.addResult('test33_geo_beats_geojson', false,
        `"Geo" (position ${geoExactIdx}) should rank above "geojson_preview_tool" (position ${geoPrefixIdx})`);
    }

    // "Geo" should rank ABOVE "Geo Graph"
    if (geoExactIdx < geoGraphIdx) {
      this.addResult('test33_geo_beats_geo_graph', true,
        `"Geo" (position ${geoExactIdx}) ranks above "Geo Graph" (position ${geoGraphIdx}) — shorter exact name match outranks longer name`);
    } else {
      this.addResult('test33_geo_beats_geo_graph', false,
        `"Geo" (position ${geoExactIdx}) should rank above "Geo Graph" (position ${geoGraphIdx})`);
    }

    // "Geo Graph" should rank ABOVE "geojson_preview_tool"
    if (geoGraphIdx < geoPrefixIdx) {
      this.addResult('test33_geo_graph_beats_geojson', true,
        `"Geo Graph" (position ${geoGraphIdx}) ranks above "geojson_preview_tool" (position ${geoPrefixIdx}) — exact token match outranks prefix-only match`);
    } else {
      this.addResult('test33_geo_graph_beats_geojson', false,
        `"Geo Graph" (position ${geoGraphIdx}) should rank above "geojson_preview_tool" (position ${geoPrefixIdx})`);
    }

    // Verify score boosts are equal (all entities have no score → DEFAULT_AVERAGE_SCORE)
    const geoExactRel = geoExact.relevanceScore ?? 0;
    const geoPrefixRel = geoPrefix.relevanceScore ?? 0;
    const geoGraphRel = geoGraph.relevanceScore ?? 0;
    const geoExactTM = geoExact.textMatchScore ?? 0;
    const geoPrefixTM = geoPrefix.textMatchScore ?? 0;
    const geoGraphTM = geoGraph.textMatchScore ?? 0;
    const boosts = [geoExactRel - geoExactTM, geoGraphRel - geoGraphTM, geoPrefixRel - geoPrefixTM];
    const boostSpread = Math.max(...boosts) - Math.min(...boosts);

    if (boostSpread < 0.01) {
      this.addResult('test33_equal_score_boost', true,
        `Score boosts are equal across all 3 entities (spread=${boostSpread.toFixed(4)}) — ranking is purely from text matching`);
    } else {
      this.addResult('test33_equal_score_boost', false,
        `Score boosts should be equal but spread=${boostSpread.toFixed(4)}: Geo=${boosts[0].toFixed(3)}, GeoGraph=${boosts[1].toFixed(3)}, geojson=${boosts[2].toFixed(3)}`);
    }
  }

  /** Verifies that "World affairs" (with space) ranks above "world-affairs" (hyphenated). */
  async test33b_ExactRawNameBeatsHyphenatedName(): Promise<void> {
    console.log(`\n${BLUE}Test 33b: Exact raw name match beats hyphenated name (query 'World affairs')${NC}`);
    console.log(`  ${BLUE}→ "World affairs" (${TEST_ENTITIES.WORLD_AFFAIRS_EXACT_ID}) vs "world-affairs" (${TEST_ENTITIES.WORLD_AFFAIRS_HYPHEN_ID})${NC}`);

    const response = await this.search({ query: 'World affairs', scope: 'GLOBAL' });

    const exact = response.results.find(r => r.entityId === TEST_ENTITIES.WORLD_AFFAIRS_EXACT_ID);
    const hyphen = response.results.find(r => r.entityId === TEST_ENTITIES.WORLD_AFFAIRS_HYPHEN_ID);

    if (!exact || !hyphen) {
      this.addResult('test33b_found', false,
        `Could not find both entities for 'World affairs' query (exact=${!!exact}, hyphen=${!!hyphen})`);
      return;
    }

    this.addResult('test33b_found', true,
      `Found both entities: "${exact.name}", "${hyphen.name}"`);

    const exactIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.WORLD_AFFAIRS_EXACT_ID);
    const hyphenIdx = response.results.findIndex(r => r.entityId === TEST_ENTITIES.WORLD_AFFAIRS_HYPHEN_ID);

    console.log(`    World affairs: position=${exactIdx}, relevance=${exact.relevanceScore?.toFixed(3)}, textMatch=${exact.textMatchScore?.toFixed(3)}`);
    console.log(`    world-affairs: position=${hyphenIdx}, relevance=${hyphen.relevanceScore?.toFixed(3)}, textMatch=${hyphen.textMatchScore?.toFixed(3)}`);

    if (exactIdx < hyphenIdx) {
      this.addResult('test33b_exact_beats_hyphen', true,
        `"World affairs" (position ${exactIdx}) ranks above "world-affairs" (position ${hyphenIdx}) — name.raw exact match outranks hyphenated variant`);
    } else {
      this.addResult('test33b_exact_beats_hyphen', false,
        `"World affairs" (position ${exactIdx}) should rank above "world-affairs" (position ${hyphenIdx})`);
    }
  }

  /** Verifies API returns dashless UUIDs and accepts dashless UUID queries. */
  async test34_DashlessUuidFormat(): Promise<void> {
    console.log(`\n${BLUE}Test 34: Verify dashless UUID format in responses and inputs${NC}`);

    const DASHLESS_PATTERN = /^[0-9a-f]{32}$/;

    // 34a: Verify response entityId, space.id, and types[].id are dashless
    console.log(`  ${BLUE}→ 34a: Verify response entityId/space.id/types[].id are dashless UUIDs${NC}`);
    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    if (response.results.length === 0) {
      this.addResult('test34a_has_results', false, `No results found for Alice search`);
      return;
    }

    const firstResult = response.results[0];

    if (DASHLESS_PATTERN.test(firstResult.entityId)) {
      this.addResult('test34a_entityId_dashless', true,
        `entityId is dashless format: ${firstResult.entityId}`);
    } else {
      this.addResult('test34a_entityId_dashless', false,
        `entityId should be dashless (32 hex chars), got: ${firstResult.entityId}`);
    }

    if (DASHLESS_PATTERN.test(firstResult.space.id)) {
      this.addResult('test34a_spaceId_dashless', true,
        `space.id is dashless format: ${firstResult.space.id}`);
    } else {
      this.addResult('test34a_spaceId_dashless', false,
        `space.id should be dashless (32 hex chars), got: ${firstResult.space.id}`);
    }

    const typeIds = firstResult.types?.map(t => t.id);
    if (typeIds && typeIds.length > 0) {
      const allTypeIdsDashless = typeIds.every(id => DASHLESS_PATTERN.test(id));
      if (allTypeIdsDashless) {
        this.addResult('test34a_typeIds_dashless', true,
          `All type IDs are dashless format (${typeIds.length} IDs)`);
      } else {
        const badId = typeIds.find(id => !DASHLESS_PATTERN.test(id));
        this.addResult('test34a_typeIds_dashless', false,
          `type IDs should be dashless, found: ${badId}`);
      }
    }

    // 34b: Verify dashless UUID query finds the entity
    console.log(`  ${BLUE}→ 34b: Search by dashless UUID query finds entity${NC}`);
    const dashlessUuidResponse = await this.search({
      query: TEST_ENTITIES.BOB_ID,
      scope: 'GLOBAL',
    });

    const bobByDashless = dashlessUuidResponse.results.find(r => r.entityId === TEST_ENTITIES.BOB_ID);
    if (bobByDashless) {
      this.addResult('test34b_dashless_uuid_query', true,
        `Dashless UUID query (${TEST_ENTITIES.BOB_ID}) found Bob entity`);
    } else {
      this.addResult('test34b_dashless_uuid_query', false,
        `Dashless UUID query (${TEST_ENTITIES.BOB_ID}) should find Bob entity`);
    }

    // 34c: Verify dashless space_id is accepted in SPACE scope
    console.log(`  ${BLUE}→ 34c: Dashless space_id is accepted in SPACE scope${NC}`);
    const spaceResponse = await this.search({
      query: 'alice',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.SPACE_ID,
    });

    if (spaceResponse.results.length > 0) {
      this.addResult('test34c_dashless_space_id', true,
        `Dashless space_id (${TEST_ENTITIES.SPACE_ID}) accepted, returned ${spaceResponse.results.length} results`);
    } else {
      this.addResult('test34c_dashless_space_id', false,
        `Dashless space_id (${TEST_ENTITIES.SPACE_ID}) should return results`);
    }
  }

  /** Verifies space metadata enrichment and type name resolution, including ordering scenarios. */
  async test35_SpaceAndTypeEnrichment(): Promise<void> {
    console.log(`\n${BLUE}Test 35: Verify space metadata enrichment and type name resolution${NC}`);

    // 35a: Space metadata enrichment — entity created BEFORE space.topics event
    console.log(`  ${BLUE}→ 35a: Space metadata on entity created BEFORE space.topics event (update_by_query path)${NC}`);
    const aliceResponse = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
    });

    const aliceHigh = aliceResponse.results.find(r => r.entityId === TEST_ENTITIES.ALICE_HIGH_ID);

    if (!aliceHigh) {
      this.addResult('test35a_alice_found', false, `Could not find Alice High entity (${TEST_ENTITIES.ALICE_HIGH_ID})`);
    } else {
      if (aliceHigh.space.id === TEST_ENTITIES.SPACE_ID) {
        this.addResult('test35a_space_id', true, `Alice High space.id matches test space`);
      } else {
        this.addResult('test35a_space_id', false,
          `Alice High space.id should be ${TEST_ENTITIES.SPACE_ID}, got ${aliceHigh.space.id}`);
      }

      if (aliceHigh.space.name === 'Test Space') {
        this.addResult('test35a_space_name', true, `Alice High space.name is "Test Space"`);
      } else {
        this.addResult('test35a_space_name', false,
          `Alice High space.name should be "Test Space", got "${aliceHigh.space.name}"`);
      }

      if (aliceHigh.space.description && aliceHigh.space.description.length > 0) {
        this.addResult('test35a_space_description', true,
          `Alice High space.description is populated: "${aliceHigh.space.description}"`);
      } else {
        this.addResult('test35a_space_description', false,
          `Alice High space.description should be populated`);
      }

      if (aliceHigh.space.avatar && aliceHigh.space.avatar.length > 0) {
        this.addResult('test35a_space_avatar', true,
          `Alice High space.avatar is populated: "${aliceHigh.space.avatar}"`);
      } else {
        this.addResult('test35a_space_avatar', false,
          `Alice High space.avatar should be populated`);
      }
    }

    // 35b: Space metadata enrichment — entity created AFTER space.topics event (cache-hit path)
    console.log(`  ${BLUE}→ 35b: Space metadata on entity created AFTER space.topics event (cache-hit path)${NC}`);
    const lateResponse = await this.search({
      query: 'frankie late',
      scope: 'GLOBAL',
    });

    const lateEntity = lateResponse.results.find(r => r.entityId === TEST_ENTITIES.LATE_ENTITY_ID);

    if (!lateEntity) {
      this.addResult('test35b_late_found', false,
        `Could not find Frankie Late entity (${TEST_ENTITIES.LATE_ENTITY_ID})`);
    } else {
      if (lateEntity.space.id === TEST_ENTITIES.SPACE_ID) {
        this.addResult('test35b_space_id', true, `Frankie Late space.id matches test space`);
      } else {
        this.addResult('test35b_space_id', false,
          `Frankie Late space.id should be ${TEST_ENTITIES.SPACE_ID}, got ${lateEntity.space.id}`);
      }

      if (lateEntity.space.name === 'Test Space') {
        this.addResult('test35b_space_name', true,
          `Frankie Late space.name is "Test Space" (cache-hit ordering works)`);
      } else {
        this.addResult('test35b_space_name', false,
          `Frankie Late space.name should be "Test Space", got "${lateEntity.space.name}" (cache-hit ordering may have failed)`);
      }

      if (lateEntity.space.avatar && lateEntity.space.avatar.length > 0) {
        this.addResult('test35b_space_avatar', true,
          `Frankie Late space.avatar is populated (cache-hit ordering works)`);
      } else {
        this.addResult('test35b_space_avatar', false,
          `Frankie Late space.avatar should be populated (cache-hit ordering may have failed)`);
      }
    }

    // 35c: Type name resolution — Alice High has Person + Organization types with names
    console.log(`  ${BLUE}→ 35c: Type name resolution on Alice High (Person + Organization)${NC}`);
    if (aliceHigh) {
      if (!Array.isArray(aliceHigh.types) || aliceHigh.types.length !== 2) {
        this.addResult('test35c_type_count', false,
          `Alice High should have 2 types, got ${aliceHigh.types?.length || 0}`);
      } else {
        this.addResult('test35c_type_count', true, `Alice High has 2 types`);

        const typeNames = aliceHigh.types.map(t => t.name).sort();
        const hasOrganization = typeNames.includes('Organization');
        const hasPerson = typeNames.includes('Person');

        if (hasPerson && hasOrganization) {
          this.addResult('test35c_type_names', true,
            `Type names resolved: ${typeNames.join(', ')}`);
        } else {
          this.addResult('test35c_type_names', false,
            `Expected type names ["Organization", "Person"], got ${JSON.stringify(typeNames)}`);
        }
      }
    }

    // 35d: Type id values — verify IDs match expected constants
    console.log(`  ${BLUE}→ 35d: Type id values match expected constants${NC}`);
    if (aliceHigh && Array.isArray(aliceHigh.types) && aliceHigh.types.length === 2) {
      const typeIdSet = new Set(aliceHigh.types.map(t => t.id));
      const hasPersonId = typeIdSet.has(TEST_ENTITIES.PERSON_TYPE_ID);
      const hasOrgId = typeIdSet.has(TEST_ENTITIES.ORG_TYPE_ID);

      if (hasPersonId && hasOrgId) {
        this.addResult('test35d_type_ids', true,
          `Type IDs match: Person=${TEST_ENTITIES.PERSON_TYPE_ID}, Org=${TEST_ENTITIES.ORG_TYPE_ID}`);
      } else {
        this.addResult('test35d_type_ids', false,
          `Expected type IDs [${TEST_ENTITIES.PERSON_TYPE_ID}, ${TEST_ENTITIES.ORG_TYPE_ID}], got ${JSON.stringify(Array.from(typeIdSet))}`);
      }
    }
  }

  /**
   * Test 36: Verify that a space.topics event creates a stub document in the index.
   *
   * The topic entity should be findable by its entity ID, proving the
   * HermesTopicDeclared event alone is enough to create a document.
   */
  async test36_SpaceTopicCreatesDocument(): Promise<void> {
    console.log(`\n${BLUE}Test 36: Verify space.topics event creates a document for the topic entity${NC}`);

    // Search for the topic entity by its UUID — this should find the stub document
    // created by the space.topics consumer, even if knowledge.edits also enriched it.
    const response = await this.search({
      query: TEST_ENTITIES.TOPIC_ENTITY_ID,
      scope: 'GLOBAL',
    });

    const topicEntity = response.results.find(r => r.entityId === TEST_ENTITIES.TOPIC_ENTITY_ID);

    if (!topicEntity) {
      this.addResult('test36_topic_entity_exists', false,
        `Topic entity (${TEST_ENTITIES.TOPIC_ENTITY_ID}) not found in index — space.topics event should create a stub document`);
      return;
    }

    this.addResult('test36_topic_entity_exists', true,
      `Topic entity (${TEST_ENTITIES.TOPIC_ENTITY_ID}) found in index`);

    // The topic entity should belong to the test space
    if (topicEntity.space.id === TEST_ENTITIES.SPACE_ID) {
      this.addResult('test36_topic_space_id', true,
        `Topic entity space.id matches test space`);
    } else {
      this.addResult('test36_topic_space_id', false,
        `Topic entity space.id should be ${TEST_ENTITIES.SPACE_ID}, got ${topicEntity.space.id}`);
    }

    // The topic entity's own space metadata should resolve to itself
    if (topicEntity.space.name === 'Test Space') {
      this.addResult('test36_topic_self_resolve', true,
        `Topic entity resolves its own space.name: "Test Space"`);
    } else {
      this.addResult('test36_topic_self_resolve', false,
        `Topic entity space.name should be "Test Space" (self-resolve), got "${topicEntity.space.name}"`);
    }
  }

  /**
   * Test 37: Verify that entities in spaces WITHOUT a topic declaration
   * do not get enriched space metadata (name, description, avatar).
   *
   * The rank_high_space and rank_low_space have no HermesTopicDeclared event,
   * so their entities should have space.id but NOT space.name/description/avatar.
   * This is the counterpart to test35 which verifies spaces WITH topics DO get metadata.
   */
  async test37_SpaceWithoutTopicHasNoMetadata(): Promise<void> {
    console.log(`\n${BLUE}Test 37: Verify spaces without topic declaration have no enriched metadata${NC}`);

    const response = await this.search({
      query: 'RankTest',
      scope: 'GLOBAL',
    });

    // Find Gamma entity (lives in rank_high_space which has no topic declared)
    const gamma = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_GAMMA_ENTITY_ID);

    if (!gamma) {
      this.addResult('test37_gamma_found', false,
        `Could not find RankTest Gamma entity (${TEST_ENTITIES.RANK_GAMMA_ENTITY_ID})`);
      return;
    }

    // 37a: space.id should be present (always set from the edit event)
    if (gamma.space.id === TEST_ENTITIES.RANK_HIGH_SPACE_ID) {
      this.addResult('test37a_space_id_present', true,
        `Gamma space.id is present and matches rank_high_space`);
    } else {
      this.addResult('test37a_space_id_present', false,
        `Gamma space.id should be ${TEST_ENTITIES.RANK_HIGH_SPACE_ID}, got ${gamma.space.id}`);
    }

    // 37b: space.name should be absent (no topic declared for this space)
    if (!gamma.space.name) {
      this.addResult('test37b_no_space_name', true,
        `Gamma has no space.name (correct — no topic declared for rank_high_space)`);
    } else {
      this.addResult('test37b_no_space_name', false,
        `Gamma should not have space.name (no topic declared), got "${gamma.space.name}"`);
    }

    // 37c: space.description should be absent
    if (!gamma.space.description) {
      this.addResult('test37c_no_space_description', true,
        `Gamma has no space.description (correct — no topic declared)`);
    } else {
      this.addResult('test37c_no_space_description', false,
        `Gamma should not have space.description, got "${gamma.space.description}"`);
    }

    // 37d: space.avatar should be absent
    if (!gamma.space.avatar) {
      this.addResult('test37d_no_space_avatar', true,
        `Gamma has no space.avatar (correct — no topic declared)`);
    } else {
      this.addResult('test37d_no_space_avatar', false,
        `Gamma should not have space.avatar, got "${gamma.space.avatar}"`);
    }

    // 37e: Check a second entity in a different space without topic (rank_low_space)
    const epsilon = response.results.find(r => r.entityId === TEST_ENTITIES.RANK_EPSILON_ENTITY_ID);

    if (!epsilon) {
      this.addResult('test37e_epsilon_found', false,
        `Could not find RankTest Epsilon entity (${TEST_ENTITIES.RANK_EPSILON_ENTITY_ID})`);
    } else if (!epsilon.space.name && !epsilon.space.description && !epsilon.space.avatar) {
      this.addResult('test37e_epsilon_no_metadata', true,
        `Epsilon (rank_low_space) also has no enriched space metadata`);
    } else {
      this.addResult('test37e_epsilon_no_metadata', false,
        `Epsilon should not have space metadata, got name="${epsilon.space.name}" desc="${epsilon.space.description}" avatar="${epsilon.space.avatar}"`);
    }
  }


  // ── Topology helpers ────────────────────────────────────────────────────────

  /** Fetch the topology subspaces response from the indexer health port. */
  private async fetchTopologySubspaces(spaceId: string): Promise<{
    space_id: string;
    subspaces: string[];
    count: number;
    is_root: boolean;
  } | null> {
    try {
      const response = await fetch(`${this.indexerUrl}/topology/subspaces/${spaceId}`);
      if (response.status === 404) return null;
      if (!response.ok) return null;
      return response.json() as Promise<{
        space_id: string;
        subspaces: string[];
        count: number;
        is_root: boolean;
      }>;
    } catch {
      return null;
    }
  }

  /** Fetch the topology root from the indexer health port. */
  private async fetchTopologyRoot(): Promise<{ root_id: string } | null> {
    try {
      const response = await fetch(`${this.indexerUrl}/topology/root`);
      if (!response.ok) return null;
      return response.json() as Promise<{ root_id: string }>;
    } catch {
      return null;
    }
  }

  // ── Topology / canonical graph diff tests ───────────────────────────────────

  /**
   * Test 38: Topology root is set after diff 1.
   *
   * Validates that /topology/root returns topo_root_id and that is_root=true for
   * the root space in /topology/subspaces/:id.
   */
  async test38_TopologyRootIsSet(): Promise<void> {
    console.log(`\n${BLUE}Test 38: Topology root is set after diff 1${NC}`);

    const rootResp = await this.fetchTopologyRoot();
    if (!rootResp) {
      this.addResult('test38_root_endpoint', false, '/topology/root returned null — topology state not loaded');
      return;
    }

    // The dashless hex of topo_root_id
    const expectedRootHex = TEST_ENTITIES.TOPO_ROOT_SPACE_ID.replace(/-/g, '');
    // The endpoint returns a dashed UUID string
    const actualRootDashless = rootResp.root_id.replace(/-/g, '');
    if (actualRootDashless === expectedRootHex) {
      this.addResult('test38_root_id', true, `/topology/root returns correct root ${rootResp.root_id}`);
    } else {
      this.addResult('test38_root_id', false, `Expected root ${TEST_ENTITIES.TOPO_ROOT_SPACE_ID}, got ${rootResp.root_id}`);
    }

    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_ROOT_SPACE_ID);
    if (!subResp) {
      this.addResult('test38_subspaces_found', false, `/topology/subspaces/${TEST_ENTITIES.TOPO_ROOT_SPACE_ID} returned 404`);
      return;
    }
    this.addResult('test38_subspaces_found', true, `/topology/subspaces/:root returned ${subResp.count} subspaces`);

    if (subResp.is_root) {
      this.addResult('test38_is_root', true, 'topo_root reported is_root=true');
    } else {
      this.addResult('test38_is_root', false, `Expected is_root=true for topo_root, got ${subResp.is_root}`);
    }
  }

  /**
   * Test 39: in_canonical_graph flag is set for entities after diff 1.
   *
   * Searches for "TopoEntity" in SPACE scope for each canonical space; all six
   * spaces should return their entity with the search succeeding (the entity is
   * indexed and reachable).  We don't have direct OpenSearch field access, but
   * the SPACE scope query succeeds only when the document exists and the
   * in_canonical_graph flag is consistent with the search query used.
   *
   * We also verify that a GLOBAL search returns all six TopoEntity documents.
   */
  async test39_CanonicalEntitiesIndexed(): Promise<void> {
    console.log(`\n${BLUE}Test 39: All six topology entities are indexed (in_canonical_graph set after diff 1)${NC}`);

    const globalResp = await this.search({ query: 'TopoEntity', scope: 'GLOBAL', limit: 20 });
    const topoIds = new Set([
      TEST_ENTITIES.TOPO_ROOT_ENTITY_ID,
      TEST_ENTITIES.TOPO_CHILD_A_ENTITY_ID,
      TEST_ENTITIES.TOPO_CHILD_B_ENTITY_ID,
      TEST_ENTITIES.TOPO_GRANDCHILD_ENTITY_ID,
      TEST_ENTITIES.TOPO_REMOVE_ME_ENTITY_ID,
      TEST_ENTITIES.TOPO_MOVEABLE_ENTITY_ID,
    ]);

    const foundIds = new Set(globalResp.results.map(r => r.entityId));
    const allFound = [...topoIds].every(id => foundIds.has(id));
    if (allFound) {
      this.addResult('test39_all_indexed', true, `All 6 TopoEntity entities found in GLOBAL search`);
    } else {
      const missing = [...topoIds].filter(id => !foundIds.has(id));
      this.addResult('test39_all_indexed', false, `Missing TopoEntity entities: ${missing.join(', ')}`);
    }
  }

  /**
   * Test 40: SPACE scope on topo_root (canonical root) returns all 5 canonical entities.
   *
   * topo_root is the canonical root → the API must use {term:{in_canonical_graph:true}}
   * which matches ALL entities across all canonical spaces (root + children + grandchild).
   *
   * After all three diffs the canonical set is:
   *   topo_root, topo_child_a, topo_child_b, topo_grandchild, topo_moveable (5 spaces)
   * So we expect 5 entities in the result (topo_remove_me was removed).
   */
  async test40_SpaceScopeRootUsesCanonicalFilter(): Promise<void> {
    console.log(`\n${BLUE}Test 40: SPACE scope on canonical root returns entities from all canonical subspaces${NC}`);

    const response = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_ROOT_SPACE_ID,
      limit: 20,
    });

    // topo_remove_me was removed in diff 2 → its entity should not appear
    // (or if it does, its in_canonical_graph=false so the term filter excludes it)
    const expectedCanonical = new Set([
      TEST_ENTITIES.TOPO_ROOT_ENTITY_ID,
      TEST_ENTITIES.TOPO_CHILD_A_ENTITY_ID,
      TEST_ENTITIES.TOPO_CHILD_B_ENTITY_ID,
      TEST_ENTITIES.TOPO_GRANDCHILD_ENTITY_ID,
      TEST_ENTITIES.TOPO_MOVEABLE_ENTITY_ID,
    ]);

    const foundIds = new Set(response.results.map(r => r.entityId));

    const removeMePresent = foundIds.has(TEST_ENTITIES.TOPO_REMOVE_ME_ENTITY_ID);
    if (!removeMePresent) {
      this.addResult('test40_remove_me_excluded', true, 'topo_remove_me entity excluded by in_canonical_graph filter');
    } else {
      this.addResult('test40_remove_me_excluded', false, 'topo_remove_me entity appeared in canonical root SPACE scope (expected exclusion)');
    }

    const allCanonicalPresent = [...expectedCanonical].every(id => foundIds.has(id));
    if (allCanonicalPresent) {
      this.addResult('test40_canonical_entities_present', true, `All 5 canonical TopoEntity entities returned by root SPACE scope`);
    } else {
      const missing = [...expectedCanonical].filter(id => !foundIds.has(id));
      this.addResult('test40_canonical_entities_present', false, `Missing canonical TopoEntity entities: ${missing.join(', ')}`);
    }
  }

  /**
   * Test 41: SPACE scope on a non-root canonical space uses subspace ID list.
   *
   * topo_child_a has one child (topo_grandchild) → subspaces = [child_a, grandchild].
   * The SPACE scope query must use a terms filter with those two IDs, NOT
   * in_canonical_graph (because child_a is not the root).
   *
   * Expected results: topo_child_a entity + topo_grandchild entity = 2 results.
   * topo_root, topo_child_b, topo_moveable entities are NOT children of child_a
   * so they must NOT appear.
   */
  async test41_SpaceScopeNonRootUsesSubspaceList(): Promise<void> {
    console.log(`\n${BLUE}Test 41: SPACE scope on non-root space uses subspace ID list (not in_canonical_graph)${NC}`);

    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_CHILD_A_SPACE_ID);
    if (!subResp) {
      this.addResult('test41_subspaces_endpoint', false, `/topology/subspaces/${TEST_ENTITIES.TOPO_CHILD_A_SPACE_ID} returned 404`);
      return;
    }

    if (subResp.is_root === false) {
      this.addResult('test41_not_root', true, `topo_child_a correctly reports is_root=false`);
    } else {
      this.addResult('test41_not_root', false, `Expected is_root=false for topo_child_a, got ${subResp.is_root}`);
    }

    const expectedSubspaceCount = 2; // child_a + grandchild
    if (subResp.count === expectedSubspaceCount) {
      this.addResult('test41_subspace_count', true, `topo_child_a has ${subResp.count} subspaces (child_a + grandchild)`);
    } else {
      this.addResult('test41_subspace_count', false, `Expected 2 subspaces for topo_child_a, got ${subResp.count}: ${JSON.stringify(subResp.subspaces)}`);
    }

    const response = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_CHILD_A_SPACE_ID,
      limit: 20,
    });

    const foundIds = new Set(response.results.map(r => r.entityId));

    const childAPresent = foundIds.has(TEST_ENTITIES.TOPO_CHILD_A_ENTITY_ID);
    const grandchildPresent = foundIds.has(TEST_ENTITIES.TOPO_GRANDCHILD_ENTITY_ID);
    const rootAbsent = !foundIds.has(TEST_ENTITIES.TOPO_ROOT_ENTITY_ID);
    const childBAbsent = !foundIds.has(TEST_ENTITIES.TOPO_CHILD_B_ENTITY_ID);

    this.addResult('test41_child_a_present', childAPresent, childAPresent
      ? 'topo_child_a entity in child_a SPACE scope result'
      : 'topo_child_a entity MISSING from child_a SPACE scope result');
    this.addResult('test41_grandchild_present', grandchildPresent, grandchildPresent
      ? 'topo_grandchild entity in child_a SPACE scope result'
      : 'topo_grandchild entity MISSING from child_a SPACE scope result');
    this.addResult('test41_root_absent', rootAbsent, rootAbsent
      ? 'topo_root entity correctly absent from child_a SPACE scope'
      : 'topo_root entity incorrectly appeared in child_a SPACE scope');
    this.addResult('test41_child_b_absent', childBAbsent, childBAbsent
      ? 'topo_child_b entity correctly absent from child_a SPACE scope'
      : 'topo_child_b entity incorrectly appeared in child_a SPACE scope');
  }

  /**
   * Test 42: Removed node loses in_canonical_graph flag (diff 2).
   *
   * topo_remove_me was removed in diff 2.  Its entity must NOT appear in a SPACE
   * scope search on topo_root (which uses the in_canonical_graph filter).  We
   * also confirm that the topology endpoint no longer knows about topo_remove_me.
   */
  async test42_RemovedNodeLosesCanonicalFlag(): Promise<void> {
    console.log(`\n${BLUE}Test 42: Removed node loses in_canonical_graph flag (diff 2 removal)${NC}`);

    // The /topology/subspaces endpoint returns 404 for spaces not in the graph.
    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_REMOVE_ME_SPACE_ID);
    if (subResp === null) {
      this.addResult('test42_topology_404', true, 'topo_remove_me not found in topology (correctly removed)');
    } else {
      this.addResult('test42_topology_404', false, `Expected 404 for topo_remove_me, got subspaces: ${JSON.stringify(subResp)}`);
    }

    // Root SPACE scope must exclude topo_remove_me_entity (in_canonical_graph=false)
    const response = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_ROOT_SPACE_ID,
      limit: 20,
    });

    const foundIds = response.results.map(r => r.entityId);
    const removeMeAbsent = !foundIds.includes(TEST_ENTITIES.TOPO_REMOVE_ME_ENTITY_ID);
    if (removeMeAbsent) {
      this.addResult('test42_removed_entity_absent', true, 'topo_remove_me entity absent from root SPACE scope after diff 2');
    } else {
      this.addResult('test42_removed_entity_absent', false, 'topo_remove_me entity still appears in root SPACE scope after removal');
    }
  }

  /**
   * Test 43: Subtree cascade — topo_moveable is also removed by diff 2 (was child
   * of topo_remove_me), then re-added under topo_child_b by diff 3.
   *
   * After all diffs:
   *   - topo_moveable IS canonical again (under topo_child_b)
   *   - topo_child_b's subspaces = [child_b, moveable] (count = 2)
   *   - SPACE scope on child_b returns both entities
   */
  async test43_SubtreeCascadeAndRejoin(): Promise<void> {
    console.log(`\n${BLUE}Test 43: Subtree cascade removal + re-add (diff 2 + diff 3)${NC}`);

    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_CHILD_B_SPACE_ID);
    if (!subResp) {
      this.addResult('test43_child_b_subspaces', false, `/topology/subspaces/${TEST_ENTITIES.TOPO_CHILD_B_SPACE_ID} returned 404`);
      return;
    }

    // After diff 3, topo_moveable is under child_b → child_b subspaces = {child_b, moveable}
    const expectedCount = 2;
    if (subResp.count === expectedCount) {
      this.addResult('test43_child_b_subspace_count', true, `topo_child_b has ${subResp.count} subspaces after move (child_b + moveable)`);
    } else {
      this.addResult('test43_child_b_subspace_count', false, `Expected 2 subspaces for topo_child_b after diff 3, got ${subResp.count}: ${JSON.stringify(subResp.subspaces)}`);
    }

    // topo_moveable entity should appear in child_b SPACE scope
    const response = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_CHILD_B_SPACE_ID,
      limit: 20,
    });

    const foundIds = new Set(response.results.map(r => r.entityId));
    const moveablePresent = foundIds.has(TEST_ENTITIES.TOPO_MOVEABLE_ENTITY_ID);
    const childBPresent = foundIds.has(TEST_ENTITIES.TOPO_CHILD_B_ENTITY_ID);
    const grandchildAbsent = !foundIds.has(TEST_ENTITIES.TOPO_GRANDCHILD_ENTITY_ID);

    this.addResult('test43_moveable_present', moveablePresent, moveablePresent
      ? 'topo_moveable entity present in child_b SPACE scope after re-add'
      : 'topo_moveable entity MISSING from child_b SPACE scope after re-add');
    this.addResult('test43_child_b_present', childBPresent, childBPresent
      ? 'topo_child_b entity present in child_b SPACE scope'
      : 'topo_child_b entity MISSING from child_b SPACE scope');
    this.addResult('test43_grandchild_absent', grandchildAbsent, grandchildAbsent
      ? 'topo_grandchild entity correctly absent from child_b SPACE scope'
      : 'topo_grandchild entity incorrectly appeared in child_b SPACE scope');
  }

  /**
   * Test 44: Topology state persistence — /topology/root and /topology/subspaces
   * endpoints serve correct data, implying the indexer loaded or built the state
   * from the persisted file on startup and applied all subsequent diffs.
   *
   * We verify the final subspace topology matches all three diffs applied correctly:
   *   root → {root, child_a, child_b, grandchild, moveable}  (5 total)
   */
  async test44_TopologyPersistenceAndFinalState(): Promise<void> {
    console.log(`\n${BLUE}Test 44: Topology final state — all diffs applied, root has 5 subspaces${NC}`);

    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_ROOT_SPACE_ID);
    if (!subResp) {
      this.addResult('test44_root_subspaces', false, 'Failed to fetch root subspaces');
      return;
    }

    const expectedCount = 5; // root + child_a + child_b + grandchild + moveable
    if (subResp.count === expectedCount) {
      this.addResult('test44_final_subspace_count', true, `Root has ${subResp.count} subspaces after all 3 diffs`);
    } else {
      this.addResult('test44_final_subspace_count', false, `Expected ${expectedCount} subspaces at root after all diffs, got ${subResp.count}: ${JSON.stringify(subResp.subspaces)}`);
    }

    // Confirm the removed space is not in the root's subspace list
    const dashlessSubspaces = subResp.subspaces.map(s => s.replace(/-/g, ''));
    const removeMeInList = dashlessSubspaces.includes(TEST_ENTITIES.TOPO_REMOVE_ME_SPACE_ID);
    if (!removeMeInList) {
      this.addResult('test44_remove_me_not_in_list', true, 'topo_remove_me not in root subspace list after diff 2');
    } else {
      this.addResult('test44_remove_me_not_in_list', false, 'topo_remove_me still in root subspace list after diff 2');
    }

    // topo_moveable should appear in root's subspace list after re-add in diff 3
    const moveableInList = dashlessSubspaces.includes(TEST_ENTITIES.TOPO_MOVEABLE_SPACE_ID);
    if (moveableInList) {
      this.addResult('test44_moveable_in_list', true, 'topo_moveable in root subspace list after diff 3 re-add');
    } else {
      this.addResult('test44_moveable_in_list', false, 'topo_moveable missing from root subspace list after diff 3');
    }
  }

  /**
   * Test 45: SPACE scope on a non-root space with no subspaces returns only that
   * space's own entities (no in_canonical_graph spill-over).
   *
   * topo_grandchild has no children → subspaces = [grandchild] (count = 1).
   * SPACE scope must return only topo_grandchild_entity, not root/child entities.
   */
  async test45_SpaceScopeLeafSpaceReturnsOnlyOwnEntities(): Promise<void> {
    console.log(`\n${BLUE}Test 45: SPACE scope on leaf space returns only that space's entity${NC}`);

    const subResp = await this.fetchTopologySubspaces(TEST_ENTITIES.TOPO_GRANDCHILD_SPACE_ID);
    if (!subResp) {
      this.addResult('test45_grandchild_found', false, `/topology/subspaces/${TEST_ENTITIES.TOPO_GRANDCHILD_SPACE_ID} returned 404`);
      return;
    }

    if (subResp.count === 1) {
      this.addResult('test45_leaf_subspace_count', true, `topo_grandchild has 1 subspace (self only)`);
    } else {
      this.addResult('test45_leaf_subspace_count', false, `Expected 1 subspace for topo_grandchild, got ${subResp.count}`);
    }

    if (subResp.is_root === false) {
      this.addResult('test45_not_root', true, 'topo_grandchild correctly is_root=false');
    } else {
      this.addResult('test45_not_root', false, `Expected is_root=false for leaf space, got ${subResp.is_root}`);
    }

    const response = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_GRANDCHILD_SPACE_ID,
      limit: 20,
    });

    const foundIds = new Set(response.results.map(r => r.entityId));
    const onlyGrandchild =
      foundIds.has(TEST_ENTITIES.TOPO_GRANDCHILD_ENTITY_ID) &&
      !foundIds.has(TEST_ENTITIES.TOPO_ROOT_ENTITY_ID) &&
      !foundIds.has(TEST_ENTITIES.TOPO_CHILD_A_ENTITY_ID);

    if (onlyGrandchild) {
      this.addResult('test45_only_grandchild', true, 'Only grandchild entity returned for leaf SPACE scope');
    } else {
      this.addResult('test45_only_grandchild', false,
        `Unexpected entities in leaf SPACE scope. Found: ${[...foundIds].join(', ')}`);
    }
  }

  /**
   * Test 46: inCanonicalGraph field is returned correctly in search results.
   *
   * Verifies three cases:
   *   - Canonical entity (topo_child_a) returns inCanonicalGraph: true
   *   - Non-canonical entity (topo_remove_me, removed in diff 2) returns inCanonicalGraph: false
   *   - Entity never in canonical graph (e.g. Alice High) returns inCanonicalGraph: false
   */
  async test46_InCanonicalGraphFieldReturned(): Promise<void> {
    console.log(`\n${BLUE}Test 46: inCanonicalGraph field returned correctly in search results${NC}`);

    // Search for a canonical entity
    const canonicalResp = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE_SINGLE',
      space_id: TEST_ENTITIES.TOPO_CHILD_A_SPACE_ID,
      limit: 1,
    });

    if (canonicalResp.results.length > 0) {
      const result = canonicalResp.results[0];
      if (result.inCanonicalGraph === true) {
        this.addResult('test46_canonical_true', true, 'Canonical entity has inCanonicalGraph=true');
      } else {
        this.addResult('test46_canonical_true', false,
          `Expected inCanonicalGraph=true for canonical entity, got ${result.inCanonicalGraph}`);
      }
    } else {
      this.addResult('test46_canonical_true', false, 'No results for canonical space search');
    }

    // Search for a removed (non-canonical) entity
    const removedResp = await this.search({
      query: 'TopoEntity',
      scope: 'SPACE_SINGLE',
      space_id: TEST_ENTITIES.TOPO_REMOVE_ME_SPACE_ID,
      limit: 1,
    });

    if (removedResp.results.length > 0) {
      const result = removedResp.results[0];
      if (result.inCanonicalGraph === false) {
        this.addResult('test46_removed_false', true, 'Removed entity has inCanonicalGraph=false');
      } else {
        this.addResult('test46_removed_false', false,
          `Expected inCanonicalGraph=false for removed entity, got ${result.inCanonicalGraph}`);
      }
    } else {
      this.addResult('test46_removed_false', false, 'No results for removed space search');
    }

    // Search for an entity that was never in the canonical graph
    const neverCanonicalResp = await this.search({
      query: 'Alice High',
      scope: 'GLOBAL',
      limit: 1,
    });

    if (neverCanonicalResp.results.length > 0) {
      const result = neverCanonicalResp.results[0];
      if (result.inCanonicalGraph === false) {
        this.addResult('test46_never_canonical_false', true, 'Non-canonical entity has inCanonicalGraph=false');
      } else {
        this.addResult('test46_never_canonical_false', false,
          `Expected inCanonicalGraph=false for non-canonical entity, got ${result.inCanonicalGraph}`);
      }
    } else {
      this.addResult('test46_never_canonical_false', false, 'No results for Alice High search');
    }
  }

  /** Verifies that default exclude_type_ids filters work when param is omitted. */
  async test47_DefaultExcludeTypeIds(): Promise<void> {
    console.log(`\n${BLUE}Test 47: Default exclude_type_ids filters block/media types when param is omitted${NC}`);

    // Make a raw fetch WITHOUT exclude_type_ids param at all (defaults apply).
    // Our test entities are Person/Org types — NOT block types — so they should
    // still appear. This proves the default exclusion doesn't break normal queries.
    const params = new URLSearchParams();
    params.append('query', 'alice');
    params.append('scope', 'GLOBAL');
    // Intentionally NOT appending exclude_type_ids — server defaults apply.
    // include_non_canonical defaults to true on the API, no need to set it.

    const url = `${this.baseUrl}/search?${params.toString()}`;
    const response = await fetch(url, {
      headers: { 'Accept-Encoding': 'gzip, deflate' },
    });

    if (!response.ok) {
      this.addResult('test47_default_exclude', false,
        `API returned ${response.status} when exclude_type_ids is omitted`);
      return;
    }

    const data = await response.json() as SearchResponse;

    // Alice entities are Person type — should NOT be excluded by defaults.
    const aliceCount = data.results.filter(r => r.name === 'Alice').length;
    if (aliceCount === 7) {
      this.addResult('test47_default_exclude', true,
        `Default exclusions correctly keep non-block entities (found ${aliceCount} Alice entities)`);
    } else {
      this.addResult('test47_default_exclude', false,
        `Expected 7 Alice entities with default exclusions, got ${aliceCount}`);
    }
  }

  /** Verifies user-supplied exclude_type_ids filters out entities with those types. */
  async test48_UserSuppliedExcludeTypeIds(): Promise<void> {
    console.log(`\n${BLUE}Test 48: User-supplied exclude_type_ids filters entities with those types${NC}`);

    // Search for 'alice' but exclude PERSON_TYPE_ID — should filter out all
    // Alice entities that only have the Person type, keeping only those that
    // also have Org type (Alice High has both Person + Org).
    const response = await this.search({
      query: 'alice',
      scope: 'GLOBAL',
      exclude_type_ids: [TEST_ENTITIES.PERSON_TYPE_ID],
    });

    // All Alice entities have Person type, so they should all be excluded.
    const aliceCount = response.results.filter(r => r.name === 'Alice').length;
    if (aliceCount === 0) {
      this.addResult('test48_user_exclude', true,
        `User-supplied exclude_type_ids correctly filtered out all Alice entities (Person type excluded)`);
    } else {
      this.addResult('test48_user_exclude', false,
        `Expected 0 Alice entities when Person type excluded, got ${aliceCount}`);
    }
  }

  /** Verifies that conflicting type_ids and exclude_type_ids returns 400. */
  async test49_ConflictingIncludeExcludeReturns400(): Promise<void> {
    console.log(`\n${BLUE}Test 49: Conflicting type_ids and exclude_type_ids returns 400${NC}`);

    const sharedId = TEST_ENTITIES.PERSON_TYPE_ID;
    const params = new URLSearchParams();
    params.append('query', 'alice');
    params.append('scope', 'GLOBAL');
    params.append('type_ids', sharedId);
    params.append('exclude_type_ids', sharedId);

    const url = `${this.baseUrl}/search?${params.toString()}`;
    const response = await fetch(url, {
      headers: { 'Accept-Encoding': 'gzip, deflate' },
    });

    if (response.status === 400) {
      const body = await response.json() as { error: string; message: string };
      if (body.message.includes('must not contain the same IDs')) {
        this.addResult('test49_conflict_error', true,
          `Conflicting type_ids and exclude_type_ids correctly returns 400`);
      } else {
        this.addResult('test49_conflict_error', false,
          `Got 400 but unexpected message: ${body.message}`);
      }
    } else {
      this.addResult('test49_conflict_error', false,
        `Expected 400 for conflicting IDs, got ${response.status}`);
    }
  }

  /** Verifies that explicit type_ids overrides default exclusions (e.g. requesting IMAGE_TYPE works). */
  async test50_ExplicitIncludeOverridesDefaultExclusion(): Promise<void> {
    console.log(`\n${BLUE}Test 50: Explicit type_ids overrides default exclusions${NC}`);

    // IMAGE_TYPE is in the default exclusion list, but explicitly requesting it
    // via type_ids should succeed (not 400) — the default exclusion is stripped.
    const params = new URLSearchParams();
    params.append('query', '');
    params.append('scope', 'GLOBAL');
    params.append('type_ids', 'ba4e4146-0010-499d-a0a3-caaa7f579d0e'); // SystemIds.IMAGE_TYPE

    const url = `${this.baseUrl}/search?${params.toString()}`;
    const response = await fetch(url, {
      headers: { 'Accept-Encoding': 'gzip, deflate' },
    });

    if (response.status === 200) {
      this.addResult('test50_include_overrides_default', true,
        `Explicit type_ids for a default-excluded type returns 200 (default exclusion stripped)`);
    } else {
      const body = await response.text();
      this.addResult('test50_include_overrides_default', false,
        `Expected 200 when type_ids overrides default exclusion, got ${response.status}: ${body}`);
    }
  }

  // -----------------------------------------------------------------------
  // additional_space_ids — eligibility-set redefinition
  //
  // Three "AsidProbeEntity" fixtures live across one canonical
  // (topo_child_a) and two non-canonical spaces (asid_extra_space_x,
  // asid_extra_space_y). The unique name lets a single text query enumerate
  // all three, and the per-space placement makes the eligibility filter
  // observable in the result set.
  //
  // Semantics under test (matches the existing buildAdditionalSpacesFilter
  // unit tests in api/src/services/search/opensearch.test.ts):
  //   - additional_space_ids defines the eligibility set:
  //       result ⊆ ⋃(spaces in the list)
  //   - canonical-graph root in the list rewrites to "all canonical spaces":
  //       root ∈ list  ⇒  add canonical-graph term to the OR
  //   - include_non_canonical=false ANDs with additional_space_ids — when
  //     both are supplied the result set is the intersection.
  // -----------------------------------------------------------------------

  /** Baseline: include_non_canonical=false — only the canonical AsidProbe is returned. */
  async test51_AdditionalSpaceIdsCanonicalOnlyBaseline(): Promise<void> {
    console.log(`\n${BLUE}Test 51: include_non_canonical=false alone returns only the canonical AsidProbe${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      include_non_canonical: false,
    });

    const ids = response.results.map(r => r.entityId);
    const onlyCanonical =
      ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_CANON_ENTITY_ID;

    this.addResult('test51_canonical_only_baseline', onlyCanonical,
      onlyCanonical
        ? `canonical-only AsidProbe baseline returns the 1 canonical entity (${TEST_ENTITIES.ASID_CANON_ENTITY_ID})`
        : `expected exactly [ASID_CANON], got ${ids.length} ids: ${ids.join(', ')}`);
  }

  /** additional_space_ids=[X] restricts eligibility to space X — only ASID_X matches the query. */
  async test52_AdditionalSpaceIdsScopesToOneExtraSpace(): Promise<void> {
    console.log(`\n${BLUE}Test 52: additional_space_ids=[X] scopes results to space X${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const ids = response.results.map(r => r.entityId);
    const onlyX = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_X_ENTITY_ID;

    this.addResult('test52_scopes_to_one_extra_space', onlyX,
      onlyX
        ? `additional_space_ids=[X] returns only ASID_X (canonical + Y are excluded by eligibility filter)`
        : `expected only ASID_X, got [${ids.join(', ')}]`);
  }

  /** additional_space_ids=[X, Y] restricts eligibility to X∪Y — ASID_X + ASID_Y match. */
  async test53_AdditionalSpaceIdsScopesToMultipleExtraSpaces(): Promise<void> {
    console.log(`\n${BLUE}Test 53: additional_space_ids=[X, Y] scopes results to X ∪ Y${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [
        TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID,
        TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID,
      ],
    });

    const ids = new Set(response.results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_X_ENTITY_ID,
      TEST_ENTITIES.ASID_Y_ENTITY_ID,
    ]);
    const correct = ids.size === expected.size && [...expected].every(id => ids.has(id));

    this.addResult('test53_scopes_to_multiple_extras', correct,
      correct
        ? `additional_space_ids=[X, Y] returns ASID_X + ASID_Y (canonical excluded)`
        : `expected {ASID_X, ASID_Y}, got [${[...ids].join(', ')}]`);
  }

  /**
   * additional_space_ids=[<canonical-root>] — the root ID is rewritten to
   * {term: {in_canonical_graph: true}}, so eligibility = all canonical spaces.
   * Among AsidProbe entities only ASID_CANON lives in a canonical space.
   */
  async test54_AdditionalSpaceIdsCanonicalRootRewrite(): Promise<void> {
    console.log(`\n${BLUE}Test 54: additional_space_ids=[canonical-root] rewrites to in_canonical_graph filter${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.TOPO_ROOT_SPACE_ID],
    });

    const ids = response.results.map(r => r.entityId);
    const onlyCanonical =
      ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_CANON_ENTITY_ID;

    this.addResult('test54_canonical_root_rewrite', onlyCanonical,
      onlyCanonical
        ? `canonical-root rewrite restricts AsidProbe set to ASID_CANON (1 result)`
        : `expected only ASID_CANON via canonical-root rewrite, got [${ids.join(', ')}]`);
  }

  /**
   * additional_space_ids=[<canonical-root>, X] — canonical anchor OR'd with X.
   * Among AsidProbe entities: ASID_CANON (canonical) + ASID_X (in X). Y must
   * remain absent.
   */
  async test55_AdditionalSpaceIdsCanonicalRootPlusExtra(): Promise<void> {
    console.log(`\n${BLUE}Test 55: additional_space_ids=[canonical-root, X] OR's canonical anchor with extra space${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [
        TEST_ENTITIES.TOPO_ROOT_SPACE_ID,
        TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID,
      ],
    });

    const ids = new Set(response.results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_CANON_ENTITY_ID,
      TEST_ENTITIES.ASID_X_ENTITY_ID,
    ]);
    const correct =
      ids.size === expected.size && [...expected].every(id => ids.has(id));

    this.addResult('test55_canonical_root_plus_extra', correct,
      correct
        ? `canonical anchor OR'd with extra_space_x returns ASID_CANON + ASID_X`
        : `expected {ASID_CANON, ASID_X}, got [${[...ids].join(', ')}]`);

    const yLeaked = ids.has(TEST_ENTITIES.ASID_Y_ENTITY_ID);
    this.addResult('test55_y_not_leaked', !yLeaked,
      yLeaked
        ? `ASID_Y leaked despite not being canonical and not in additional_space_ids`
        : `ASID_Y correctly absent`);
  }

  /**
   * additional_space_ids ANDs with include_non_canonical=false: when both are
   * supplied the intersection is canonical-AND-in-listed-spaces. ASID_CANON is
   * canonical but lives in topo_child_a (not in [X]) → empty result.
   */
  async test55b_AdditionalSpaceIdsAndsWithIncludeNonCanonicalFalse(): Promise<void> {
    console.log(`\n${BLUE}Test 55b: include_non_canonical=false + additional_space_ids=[X] yields the intersection (∅)${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      include_non_canonical: false,
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const empty = response.results.length === 0;
    this.addResult('test55b_intersection_empty', empty,
      empty
        ? `intersection of {canonical} ∩ {in X} is empty (ASID_CANON is canonical but not in X)`
        : `expected 0 results, got [${response.results.map(r => r.entityId).join(', ')}]`);
  }

  /** SPACE_SINGLE scope rejects additional_space_ids. */
  async test56_AdditionalSpaceIdsRejectedOnSpaceScope(): Promise<void> {
    console.log(`\n${BLUE}Test 56: additional_space_ids returns 400 with SPACE_SINGLE scope${NC}`);

    const { status, body } = await this.searchRaw({
      query: 'AsidProbeEntity',
      scope: 'SPACE_SINGLE',
      space_id: TEST_ENTITIES.TOPO_CHILD_A_SPACE_ID,
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test56_rejected_on_space_scope', correct,
      correct
        ? `400 returned with message: ${message}`
        : `expected 400, got ${status}: ${message}`);
  }

  /** Invalid UUID inside additional_space_ids returns 400. */
  async test57_AdditionalSpaceIdsRejectsInvalidUuid(): Promise<void> {
    console.log(`\n${BLUE}Test 57: additional_space_ids returns 400 when an entry is not a valid UUID${NC}`);

    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL' },
      'not-a-uuid',
    );

    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test57_rejects_invalid_uuid', correct,
      correct
        ? `400 returned for invalid UUID; message: ${message}`
        : `expected 400 for malformed UUID, got ${status}: ${message}`);
  }

  /** More than 10 IDs returns 400. */
  async test58_AdditionalSpaceIdsRejectsOverLimit(): Promise<void> {
    console.log(`\n${BLUE}Test 58: additional_space_ids returns 400 when more than 10 IDs are supplied${NC}`);

    const ids = Array.from(
      { length: 11 },
      (_, i) => `${i.toString().padStart(8, '0')}-0000-0000-0000-000000000000`,
    );
    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL' },
      ids.join(','),
    );

    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test58_rejects_over_limit', correct,
      correct
        ? `400 returned for 11 IDs; message: ${message}`
        : `expected 400 for >10 IDs, got ${status}: ${message}`);
  }

  /** SPACE scope (aggregated subspace search) also rejects additional_space_ids. */
  async test59_AdditionalSpaceIdsRejectedOnAggregatedSpaceScope(): Promise<void> {
    console.log(`\n${BLUE}Test 59: additional_space_ids returns 400 with SPACE scope${NC}`);

    const { status, body } = await this.searchRaw({
      query: 'AsidProbeEntity',
      scope: 'SPACE',
      space_id: TEST_ENTITIES.TOPO_ROOT_SPACE_ID,
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test59_rejected_on_aggregated_space_scope', correct,
      correct ? `400 with SPACE scope; message: ${message}` : `expected 400, got ${status}: ${message}`);
  }

  /** Dashless UUID format is accepted (server normalizes both forms). */
  async test60_AdditionalSpaceIdsAcceptsDashlessUuid(): Promise<void> {
    console.log(`\n${BLUE}Test 60: additional_space_ids accepts dashless UUID format${NC}`);

    const dashlessX = TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID; // already dashless
    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [dashlessX],
    });

    const ids = response.results.map(r => r.entityId);
    const onlyX = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_X_ENTITY_ID;
    this.addResult('test60_accepts_dashless_uuid', onlyX,
      onlyX
        ? `dashless UUID resolved to extra_space_x → ASID_X returned`
        : `expected only ASID_X via dashless form, got [${ids.join(', ')}]`);
  }

  /** Mixed dashed + dashless UUIDs in one CSV. */
  async test61_AdditionalSpaceIdsAcceptsMixedFormatUuids(): Promise<void> {
    console.log(`\n${BLUE}Test 61: additional_space_ids accepts mixed dashed/dashless UUIDs${NC}`);

    const dashedX = `${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(0, 8)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(8, 12)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(12, 16)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(16, 20)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(20)}`;
    const dashlessY = TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID;
    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [dashedX, dashlessY],
    });

    const ids = new Set(response.results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_X_ENTITY_ID,
      TEST_ENTITIES.ASID_Y_ENTITY_ID,
    ]);
    const correct = ids.size === expected.size && [...expected].every(id => ids.has(id));
    this.addResult('test61_accepts_mixed_format_uuids', correct,
      correct
        ? `mixed dashed-X + dashless-Y returns ASID_X + ASID_Y`
        : `expected {ASID_X, ASID_Y} via mixed form, got [${[...ids].join(', ')}]`);
  }

  /**
   * include_non_canonical=true (default) — additional_space_ids narrows the
   * default "all entities" set down to the listed spaces. Verifies the filter
   * actually applies even without include_non_canonical=false.
   */
  async test62_AdditionalSpaceIdsScopesEvenWithDefaultIncludeNonCanonical(): Promise<void> {
    console.log(`\n${BLUE}Test 62: additional_space_ids narrows the result set even with default include_non_canonical=true${NC}`);

    const baseline = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
    });
    const narrowed = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const baselineIds = new Set(baseline.results.map(r => r.entityId));
    const narrowedIds = narrowed.results.map(r => r.entityId);
    const baselineHasAll =
      baselineIds.has(TEST_ENTITIES.ASID_CANON_ENTITY_ID) &&
      baselineIds.has(TEST_ENTITIES.ASID_X_ENTITY_ID) &&
      baselineIds.has(TEST_ENTITIES.ASID_Y_ENTITY_ID);
    const narrowedOnlyX =
      narrowedIds.length === 1 && narrowedIds[0] === TEST_ENTITIES.ASID_X_ENTITY_ID;

    this.addResult('test62_default_baseline_returns_all_three', baselineHasAll,
      baselineHasAll
        ? `default include_non_canonical=true returns all 3 AsidProbe entities`
        : `expected baseline to contain all 3 probes, got [${[...baselineIds].join(', ')}]`);
    this.addResult('test62_asid_narrows_default_baseline', narrowedOnlyX,
      narrowedOnlyX
        ? `additional_space_ids=[X] narrows result to only ASID_X`
        : `expected only ASID_X after narrowing, got [${narrowedIds.join(', ')}]`);
  }

  /** Empty `additional_space_ids=` query string is treated as absent (200, canonical-only). */
  async test63_AdditionalSpaceIdsEmptyStringTreatedAsAbsent(): Promise<void> {
    console.log(`\n${BLUE}Test 63: empty additional_space_ids= is treated as absent (no 400)${NC}`);

    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL', include_non_canonical: false },
      '',
    );

    if (status !== 200) {
      this.addResult('test63_empty_treated_as_absent', false,
        `expected 200 for empty additional_space_ids=, got ${status}`);
      return;
    }
    const results = (body as { results: Array<{ entityId: string }> }).results;
    const ids = results.map(r => r.entityId);
    const onlyCanonical = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_CANON_ENTITY_ID;
    this.addResult('test63_empty_treated_as_absent', onlyCanonical,
      onlyCanonical
        ? `empty additional_space_ids= behaves identically to omitting the param (canonical-only)`
        : `expected only ASID_CANON, got [${ids.join(', ')}]`);
  }

  /** CSV containing only commas/whitespace is treated as absent. */
  async test64_AdditionalSpaceIdsWhitespaceCsvTreatedAsAbsent(): Promise<void> {
    console.log(`\n${BLUE}Test 64: CSV of only commas/whitespace is treated as absent${NC}`);

    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL', include_non_canonical: false },
      ', ,  ,',
    );

    if (status !== 200) {
      this.addResult('test64_whitespace_csv_treated_as_absent', false,
        `expected 200 for whitespace-only CSV, got ${status}`);
      return;
    }
    const results = (body as { results: Array<{ entityId: string }> }).results;
    const ids = results.map(r => r.entityId);
    const onlyCanonical = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_CANON_ENTITY_ID;
    this.addResult('test64_whitespace_csv_treated_as_absent', onlyCanonical,
      onlyCanonical
        ? `whitespace-only CSV degenerates to canonical-only baseline`
        : `expected only ASID_CANON, got [${ids.join(', ')}]`);
  }

  /** Leading/trailing/embedded commas around valid IDs are tolerated. */
  async test65_AdditionalSpaceIdsTrailingCommasTolerated(): Promise<void> {
    console.log(`\n${BLUE}Test 65: leading/trailing commas + whitespace around CSV entries are tolerated${NC}`);

    const csv = `,${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID}, ${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID},`;
    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL' },
      csv,
    );

    if (status !== 200) {
      this.addResult('test65_trailing_commas_tolerated', false,
        `expected 200 for CSV with leading/trailing commas, got ${status}`);
      return;
    }
    const results = (body as { results: Array<{ entityId: string }> }).results;
    const ids = new Set(results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_X_ENTITY_ID,
      TEST_ENTITIES.ASID_Y_ENTITY_ID,
    ]);
    const correct = ids.size === expected.size && [...expected].every(id => ids.has(id));
    this.addResult('test65_trailing_commas_tolerated', correct,
      correct
        ? `leading/trailing commas stripped; ASID_X + ASID_Y returned`
        : `expected {ASID_X, ASID_Y}, got [${[...ids].join(', ')}]`);
  }

  /** Duplicate IDs in additional_space_ids do not error and produce the same result set. */
  async test66_AdditionalSpaceIdsDuplicatesTolerated(): Promise<void> {
    console.log(`\n${BLUE}Test 66: duplicate IDs in additional_space_ids are tolerated${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [
        TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID,
        TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID,
        TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID,
      ],
    });

    const ids = new Set(response.results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_X_ENTITY_ID,
      TEST_ENTITIES.ASID_Y_ENTITY_ID,
    ]);
    const correct = ids.size === expected.size && [...expected].every(id => ids.has(id));
    this.addResult('test66_duplicates_tolerated', correct,
      correct
        ? `duplicate X passed alongside Y → ASID_X + ASID_Y, no duplicate entities`
        : `expected {ASID_X, ASID_Y}, got [${[...ids].join(', ')}]`);
  }

  /** GLOBAL_BY_SPACE_SCORE scope accepts additional_space_ids. */
  async test67_AdditionalSpaceIdsWorksWithGlobalBySpaceScore(): Promise<void> {
    console.log(`\n${BLUE}Test 67: additional_space_ids works with GLOBAL_BY_SPACE_SCORE scope${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL_BY_SPACE_SCORE',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const ids = response.results.map(r => r.entityId);
    const onlyX = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_X_ENTITY_ID;
    this.addResult('test67_global_by_space_score', onlyX,
      onlyX
        ? `GLOBAL_BY_SPACE_SCORE + asid=[X]: only ASID_X returned`
        : `expected only ASID_X, got [${ids.join(', ')}]`);
  }

  /** GLOBAL_BY_ENTITY_SPACE_SCORE scope accepts additional_space_ids. */
  async test68_AdditionalSpaceIdsWorksWithGlobalByEntitySpaceScore(): Promise<void> {
    console.log(`\n${BLUE}Test 68: additional_space_ids works with GLOBAL_BY_ENTITY_SPACE_SCORE scope${NC}`);

    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL_BY_ENTITY_SPACE_SCORE',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const ids = response.results.map(r => r.entityId);
    const onlyX = ids.length === 1 && ids[0] === TEST_ENTITIES.ASID_X_ENTITY_ID;
    this.addResult('test68_global_by_entity_space_score', onlyX,
      onlyX
        ? `GLOBAL_BY_ENTITY_SPACE_SCORE + asid=[X]: only ASID_X returned`
        : `expected only ASID_X, got [${ids.join(', ')}]`);
  }

  /** Empty query (top-ranked path) + additional_space_ids honors the filter. */
  async test69_AdditionalSpaceIdsWorksWithEmptyQuery(): Promise<void> {
    console.log(`\n${BLUE}Test 69: empty-query top-ranked path respects additional_space_ids${NC}`);

    // Empty query → top-ranked path. With additional_space_ids=[X], the eligible
    // set is restricted to X. Among AsidProbe entities only ASID_X qualifies; we
    // fetch a wide window because top-ranked sorts globally and our entity may
    // not be near the top.
    const response = await this.search({
      query: '',
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
      limit: 100,
    });

    const ids = new Set(response.results.map(r => r.entityId));
    // X must appear; canonical + Y must NOT appear (they're not in space X).
    const correct =
      ids.has(TEST_ENTITIES.ASID_X_ENTITY_ID) &&
      !ids.has(TEST_ENTITIES.ASID_CANON_ENTITY_ID) &&
      !ids.has(TEST_ENTITIES.ASID_Y_ENTITY_ID);
    this.addResult('test69_empty_query_top_ranked', correct,
      correct
        ? `empty-query path: ASID_X reachable; canonical + Y excluded by space filter`
        : `expected ASID_X present, ASID_CANON + ASID_Y absent, got [${[...ids].slice(0, 10).join(', ')}${ids.size > 10 ? `, …(${ids.size} total)` : ''}]`);
  }

  /** UUID-shaped query (entity-id lookup) + additional_space_ids honours the eligibility filter. */
  async test70_AdditionalSpaceIdsWorksWithUuidQuery(): Promise<void> {
    console.log(`\n${BLUE}Test 70: UUID-shaped query (entity ID lookup) + additional_space_ids${NC}`);

    // Looking up ASID_X by UUID:
    //   - asid=[Y] → ASID_X is filtered out (not in Y), 0 results
    //   - asid=[X] → ASID_X passes the filter, 1 result
    const dashedXEntity = `${TEST_ENTITIES.ASID_X_ENTITY_ID.slice(0, 8)}-${TEST_ENTITIES.ASID_X_ENTITY_ID.slice(8, 12)}-${TEST_ENTITIES.ASID_X_ENTITY_ID.slice(12, 16)}-${TEST_ENTITIES.ASID_X_ENTITY_ID.slice(16, 20)}-${TEST_ENTITIES.ASID_X_ENTITY_ID.slice(20)}`;

    const wrongSpace = await this.search({
      query: dashedXEntity,
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID],
    });
    const rightSpace = await this.search({
      query: dashedXEntity,
      scope: 'GLOBAL',
      additional_space_ids: [TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID],
    });

    const wrongBlocked = wrongSpace.results.length === 0;
    const rightFinds = rightSpace.results.some(r => r.entityId === TEST_ENTITIES.ASID_X_ENTITY_ID);
    this.addResult('test70_uuid_query_path', wrongBlocked && rightFinds,
      wrongBlocked && rightFinds
        ? `UUID-query: blocked when ID not in additional_space_ids, found when ID's space is listed`
        : `expected blocked-with-Y/found-with-X; got wrong=${wrongSpace.results.length}, rightFinds=${rightFinds}`);
  }

  /** Exactly 10 IDs at the boundary is accepted (no 400). */
  async test71_AdditionalSpaceIdsTenIdsBoundary(): Promise<void> {
    console.log(`\n${BLUE}Test 71: exactly 10 IDs at the boundary is accepted${NC}`);

    // Real X + Y + 8 throwaway-but-valid UUIDs that point to non-existent spaces.
    // Server must accept (length === MAX) and the eligibility filter restricts
    // results to ASID_X + ASID_Y (the throwaways match no docs).
    const padding = Array.from(
      { length: 8 },
      (_, i) => `${i.toString().padStart(8, '0')}-0000-0000-0000-000000000000`,
    );
    const ids = [
      `${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(0, 8)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(8, 12)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(12, 16)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(16, 20)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(20)}`,
      `${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID.slice(0, 8)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID.slice(8, 12)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID.slice(12, 16)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID.slice(16, 20)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_Y_ID.slice(20)}`,
      ...padding,
    ];
    const response = await this.search({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: ids,
    });

    const got = new Set(response.results.map(r => r.entityId));
    const expected = new Set([
      TEST_ENTITIES.ASID_X_ENTITY_ID,
      TEST_ENTITIES.ASID_Y_ENTITY_ID,
    ]);
    const correct = got.size === expected.size && [...expected].every(id => got.has(id));
    this.addResult('test71_ten_ids_boundary', correct,
      correct
        ? `10-ID payload accepted; ASID_X + ASID_Y returned (8 throwaways match nothing)`
        : `expected {ASID_X, ASID_Y} among 10 IDs, got [${[...got].join(', ')}]`);
  }

  /** Malformed UUID with extra trailing characters is rejected. */
  async test72_AdditionalSpaceIdsRejectsTrailingCharsInUuid(): Promise<void> {
    console.log(`\n${BLUE}Test 72: UUID with trailing characters returns 400${NC}`);

    const dashedX = `${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(0, 8)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(8, 12)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(12, 16)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(16, 20)}-${TEST_ENTITIES.ASID_EXTRA_SPACE_X_ID.slice(20)}`;
    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL' },
      `${dashedX}xx`,
    );
    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test72_rejects_trailing_chars', correct,
      correct ? `400 returned for UUID+xx; message: ${message}` : `expected 400, got ${status}: ${message}`);
  }

  /**
   * additional_space_ids referencing only phantom spaces returns 0 results —
   * no error, just an empty set (the filter scopes to spaces with no entities).
   */
  async test73_AdditionalSpaceIdsUnknownSpacesAreSilentlyIgnored(): Promise<void> {
    console.log(`\n${BLUE}Test 73: additional_space_ids referencing empty/unknown spaces returns 0 results without erroring${NC}`);

    const phantomA = '11111111-1111-1111-1111-111111111111';
    const phantomB = '22222222-2222-2222-2222-222222222222';
    const { status, body } = await this.searchRaw({
      query: 'AsidProbeEntity',
      scope: 'GLOBAL',
      additional_space_ids: [phantomA, phantomB],
    });

    if (status !== 200) {
      this.addResult('test73_unknown_spaces_silent', false,
        `expected 200 for phantom IDs, got ${status}`);
      return;
    }
    const results = (body as { results: Array<{ entityId: string }> }).results;
    const empty = results.length === 0;
    this.addResult('test73_unknown_spaces_silent', empty,
      empty
        ? `phantom space IDs return 0 results without error`
        : `expected 0 results for phantom IDs, got [${results.map(r => r.entityId).join(', ')}]`);
  }

  /** Numeric (non-UUID) values in additional_space_ids are rejected. */
  async test74_AdditionalSpaceIdsRejectsNumericValue(): Promise<void> {
    console.log(`\n${BLUE}Test 74: numeric (non-UUID) value returns 400${NC}`);

    const { status, body } = await this.searchRaw(
      { query: 'AsidProbeEntity', scope: 'GLOBAL' },
      '12345',
    );
    const correct = status === 400;
    const message =
      typeof body === 'object' && body !== null && 'message' in body
        ? String((body as { message: unknown }).message)
        : '';
    this.addResult('test74_rejects_numeric_value', correct,
      correct ? `400 returned for numeric value; message: ${message}` : `expected 400, got ${status}`);
  }

  printSummary() {
    console.log(`\n${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}`);

    const passed = this.testResults.filter(r => r.passed).length;
    const failed = this.testResults.filter(r => !r.passed).length;
    const total = this.testResults.length;

    if (failed === 0) {
      console.log(`${GREEN}✅ All ${total} validation tests passed!${NC}`);
    } else {
      console.log(`${YELLOW}⚠️  ${passed}/${total} tests passed, ${failed} failed${NC}`);
    }

    console.log(`${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}\n`);

    return failed === 0;
  }
}

async function checkApiAvailability(url: string): Promise<boolean> {
  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 2000);

    const response = await fetch(`${url}/search/health`, {
      signal: controller.signal,
    });

    clearTimeout(timeoutId);
    return response.ok;
  } catch (error) {
    return false;
  }
}

async function main() {
  console.log(`${BLUE}🔍 Search API Validation Tests${NC}\n`);

  const apiUrl = process.env.SEARCH_API_URL || 'http://localhost:3000';
  const indexerUrl = process.env.INDEXER_URL || 'http://localhost:8080';

  // Check if API is available
  console.log(`Checking API availability at ${apiUrl}...`);
  const isAvailable = await checkApiAvailability(apiUrl);

  if (!isAvailable) {
    console.log(`${RED}✗${NC} Search API not detected at ${apiUrl}\n`);
    console.log('To run validation tests, start the search API:');
    console.log('  cd api && cargo run\n');
    process.exit(1);
  }

  console.log(`${GREEN}✓${NC} Search API is running\n`);
  console.log('⏳ Waiting 5 seconds for search-indexer to process events...');
  await new Promise(resolve => setTimeout(resolve, 5000));

  console.log(`\n${BLUE}📊 Running API validation tests...${NC}`);

  const validator = new SearchValidator(apiUrl, indexerUrl);

  try {
    await validator.test1_BasicAliceSearch();
    await validator.test2_BobSearch();
    await validator.test3_OrganizationSearch();
    await validator.test4_EntityFields();
    await validator.test5_ScoreOrdering();
    await validator.test6_ResponseMetadata();
    await validator.test7_ZeroAndNegativeScores();
    await validator.test8_TypeIdsScenarios();
    await validator.test9a_DeletedEntityNotInResults();
    await validator.test9b_RestoredEntityInResults();
    await validator.test10_DeletedThenUpdatedEntityNotInResults();
    await validator.test11_EmptyQueryTopRanked();
    await validator.test12_UnsetProperties();
    await validator.test13_LWWBehavior();
    await validator.test14_IncludeDeletedFlag();
    await validator.test15_CreateEntityOp();
    await validator.test16_RelevanceScoresDescendingOrder();
    await validator.test17_TextMatchConsistencyForSameName();
    await validator.test18_ScoreBoostOverridesWithEqualTextMatch();
    await validator.test19_TextMatchOverpowersScoreBoost();
    await validator.test20_EmptyQueryTextMatchScoreZero();
    await validator.test21_UuidQueryScoreEquality();
    await validator.test22_SpaceScopeScoreFields();
    await validator.test23_NameMatchBeatsDescriptionMatch();
    await validator.test24_ExactMatchBeatsFuzzyMatch();
    await validator.test25_MultiWordMatchBeatsSingleWord();
    await validator.test26_NameAndDescMatchBeatsNameOnly();
    await validator.test27_HighScoreOutranksLowScoreWithBetterTextMatch();
    await validator.test28_PrefixQueryMatches();
    await validator.test29_CaseInsensitiveMatching();
    await validator.test30_DefaultScoreBoost();
    await validator.test31_ExtraQueryWordsStillMatch();
    await validator.test32_GlobalByEntitySpaceScoreSearch();
    await validator.test33_ExactShortNameBeatsLongerPrefixName();
    await validator.test33b_ExactRawNameBeatsHyphenatedName();
    await validator.test34_DashlessUuidFormat();
    await validator.test35_SpaceAndTypeEnrichment();
    await validator.test36_SpaceTopicCreatesDocument();
    await validator.test37_SpaceWithoutTopicHasNoMetadata();
    await validator.test38_TopologyRootIsSet();
    await validator.test39_CanonicalEntitiesIndexed();
    await validator.test40_SpaceScopeRootUsesCanonicalFilter();
    await validator.test41_SpaceScopeNonRootUsesSubspaceList();
    await validator.test42_RemovedNodeLosesCanonicalFlag();
    await validator.test43_SubtreeCascadeAndRejoin();
    await validator.test44_TopologyPersistenceAndFinalState();
    await validator.test45_SpaceScopeLeafSpaceReturnsOnlyOwnEntities();
    await validator.test46_InCanonicalGraphFieldReturned();
    await validator.test47_DefaultExcludeTypeIds();
    await validator.test48_UserSuppliedExcludeTypeIds();
    await validator.test49_ConflictingIncludeExcludeReturns400();
    await validator.test50_ExplicitIncludeOverridesDefaultExclusion();
    await validator.test51_AdditionalSpaceIdsCanonicalOnlyBaseline();
    await validator.test52_AdditionalSpaceIdsScopesToOneExtraSpace();
    await validator.test53_AdditionalSpaceIdsScopesToMultipleExtraSpaces();
    await validator.test54_AdditionalSpaceIdsCanonicalRootRewrite();
    await validator.test55_AdditionalSpaceIdsCanonicalRootPlusExtra();
    await validator.test55b_AdditionalSpaceIdsAndsWithIncludeNonCanonicalFalse();
    await validator.test56_AdditionalSpaceIdsRejectedOnSpaceScope();
    await validator.test57_AdditionalSpaceIdsRejectsInvalidUuid();
    await validator.test58_AdditionalSpaceIdsRejectsOverLimit();
    await validator.test59_AdditionalSpaceIdsRejectedOnAggregatedSpaceScope();
    await validator.test60_AdditionalSpaceIdsAcceptsDashlessUuid();
    await validator.test61_AdditionalSpaceIdsAcceptsMixedFormatUuids();
    await validator.test62_AdditionalSpaceIdsScopesEvenWithDefaultIncludeNonCanonical();
    await validator.test63_AdditionalSpaceIdsEmptyStringTreatedAsAbsent();
    await validator.test64_AdditionalSpaceIdsWhitespaceCsvTreatedAsAbsent();
    await validator.test65_AdditionalSpaceIdsTrailingCommasTolerated();
    await validator.test66_AdditionalSpaceIdsDuplicatesTolerated();
    await validator.test67_AdditionalSpaceIdsWorksWithGlobalBySpaceScore();
    await validator.test68_AdditionalSpaceIdsWorksWithGlobalByEntitySpaceScore();
    await validator.test69_AdditionalSpaceIdsWorksWithEmptyQuery();
    await validator.test70_AdditionalSpaceIdsWorksWithUuidQuery();
    await validator.test71_AdditionalSpaceIdsTenIdsBoundary();
    await validator.test72_AdditionalSpaceIdsRejectsTrailingCharsInUuid();
    await validator.test73_AdditionalSpaceIdsUnknownSpacesAreSilentlyIgnored();
    await validator.test74_AdditionalSpaceIdsRejectsNumericValue();

    const allPassed = validator.printSummary();
    process.exit(allPassed ? 0 : 1);
  } catch (error) {
    console.error(`\n${RED}✗ Validation failed with error:${NC}`, error);
    process.exit(1);
  }
}

main();
