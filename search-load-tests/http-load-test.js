/**
 * HTTP Load Test for Search API
 * 
 * Tests the search API endpoint with realistic query patterns.
 * 
 * Environment variables:
 *   - API_URL: Base URL of the search API (default: http://localhost:3000)
 *   - PROFILE: Load profile - light, moderate, heavy, stress (default: moderate)
 *   - TEST_SPACE_ID: Optional space ID for scoped searches
 * 
 * Usage:
 *   k6 run http-load-test.js -e API_URL=http://localhost:3000 -e PROFILE=moderate
 */

import http from 'k6/http';
import { check, sleep } from 'k6';
import { Rate, Trend, Counter } from 'k6/metrics';
import { generateQuery } from './lib/queries.js';
import { HTTP_PROFILES, THRESHOLDS } from './config/profiles.js';

// Custom metrics
const errorRate = new Rate('http_error_rate');
const searchLatency = new Trend('search_latency', true);
const searchRequests = new Counter('search_requests');
const searchByType = {
  simple: new Counter('search_simple'),
  multiWord: new Counter('search_multi_word'),
  typos: new Counter('search_typos'),
  long: new Counter('search_long'),
  edge: new Counter('search_edge'),
  prefix: new Counter('search_prefix'),
};

// Configuration
const BASE_URL = __ENV.API_URL || 'http://localhost:3000';
const PROFILE_NAME = __ENV.PROFILE || 'moderate';
const TEST_SPACE_ID = __ENV.TEST_SPACE_ID || null;
const SPACE_SCOPED_RATIO = parseFloat(__ENV.SPACE_SCOPED_RATIO || '0.3'); // 30% of queries are space-scoped

// Get profile configuration
const profile = HTTP_PROFILES[PROFILE_NAME];
if (!profile) {
  throw new Error(`Unknown profile: ${PROFILE_NAME}. Available: ${Object.keys(HTTP_PROFILES).join(', ')}`);
}

// k6 options
export const options = {
  scenarios: {
    search_load: {
      executor: 'constant-arrival-rate',
      rate: profile.rps,
      timeUnit: '1s',
      duration: profile.duration,
      preAllocatedVUs: profile.preAllocatedVUs,
      maxVUs: profile.maxVUs,
    },
  },
  thresholds: THRESHOLDS.http,
};

// Setup function - runs once before the test
export function setup() {
  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Search API HTTP Load Test                         ║
╠════════════════════════════════════════════════════════════════╣
║ API URL:     ${BASE_URL.padEnd(46)}║
║ Profile:     ${PROFILE_NAME.padEnd(46)}║
║ RPS:         ${String(profile.rps).padEnd(46)}║
║ Duration:    ${profile.duration.padEnd(46)}║
║ Space ID:    ${(TEST_SPACE_ID || 'random').padEnd(46)}║
╚════════════════════════════════════════════════════════════════╝
  `);

  // Verify API is reachable
  const healthCheck = http.get(`${BASE_URL}/health`, { timeout: '5s' });
  if (healthCheck.status !== 200) {
    console.warn(`Warning: Health check returned status ${healthCheck.status}`);
  }

  return { startTime: Date.now() };
}

// Main test function - runs for each virtual user iteration
export default function (data) {
  // Generate query
  const { type, query } = generateQuery();

  // Build URL with query parameters
  const params = new URLSearchParams();
  params.append('q', query);

  // Optionally add space_id scope
  if (TEST_SPACE_ID && Math.random() < SPACE_SCOPED_RATIO) {
    params.append('space_id', TEST_SPACE_ID);
  }

  const url = `${BASE_URL}/search?${params.toString()}`;

  // Make request
  const start = Date.now();
  const res = http.get(url, {
    headers: {
      'Content-Type': 'application/json',
      'Accept': 'application/json',
    },
    tags: {
      query_type: type,
      name: 'search',
    },
  });
  const duration = Date.now() - start;

  // Record metrics
  searchLatency.add(duration);
  searchRequests.add(1);

  // Track by query type
  if (searchByType[type]) {
    searchByType[type].add(1);
  }

  // Validate response
  const success = check(res, {
    'status is 200': (r) => r.status === 200,
    'response is JSON': (r) => {
      try {
        JSON.parse(r.body);
        return true;
      } catch {
        return false;
      }
    },
    'response has results array': (r) => {
      try {
        const body = JSON.parse(r.body);
        return Array.isArray(body.results) || Array.isArray(body.hits) || Array.isArray(body.data);
      } catch {
        return false;
      }
    },
    'response time < 500ms': (r) => r.timings.duration < 500,
  });

  errorRate.add(!success);

  // Log slow requests
  if (duration > 1000) {
    console.log(`Slow request: ${duration}ms for query "${query}" (type: ${type})`);
  }

  // Log errors
  if (res.status !== 200) {
    console.log(`Error: ${res.status} for query "${query}" - ${res.body?.substring(0, 200)}`);
  }
}

// Teardown function - runs once after the test
export function teardown(data) {
  const totalTime = (Date.now() - data.startTime) / 1000;
  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Test Complete                                     ║
╠════════════════════════════════════════════════════════════════╣
║ Total Duration: ${totalTime.toFixed(2).padEnd(44)}s║
╚════════════════════════════════════════════════════════════════╝
  `);
}

