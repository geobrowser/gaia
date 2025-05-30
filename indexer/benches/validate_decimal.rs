use criterion::{criterion_group, criterion_main, Criterion};
use indexer::validators::validate_decimal::validate_two_decimal_places;

fn benchmark_validate_decimal(c: &mut Criterion) {
    let inputs = ["12.34", "0.99", "100.00", "5.6", "7", "12.345", "abc"];

    c.bench_function("validate_two_decimal_places", |b| {
        b.iter(|| {
            for &input in &inputs {
                let _ = validate_two_decimal_places(std::hint::black_box(input));
            }
        })
    });
}

criterion_group!(benches, benchmark_validate_decimal);
criterion_main!(benches);
