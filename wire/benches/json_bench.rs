use criterion::{Criterion, black_box, criterion_group, criterion_main};
use serde_json;
use std::fs;
use wire::deserialize::deserialize_from_json;

fn bench_json_deserialize_basic(c: &mut Criterion) {
    // Load the JSON data once for reuse
    let json_str =
        fs::read_to_string("data/ops.json").expect("Failed to read ops.json file for benchmarking");

    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse JSON for benchmarking");

    c.bench_function("json_deserialize_basic", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(json_value.clone()));
            black_box(result)
        })
    });
}

fn bench_json_deserialize_repeated(c: &mut Criterion) {
    let json_str =
        fs::read_to_string("data/ops.json").expect("Failed to read ops.json file for benchmarking");

    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse JSON for benchmarking");

    let mut group = c.benchmark_group("json_deserialize_repeated");

    group.bench_function("deserialize_1x", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(json_value.clone()));
            black_box(result)
        })
    });

    group.bench_function("deserialize_10x", |b| {
        b.iter(|| {
            for _ in 0..10 {
                let result = deserialize_from_json(black_box(json_value.clone()));
                black_box(result);
            }
        })
    });

    group.finish();
}

fn bench_json_deserialize_memory_patterns(c: &mut Criterion) {
    let json_str =
        fs::read_to_string("data/ops.json").expect("Failed to read ops.json file for benchmarking");

    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse JSON for benchmarking");

    let mut group = c.benchmark_group("json_memory_patterns");

    // Reuse existing JSON value (minimal allocation)
    group.bench_function("reuse_json_value", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(json_value.clone()));
            black_box(result)
        })
    });

    // Parse fresh from string each time
    group.bench_function("parse_fresh_each_time", |b| {
        b.iter(|| {
            let json_value: serde_json::Value =
                serde_json::from_str(black_box(&json_str)).expect("JSON parsing should succeed");
            let result = deserialize_from_json(black_box(json_value));
            black_box(result)
        })
    });

    group.finish();
}

fn bench_json_deserialize_error_cases(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_error_cases");

    // Empty JSON object
    let empty_json = serde_json::json!({});
    group.bench_function("empty_json_object", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(empty_json.clone()));
            black_box(result)
        })
    });

    // JSON null
    let null_json = serde_json::Value::Null;
    group.bench_function("json_null", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(null_json.clone()));
            black_box(result)
        })
    });

    // Wrong type - string instead of object
    let string_json = serde_json::json!("not an object");
    group.bench_function("json_wrong_type_string", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(string_json.clone()));
            black_box(result)
        })
    });

    // Wrong type - array instead of object
    let array_json = serde_json::json!([1, 2, 3]);
    group.bench_function("json_wrong_type_array", |b| {
        b.iter(|| {
            let result = deserialize_from_json(black_box(array_json.clone()));
            black_box(result)
        })
    });

    group.finish();
}

fn bench_json_deserialize_with_clone_cost(c: &mut Criterion) {
    let json_str =
        fs::read_to_string("data/ops.json").expect("Failed to read ops.json file for benchmarking");

    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Failed to parse JSON for benchmarking");

    let mut group = c.benchmark_group("json_clone_cost");

    // Benchmark just the clone operation
    group.bench_function("json_value_clone_only", |b| {
        b.iter(|| {
            let cloned = black_box(json_value.clone());
            black_box(cloned)
        })
    });

    // Benchmark deserialize without clone (using reference, won't work but shows the difference)
    // Note: This won't compile, but shows what we're measuring against

    // Benchmark clone + deserialize
    group.bench_function("clone_and_deserialize", |b| {
        b.iter(|| {
            let cloned = json_value.clone();
            let result = deserialize_from_json(black_box(cloned));
            black_box(result)
        })
    });

    group.finish();
}

criterion_group!(
    json_benches,
    bench_json_deserialize_basic,
    bench_json_deserialize_repeated,
    bench_json_deserialize_memory_patterns,
    bench_json_deserialize_error_cases,
    bench_json_deserialize_with_clone_cost
);

criterion_main!(json_benches);
