use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use landro_chunker::Chunker;

fn quick_chunking_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("quick_chunk");
    
    // Small test for quick validation
    let size = 64 * 1024; // 64KB
    group.throughput(Throughput::Bytes(size as u64));
    
    let chunker = Chunker::default();
    let data = vec![42u8; size];
    
    group.bench_function("64KB_chunking", |b| {
        b.iter(|| {
            let chunks = chunker.chunk_bytes(black_box(&data)).unwrap();
            black_box(chunks);
        });
    });
    
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = quick_chunking_bench
);
criterion_main!(benches);