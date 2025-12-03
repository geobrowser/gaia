# High-Level Design for Tracking Multiple Graphs with Group Abstractions

To support tracking multiple graph views while incorporating abstract groups (where nodes can reference Group IDs as "children" instead of individual nodes, allowing transient memberships), we'll extend the graph storage to handle both explicit node edges and group references. Groups remain metadata (as in the previous design), but now they can be "dereferenced" dynamically during queries to resolve to current members. This maintains the abstraction: a node adding "Group A" as a child means it's connected to all current/future members of Group A, without storing redundant edges.

## Core Concepts
- **Graphs to Track**:
  1. **Global Graph**: Full adjacency list of all nodes and their direct edges (including resolved group references for queries).
  2. **Local Graph (per Node)**: Subset of the global graph—only the direct children (edges) of a specific node.
  3. **Transitive DAG (per Node)**: Transitive closure from each node, computed as a DAG (acyclic, with cycles broken as before).
  4. **Canonical Graph**: Transitive DAG from a fixed Root Node, including only "trusted" nodes (added individually or via explicit edges). Group references are resolved at runtime to trusted members, preserving abstract semantics via edge metadata. Note: When adding a node to the Canonical Graph, include its full transitive tree (all reachable trusted nodes). For topic edges in the transitive tree, only add nodes that are already in the canonical graph.
- **Group Abstractions**:
  - Nodes can add/remove "children" that are either individual nodes or Group IDs.
  - Group references are stored separately from explicit edges (e.g., a node has a list of "child groups").
  - Memberships are dynamic: Groups change over time, so queries must resolve groups to current members on-the-fly.
  - Transient Groups: Nodes in a group can change without updating all referencing edges—resolution happens at query time.
- **Operations**:
  - **Add/Remove Nodes**: Individually or via groups (e.g., adding a node to a group affects all nodes referencing that group).
  - **Add/Remove Edges**: Direct (node-to-node) or group-based (node-to-group reference).
  - **Queries**: For local/transitive graphs, resolve group references to actual nodes before computation.

## Storage Structure
Build on the previous group design, adding group references to adjacency, edge metadata, and trusted node tracking. Use bidirectional mappings for efficiency.

- **Node Registry**:
  - `Map<NodeId, { name: string }>`: Tracks all nodes (spaces) that have been created with their names.

- **Global Adjacency**:
  - Explicit Edges: `Map<NodeId, Map<NodeId, EdgeMetadata>>` (direct node-to-node edges with metadata per edge).
  - Group References: `Map<NodeId, Map<GroupId, EdgeMetadata>>` (node's "child groups" with metadata per reference).
  - Combined: When querying, merge explicit edges + resolved group members.

- **Group Memberships**:
  - `Map<GroupId, Set<NodeId>>` (group-to-nodes).
  - `Map<NodeId, Set<GroupId>>` (node-to-groups - reverse mapping for efficiency).

- **Trusted Nodes**:
  - No separate storage. "Trusted" nodes are dynamically computed as nodes reachable from the root via explicit edges only. The canonical graph IS the trusted graph.

- **Edge Metadata**:
  - Simple structure: `{ type: EdgeType }` where EdgeType is `'editor' | 'member' | 'trusted' | 'related' | \`topic:${string}\``
  - In DAG hierarchies (`TreeNode` edges), metadata classifies edge sources and preserves group semantics (e.g., 'topic:abc-123-uuid' for topic-based groupings).

- **Transitive Data**:
  - Computed on-demand (no caching in current implementation).
  - Returns: `{ tree: TreeNode, flat: Set<NodeId> }` containing both hierarchical tree and flat set representations.
  - Future optimization: Cache `Map<NodeId, DAGResult>` and invalidate on changes (individual or group-based).

- **Local Graph (Derived)**:
  - Not stored separately; computed on-demand from global adjacency + group resolution for a node.

- **Canonical Graph (Derived)**:
  - Not stored; computed from Root Node using trusted nodes and resolved groups. Cached as `TreeNode` with metadata.

## Storage Options
1. **Hybrid (RocksDB + Redis)**: RocksDB for durable writes with eventual consistency, Redis for fast cached reads to support quick periodic queries.
2. **Redis Only**: In-memory with persistence for fast reads/writes and durability, ideal for small-to-medium graphs with frequent changes.
3. **RocksDB Only**: Disk-based KV store for cost-effective durable writes and reasonably fast reads on large datasets.
4. **Neo4j**: Graph-native database for optimized transitive queries and metadata-rich edges, with good durability.
5. **In-Memory (TypeScript)**: Maps/Sets for all structures, fast for small graphs with caching for transitive data.
6. **PostgreSQL**: Relational tables for edges/groups with ACID transactions, suitable for complex queries on moderate datasets.

## Key Operations and Queries
- **Add Node**: Add to node registry with name: `addNode(nodeId, name)`.
- **Add Edge (Individual)**: Update explicit adjacency with edge metadata `{ type: EdgeType }` where type is 'editor', 'member', 'trusted', or 'related'. No separate trusted node tracking - trust is derived from reachability.
- **Add Edge (Group/Topic)**: Add group reference to node's "child groups" with edge metadata `{ type: 'topic:${topicId}' }`; no immediate resolution.
- **Add Node to Group**: Update group memberships bidirectionally (group-to-nodes and node-to-groups). Since there's no caching, no invalidation needed.
- **Remove Edge**: Remove from explicit edges or group references.
- **Remove Node from Group**: Remove from both group membership mappings.
- **Queries**:
  - **Global Graph**: Return explicit adjacency + resolved group references (e.g., for node X, children = explicit + members of its child groups).
  - **Local Graph**: For node X, return its resolved children (explicit edges + current members of referenced groups).
  - **Transitive DAG**: Compute from resolved global graph on-demand with edge metadata. No caching.
  - **Canonical Graph**: Compute transitive DAG from Root Node using explicit edges only (BFS Phase 1), then add topic edges that connect already-canonical nodes (Phase 2). This defines "trusted" as reachable from root.
- **Resolution**: Function to "expand" groups: For a node's child groups, union their current members with explicit children, attaching metadata with the topic type.
- **Updates**: No cache invalidation needed since all queries compute on-demand.

## Considerations
- **Performance**: Resolution adds O(group size) overhead per query. Current implementation computes on-demand without caching. For production use, consider caching transitive DAGs and canonical graph with invalidation on changes. Edge metadata storage is minimal (just type field).
- **Consistency**: Group resolution happens at query time, ensuring consistency. BFS with visited set prevents cycles in transitive graphs. Trusted nodes are computed atomically during canonical graph query.
- **Scalability**: For large groups, resolution overhead scales with group size. Consider lazy resolution or pagination for very large groups. No cache invalidation overhead since there's no caching.
- **Edge Cases**: Empty groups (return no children), circular group references (prevented by BFS visited set), nodes not in canonical graph (excluded from topic edge resolution in canonical view).
- **Semantics Preservation**: Edge metadata `{ type: EdgeType }` retains abstraction origins, allowing queries to distinguish 'editor'/'member'/'trusted'/'related' (explicit) vs. 'topic:${uuid}' (group-derived) connections.
- **Abstraction Benefits**: Simplifies bulk operations (e.g., "add project team as children") without duplicating edges. Topic edges update automatically as group membership changes. Canonical Graph provides a trusted, root-focused view computed from reachability.

This design keeps groups abstract while enabling dynamic resolution for all graph views. It integrates with your existing adjacency/transitive logic. Next, we can prototype in code!
