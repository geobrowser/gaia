/**
 * Kafka Load Test for Search Indexer
 * 
 * Produces HermesEdit messages to Kafka to test the search indexer ingestion pipeline.
 * 
 * Environment variables:
 *   - KAFKA_BROKERS: Comma-separated list of Kafka brokers (default: localhost:9092)
 *   - KAFKA_TOPIC: Topic to produce to (default: knowledge.edits)
 *   - KAFKA_PROFILE: Load profile - light, moderate, heavy, stress (default: moderate)
 *   - KAFKA_USERNAME: Optional SASL username
 *   - KAFKA_PASSWORD: Optional SASL password
 * 
 * Usage:
 *   k6 run kafka-load-test.js -e KAFKA_BROKERS=localhost:9092 -e KAFKA_PROFILE=moderate
 * 
 * Requires k6 with xk6-kafka extension:
 *   xk6 build --with github.com/mostafa/xk6-kafka@latest
 */

import { Writer, Connection, CODEC_SNAPPY, createTopic } from 'k6/x/kafka';
import { sleep } from 'k6';
import { Rate, Trend, Counter } from 'k6/metrics';
import { check } from 'k6';
import { generateDocument } from './lib/documents.js';
import { createTestHermesEdit } from './lib/protobuf.js';
import { KAFKA_PROFILES, THRESHOLDS } from './config/profiles.js';

// Custom metrics
const eventsProduced = new Counter('events_produced');
const produceLatency = new Trend('produce_latency', true);
const produceErrors = new Rate('kafka_error_rate');
const bytesProduced = new Counter('bytes_produced');

// Document size metrics
const docWithDesc = new Counter('docs_with_description');
const docWithoutDesc = new Counter('docs_without_description');
const avgNameLength = new Trend('avg_name_length');
const avgDescLength = new Trend('avg_desc_length');

// Configuration
const BROKERS = (__ENV.KAFKA_BROKERS || 'localhost:9092').split(',');
const TOPIC = __ENV.KAFKA_TOPIC || 'knowledge.edits';
const PROFILE_NAME = __ENV.KAFKA_PROFILE || 'moderate';
const KAFKA_USERNAME = __ENV.KAFKA_USERNAME || null;
const KAFKA_PASSWORD = __ENV.KAFKA_PASSWORD || null;

// Get profile configuration
const profile = KAFKA_PROFILES[PROFILE_NAME];
if (!profile) {
  throw new Error(`Unknown profile: ${PROFILE_NAME}. Available: ${Object.keys(KAFKA_PROFILES).join(', ')}`);
}

// Kafka writer configuration
const writerConfig = {
  brokers: BROKERS,
  topic: TOPIC,
  compression: CODEC_SNAPPY,
  autoCreateTopic: true,
};

// Add SASL config if credentials provided
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

// k6 options
export const options = {
  scenarios: {
    kafka_producer: {
      executor: 'constant-arrival-rate',
      rate: profile.eps,
      timeUnit: '1s',
      duration: profile.duration,
      preAllocatedVUs: profile.preAllocatedVUs,
      maxVUs: profile.maxVUs,
    },
  },
  thresholds: THRESHOLDS.kafka,
};

// Setup function - runs once before the test
export function setup() {
  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Kafka Ingestion Load Test                         ║
╠════════════════════════════════════════════════════════════════╣
║ Brokers:     ${BROKERS.join(',').substring(0, 46).padEnd(46)}║
║ Topic:       ${TOPIC.padEnd(46)}║
║ Profile:     ${PROFILE_NAME.padEnd(46)}║
║ EPS:         ${String(profile.eps).padEnd(46)}║
║ Duration:    ${profile.duration.padEnd(46)}║
║ SASL:        ${(KAFKA_USERNAME ? 'enabled' : 'disabled').padEnd(46)}║
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
    
    // Wait for metadata propagation
    console.log('Waiting for metadata propagation...');
    sleep(3);
  } catch (e) {
    // Topic might already exist, which is fine
    console.log(`Topic creation: ${e}`);
    sleep(2);
  }

  return { startTime: Date.now() };
}

// Initialize writer per VU
export function handleSummary(data) {
  return {
    stdout: JSON.stringify(data, null, 2),
  };
}

// Main test function - runs for each virtual user iteration
export default function (data) {
  // Initialize writer if not already done (per VU)
  if (!writer) {
    writer = new Writer(writerConfig);
  }

  // Generate document
  const doc = generateDocument();

  // Track document characteristics
  avgNameLength.add(doc.name.length);
  if (doc.description) {
    docWithDesc.add(1);
    avgDescLength.add(doc.description.length);
  } else {
    docWithoutDesc.add(1);
  }

  // Create HermesEdit message
  const { bytes, entityId } = createTestHermesEdit(doc.name, doc.description);

  // Produce to Kafka
  // Note: xk6-kafka has issues with binary data, so we need to handle bytes carefully
  const start = Date.now();
  try {
    // Convert Uint8Array to regular array for xk6-kafka
    const byteArray = Array.from(bytes);
    
    writer.produce({
      messages: [
        {
          // Use simple string key (no special characters that might be misinterpreted)
          key: entityId.replace(/-/g, ''),
          // Pass as array of numbers
          value: byteArray,
        },
      ],
    });

    const duration = Date.now() - start;

    // Record metrics
    eventsProduced.add(1);
    produceLatency.add(duration);
    bytesProduced.add(bytes.length);

    check(null, {
      'produce successful': () => true,
      'produce latency < 100ms': () => duration < 100,
    });
  } catch (e) {
    produceErrors.add(1);
    console.error(`Produce error: ${e}`);

    check(null, {
      'produce successful': () => false,
    });
  }
}

// Teardown function - runs once after the test
export function teardown(data) {
  if (writer) {
    writer.close();
  }

  const totalTime = (Date.now() - data.startTime) / 1000;
  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Test Complete                                     ║
╠════════════════════════════════════════════════════════════════╣
║ Total Duration: ${totalTime.toFixed(2).padEnd(44)}s║
╚════════════════════════════════════════════════════════════════╝
  `);
}

