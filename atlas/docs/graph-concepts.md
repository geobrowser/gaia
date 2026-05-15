# Atlas Graph System - Core Concepts

## Overview

**Query-Time Computation:** The transitive and canonical graphs are computed on-demand at query time rather than being stored or incrementally maintained. This design allows topic edges to dynamically resolve to their current group membership without requiring updates when membership changes—a node can be added or removed from a topic and subsequent queries will immediately reflect the new graph structure. Computing at query time also avoids the complexity of maintaining consistency when edges are added or removed, as each query operates on a fresh, consistent snapshot of the current graph state.

**Trade-offs:** This approach prioritizes flexibility and correctness over query performance, making it well-suited for graphs where membership and edges change frequently relative to query frequency. The cost is that each query performs a full graph traversal, which could become expensive for very large graphs or high query volumes. However, the stateless nature simplifies reasoning about the system and eliminates entire classes of bugs related to stale or inconsistent cached data.

## 1. Core Concepts

### Graphs We Track

The system tracks four different graph views:

1. **Global Graph**: The complete graph containing all nodes and edges
   - Includes explicit edges (direct node-to-node connections)
   - Includes group references (edges to topics that dynamically resolve to their members)
   - This is the source of truth for all graph data

2. **Local Graph** (per node): Direct neighborhood view
   - Shows only the immediate children of a specific node
   - Includes both explicit edges and resolved topic memberships
   - Gives a one-hop view of connectivity

3. **Transitive Graph** (per node): Complete reachability view
   - All nodes reachable by following edges from a starting node
   - Follows edges transitively (child of child of child...)
   - Represented as both a tree structure and a flat set
   - Always acyclic (cycles are broken to form a DAG)

4. **Canonical Graph**: Trusted subgraph from a designated root
   - A special transitive graph computed from a root node
   - Represents the "trusted" portion of the graph
   - Uses restricted rules for topic edges (see below)
   - Defines the authoritative view of the graph hierarchy

### Edge Types

The system supports two categories of edges:

**Explicit Edges** - Direct node-to-node connections with semantic types:
- Editor relationships
- Member relationships
- Trust relationships
- Generic related relationships

**Topic Edges** - Indirect connections through group membership:
- A node references an entire topic/group
- Resolves dynamically to current group members at query time
- Membership can change without updating the edge
- Provides abstraction over bulk relationships

### Update Operations

The system supports operations in three categories:

**Node Management:**
- Add/remove nodes from the graph
- Query node existence and metadata

**Explicit Edge Management:**
- Add/remove direct edges between nodes
- Specify edge semantic type

**Topic/Group Management:**
- Create topic groups
- Add/remove nodes from topics (group membership)
- Add/remove topic edges (node-to-group references)

### Special Rules for Canonical Graph

The canonical graph uses restricted computation rules to maintain a trust boundary:

1. **Trust via Explicit Edges Only:**
   - A node becomes "canonical" (trusted) only if reachable from root via explicit edges
   - Topic edges cannot grant trust or add new nodes to the canonical set

2. **Topic Edges as Secondary Connections:**
   - Topic edges are processed after establishing the canonical node set
   - Topic edges only connect nodes that are already canonical
   - If a topic contains non-canonical members, those members are filtered out

3. **Two-Phase Computation:**
   - First: traverse explicit edges only to establish canonical nodes
   - Second: add topic edges between canonical nodes

4. **Transitive Inclusion:**
   - When a node is added to the canonical graph, all nodes reachable from it (via explicit edges) are also included
   - This ensures complete subtrees of trusted nodes are captured

## 2. Computing Transitive and Canonical Graphs

### Transitive DAG Computation

**Algorithm**: BFS with cycle avoidance

```
1. Initialize:
   - visited set (tracks nodes we've seen)
   - nodeMap (TreeNode instances for building hierarchy)
   - queue (BFS queue)
   - flat set (all reachable node IDs)

2. Start with root node:
   - Create TreeNode for root
   - Add to visited, queue, and flat set

3. BFS Traversal:
   - While queue not empty:
     - Dequeue current node
     - Resolve children (explicit edges + topic edges)
     - For each child:
       - If NOT visited:
         - Mark as visited
         - Create TreeNode
         - Add to parent's children
         - Add to queue (continue traversal)
       - If already visited:
         - Skip (maintains DAG, breaks cycles)

4. Return:
   - Tree structure (TreeNode hierarchy)
   - Flat set (all reachable node IDs)
```

**Key Details:**
- Children resolution combines explicit edges and group references
- Group references resolve to current members at query time
- First visit wins - subsequent paths to same node are ignored
- No caching - computed fresh on each query

### Canonical Graph Computation

**Algorithm**: Two-phase BFS with explicit-first traversal

```
Phase 1 - Explicit Edges Only:
1. Initialize same structures as transitive DAG
2. Add deferred topic edges collection
3. BFS with ONLY explicit edges:
   - For each node in queue:
     - Process explicit edges (add new nodes, continue BFS)
     - Collect topic edges for later (don't process yet)
   - visited set now contains all canonical nodes

Phase 2 - Topic Edge Addition (Transitive):
1. For each deferred topic edge:
   - Get source node from tree
   - Resolve topic to group members
   - For each member:
     - If member in visited set (canonical):
       - Recursively add member and its full transitive subtree:
         - Add edge to member (if not already present)
         - Recursively add all descendants of member (explicit + topic edges)
         - Only include descendants that are also canonical
         - Skip edges that already exist
     - If member NOT in visited set:
       - Skip (not canonical, don't add)

2. Return tree and flat set (flat set unchanged from Phase 1)

Key: Topic edges add the FULL transitive subtree of canonical members, not just direct edges
```

**Why Two Phases?**
- Ensures topic edges don't expand the trusted node set
- Topic edges only create connections between already-trusted nodes
- Preserves semantic meaning: trust flows through explicit edges only

## 3. Topic Edge Handling in Canonical Graph

### Topic Edge Semantics

**In Transitive DAG:**
- Topic edges resolve to ALL current group members
- Can introduce new nodes to the graph
- Treated equivalently to explicit edges during traversal

**In Canonical Graph:**
- Topic edges resolve ONLY to members already in canonical set
- Cannot introduce new nodes
- Act as supplementary connections between trusted nodes

### Resolution Process

1. **During Phase 1** (explicit edge BFS):
   - Topic edges are detected but not processed
   - Stored in `deferredTopicEdges` array with:
     - Source node ID
     - Group/topic ID
     - Edge metadata

2. **During Phase 2** (topic edge addition with transitiveness):
   - For each deferred edge:
     - Look up group members
     - Filter to only canonical members (in visited set)
     - For each canonical member:
       - Recursively add member's full transitive subtree
       - Include all descendants (explicit + topic edges) that are canonical
       - Add edges to tree structure without modifying visited set
       - Skip edges that already exist

### Example 1: Basic Topic Edge

```
Explicit edges:  Root -> A -> C
                 Root -> B
Topic "team":    Members = {A}
Topic edge:      Root -> Topic "team"

Phase 1 Result:
  Canonical nodes = {Root, A, B, C}

Phase 2 Processing:
  Root -> Topic "team" resolves to {A}
  - A: already canonical ✓ add edge to A
    - A has child C (canonical) ✓ recursively add A->C

Final Canonical Graph:
  Nodes: {Root, A, B, C}
  Edges: Root->A (explicit), A->C (explicit), Root->B (explicit),
         Root->A (topic - already exists), A->C (topic - included transitively)

Tree structure:
  Root
    ├─ A (explicit)
    │  └─ C (explicit)
    ├─ B (explicit)
    └─ A (topic) - references same TreeNode as explicit A with full subtree
       └─ C (included transitively)
```

### Example 2: Transitive Subtree Inclusion

```
Explicit edges:  Root -> A
                 Root -> X -> B -> C
Topic "team":    Members = {X}
Topic edge:      A -> Topic "team"

Phase 1 Result:
  Canonical nodes = {Root, A, X, B, C}

Phase 2 Processing:
  A -> Topic "team" resolves to {X}
  - X: already canonical ✓ add edge A->X
    - X has child B (canonical) ✓ recursively add X->B
      - B has child C (canonical) ✓ recursively add B->C

Final Canonical Graph:
  Tree structure:
  Root
    ├─ A (explicit)
    │  └─ X (topic:team)
    │     └─ B (explicit - included transitively)
    │        └─ C (explicit - included transitively)
    └─ X (explicit)
       └─ B (explicit)
          └─ C (explicit)

Key: Topic edge A->X includes X's FULL transitive subtree (B, C)
```

## 4. Cycle Handling

### In Tracked Graphs (Storage)

**Cycles are allowed** in the underlying storage:
- Explicit edges can create cycles (A->B->C->A)
- Topic edges can create cycles
- No validation prevents cycle creation
- Storage faithfully represents the graph as specified

### In Computed Graphs (DAG Results)

**Cycles are broken** during computation to create DAGs:

**Mechanism**: First-visit wins in BFS traversal

```
Example cycle: A -> B -> C -> A

BFS traversal:
1. Visit A (first time) - add to tree, add to visited
2. Visit B (first time) - add as child of A, add to visited
3. Visit C (first time) - add as child of B, add to visited
4. Encounter A again (from C) - already visited, SKIP

Result: Tree shows A -> B -> C (cycle broken)
```

**Properties:**
- The visited set prevents revisiting nodes
- The first path to a node determines its position in the tree
- Alternative paths (including back-edges) are silently dropped
- Resulting structure is always a valid DAG
- The flat set contains each node exactly once

**Implications:**
- DAG structure depends on BFS traversal order
- Different traversal orders could produce different trees
- All reachable nodes are included regardless of cycles
- No error or warning when cycles are encountered
- Deterministic for a given graph (BFS order is stable)

### Cycle Detection

The system does NOT explicitly detect or report cycles:
- No cycle detection algorithm runs
- No error is thrown for cyclic graphs
- Cycles are implicitly handled by the visited set
- Users won't know if their graph has cycles unless they inspect it separately
