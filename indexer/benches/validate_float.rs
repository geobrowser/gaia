use criterion::{criterion_group, criterion_main, Criterion};
use indexer::validators::validate_float::{validate_float, validate_float_comprehensive};

fn benchmark_validate_float(c: &mut Criterion) {
    let mut group = c.benchmark_group("float_validation");
    
    let inputs = [
        "123.45",       // Valid float
        "0.0",          // Valid zero
        "-42.75",       // Valid negative
        "1e10",         // Valid scientific notation
        "",             // Empty string
        "abc",          // Invalid characters
        "123..456",     // Multiple decimal points
        "123.456.789",  // Multiple decimal points
        "1.2e-3",       // Scientific notation with negative exponent
        "Infinity",     // Special float value
        "NaN",          // Not a Number
        "1,234.56",     // Comma as thousand separator (invalid in Rust)
    ];

    // Benchmark the simple validator
    group.bench_function("simple_validate_float", |b| {
        b.iter(|| {
            for &input in &inputs {
                let _ = validate_float(std::hint::black_box(input));
            }
        })
    });

    // Benchmark the comprehensive validator
    group.bench_function("comprehensive_validate_float", |b| {
        b.iter(|| {
            for &input in &inputs {
                let _ = validate_float_comprehensive(std::hint::black_box(input));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_validate_float);
criterion_main!(benches);