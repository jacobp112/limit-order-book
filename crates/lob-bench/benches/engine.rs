//! Criterion benchmarks: each workload's command batch, timed from a
//! freshly restored state. Throughput is reported per command.

use std::hint::black_box;
use std::time::Duration;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use lob::Event;

fn engine(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    for w in lob_bench::all() {
        group.throughput(Throughput::Elements(w.commands.len() as u64));
        group.bench_function(w.name, |b| {
            b.iter_batched(
                || (w.engine(), Vec::<Event>::with_capacity(256)),
                |(mut engine, mut out)| {
                    for cmd in &w.commands {
                        out.clear();
                        engine.apply(*cmd, &mut out);
                        black_box(&out);
                    }
                    engine
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, engine);
criterion_main!(benches);
