/**
 * Load test profiles for different environments and intensities.
 */

export const HTTP_PROFILES = {
  // Light load - development testing
  light: {
    rps: 10,
    duration: '1m',
    preAllocatedVUs: 10,
    maxVUs: 50,
  },
  // Moderate load - integration testing
  moderate: {
    rps: 100,
    duration: '5m',
    preAllocatedVUs: 50,
    maxVUs: 200,
  },
  // Heavy load - staging/pre-prod testing
  heavy: {
    rps: 500,
    duration: '10m',
    preAllocatedVUs: 100,
    maxVUs: 500,
  },
  // Stress test - find breaking points
  stress: {
    rps: 1000,
    duration: '15m',
    preAllocatedVUs: 200,
    maxVUs: 1000,
  },
};

export const KAFKA_PROFILES = {
  // Light load - development testing
  light: {
    eps: 50, // events per second
    duration: '1m',
    preAllocatedVUs: 10,
    maxVUs: 50,
  },
  // Moderate load - integration testing
  moderate: {
    eps: 200,
    duration: '5m',
    preAllocatedVUs: 20,
    maxVUs: 100,
  },
  // Heavy load - staging/pre-prod testing
  heavy: {
    eps: 1000,
    duration: '10m',
    preAllocatedVUs: 50,
    maxVUs: 200,
  },
  // Stress test - find breaking points
  stress: {
    eps: 5000,
    duration: '15m',
    preAllocatedVUs: 100,
    maxVUs: 500,
  },
};

// Index size targets for seeding
export const INDEX_SIZES = {
  small: 10_000,
  medium: 100_000,
  large: 1_000_000,
  xlarge: 10_000_000,
};

// Thresholds for pass/fail criteria
export const THRESHOLDS = {
  http: {
    'http_req_duration': ['p(95)<500', 'p(99)<1000'],
    'http_error_rate': ['rate<0.01'],
    'search_latency': ['p(95)<300'],
  },
  kafka: {
    'produce_latency': ['p(95)<100', 'p(99)<200'],
    'kafka_error_rate': ['rate<0.001'],
  },
};

