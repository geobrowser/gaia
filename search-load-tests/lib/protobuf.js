/**
 * Protobuf encoding utilities for HermesEdit messages.
 * 
 * This module provides utilities to encode HermesEdit protobuf messages
 * compatible with the search-indexer consumer.
 * 
 * Message structure (from hermes-schema/proto/knowledge.proto):
 * 
 * message HermesEdit {
 *   bytes id = 1;
 *   string name = 2;
 *   repeated grc20.Op ops = 3;
 *   repeated bytes authors = 4;
 *   optional bytes language = 5;
 *   bytes space_id = 6;
 *   bool is_canonical = 7;
 *   blockchain_metadata.BlockchainMetadata meta = 8;
 * }
 */

// k6 doesn't have TextEncoder, so we implement UTF-8 encoding manually
function stringToUtf8Bytes(str) {
  const bytes = [];
  for (let i = 0; i < str.length; i++) {
    let charCode = str.charCodeAt(i);
    if (charCode < 0x80) {
      bytes.push(charCode);
    } else if (charCode < 0x800) {
      bytes.push(0xc0 | (charCode >> 6), 0x80 | (charCode & 0x3f));
    } else if (charCode < 0xd800 || charCode >= 0xe000) {
      bytes.push(
        0xe0 | (charCode >> 12),
        0x80 | ((charCode >> 6) & 0x3f),
        0x80 | (charCode & 0x3f)
      );
    } else {
      // Surrogate pair
      i++;
      charCode = 0x10000 + (((charCode & 0x3ff) << 10) | (str.charCodeAt(i) & 0x3ff));
      bytes.push(
        0xf0 | (charCode >> 18),
        0x80 | ((charCode >> 12) & 0x3f),
        0x80 | ((charCode >> 6) & 0x3f),
        0x80 | (charCode & 0x3f)
      );
    }
  }
  return new Uint8Array(bytes);
}

/**
 * Convert a UUID string to a 16-byte Uint8Array
 * @param {string} uuid - UUID string (with or without dashes)
 * @returns {Uint8Array} 16-byte array
 */
export function uuidToBytes(uuid) {
  const hex = uuid.replace(/-/g, '');
  if (hex.length !== 32) {
    throw new Error(`Invalid UUID length: ${uuid}`);
  }
  const bytes = new Uint8Array(16);
  for (let i = 0; i < 16; i++) {
    bytes[i] = parseInt(hex.substr(i * 2, 2), 16);
  }
  return bytes;
}

/**
 * Generate a random UUID v4
 * @returns {string} UUID string
 */
export function generateUUID() {
  // Simple UUID v4 generation
  const bytes = new Uint8Array(16);
  for (let i = 0; i < 16; i++) {
    bytes[i] = Math.floor(Math.random() * 256);
  }
  // Set version (4) and variant (10xx)
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;

  const hex = Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

// Known property IDs from sdk/src/core/ids.rs
export const PROPERTY_IDS = {
  NAME: 'a126ca53-0c8e-48d6-b888-82c734c38935',
  DESCRIPTION: '9b1f76ff-9711-404c-861e-59dc3fa7d037',
  AVATAR: '1155beff-fad5-49b7-a2e0-da4777b8792c',
};

// Pre-computed property ID bytes
export const PROPERTY_BYTES = {
  NAME: uuidToBytes(PROPERTY_IDS.NAME),
  DESCRIPTION: uuidToBytes(PROPERTY_IDS.DESCRIPTION),
  AVATAR: uuidToBytes(PROPERTY_IDS.AVATAR),
};

/**
 * Protobuf wire type constants
 */
const WIRE_TYPE = {
  VARINT: 0,
  FIXED64: 1,
  LENGTH_DELIMITED: 2,
  FIXED32: 5,
};

/**
 * Simple protobuf encoder class
 */
class ProtoEncoder {
  constructor() {
    this.buffer = [];
  }

  /**
   * Write a varint
   */
  writeVarint(value) {
    while (value > 127) {
      this.buffer.push((value & 0x7f) | 0x80);
      value >>>= 7;
    }
    this.buffer.push(value);
  }

  /**
   * Write a 64-bit varint (for uint64)
   */
  writeVarint64(value) {
    // Handle BigInt or number
    let v = typeof value === 'bigint' ? value : BigInt(value);
    while (v > 127n) {
      this.buffer.push(Number(v & 0x7fn) | 0x80);
      v >>= 7n;
    }
    this.buffer.push(Number(v));
  }

  /**
   * Write a field tag
   */
  writeTag(fieldNumber, wireType) {
    this.writeVarint((fieldNumber << 3) | wireType);
  }

  /**
   * Write bytes field
   */
  writeBytes(fieldNumber, bytes) {
    if (!bytes || bytes.length === 0) return;
    this.writeTag(fieldNumber, WIRE_TYPE.LENGTH_DELIMITED);
    this.writeVarint(bytes.length);
    for (const b of bytes) {
      this.buffer.push(b);
    }
  }

  /**
   * Write string field
   */
  writeString(fieldNumber, str) {
    if (!str) return;
    const encoded = stringToUtf8Bytes(str);
    this.writeBytes(fieldNumber, encoded);
  }

  /**
   * Write bool field
   */
  writeBool(fieldNumber, value) {
    this.writeTag(fieldNumber, WIRE_TYPE.VARINT);
    this.buffer.push(value ? 1 : 0);
  }

  /**
   * Write uint64 field
   */
  writeUint64(fieldNumber, value) {
    this.writeTag(fieldNumber, WIRE_TYPE.VARINT);
    this.writeVarint64(value);
  }

  /**
   * Write uint32 field
   */
  writeUint32(fieldNumber, value) {
    this.writeTag(fieldNumber, WIRE_TYPE.VARINT);
    this.writeVarint(value);
  }

  /**
   * Write embedded message
   */
  writeMessage(fieldNumber, messageBytes) {
    if (!messageBytes || messageBytes.length === 0) return;
    this.writeTag(fieldNumber, WIRE_TYPE.LENGTH_DELIMITED);
    this.writeVarint(messageBytes.length);
    for (const b of messageBytes) {
      this.buffer.push(b);
    }
  }

  /**
   * Get the encoded bytes
   */
  finish() {
    return new Uint8Array(this.buffer);
  }
}

/**
 * Encode a Value message (grc20.Value)
 * message Value {
 *   bytes property = 1;
 *   string value = 2;
 * }
 */
function encodeValue(propertyBytes, valueStr) {
  const encoder = new ProtoEncoder();
  encoder.writeBytes(1, propertyBytes);
  encoder.writeString(2, valueStr);
  return encoder.finish();
}

/**
 * Encode an Entity message (grc20.Entity)
 * message Entity {
 *   bytes id = 1;
 *   repeated Value values = 2;
 * }
 */
function encodeEntity(entityIdBytes, values) {
  const encoder = new ProtoEncoder();
  encoder.writeBytes(1, entityIdBytes);
  for (const v of values) {
    encoder.writeMessage(2, v);
  }
  return encoder.finish();
}

/**
 * Encode an Op message with UpdateEntity payload
 * message Op {
 *   oneof payload {
 *     Entity update_entity = 1;
 *     ...
 *   }
 * }
 */
function encodeOp(entityBytes) {
  const encoder = new ProtoEncoder();
  encoder.writeMessage(1, entityBytes); // update_entity = 1
  return encoder.finish();
}

/**
 * Encode BlockchainMetadata
 * message BlockchainMetadata {
 *   uint64 created_at = 1;
 *   bytes created_by = 2;
 *   uint64 block_number = 3;
 *   string cursor = 4;
 *   uint32 sequence = 5;
 *   bool is_last = 6;
 * }
 */
function encodeBlockchainMetadata(meta) {
  const encoder = new ProtoEncoder();
  if (meta.createdAt) encoder.writeUint64(1, meta.createdAt);
  if (meta.createdBy) encoder.writeBytes(2, meta.createdBy);
  if (meta.blockNumber) encoder.writeUint64(3, meta.blockNumber);
  if (meta.cursor) encoder.writeString(4, meta.cursor);
  if (meta.sequence !== undefined) encoder.writeUint32(5, meta.sequence);
  if (meta.isLast !== undefined) encoder.writeBool(6, meta.isLast);
  return encoder.finish();
}

/**
 * Encode a complete HermesEdit message
 * 
 * @param {Object} edit - The edit object
 * @param {Uint8Array} edit.id - 16-byte edit ID
 * @param {string} edit.name - Edit name
 * @param {Uint8Array} edit.entityId - 16-byte entity ID
 * @param {Uint8Array} edit.spaceId - 16-byte space ID
 * @param {string} edit.entityName - Entity name value
 * @param {string|null} edit.entityDescription - Entity description (optional)
 * @param {Uint8Array} edit.authorId - 16-byte author ID
 * @param {Object} edit.meta - Blockchain metadata
 * @returns {Uint8Array} Encoded protobuf bytes
 */
export function encodeHermesEdit(edit) {
  // Build values array
  const values = [encodeValue(PROPERTY_BYTES.NAME, edit.entityName)];
  if (edit.entityDescription) {
    values.push(encodeValue(PROPERTY_BYTES.DESCRIPTION, edit.entityDescription));
  }

  // Build entity
  const entityBytes = encodeEntity(edit.entityId, values);

  // Build op with update_entity
  const opBytes = encodeOp(entityBytes);

  // Build metadata
  const metaBytes = encodeBlockchainMetadata(edit.meta);

  // Build HermesEdit
  const encoder = new ProtoEncoder();
  encoder.writeBytes(1, edit.id);           // id = 1
  encoder.writeString(2, edit.name);        // name = 2
  encoder.writeMessage(3, opBytes);         // ops = 3 (repeated, but we send one)
  encoder.writeBytes(4, edit.authorId);     // authors = 4 (repeated, but we send one)
  // skip language = 5
  encoder.writeBytes(6, edit.spaceId);      // space_id = 6
  encoder.writeBool(7, true);               // is_canonical = 7
  encoder.writeMessage(8, metaBytes);       // meta = 8

  return encoder.finish();
}

/**
 * Convert a Uint8Array to an ArrayBuffer that k6 can properly handle
 * @param {Uint8Array} uint8Array 
 * @returns {ArrayBuffer}
 */
function toArrayBuffer(uint8Array) {
  // Create a new ArrayBuffer and copy the data
  const buffer = new ArrayBuffer(uint8Array.length);
  const view = new Uint8Array(buffer);
  for (let i = 0; i < uint8Array.length; i++) {
    view[i] = uint8Array[i];
  }
  return buffer;
}

/**
 * Create a complete HermesEdit for load testing
 * 
 * @param {string} entityName - The entity name
 * @param {string|null} entityDescription - The entity description (optional)
 * @returns {Object} Object containing encoded bytes and metadata
 */
export function createTestHermesEdit(entityName, entityDescription = null) {
  const editId = generateUUID();
  const entityId = generateUUID();
  const spaceId = generateUUID();
  const authorId = generateUUID();
  const creatorId = generateUUID();

  const edit = {
    id: uuidToBytes(editId),
    name: `load-test-${Date.now()}`,
    entityId: uuidToBytes(entityId),
    spaceId: uuidToBytes(spaceId),
    entityName,
    entityDescription,
    authorId: uuidToBytes(authorId),
    meta: {
      createdAt: BigInt(Math.floor(Date.now() / 1000)),
      createdBy: uuidToBytes(creatorId),
      blockNumber: BigInt(Math.floor(Math.random() * 10000000)),
      cursor: `load-${Date.now()}-${Math.floor(Math.random() * 1000000)}`,
      sequence: 0,
      isLast: true,
    },
  };

  const uint8Array = encodeHermesEdit(edit);
  
  // Return the Uint8Array directly - k6-kafka should handle it
  return {
    bytes: uint8Array,
    entityId,
    spaceId,
    editId,
  };
}

