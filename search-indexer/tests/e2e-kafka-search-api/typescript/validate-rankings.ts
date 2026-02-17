#!/usr/bin/env tsx

/**
 * Ranking Test Validator
 *
 * Reads ranking test cases from the shared JSON config (same file used by
 * the Rust publisher) and validates that the search API returns results in
 * the expected order.
 *
 * Adding a new ranking test = adding a JSON object to test-cases.json.
 * No code changes needed.
 */

import { readFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

import type { SearchResponse } from '../../../../api/src/services/search/types';

// Colors for terminal output
const GREEN = '\x1b[0;32m';
const RED = '\x1b[0;31m';
const YELLOW = '\x1b[1;33m';
const BLUE = '\x1b[0;34m';
const NC = '\x1b[0m';

// ---------- JSON config types ----------

interface RankingEntity {
  name: string;
  description: string;
  global_score?: number;
  entity_space_score?: number;
  space_score?: number;
}

interface RankingTestCase {
  name: string;
  query: string;
  scope?: string;
  entities: RankingEntity[];
}

interface RankingTestSuite {
  uniform_score: number;
  space_id: string;
  space_id_prefix: string;
  uuid_prefix: string;
  test_cases: RankingTestCase[];
}

// ---------- UUID generation (mirrors Rust logic) ----------

function entityId(prefix: string, testIdx: number, entityIdx: number): string {
  const testHex = testIdx.toString(16).padStart(2, '0');
  const entityHex = entityIdx.toString(16).padStart(2, '0');
  // Prefix already contains hyphens and is sized so appending 4 hex chars
  // completes a valid 36-char UUID (e.g. "...000000bb" + "0100")
  return `${prefix}${testHex}${entityHex}`;
}

function entitySpaceId(prefix: string, testIdx: number, entityIdx: number): string {
  const testHex = testIdx.toString(16).padStart(2, '0');
  const entityHex = entityIdx.toString(16).padStart(2, '0');
  return `${prefix}${testHex}${entityHex}`;
}

// ---------- Search helper ----------

async function search(
  baseUrl: string,
  query: string,
  spaceId?: string,
  scope?: string,
  limit = 50,
): Promise<SearchResponse> {
  const params = new URLSearchParams();
  params.append('query', query);
  if (spaceId) params.append('space_id', spaceId);
  if (scope) params.append('scope', scope);
  params.append('limit', limit.toString());

  const url = `${baseUrl}/search?${params.toString()}`;
  const response = await fetch(url, {
    headers: { 'Accept-Encoding': 'gzip, deflate' },
  });

  if (!response.ok) {
    throw new Error(`API returned ${response.status}: ${await response.text()}`);
  }

  return response.json() as Promise<SearchResponse>;
}

// ---------- Validation ----------

interface TestResult {
  name: string;
  passed: boolean;
  message: string;
}

async function validateTestCase(
  baseUrl: string,
  suite: RankingTestSuite,
  testIdx: number,
  testCase: RankingTestCase,
): Promise<TestResult> {
  // Build expected entity IDs in order
  const expectedIds = testCase.entities.map((_, entityIdx) =>
    entityId(suite.uuid_prefix, testIdx, entityIdx),
  );

  const scope = testCase.scope;
  let searchSpaceId: string | undefined;
  if (scope === 'SPACE_SINGLE') {
    searchSpaceId = suite.space_id;
  }
  const response = await search(baseUrl, testCase.query, searchSpaceId, scope);

  // Filter results to only our test entities
  const expectedSet = new Set(expectedIds);
  const matchedResults = response.results.filter(r => expectedSet.has(r.entityId));

  // Check that all expected entities were found
  const foundIds = matchedResults.map(r => r.entityId);
  const missingIds = expectedIds.filter(id => !foundIds.includes(id));

  if (missingIds.length > 0) {
    const missingNames = missingIds.map(id => {
      const idx = expectedIds.indexOf(id);
      return testCase.entities[idx].name;
    });
    return {
      name: testCase.name,
      passed: false,
      message: `Missing entities in results: ${missingNames.join(', ')} (${missingIds.join(', ')})`,
    };
  }

  // Check ordering: the matched results should appear in the same order as the entities array
  const actualOrder = foundIds;
  const orderCorrect = expectedIds.every((id, i) => actualOrder[i] === id);

  if (!orderCorrect) {
    const expectedNames = expectedIds.map((id, i) => `${i + 1}. ${testCase.entities[i].name}`);
    const actualNames = actualOrder.map((id, i) => {
      const idx = expectedIds.indexOf(id);
      return `${i + 1}. ${testCase.entities[idx].name}`;
    });
    return {
      name: testCase.name,
      passed: false,
      message: [
        `Order mismatch for query "${testCase.query}"`,
        `  Expected: ${expectedNames.join(' > ')}`,
        `  Actual:   ${actualNames.join(' > ')}`,
      ].join('\n'),
    };
  }

  return {
    name: testCase.name,
    passed: true,
    message: `Query "${testCase.query}" — ${expectedIds.length} entities in correct order`,
  };
}

// ---------- Main ----------

async function checkApiAvailability(url: string): Promise<boolean> {
  try {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 2000);
    const response = await fetch(`${url}/search/health`, { signal: controller.signal });
    clearTimeout(timeoutId);
    return response.ok;
  } catch {
    return false;
  }
}

async function main() {
  console.log(`${BLUE}🔍 Search Ranking Validation Tests${NC}\n`);

  // Load test cases from shared JSON config
  const __dirname = dirname(fileURLToPath(import.meta.url));
  const configPath = join(__dirname, '..', 'ranking-tests', 'test-cases.json');
  const suite: RankingTestSuite = JSON.parse(readFileSync(configPath, 'utf-8'));

  console.log(`Loaded ${suite.test_cases.length} ranking test cases from test-cases.json`);
  console.log(`UUID prefix: ${suite.uuid_prefix}`);
  console.log(`Space ID: ${suite.space_id}`);
  console.log(`Uniform score: ${suite.uniform_score}\n`);

  const apiUrl = process.env.SEARCH_API_URL || 'http://localhost:3000';

  // Check API availability
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

  console.log(`\n${BLUE}📊 Running ranking validation tests...${NC}\n`);

  const results: TestResult[] = [];

  for (let i = 0; i < suite.test_cases.length; i++) {
    const testCase = suite.test_cases[i];
    const result = await validateTestCase(apiUrl, suite, i, testCase);
    results.push(result);

    const icon = result.passed ? `${GREEN}✓${NC}` : `${RED}✗${NC}`;
    console.log(`${icon} Test ${i + 1}: ${result.name}`);
    if (result.passed) {
      console.log(`  ${result.message}`);
    } else {
      console.log(`  ${RED}${result.message}${NC}`);
    }
  }

  // Summary
  const passed = results.filter(r => r.passed).length;
  const failed = results.filter(r => !r.passed).length;

  console.log(`\n${'─'.repeat(60)}`);
  console.log(`${BLUE}Ranking Test Summary${NC}`);
  console.log(`  Total:  ${results.length}`);
  console.log(`  ${GREEN}Passed: ${passed}${NC}`);
  if (failed > 0) {
    console.log(`  ${RED}Failed: ${failed}${NC}`);
  }
  console.log(`${'─'.repeat(60)}`);

  process.exit(failed > 0 ? 1 : 0);
}

main();
