/**
 * Query generation for HTTP search load testing.
 * Simulates realistic search query patterns with various types.
 */

// Word pools for generating realistic queries
const SIMPLE_WORDS = [
  'blockchain', 'protocol', 'network', 'entity', 'data',
  'web3', 'crypto', 'token', 'smart', 'contract',
  'decentralized', 'distributed', 'consensus', 'node', 'chain',
];

const DOMAIN_WORDS = [
  'knowledge', 'graph', 'semantic', 'ontology', 'relation',
  'property', 'type', 'schema', 'attribute', 'value',
  'space', 'community', 'governance', 'proposal', 'vote',
];

const TYPO_WORDS = [
  'blockchan', 'protocl', 'netwerk', 'entitiy', 'descrption',
  'blokchain', 'protcol', 'netwrok', 'entiyt', 'descripion',
  'bockchain', 'protoocl', 'newtork', 'etnity', 'descritpion',
];

// Edge cases that are still VALID queries (should return 200)
// Removed: empty strings, whitespace-only, single char (API requires min 2 chars)
const EDGE_CASES = [
  'ab',                  // minimum valid length (2 chars)
  'abc',                 // three chars
  'αβγδε',               // unicode greek
  '中文测试',             // unicode chinese
  '日本語テスト',         // unicode japanese
  '"quoted string"',     // quotes
  "it's a test",         // apostrophe
  'test-with-dashes',    // dashes
  'test_with_underscores', // underscores
  'CamelCaseQuery',      // mixed case
  'ALLCAPS',             // all caps
  'very long query with many words to test longer search input handling', // long query
];

/**
 * Pick a random element from an array
 */
function pickRandom(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

/**
 * Pick based on weighted distribution
 * @param {Array} options - Array of options
 * @param {Array} weights - Array of weights (should sum to 1)
 */
function pickWeighted(options, weights) {
  const r = Math.random();
  let cumulative = 0;
  for (let i = 0; i < options.length; i++) {
    cumulative += weights[i];
    if (r < cumulative) return options[i];
  }
  return options[options.length - 1];
}

/**
 * Query type generators
 */
export const QUERY_TYPES = {
  // Simple single-word queries
  simple: () => pickRandom([...SIMPLE_WORDS, ...DOMAIN_WORDS]),

  // Multi-word queries (2-3 words)
  multiWord: () => {
    const allWords = [...SIMPLE_WORDS, ...DOMAIN_WORDS];
    const count = Math.random() > 0.5 ? 2 : 3;
    const words = [];
    for (let i = 0; i < count; i++) {
      words.push(pickRandom(allWords));
    }
    return words.join(' ');
  },

  // Queries with intentional typos (tests fuzzy matching)
  typos: () => pickRandom(TYPO_WORDS),

  // Longer phrase queries (4-6 words)
  long: () => {
    const allWords = [...SIMPLE_WORDS, ...DOMAIN_WORDS];
    const count = 4 + Math.floor(Math.random() * 3);
    const words = [];
    for (let i = 0; i < count; i++) {
      words.push(pickRandom(allWords));
    }
    return words.join(' ');
  },

  // Edge cases and special inputs
  edge: () => pickRandom(EDGE_CASES),

  // Prefix queries (partial words)
  prefix: () => {
    const word = pickRandom([...SIMPLE_WORDS, ...DOMAIN_WORDS]);
    const prefixLen = Math.floor(word.length * (0.3 + Math.random() * 0.4));
    return word.substring(0, Math.max(2, prefixLen));
  },
};

/**
 * Generate a query with its type for metrics tagging
 * Distribution:
 *   - 35% simple single-word
 *   - 25% multi-word
 *   - 15% typos (fuzzy)
 *   - 10% long phrases
 *   - 10% edge cases
 *   - 5% prefix queries
 */
export function generateQuery() {
  const types = ['simple', 'multiWord', 'typos', 'long', 'edge', 'prefix'];
  const weights = [0.35, 0.25, 0.15, 0.10, 0.10, 0.05];

  const type = pickWeighted(types, weights);
  return {
    type,
    query: QUERY_TYPES[type](),
  };
}

/**
 * Generate a batch of queries (useful for pre-generating test data)
 */
export function generateQueryBatch(count) {
  const queries = [];
  for (let i = 0; i < count; i++) {
    queries.push(generateQuery());
  }
  return queries;
}

/**
 * Get query configuration for logging
 */
export function getQueryConfig() {
  return {
    types: ['simple', 'multiWord', 'typos', 'long', 'edge', 'prefix'],
    weights: [0.35, 0.25, 0.15, 0.10, 0.10, 0.05],
    wordPools: {
      simpleWords: SIMPLE_WORDS.length,
      domainWords: DOMAIN_WORDS.length,
      typoWords: TYPO_WORDS.length,
      edgeCases: EDGE_CASES.length,
    },
  };
}

