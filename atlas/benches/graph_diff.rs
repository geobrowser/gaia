use atlas::events::{SpaceId, TopicId};
use atlas::graph::EdgeType;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use roaring::RoaringTreemap;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    distance: u32,
    parent: SpaceId,
    edge_type: EdgeType,
    topic_id: Option<TopicId>,
}

struct BenchData {
    old_vec: Vec<(SpaceId, Position)>,
    new_vec: Vec<(SpaceId, Position)>,
    old_btree: BTreeMap<SpaceId, Position>,
    new_btree: BTreeMap<SpaceId, Position>,
    old_set: RoaringTreemap,
    new_set: RoaringTreemap,
    old_positions_by_id: Vec<Option<Position>>,
    new_positions_by_id: Vec<Option<Position>>,
}

struct BenchContext {
    pool: Vec<SpaceId>,
    mapping: HashMap<SpaceId, u64>,
}

fn random_space_ids(n: usize, rng: &mut StdRng) -> Vec<SpaceId> {
    let mut seen: HashSet<SpaceId> = HashSet::with_capacity(n * 2);
    let mut ids = Vec::with_capacity(n);
    while ids.len() < n {
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);
        if seen.insert(bytes) {
            ids.push(bytes);
        }
    }
    ids
}

fn build_id_pool(pool_size: usize, seed: u64) -> BenchContext {
    let mut rng = StdRng::seed_from_u64(seed);
    let pool = random_space_ids(pool_size, &mut rng);
    let mapping: HashMap<SpaceId, u64> = pool
        .iter()
        .enumerate()
        .map(|(idx, id)| (*id, idx as u64))
        .collect();
    BenchContext { pool, mapping }
}

fn random_topic_id(rng: &mut StdRng) -> TopicId {
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    bytes
}

fn random_edge_type(rng: &mut StdRng) -> EdgeType {
    match rng.gen_range(0..3) {
        0 => EdgeType::Verified,
        1 => EdgeType::Related,
        _ => EdgeType::Topic,
    }
}

fn build_positions(ids: &[SpaceId], rng: &mut StdRng) -> HashMap<SpaceId, Position> {
    let mut positions = HashMap::with_capacity(ids.len() * 2);
    for id in ids {
        let parent = ids[rng.gen_range(0..ids.len())];
        let edge_type = random_edge_type(rng);
        let topic_id = if edge_type == EdgeType::Topic {
            Some(random_topic_id(rng))
        } else {
            None
        };
        let distance = rng.gen_range(1..=6);
        positions.insert(
            *id,
            Position {
                distance,
                parent,
                edge_type,
                topic_id,
            },
        );
    }
    positions
}

fn mutate_positions(
    old_ids: &[SpaceId],
    old_positions: &HashMap<SpaceId, Position>,
    pool: &[SpaceId],
    change_rate: f64,
    rng: &mut StdRng,
) -> (Vec<SpaceId>, HashMap<SpaceId, Position>) {
    let mut new_ids: Vec<SpaceId> = old_ids.to_vec();
    new_ids.shuffle(rng);

    let total = new_ids.len();
    let remove_count = ((total as f64) * change_rate).round() as usize;
    let add_count = ((total as f64) * change_rate).round() as usize;
    let move_count = ((total as f64) * change_rate).round() as usize;

    let remove_count = remove_count.max(1).min(total);

    let removed: HashSet<SpaceId> = new_ids.drain(0..remove_count).collect();
    let mut positions: HashMap<SpaceId, Position> =
        old_positions.iter().map(|(k, v)| (*k, *v)).collect();

    for id in &removed {
        positions.remove(id);
    }

    let mut seen: HashSet<SpaceId> = new_ids.iter().copied().collect();
    let mut added_ids: Vec<SpaceId> = Vec::with_capacity(add_count);
    let mut pool_shuffled: Vec<SpaceId> = pool.to_vec();
    pool_shuffled.shuffle(rng);
    let available = pool_shuffled.len().saturating_sub(seen.len());
    let add_target = add_count.min(available);
    for id in pool_shuffled.into_iter() {
        if added_ids.len() >= add_target {
            break;
        }
        if seen.insert(id) {
            added_ids.push(id);
        }
    }
    for id in &added_ids {
        new_ids.push(*id);
    }
    for id in &added_ids {
        let parent = new_ids[rng.gen_range(0..new_ids.len())];
        let edge_type = random_edge_type(rng);
        let topic_id = if edge_type == EdgeType::Topic {
            Some(random_topic_id(rng))
        } else {
            None
        };
        let distance = rng.gen_range(1..=6);
        positions.insert(
            *id,
            Position {
                distance,
                parent,
                edge_type,
                topic_id,
            },
        );
    }

    let mut movable: Vec<SpaceId> = new_ids.to_vec();
    movable.shuffle(rng);
    for id in movable.into_iter().take(move_count.min(new_ids.len())) {
        if let Some(pos) = positions.get_mut(&id) {
            pos.distance = pos.distance.saturating_add(1);
            pos.parent = new_ids[rng.gen_range(0..new_ids.len())];
            pos.edge_type = match pos.edge_type {
                EdgeType::Verified => EdgeType::Related,
                _ => EdgeType::Verified,
            };
            if pos.edge_type == EdgeType::Topic && pos.topic_id.is_none() {
                pos.topic_id = Some(random_topic_id(rng));
            }
            if pos.edge_type != EdgeType::Topic {
                pos.topic_id = None;
            }
        }
    }

    (new_ids, positions)
}

fn sort_by_space_id(a: &SpaceId, b: &SpaceId) -> Ordering {
    a.cmp(b)
}

fn diff_sorted_merge(old: &[(SpaceId, Position)], new: &[(SpaceId, Position)]) -> usize {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut changes = 0usize;

    while i < old.len() || j < new.len() {
        if i == old.len() {
            changes += new.len() - j;
            break;
        }
        if j == new.len() {
            changes += old.len() - i;
            break;
        }

        let (old_id, old_pos) = &old[i];
        let (new_id, new_pos) = &new[j];

        match old_id.cmp(new_id) {
            Ordering::Equal => {
                if old_pos != new_pos {
                    changes += 2;
                }
                i += 1;
                j += 1;
            }
            Ordering::Less => {
                changes += 1;
                i += 1;
            }
            Ordering::Greater => {
                changes += 1;
                j += 1;
            }
        }
    }

    changes
}

fn diff_btree(old: &BTreeMap<SpaceId, Position>, new: &BTreeMap<SpaceId, Position>) -> usize {
    let mut changes = 0usize;
    let mut old_iter = old.iter().peekable();
    let mut new_iter = new.iter().peekable();

    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                for _ in old_iter {
                    changes += 1;
                }
                break;
            }
            (None, Some(_)) => {
                for _ in new_iter {
                    changes += 1;
                }
                break;
            }
            (Some((old_id, old_pos)), Some((new_id, new_pos))) => match old_id.cmp(new_id) {
                Ordering::Equal => {
                    if *old_pos != *new_pos {
                        changes += 2;
                    }
                    old_iter.next();
                    new_iter.next();
                }
                Ordering::Less => {
                    changes += 1;
                    old_iter.next();
                }
                Ordering::Greater => {
                    changes += 1;
                    new_iter.next();
                }
            },
        }
    }

    changes
}

fn diff_roaring(
    old_set: &RoaringTreemap,
    new_set: &RoaringTreemap,
    old_positions: &[Option<Position>],
    new_positions: &[Option<Position>],
) -> usize {
    let mut changes = 0usize;

    let removed = old_set - new_set;
    for _ in removed.iter() {
        changes += 1;
    }

    let added = new_set - old_set;
    for _ in added.iter() {
        changes += 1;
    }

    let common = old_set & new_set;
    for id in common.iter() {
        let idx = id as usize;
        if old_positions[idx] != new_positions[idx] {
            changes += 2;
        }
    }

    changes
}

fn build_bench_data_with_rate(
    context: &BenchContext,
    size: usize,
    change_rate: f64,
    seed: u64,
) -> BenchData {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut pool = context.pool.clone();
    pool.shuffle(&mut rng);
    let old_ids = pool.into_iter().take(size).collect::<Vec<_>>();
    let old_positions = build_positions(&old_ids, &mut rng);
    let (_new_ids, new_positions) = mutate_positions(
        &old_ids,
        &old_positions,
        &context.pool,
        change_rate,
        &mut rng,
    );

    let mut old_vec: Vec<(SpaceId, Position)> =
        old_positions.iter().map(|(id, pos)| (*id, *pos)).collect();
    old_vec.sort_unstable_by(|a, b| sort_by_space_id(&a.0, &b.0));

    let mut new_vec: Vec<(SpaceId, Position)> =
        new_positions.iter().map(|(id, pos)| (*id, *pos)).collect();
    new_vec.sort_unstable_by(|a, b| sort_by_space_id(&a.0, &b.0));

    let old_btree: BTreeMap<SpaceId, Position> =
        old_positions.iter().map(|(id, pos)| (*id, *pos)).collect();
    let new_btree: BTreeMap<SpaceId, Position> =
        new_positions.iter().map(|(id, pos)| (*id, *pos)).collect();

    let id_mapping = &context.mapping;
    let next_id = id_mapping.len() as u64;
    let mut old_set = RoaringTreemap::new();
    let mut new_set = RoaringTreemap::new();
    let mut old_positions_by_id: Vec<Option<Position>> = vec![None; next_id as usize];
    let mut new_positions_by_id: Vec<Option<Position>> = vec![None; next_id as usize];

    for (id, pos) in &old_positions {
        let mapped = id_mapping[id];
        old_set.insert(mapped);
        old_positions_by_id[mapped as usize] = Some(*pos);
    }

    for (id, pos) in &new_positions {
        let mapped = id_mapping[id];
        new_set.insert(mapped);
        new_positions_by_id[mapped as usize] = Some(*pos);
    }

    BenchData {
        old_vec,
        new_vec,
        old_btree,
        new_btree,
        old_set,
        new_set,
        old_positions_by_id,
        new_positions_by_id,
    }
}

fn build_sorted_vec(positions: &HashMap<SpaceId, Position>) -> Vec<(SpaceId, Position)> {
    let mut vec: Vec<(SpaceId, Position)> = positions.iter().map(|(id, pos)| (*id, *pos)).collect();
    vec.sort_unstable_by(|a, b| sort_by_space_id(&a.0, &b.0));
    vec
}

fn build_btree_map(positions: &HashMap<SpaceId, Position>) -> BTreeMap<SpaceId, Position> {
    positions.iter().map(|(id, pos)| (*id, *pos)).collect()
}

fn build_roaring_structures(
    positions: &HashMap<SpaceId, Position>,
    mapping: &HashMap<SpaceId, u64>,
) -> (RoaringTreemap, Vec<Option<Position>>) {
    let mut set = RoaringTreemap::new();
    let mut positions_by_id: Vec<Option<Position>> = vec![None; mapping.len()];

    for (id, pos) in positions {
        let mapped = mapping[id] as usize;
        set.insert(mapped as u64);
        positions_by_id[mapped] = Some(*pos);
    }

    (set, positions_by_id)
}

fn bench_graph_diff(c: &mut Criterion) {
    let context = build_id_pool(200_000, 7);
    let data = build_bench_data_with_rate(&context, 100_000, 0.01, 42);

    c.bench_function("graph_diff_sorted_merge", |b| {
        b.iter(|| {
            let changes = diff_sorted_merge(black_box(&data.old_vec), black_box(&data.new_vec));
            black_box(changes);
        })
    });

    c.bench_function("graph_diff_btree_merge", |b| {
        b.iter(|| {
            let changes = diff_btree(black_box(&data.old_btree), black_box(&data.new_btree));
            black_box(changes);
        })
    });

    c.bench_function("graph_diff_roaring", |b| {
        b.iter(|| {
            let changes = diff_roaring(
                black_box(&data.old_set),
                black_box(&data.new_set),
                black_box(&data.old_positions_by_id),
                black_box(&data.new_positions_by_id),
            );
            black_box(changes);
        })
    });

    let rates = [0.001_f64, 0.01, 0.05, 0.2, 0.5];
    let size = 100_000usize;

    let mut group = c.benchmark_group("graph_diff_rates_sorted_merge");
    for rate in rates {
        let data = build_bench_data_with_rate(&context, size, rate, 42);
        group.bench_with_input(format!("{:.3}", rate), &data, |b, data| {
            b.iter(|| {
                let changes = diff_sorted_merge(black_box(&data.old_vec), black_box(&data.new_vec));
                black_box(changes);
            })
        });
    }
    group.finish();

    let mut rng = StdRng::seed_from_u64(123);
    let mut pool = context.pool.clone();
    pool.shuffle(&mut rng);
    let old_ids = pool.into_iter().take(size).collect::<Vec<_>>();
    let old_positions = build_positions(&old_ids, &mut rng);
    let (_new_ids, new_positions) =
        mutate_positions(&old_ids, &old_positions, &context.pool, 0.01, &mut rng);

    let mut group = c.benchmark_group("graph_diff_build_structures");
    group.bench_function("sorted_vec", |b| {
        b.iter(|| {
            let vec = build_sorted_vec(black_box(&old_positions));
            black_box(vec);
        })
    });

    group.bench_function("btree_map", |b| {
        b.iter(|| {
            let map = build_btree_map(black_box(&old_positions));
            black_box(map);
        })
    });

    group.bench_function("roaring_old", |b| {
        b.iter(|| {
            let (set, positions) =
                build_roaring_structures(black_box(&old_positions), &context.mapping);
            black_box(set);
            black_box(positions);
        })
    });

    group.bench_function("roaring_new", |b| {
        b.iter(|| {
            let (set, positions) =
                build_roaring_structures(black_box(&new_positions), &context.mapping);
            black_box(set);
            black_box(positions);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_graph_diff);
criterion_main!(benches);
