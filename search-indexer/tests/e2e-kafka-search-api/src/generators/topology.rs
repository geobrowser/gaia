use anyhow::Result;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::topology::{
    CanonicalGraphDiff, ChangeType, EdgeInfo, NodeChange, VerifiedEdge, RelatedEdge,
    edge_info::EdgeType,
};

/// A node to add/move/remove in a diff.
pub struct DiffNode {
    pub space_id: Uuid,
    pub change: NodeChangeKind,
}

pub enum NodeChangeKind {
    Added {
        parent_id: Uuid,
        distance: u32,
        edge: EdgeKind,
    },
    Removed,
    #[allow(dead_code)]
    Moved {
        parent_id: Uuid,
        distance: u32,
        edge: EdgeKind,
    },
}

pub enum EdgeKind {
    Verified,
    Related,
}

/// Encode a `CanonicalGraphDiff` protobuf message ready to publish on the
/// `topology.canonical` Kafka topic.
pub fn create_canonical_graph_diff(root_id: Uuid, nodes: Vec<DiffNode>) -> Result<Vec<u8>> {
    let changes: Vec<NodeChange> = nodes
        .into_iter()
        .map(|n| {
            let space_id = n.space_id.as_bytes().to_vec();
            match n.change {
                NodeChangeKind::Added {
                    parent_id,
                    distance,
                    edge,
                } => NodeChange {
                    space_id,
                    change_type: ChangeType::Added as i32,
                    distance: Some(distance),
                    parent_edge: Some(make_edge_info(parent_id, edge)),
                },
                NodeChangeKind::Removed => NodeChange {
                    space_id,
                    change_type: ChangeType::Removed as i32,
                    distance: None,
                    parent_edge: None,
                },
                NodeChangeKind::Moved {
                    parent_id,
                    distance,
                    edge,
                } => NodeChange {
                    space_id,
                    change_type: ChangeType::Moved as i32,
                    distance: Some(distance),
                    parent_edge: Some(make_edge_info(parent_id, edge)),
                },
            }
        })
        .collect();

    let diff = CanonicalGraphDiff {
        root_id: root_id.as_bytes().to_vec(),
        changes,
        meta: None,
    };

    let mut buf = Vec::new();
    diff.encode(&mut buf)?;
    Ok(buf)
}

fn make_edge_info(parent_id: Uuid, kind: EdgeKind) -> EdgeInfo {
    let edge_type = match kind {
        EdgeKind::Verified => EdgeType::Verified(VerifiedEdge {}),
        EdgeKind::Related => EdgeType::Related(RelatedEdge {}),
    };
    EdgeInfo {
        parent_id: parent_id.as_bytes().to_vec(),
        edge_type: Some(edge_type),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_simple_diff() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();

        let result = create_canonical_graph_diff(
            root,
            vec![DiffNode {
                space_id: child,
                change: NodeChangeKind::Added {
                    parent_id: root,
                    distance: 1,
                    edge: EdgeKind::Verified,
                },
            }],
        );

        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        let decoded = CanonicalGraphDiff::decode(&bytes[..]).unwrap();
        assert_eq!(decoded.root_id, root.as_bytes().to_vec());
        assert_eq!(decoded.changes.len(), 1);
        assert_eq!(decoded.changes[0].change_type, ChangeType::Added as i32);
    }

    #[test]
    fn test_encode_remove_diff() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();

        let bytes = create_canonical_graph_diff(
            root,
            vec![DiffNode {
                space_id: child,
                change: NodeChangeKind::Removed,
            }],
        )
        .unwrap();

        let decoded = CanonicalGraphDiff::decode(&bytes[..]).unwrap();
        assert_eq!(decoded.changes[0].change_type, ChangeType::Removed as i32);
        assert!(decoded.changes[0].distance.is_none());
        assert!(decoded.changes[0].parent_edge.is_none());
    }
}
