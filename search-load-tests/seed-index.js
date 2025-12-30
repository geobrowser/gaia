/**
 * Index Seeding Script
 * 
 * Pre-populates the search index with documents via Kafka.
 * Run this before load testing to establish baseline index size.
 * 
 * Environment variables:
 *   - KAFKA_BROKERS: Comma-separated list of Kafka brokers (default: localhost:9092)
 *   - KAFKA_TOPIC: Topic to produce to (default: knowledge.edits)
 *   - TARGET_DOCS: Number of documents to seed (default: 10000)
 *   - BATCH_SIZE: Batch size for producing (default: 100)
 *   - KAFKA_USERNAME: Optional SASL username
 *   - KAFKA_PASSWORD: Optional SASL password
 * 
 * Usage:
 *   k6 run seed-index.js -e TARGET_DOCS=1000000 -e KAFKA_BROKERS=localhost:9092
 * 
 * Requires k6 with xk6-kafka extension
 */

import { Writer, Connection, CODEC_SNAPPY, createTopic } from 'k6/x/kafka';
import { sleep } from 'k6';
import { Counter, Trend, Rate } from 'k6/metrics';
import { generateDocument } from './lib/documents.js';
import { createTestHermesEdit } from './lib/protobuf.js';
import { INDEX_SIZES } from './config/profiles.js';

// Metrics
const docsSeeded = new Counter('docs_seeded');
const batchLatency = new Trend('batch_latency', true);
const seedErrors = new Rate('seed_errors');

// Configuration
const BROKERS = (__ENV.KAFKA_BROKERS || 'localhost:9092').split(',');
const TOPIC = __ENV.KAFKA_TOPIC || 'knowledge.edits';
const TARGET_DOCS = parseInt(__ENV.TARGET_DOCS || '10000');
const BATCH_SIZE = parseInt(__ENV.BATCH_SIZE || '100');
const KAFKA_USERNAME = __ENV.KAFKA_USERNAME || null;
const KAFKA_PASSWORD = __ENV.KAFKA_PASSWORD || null;

// Calculate iterations needed
const ITERATIONS = Math.ceil(TARGET_DOCS / BATCH_SIZE);

// Kafka writer configuration
const writerConfig = {
  brokers: BROKERS,
  topic: TOPIC,
  compression: CODEC_SNAPPY,
  autoCreateTopic: true,
};

if (KAFKA_USERNAME && KAFKA_PASSWORD) {
  writerConfig.sasl = {
    algorithm: 'plain',
    username: KAFKA_USERNAME,
    password: KAFKA_PASSWORD,
  };
  writerConfig.tls = {
    enableTls: true,
  };
}

let writer = null;
let totalSeeded = 0;

// k6 options - use shared iterations to control total work
export const options = {
  scenarios: {
    seeding: {
      executor: 'shared-iterations',
      vus: 10, // Use 10 parallel workers
      iterations: ITERATIONS,
      maxDuration: '2h', // Allow up to 2 hours for large indexes
    },
  },
  thresholds: {
    'seed_errors': ['rate<0.01'], // Less than 1% error rate
  },
};

// Setup function
export function setup() {
  // Find matching preset if any
  let preset = 'custom';
  for (const [name, size] of Object.entries(INDEX_SIZES)) {
    if (size === TARGET_DOCS) {
      preset = name;
      break;
    }
  }

  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Search Index Seeding                              ║
╠════════════════════════════════════════════════════════════════╣
║ Target Docs:  ${TARGET_DOCS.toLocaleString().padEnd(45)}║
║ Preset:       ${preset.padEnd(45)}║
║ Batch Size:   ${BATCH_SIZE.toLocaleString().padEnd(45)}║
║ Iterations:   ${ITERATIONS.toLocaleString().padEnd(45)}║
║ Brokers:      ${BROKERS.join(',').substring(0, 45).padEnd(45)}║
║ Topic:        ${TOPIC.padEnd(45)}║
╚════════════════════════════════════════════════════════════════╝
  `);

  // Create topic if it doesn't exist
  try {
    createTopic({
      address: BROKERS[0],
      topic: TOPIC,
      numPartitions: 3,
      replicationFactor: 1,
    });
    console.log(`Topic '${TOPIC}' created or already exists`);
    sleep(3);
  } catch (e) {
    console.log(`Topic creation: ${e}`);
    sleep(2);
  }

  return { startTime: Date.now(), targetDocs: TARGET_DOCS };
}

// Main function - each iteration seeds BATCH_SIZE documents
export default function (data) {
  // Initialize writer per VU
  if (!writer) {
    writer = new Writer(writerConfig);
  }

  const messages = [];
  const actualBatchSize = Math.min(BATCH_SIZE, data.targetDocs - totalSeeded);

  if (actualBatchSize <= 0) {
    return; // Already seeded enough
  }

  // Generate batch of documents
  for (let i = 0; i < actualBatchSize; i++) {
    const doc = generateDocument();
    const { bytes, entityId } = createTestHermesEdit(doc.name, doc.description);

    // Convert Uint8Array to regular array for xk6-kafka
    const byteArray = Array.from(bytes);

    messages.push({
      key: entityId.replace(/-/g, ''),
      value: byteArray,
    });
  }

  // Produce batch
  const start = Date.now();
  try {
    writer.produce({ messages });
    const duration = Date.now() - start;

    docsSeeded.add(messages.length);
    batchLatency.add(duration);
    totalSeeded += messages.length;

    // Progress logging (every 10 batches)
    if (totalSeeded % (BATCH_SIZE * 10) === 0) {
      const percent = ((totalSeeded / data.targetDocs) * 100).toFixed(1);
      console.log(`Progress: ${totalSeeded.toLocaleString()} / ${data.targetDocs.toLocaleString()} (${percent}%)`);
    }
  } catch (e) {
    seedErrors.add(1);
    console.error(`Batch produce error: ${e}`);
  }
}

// Teardown function
export function teardown(data) {
  if (writer) {
    writer.close();
  }

  const totalTime = (Date.now() - data.startTime) / 1000;
  const docsPerSecond = totalSeeded / totalTime;

  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Seeding Complete                                  ║
╠════════════════════════════════════════════════════════════════╣
║ Total Duration:   ${totalTime.toFixed(2).padEnd(42)}s║
║ Docs Seeded:      ${totalSeeded.toLocaleString().padEnd(42)}║
║ Throughput:       ${docsPerSecond.toFixed(0).padEnd(39)} docs/s║
╚════════════════════════════════════════════════════════════════╝

Note: Wait for the search indexer to process all messages before running load tests.
Check consumer lag to verify indexing is complete.
  `);
}

