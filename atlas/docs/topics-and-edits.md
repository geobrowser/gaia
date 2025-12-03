# Topics and Edits Data Model

## Overview

This document describes the event-sourced system for organizing communities, topics, and data through a dual-tree hierarchy structure that enables emergent, self-sovereign coordination.

## Core Concepts

### Communities
- Autonomous entities identified by UUID
- Can announce membership in multiple topics
- Can point to other communities or abstract topics
- Emit data tagged with their announced topics

### Topics
- Abstract categories identified by UUID
- Organized into emergent hierarchies through community declarations
- Not explicitly defined as trees - structure emerges from community subtopic pointers
- Enable discovery and aggregation of related data

### Dual-Tree Structure

The system maintains two parallel, overlapping structures:

**Tree 1: Topic Hierarchy**
- Emergent structure built from subtopic declarations
- Communities declare subtopics with explicit parent topics
- Enables topic-based aggregation and discovery

**Tree 2: Community Tree**
- Explicit community-to-community pointers
- Implicit communities reached through traversal
- Enables economic coordination and delegation

## Event Model

The system is built through an ordered event stream. Events must be processed in order to maintain consistency.

### Structural Events

#### CreateCommunity
```
CreateCommunity {
  id: UUID
}
```
Creates a new community with a unique identifier.

#### AnnounceTopic
```
AnnounceTopic {
  communityId: UUID,
  topicId: UUID
}
```
Community announces membership in a topic. Required before emitting data tagged with this topic.

#### RemoveTopic
```
RemoveTopic {
  communityId: UUID,
  topicId: UUID
}
```
Removes topic from community's announced topics. **Cascading effect**: Also removes any subtopic pointers that reference this topic as their parent.

#### AddSubtopic
```
AddSubtopic {
  communityId: UUID,
  subtopicId: UUID,
  parentTopicId: UUID
}
```
Declares an abstract topic as a subtopic with explicit parent reference.

**Constraints:**
- `parentTopicId` must be in the community's announced topics
- Creates the topic hierarchy relationship: parentTopicId → subtopicId

#### RemoveSubtopic
```
RemoveSubtopic {
  communityId: UUID,
  subtopicId: UUID,
  parentTopicId: UUID
}
```
Removes a subtopic pointer declaration.

#### AddCommunityPointer
```
AddCommunityPointer {
  communityId: UUID,
  targetCommunityId: UUID
}
```
Creates an explicit pointer to another concrete community. Enables community tree traversal.

#### RemoveCommunityPointer
```
RemoveCommunityPointer {
  communityId: UUID,
  targetCommunityId: UUID
}
```
Removes a community pointer.

### Data Events

#### EditPublished
```
EditPublished {
  editUrl: string,
  topicIds: UUID[],
  sourceId: UUID
}
```
Emits data/content from a community or individual.

**Fields:**
- `editUrl`: External location where the actual data/content exists
- `topicIds`: Topics this data is tagged with (can be multiple)
- `sourceId`: Community or individual UUID that created this data

**Validation (pre-emission):**
- If `sourceId` is a community, all `topicIds` must be in that community's announced topics
- Communities cannot emit data for subtopics unless those subtopics are also announced
- Event stream consumers can assume validation has occurred

## Community State

Each community maintains:

```
Community {
  id: UUID,
  announcedTopics: Set<UUID>,
  subtopicPointers: Array<{
    subtopicId: UUID,
    parentTopicId: UUID
  }>,
  communityPointers: Set<UUID>
}
```

## Aggregation Algorithm

When aggregating data from a root topic, the system performs three steps:

### Step 1: Build Topic Hierarchy

Starting from root topic A:
1. Find all communities with topic A in their announced topics
2. For each community, collect subtopic pointers where `parentTopicId = A`
3. Recursively traverse discovered subtopics
4. Result: Set of all topic UUIDs reachable from A: `{A, B, C, D, E...}`

### Step 2: Build Community Tree

1. Find all communities announcing any topic in `{A, B, C, D, E...}` (explicit communities)
2. From each community, follow community pointers to reach other concrete communities (implicit communities)
3. Recursively traverse to build complete community tree
4. Result: Set of all community UUIDs in the tree

### Step 3: Collect Data

Filter `EditPublished` events where:
- `topicIds ∩ {A, B, C, D, E...} ≠ ∅` (data tagged with topics in hierarchy)
- `sourceId ∈ community tree` (from communities in the tree)

## Design Principles

### Self-Sovereignty
- No gatekeeping: Any community can announce any topic
- Multi-homing: Communities can belong to multiple topics
- Exit rights: Communities can change announcements and pointers at any time
- No single community "owns" a topic

### Emergent Coordination
- Topic hierarchies form organically through community declarations
- Meta-communities emerge by curating/funding specialized communities
- Competition at every level prevents ossification

### Incentive Alignment

**Specialization Incentive:**
- Communities announcing specific topics get better discovery in relevant searches
- Higher discovery → more usage → more rewards

**Economic Delegation:**
- Communities can fund other communities via pointers
- Enables separation of work between curators and producers

**Quality Rewards:**
- Data quality, relevance, and consumption drive rewards
- Both individuals and communities are rewarded

**Open Competition:**
- Multiple communities can compete on the same topics
- Prevents monopolization
- Market naturally rewards quality

### Natural Self-Correction

Communities are incentivized to accurately self-categorize because:
1. **Specificity = Discovery**: Announcing specific topics makes content discoverable to the right audience
2. **Broad = Buried**: Announcing only broad topics means competing with everyone, low visibility
3. **Honest Tagging**: Data tags must match announced topics (enforced by validation)
4. **Reputation & Curation**: Communities can be voted on and curated, filtering malicious actors

## Example Scenario

**Community "Rust Async Experts" (UUID: community-123)**

Events:
```
CreateCommunity { id: community-123 }
AnnounceTopic { communityId: community-123, topicId: rust-topic }
AnnounceTopic { communityId: community-123, topicId: async-topic }
AddSubtopic {
  communityId: community-123,
  subtopicId: tokio-topic,
  parentTopicId: rust-topic
}
EditPublished {
  editUrl: "https://...",
  topicIds: [rust-topic, async-topic],
  sourceId: community-123
}
```

**Aggregation from "Rust" topic:**
1. Topic hierarchy includes: rust-topic → tokio-topic
2. Community tree includes: community-123 (announces rust-topic)
3. Collected data includes: EditPublished event (tagged with rust-topic)

**Aggregation from "Technology" topic (if Rust is a subtopic):**
1. If another community declares rust-topic as subtopic of tech-topic
2. Topic hierarchy includes: tech-topic → rust-topic → tokio-topic
3. Community tree includes: community-123 + the other community
4. Same EditPublished event is included (tagged with rust-topic, which is in hierarchy)

## Implementation Considerations

- Events must be processed in order
- State can be rebuilt from event stream
- Validation happens before events enter the stream
- Consumers can trust event validity
- Cycles in community pointers should be detected during traversal (DAG constraint)
- Topic hierarchy building should track visited topics to prevent infinite loops
