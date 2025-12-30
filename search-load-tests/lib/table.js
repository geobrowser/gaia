/**
 * Simple table printing utilities for k6 console output.
 */

// Inner width (between the │ characters, not including them)
const INNER_WIDTH = 65;

/**
 * Create a horizontal line
 */
function line(char = '─', left = '├', right = '┤') {
  return left + char.repeat(INNER_WIDTH) + right;
}

/**
 * Create a row with content (left-aligned with padding)
 * Format: │ content...padding │
 */
function row(content) {
  // We have INNER_WIDTH chars between │ and │
  // Format: " " + content + padding + " " = INNER_WIDTH
  // So content can be at most INNER_WIDTH - 2
  const maxContent = INNER_WIDTH - 2;
  const truncated = content.length > maxContent ? content.substring(0, maxContent) : content;
  const padding = INNER_WIDTH - 2 - truncated.length - 1;
  return '│ ' + truncated + ' '.repeat(padding) + ' │';
}

/**
 * Create a title row (centered)
 */
function title(text) {
  // Center the text within INNER_WIDTH
  const truncated = text.length > INNER_WIDTH ? text.substring(0, INNER_WIDTH) : text;
  const padding = INNER_WIDTH - truncated.length;
  const leftPad = Math.floor(padding / 2);
  const rightPad = padding - leftPad - 1;
  return '│' + ' '.repeat(leftPad) + truncated + ' '.repeat(rightPad) + '│';
}

/**
 * Print a formatted table
 * @param {string} titleText - Table title
 * @param {Array} sections - Array of { header?: string, rows: Array<{label, value}> }
 */
export function printTable(titleText, sections) {
  const lines = [];
  
  // Top border
  lines.push(line('─', '┌', '┐'));
  
  // Title
  lines.push(title(titleText));
  
  // Sections
  sections.forEach((section, sectionIndex) => {
    // Section separator
    lines.push(line('─', '├', '┤'));
    
    // Section header if present
    if (section.header) {
      lines.push(row(section.header));
    }
    
    // Rows
    section.rows.forEach(r => {
      if (typeof r === 'string') {
        lines.push(row(r));
      } else if (r.label && r.value !== undefined) {
        const formatted = `  ${r.label.padEnd(18)} ${String(r.value)}`;
        lines.push(row(formatted));
      }
    });
  });
  
  // Bottom border
  lines.push(line('─', '└', '┘'));
  
  return lines.join('\n');
}

/**
 * Print test configuration
 */
export function printTestConfig(config) {
  return printTable('TEST CONFIGURATION', [
    {
      rows: [
        { label: 'API URL:', value: config.apiUrl || 'N/A' },
        { label: 'Kafka Brokers:', value: config.kafkaBrokers || 'N/A' },
        { label: 'Kafka Topic:', value: config.kafkaTopic || 'N/A' },
        { label: 'HTTP RPS:', value: config.httpRps || 'N/A' },
        { label: 'Kafka EPS:', value: config.kafkaEps || 'N/A' },
        { label: 'Duration:', value: config.duration || 'N/A' },
        { label: 'Debug Mode:', value: config.debug ? 'enabled' : 'disabled' },
      ],
    },
  ]);
}

/**
 * Print query configuration
 */
export function printQueryConfig(queryConfig) {
  const typeRows = queryConfig.types.map((type, i) => ({
    label: type + ':',
    value: (queryConfig.weights[i] * 100).toFixed(0) + '%',
  }));
  
  return printTable('QUERY CONFIGURATION', [
    {
      header: 'Query Types & Weights:',
      rows: typeRows,
    },
    {
      header: 'Word Pool Sizes:',
      rows: [
        { label: 'Simple words:', value: queryConfig.wordPools.simpleWords },
        { label: 'Domain words:', value: queryConfig.wordPools.domainWords },
        { label: 'Typo words:', value: queryConfig.wordPools.typoWords },
        { label: 'Edge cases:', value: queryConfig.wordPools.edgeCases },
      ],
    },
  ]);
}

/**
 * Print document configuration
 */
export function printDocumentConfig(docConfig) {
  return printTable('DOCUMENT CONFIGURATION', [
    {
      header: 'Name Lengths:',
      rows: [
        { label: 'Short:', value: `${docConfig.nameLengths.short.range} (${docConfig.nameLengths.short.weight})` },
        { label: 'Medium:', value: `${docConfig.nameLengths.medium.range} (${docConfig.nameLengths.medium.weight})` },
        { label: 'Long:', value: `${docConfig.nameLengths.long.range} (${docConfig.nameLengths.long.weight})` },
      ],
    },
    {
      header: 'Description Lengths:',
      rows: [
        { label: 'None:', value: `${docConfig.descLengths.none.range} (${docConfig.descLengths.none.weight})` },
        { label: 'Short:', value: `${docConfig.descLengths.short.range} (${docConfig.descLengths.short.weight})` },
        { label: 'Medium:', value: `${docConfig.descLengths.medium.range} (${docConfig.descLengths.medium.weight})` },
        { label: 'Long:', value: `${docConfig.descLengths.long.range} (${docConfig.descLengths.long.weight})` },
      ],
    },
    {
      header: 'Word Pool Sizes:',
      rows: [
        { label: 'Nouns:', value: docConfig.wordPools.nouns },
        { label: 'Adjectives:', value: docConfig.wordPools.adjectives },
        { label: 'Verbs:', value: docConfig.wordPools.verbs },
        { label: 'Domain phrases:', value: docConfig.wordPools.domainPhrases },
      ],
    },
  ]);
}

