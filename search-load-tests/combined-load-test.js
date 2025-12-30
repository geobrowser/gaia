/**
 * Combined HTTP + Kafka Load Test
 * 
 * Runs both HTTP search queries and Kafka event production simultaneously
 * to test the system under realistic mixed load conditions.
 * 
 * Environment variables:
 *   - API_URL: Base URL of the search API (default: http://localhost:3000)
 *   - KAFKA_BROKERS: Comma-separated list of Kafka brokers (default: localhost:9092)
 *   - KAFKA_TOPIC: Topic to produce to (default: knowledge.edits)
 *   - HTTP_RPS: HTTP requests per second (default: 100)
 *   - KAFKA_EPS: Kafka events per second (default: 200)
 *   - DURATION: Test duration (default: 5m)
 * 
 * Usage:
 *   k6 run combined-load-test.js \
 *     -e API_URL=http://localhost:3000 \
 *     -e KAFKA_BROKERS=localhost:9092 \
 *     -e HTTP_RPS=500 \
 *     -e KAFKA_EPS=1000 \
 *     -e DURATION=10m
 */

import http from 'k6/http';
import { Writer, CODEC_SNAPPY, createTopic } from 'k6/x/kafka';
import { check, sleep } from 'k6';
import { Rate, Trend, Counter } from 'k6/metrics';
import { generateQuery, getQueryConfig } from './lib/queries.js';
import { generateDocument, getDocumentConfig } from './lib/documents.js';
import { printTestConfig, printQueryConfig, printDocumentConfig } from './lib/table.js';
import { createTestHermesEdit } from './lib/protobuf.js';
import { THRESHOLDS } from './config/profiles.js';

// HTTP Metrics
const httpErrorRate = new Rate('http_error_rate');
const searchLatency = new Trend('search_latency', true);
const searchRequests = new Counter('search_requests');

// Error tracking by type
const http4xxErrors = new Counter('http_4xx_errors');
const http5xxErrors = new Counter('http_5xx_errors');
const httpConnectionErrors = new Counter('http_connection_errors');
const httpTimeoutErrors = new Counter('http_timeout_errors');

// Kafka Metrics
const kafkaErrorRate = new Rate('kafka_error_rate');
const produceLatency = new Trend('produce_latency', true);
const eventsProduced = new Counter('events_produced');

// Configuration
const API_URL = __ENV.API_URL || 'http://localhost:3000';
const KAFKA_BROKERS = (__ENV.KAFKA_BROKERS || 'localhost:9092').split(',');
const KAFKA_TOPIC = __ENV.KAFKA_TOPIC || 'knowledge.edits';
const HTTP_RPS = parseInt(__ENV.HTTP_RPS || '100');
const KAFKA_EPS = parseInt(__ENV.KAFKA_EPS || '200');
const DURATION = __ENV.DURATION || '5m';

// Kafka writer config
const writerConfig = {
  brokers: KAFKA_BROKERS,
  topic: KAFKA_TOPIC,
  compression: CODEC_SNAPPY,
  autoCreateTopic: true,
};

let writer = null;

// k6 options with dual scenarios
export const options = {
  scenarios: {
    // HTTP search load
    http_queries: {
      executor: 'constant-arrival-rate',
      exec: 'httpSearch',
      rate: HTTP_RPS,
      timeUnit: '1s',
      duration: DURATION,
      preAllocatedVUs: Math.min(50, HTTP_RPS),
      maxVUs: Math.max(200, HTTP_RPS * 2),
    },
    // Kafka ingestion load
    kafka_ingest: {
      executor: 'constant-arrival-rate',
      exec: 'kafkaProduce',
      rate: KAFKA_EPS,
      timeUnit: '1s',
      duration: DURATION,
      preAllocatedVUs: Math.min(20, Math.ceil(KAFKA_EPS / 10)),
      maxVUs: Math.max(100, KAFKA_EPS),
    },
  },
  thresholds: {
    // Combined thresholds
    ...THRESHOLDS.http,
    ...THRESHOLDS.kafka,
  },
};

// Setup function
export function setup() {
  // Print test configuration
  console.log('\n' + printTestConfig({
    apiUrl: API_URL,
    kafkaBrokers: KAFKA_BROKERS.join(','),
    kafkaTopic: KAFKA_TOPIC,
    httpRps: HTTP_RPS,
    kafkaEps: KAFKA_EPS,
    duration: DURATION,
    debug: __ENV.DEBUG,
  }));

  // Print query configuration
  const queryConfig = getQueryConfig();
  console.log('\n' + printQueryConfig(queryConfig));

  // Print document configuration
  const docConfig = getDocumentConfig();
  console.log('\n' + printDocumentConfig(docConfig));

  // Create Kafka topic if it doesn't exist
  try {
    createTopic({
      address: KAFKA_BROKERS[0],
      topic: KAFKA_TOPIC,
      numPartitions: 3,
      replicationFactor: 1,
    });
    console.log(`Topic '${KAFKA_TOPIC}' created or already exists`);
    sleep(3);
  } catch (e) {
    console.log(`Topic creation: ${e}`);
    sleep(2);
  }

  return { startTime: Date.now() };
}

/**
 * HTTP Search scenario
 */
export function httpSearch() {
  const { type, query } = generateQuery();
  const url = `${API_URL}/search?q=${encodeURIComponent(query)}`;
  
  // Debug log for each query
  if (__ENV.DEBUG) {
    console.log(`[DEBUG] Search query: "${query}" (type: ${type})`);
  }

  const start = Date.now();
  const res = http.get(url, {
    headers: {
      'Content-Type': 'application/json',
      'Accept': 'application/json',
    },
    tags: {
      scenario: 'http',
      query_type: type,
    },
  });
  const duration = Date.now() - start;

  searchLatency.add(duration);
  searchRequests.add(1);

  const success = check(res, {
    'http: status 200': (r) => r.status === 200,
    'http: valid json': (r) => {
      try {
        JSON.parse(r.body);
        return true;
      } catch {
        return false;
      }
    },
  });

  httpErrorRate.add(!success);

  // Track and log details for failed requests
  if (!success) {
    // Categorize errors
    if (res.status === 0) {
      // Connection error or timeout
      if (res.error && res.error.includes('timeout')) {
        httpTimeoutErrors.add(1);
      } else {
        httpConnectionErrors.add(1);
      }
    } else if (res.status >= 400 && res.status < 500) {
      http4xxErrors.add(1);
    } else if (res.status >= 500) {
      http5xxErrors.add(1);
    }

    // Log error details
    const errorInfo = {
      status: res.status,
      query: query,
      queryType: type,
      error: res.error || 'none',
      body: res.body ? res.body.substring(0, 200) : 'empty',
      duration: duration,
    };
    console.error(`HTTP Error: ${JSON.stringify(errorInfo)}`);
  }
}

/**
 * Kafka Produce scenario
 */
export function kafkaProduce() {
  // Initialize writer per VU
  if (!writer) {
    writer = new Writer(writerConfig);
  }

  const doc = generateDocument();
  
  // Debug log for each document
  if (__ENV.DEBUG) {
    console.log(`[DEBUG] Document: name="${doc.name.substring(0, 50)}..." desc="${doc.description ? doc.description.substring(0, 50) + '...' : 'null'}"`);
  }
  
  const { bytes, entityId } = createTestHermesEdit(doc.name, doc.description);

  // Convert Uint8Array to regular array for xk6-kafka
  const byteArray = Array.from(bytes);

  const start = Date.now();
  try {
    writer.produce({
      messages: [
        {
          key: entityId.replace(/-/g, ''),
          value: byteArray,
        },
      ],
    });

    const duration = Date.now() - start;
    produceLatency.add(duration);
    eventsProduced.add(1);

    check(null, {
      'kafka: produce successful': () => true,
    });
  } catch (e) {
    kafkaErrorRate.add(1);
    console.error(`Kafka produce error: ${e}`);

    check(null, {
      'kafka: produce successful': () => false,
    });
  }
}

// Teardown function
export function teardown(data) {
  if (writer) {
    writer.close();
  }

  const totalTime = (Date.now() - data.startTime) / 1000;
  console.log(`
╔════════════════════════════════════════════════════════════════╗
║              Combined Test Complete                            ║
╠════════════════════════════════════════════════════════════════╣
║ Total Duration: ${totalTime.toFixed(2)}s ${" ".repeat(45 - totalTime.toFixed(2).length)}║
╚════════════════════════════════════════════════════════════════╝
  `);
}

