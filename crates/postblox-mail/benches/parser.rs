use criterion::{black_box, criterion_group, criterion_main, Criterion};
use postblox_mail::parser::{parse_with_options, ParseOptions};

const SIMPLE_TEXT: &[u8] = include_bytes!("../tests/fixtures/simple_text.eml");
const MULTIPART: &[u8] = include_bytes!("../tests/fixtures/multipart.eml");
const ATTACHMENT_MULTIPART: &[u8] = include_bytes!("../tests/fixtures/attachment_multipart.eml");

fn bench_parser(c: &mut Criterion) {
    let options = ParseOptions::without_raw_headers();
    let mut group = c.benchmark_group("parse");
    let fixtures = [
        ("simple_text_without_raw_headers", SIMPLE_TEXT),
        ("multipart_without_raw_headers", MULTIPART),
        ("attachment_without_raw_headers", ATTACHMENT_MULTIPART),
    ];

    for (name, fixture) in fixtures {
        group.bench_with_input(name, fixture, |b, input| {
            b.iter(|| parse_with_options(black_box(input), options).expect("fixture parses"));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parser);
criterion_main!(benches);
