use std::sync::atomic::AtomicU32;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_decimal::Decimal;
use stats_alloc::{Region, StatsAlloc, INSTRUMENTED_SYSTEM};

use tonic_lib::engine::Engine;
use tonic_lib::storage::grid::{Cell, CellValue};
use tonic_lib::storage::types::AbsoluteCellId;

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

fn id(row: u32, col: u32) -> AbsoluteCellId {
    AbsoluteCellId {
        sheet_id: 0,
        row,
        col,
    }
}

// --- scenario builders ---

/// B0=A0+1, B1=B0+1, ..., B(n-1)=B(n-2)+1
/// fully serial dependency chain - worst case for parallelism
fn setup_chain(n: u32) -> Engine {
    let mut engine = Engine::new();
    let g = engine.start_batch();
    engine.insert_value(&g, id(0, 0), CellValue::Number(Decimal::from(1)));
    engine.end_batch(g);

    let g = engine.start_batch();
    engine.parse_and_insert_string(&g, id(0, 1), "=A1+1");
    engine.end_batch(g);

    for i in 1..n {
        let g = engine.start_batch();
        engine.parse_and_insert_string(&g, id(i, 1), &format!("=B{}+1", i));
        engine.end_batch(g);
    }
    engine
}

/// B0..B(n-1) all depend on A0 - maximally parallel, tests worker dispatch
fn setup_independent(n: u32) -> Engine {
    let mut engine = Engine::new();
    let g = engine.start_batch();
    engine.insert_value(&g, id(0, 0), CellValue::Number(Decimal::from(1)));
    engine.end_batch(g);

    for i in 0..n {
        let g = engine.start_batch();
        engine.parse_and_insert_string(&g, id(i, 1), "=A1*2");
        engine.end_batch(g);
    }
    engine
}

/// n values in column A, one sum formula over the entire range.
/// tests range aggregation with large dependency vertex
fn setup_wide_sum(n: u32) -> Engine {
    let mut engine = Engine::new();
    for i in 0..n {
        let g = engine.start_batch();
        engine.insert_value(&g, id(i, 0), CellValue::Number(Decimal::from(i as i64)));
        engine.end_batch(g);
    }
    let g = engine.start_batch();
    engine.parse_and_insert_string(&g, id(0, 1), &format!("=sum(A1:A{})", n));
    engine.end_batch(g);
    engine
}

fn trigger_eval(engine: &mut Engine) {
    let g = engine.start_batch();
    engine.insert_value(&g, id(0, 0), CellValue::Number(Decimal::from(42)));
    engine.eval_batch_for_bench();
}

// --- benchmarks ---

fn bench_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("chain");
    group.sample_size(30);
    group.measurement_time(std::time::Duration::from_secs(10));
    for n in [100, 1_000, 10_000, 50_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut engine = setup_chain(n);
            b.iter(|| {
                trigger_eval(&mut engine);
                black_box(&engine);
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("independent");
    group.sample_size(30);
    group.measurement_time(std::time::Duration::from_secs(10));
    for n in [100, 1_000, 10_000, 50_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut engine = setup_independent(n);
            b.iter(|| {
                trigger_eval(&mut engine);
                black_box(&engine);
            });
        });
    }
    group.finish();

    let mut group = c.benchmark_group("wide_sum");
    group.sample_size(30);
    group.measurement_time(std::time::Duration::from_secs(10));
    for n in [100, 1_000, 10_000, 50_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut engine = setup_wide_sum(n);
            b.iter(|| {
                trigger_eval(&mut engine);
                black_box(&engine);
            });
        });
    }
    group.finish();
}

fn bench_alloc(c: &mut Criterion) {
    for (label, n) in [
        ("chain_1k", 1_000u32),
        ("independent_10k", 10_000),
        ("wide_sum_10k", 10_000),
    ] {
        let mut engine = match label {
            "chain_1k" => setup_chain(n),
            "independent_10k" => setup_independent(n),
            _ => setup_wide_sum(n),
        };

        // warmup
        trigger_eval(&mut engine);

        // measure
        let reg = Region::new(&GLOBAL);
        trigger_eval(&mut engine);
        let s = reg.change();

        let net =
            s.bytes_allocated as i64 - s.bytes_deallocated as i64 + s.bytes_reallocated as i64;
        eprintln!(
            "alloc [{label}]  allocs={} deallocs={} reallocs={}  +{}B -{}B  net={}B",
            s.allocations,
            s.deallocations,
            s.reallocations,
            s.bytes_allocated,
            s.bytes_deallocated,
            net
        );
        assert!(net < 1_000_000, "leak in {label}: {net}B net after eval");
    }
}

criterion_group!(benches, bench_eval, bench_alloc);
criterion_main!(benches);
