/**
 * Document generation utilities for load testing.
 * Generates realistic entity names and descriptions with varying lengths.
 */

// Word pools for generating realistic content
const NOUNS = [
  'blockchain', 'protocol', 'network', 'entity', 'system',
  'platform', 'service', 'application', 'framework', 'module',
  'component', 'interface', 'database', 'storage', 'layer',
  'node', 'cluster', 'instance', 'container', 'process',
  'knowledge', 'graph', 'ontology', 'schema', 'model',
  'space', 'community', 'organization', 'governance', 'proposal',
];

const ADJECTIVES = [
  'decentralized', 'distributed', 'semantic', 'scalable', 'efficient',
  'secure', 'reliable', 'performant', 'modular', 'extensible',
  'interoperable', 'autonomous', 'federated', 'cryptographic', 'verifiable',
  'immutable', 'transparent', 'permissionless', 'trustless', 'censorship-resistant',
];

const VERBS = [
  'enables', 'provides', 'supports', 'implements', 'manages',
  'processes', 'stores', 'transfers', 'validates', 'verifies',
  'orchestrates', 'coordinates', 'synchronizes', 'aggregates', 'transforms',
];

const DOMAIN_PHRASES = [
  'knowledge graph infrastructure',
  'decentralized identity management',
  'semantic data indexing',
  'distributed consensus mechanism',
  'cryptographic proof verification',
  'smart contract execution',
  'cross-chain interoperability',
  'on-chain governance framework',
  'token economics model',
  'peer-to-peer networking',
];

/**
 * Pick a random element from an array
 */
function pickRandom(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

/**
 * Pick N random unique elements from an array
 */
function pickRandomN(arr, n) {
  const shuffled = [...arr].sort(() => Math.random() - 0.5);
  return shuffled.slice(0, Math.min(n, arr.length));
}

/**
 * Pick based on weighted distribution
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
 * Generate a realistic entity name
 * @param {number} targetLength - Approximate target length
 * @returns {string} Generated name
 */
export function generateName(targetLength = 30) {
  let name = '';

  if (targetLength < 15) {
    // Short names: just a noun or adjective + noun
    name = Math.random() > 0.5 ? pickRandom(NOUNS) : `${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)}`;
  } else if (targetLength < 50) {
    // Medium names: adjective + noun + noun or phrase
    const patterns = [
      () => `${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)} ${pickRandom(NOUNS)}`,
      () => `${pickRandom(NOUNS)} for ${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)}`,
      () => pickRandom(DOMAIN_PHRASES),
    ];
    name = pickRandom(patterns)();
  } else {
    // Long names: multiple words or phrases
    const parts = [];
    while (parts.join(' ').length < targetLength) {
      if (Math.random() > 0.5) {
        parts.push(`${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)}`);
      } else {
        parts.push(pickRandom(DOMAIN_PHRASES));
      }
    }
    name = parts.join(' - ');
  }

  // Trim to approximate target length
  if (name.length > targetLength * 1.2) {
    name = name.substring(0, targetLength);
    // Clean up partial words
    const lastSpace = name.lastIndexOf(' ');
    if (lastSpace > targetLength * 0.7) {
      name = name.substring(0, lastSpace);
    }
  }

  return name;
}

/**
 * Generate a realistic entity description
 * @param {number} targetLength - Approximate target length
 * @returns {string} Generated description
 */
export function generateDescription(targetLength = 200) {
  const sentences = [];
  let currentLength = 0;

  while (currentLength < targetLength) {
    const sentence = generateSentence();
    sentences.push(sentence);
    currentLength += sentence.length + 1; // +1 for space
  }

  let description = sentences.join(' ');

  // Trim to approximate target length
  if (description.length > targetLength * 1.1) {
    description = description.substring(0, targetLength);
    // Clean up at sentence boundary if possible
    const lastPeriod = description.lastIndexOf('.');
    if (lastPeriod > targetLength * 0.8) {
      description = description.substring(0, lastPeriod + 1);
    }
  }

  return description;
}

/**
 * Generate a single sentence
 */
function generateSentence() {
  const patterns = [
    // "This [noun] [verb] [adjective] [noun]."
    () =>
      `This ${pickRandom(NOUNS)} ${pickRandom(VERBS)} ${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)}.`,
    // "The [adjective] [noun] [verb] [noun] and [noun]."
    () =>
      `The ${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)} ${pickRandom(VERBS)} ${pickRandom(NOUNS)} and ${pickRandom(NOUNS)}.`,
    // "[Adjective] [noun] is essential for [noun]."
    () => `${capitalize(pickRandom(ADJECTIVES))} ${pickRandom(NOUNS)} is essential for ${pickRandom(NOUNS)}.`,
    // "It [verb] [domain phrase]."
    () => `It ${pickRandom(VERBS)} ${pickRandom(DOMAIN_PHRASES)}.`,
    // "[Noun] [verb] through [adjective] [noun]."
    () =>
      `${capitalize(pickRandom(NOUNS))} ${pickRandom(VERBS)} through ${pickRandom(ADJECTIVES)} ${pickRandom(NOUNS)}.`,
  ];

  return pickRandom(patterns)();
}

function capitalize(str) {
  return str.charAt(0).toUpperCase() + str.slice(1);
}

/**
 * Name length distribution configurations
 */
export const NAME_LENGTH_PROFILES = {
  short: { min: 5, max: 20 },
  medium: { min: 20, max: 100 },
  long: { min: 100, max: 300 },
};

/**
 * Description length distribution configurations
 */
export const DESC_LENGTH_PROFILES = {
  none: null, // No description
  short: { min: 10, max: 100 },
  medium: { min: 100, max: 500 },
  long: { min: 500, max: 2000 },
};

/**
 * Generate a document (name + optional description) with realistic distribution
 * Distribution:
 *   Names: 40% short, 40% medium, 20% long
 *   Descriptions: 20% none, 30% short, 30% medium, 20% long
 */
export function generateDocument() {
  // Select name length profile
  const nameLenProfile = pickWeighted(
    [NAME_LENGTH_PROFILES.short, NAME_LENGTH_PROFILES.medium, NAME_LENGTH_PROFILES.long],
    [0.4, 0.4, 0.2]
  );
  const nameLen = Math.floor(
    Math.random() * (nameLenProfile.max - nameLenProfile.min) + nameLenProfile.min
  );

  // Select description length profile
  const descLenProfile = pickWeighted(
    [DESC_LENGTH_PROFILES.none, DESC_LENGTH_PROFILES.short, DESC_LENGTH_PROFILES.medium, DESC_LENGTH_PROFILES.long],
    [0.2, 0.3, 0.3, 0.2]
  );

  let description = null;
  if (descLenProfile !== null) {
    const descLen = Math.floor(
      Math.random() * (descLenProfile.max - descLenProfile.min) + descLenProfile.min
    );
    description = generateDescription(descLen);
  }

  return {
    name: generateName(nameLen),
    description,
  };
}

/**
 * Generate a batch of documents
 */
export function generateDocumentBatch(count) {
  const docs = [];
  for (let i = 0; i < count; i++) {
    docs.push(generateDocument());
  }
  return docs;
}

/**
 * Get document configuration for logging
 */
export function getDocumentConfig() {
  return {
    nameLengths: {
      short: { range: '5-20 chars', weight: '40%' },
      medium: { range: '20-100 chars', weight: '40%' },
      long: { range: '100-300 chars', weight: '20%' },
    },
    descLengths: {
      none: { range: 'null', weight: '20%' },
      short: { range: '10-100 chars', weight: '30%' },
      medium: { range: '100-500 chars', weight: '30%' },
      long: { range: '500-2000 chars', weight: '20%' },
    },
    wordPools: {
      nouns: NOUNS.length,
      adjectives: ADJECTIVES.length,
      verbs: VERBS.length,
      domainPhrases: DOMAIN_PHRASES.length,
    },
  };
}

