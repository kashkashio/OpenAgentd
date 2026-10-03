//! Image-cap benchmark: the old read flow vs the new one, per-stage costs,
//! and the per-request path. Release build only:
//!
//! ```bash
//! cd appv3 && cargo test --release -p appv3-providers --test image_bench -- --ignored --nocapture
//! ```
//!
//! Fixtures are synthetic and seeded, so runs are reproducible. Each case
//! reports median / p95 in ms; budgets from the plan are checked at the end.

use appv3_providers::images::{self, fixtures, MAX_IMAGE_EDGE};
use appv3_providers::{ChatMessage, ContentBlock, MessageMeta};
use base64::Engine;
use fast_image_resize as fr;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::{DynamicImage, ImageEncoder, ImageFormat, ImageReader};
use std::io::Cursor;
use std::time::{Duration, Instant};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Clone, Copy)]
struct Stat {
    median: f64,
    p95: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn measure(warmup: usize, iters: usize, mut f: impl FnMut()) -> Stat {
    for _ in 0..warmup {
        f();
    }
    let mut v: Vec<f64> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            ms(t.elapsed())
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pick = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    Stat { median: pick(0.5), p95: pick(0.95) }
}

fn row(name: &str, s: Stat, extra: &str) {
    println!("| {name:<52} | {:>9.3} | {:>9.3} | {extra}", s.median, s.p95);
}

fn header(title: &str) {
    println!("\n### {title}\n| {:<52} | {:>9} | {:>9} | notes", "case", "median ms", "p95 ms");
    println!("|{}|{}|{}|---", "-".repeat(54), "-".repeat(11), "-".repeat(11));
}

struct Fixture {
    name: &'static str,
    bytes: Vec<u8>,
    iters: usize,
}

fn fixtures_set() -> Vec<Fixture> {
    vec![
        Fixture { name: "A 1280x800 PNG screenshot (fits)", bytes: fixtures::png(1280, 800), iters: 20 },
        Fixture { name: "B 2880x1800 PNG retina screenshot", bytes: fixtures::png(2880, 1800), iters: 20 },
        Fixture { name: "C 4032x3024 JPEG photo", bytes: fixtures::jpeg(4032, 3024), iters: 20 },
        Fixture { name: "D 6000x4000 PNG noise (worst case)", bytes: fixtures::encode(&fixtures::noise(6000, 4000), ImageFormat::Png), iters: 5 },
    ]
}

fn decode(bytes: &[u8]) -> DynamicImage {
    ImageReader::new(Cursor::new(bytes)).with_guessed_format().unwrap().decode().unwrap()
}

fn target(w: u32, h: u32) -> (u32, u32) {
    let s = MAX_IMAGE_EDGE as f64 / w.max(h) as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}

fn fir_resize_with(rgb: &[u8], (w, h): (u32, u32), (tw, th): (u32, u32), filter: fr::FilterType) -> Vec<u8> {
    let src = fr::images::Image::from_vec_u8(w, h, rgb.to_vec(), fr::PixelType::U8x3).unwrap();
    let mut dst = fr::images::Image::new(tw, th, fr::PixelType::U8x3);
    fr::Resizer::new().resize(&src, &mut dst, &fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(filter))).unwrap();
    dst.into_vec()
}

fn fir_resize(rgb: &[u8], dims: (u32, u32), to: (u32, u32)) -> Vec<u8> {
    fir_resize_with(rgb, dims, to, images::RESIZE_FILTER)
}

fn png_encode(rgb: &[u8], (w, h): (u32, u32), c: CompressionType) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, c, PngFilter::Adaptive).write_image(rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
    out
}

fn tool_msg(parts: Vec<ContentBlock>) -> ChatMessage {
    ChatMessage::Tool { content: Some("[Image]".into()), tool_call_id: "c".into(), name: Some("read".into()), parts: Some(parts), meta: MessageMeta::default() }
}

fn image_part(bytes: &[u8]) -> ContentBlock {
    ContentBlock::ImageData { data: B64.encode(bytes).into(), media_type: "image/png".into() }
}

/// A distinct 2880×1800 screenshot per `i` (so the resize cache cannot hit).
fn distinct_b(i: u32) -> Vec<u8> {
    let mut img = fixtures::screenshot(2880, 1800).into_rgb8();
    img.put_pixel(i, 0, image::Rgb([i as u8, 0, 0]));
    fixtures::encode(&DynamicImage::ImageRgb8(img), ImageFormat::Png)
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn image_cap_benchmark() {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    println!("# image cap benchmark — {} {} cores, debug_assertions={}", std::env::consts::ARCH, cores, cfg!(debug_assertions));
    let fx = fixtures_set();
    let dir = std::env::temp_dir().join(format!("oad-image-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut budgets: Vec<(String, f64, f64)> = vec![];

    // 1. Read flow: old (fs::read + base64) vs new (fs::read + fit + base64).
    header("1. read flow: old vs new");
    let mut single_b_new = 0.0;
    for f in &fx {
        let path = dir.join(f.name.split(' ').next().unwrap());
        std::fs::write(&path, &f.bytes).unwrap();
        let old = measure(3, f.iters, || {
            let raw = std::fs::read(&path).unwrap();
            std::hint::black_box(B64.encode(&raw));
        });
        let mut out_len = 0;
        let new = measure(3, f.iters, || {
            let raw = std::fs::read(&path).unwrap();
            let bytes = images::fit_image_bytes(&raw, MAX_IMAGE_EDGE).map(|f| f.bytes).unwrap_or(raw);
            out_len = bytes.len();
            std::hint::black_box(B64.encode(&bytes));
        });
        row(&format!("{} — old", f.name), old, &format!("in {} KB", f.bytes.len() / 1024));
        row(&format!("{} — new", f.name), new, &format!("out {} KB, Δ median {:+.3} ms", out_len / 1024, new.median - old.median));
        match f.name.chars().next() {
            Some('A') => budgets.push(("read flow A (fits): Δ vs old".into(), new.median - old.median, 0.5)),
            Some('B') => {
                single_b_new = new.median;
                budgets.push(("read flow B (2880x1800 PNG): new total".into(), new.median, 120.0));
            }
            Some('C') => budgets.push(("read flow C (4032x3024 JPEG): new total".into(), new.median, 150.0)),
            _ => {}
        }
    }

    // 2. Stage breakdown.
    header("2. stage breakdown (new flow)");
    for f in fx.iter().filter(|f| f.name.starts_with('B') || f.name.starts_with('C')) {
        let img = decode(&f.bytes);
        let dims = (img.width(), img.height());
        let to = target(dims.0, dims.1);
        let rgb = img.to_rgb8().into_raw();
        let resized = fir_resize(&rgb, dims, to);
        let is_jpeg = f.name.contains("JPEG");
        row(
            &format!("{} — header", f.name),
            measure(3, f.iters, || {
                std::hint::black_box(images::dimensions(&f.bytes));
            }),
            "",
        );
        row(&format!("{} — decode", f.name), measure(3, f.iters, || drop(std::hint::black_box(decode(&f.bytes)))), "");
        row(&format!("{} — to rgb8", f.name), measure(3, f.iters, || drop(std::hint::black_box(img.to_rgb8()))), "");
        row(
            &format!("{} — resize (fast_image_resize)", f.name),
            measure(3, f.iters, || drop(std::hint::black_box(fir_resize(&rgb, dims, to)))),
            &format!("{}x{} → {}x{}", dims.0, dims.1, to.0, to.1),
        );
        let enc = if is_jpeg {
            measure(3, f.iters, || {
                let mut out = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).write_image(&resized, to.0, to.1, image::ExtendedColorType::Rgb8).unwrap();
                std::hint::black_box(out);
            })
        } else {
            measure(3, f.iters, || drop(std::hint::black_box(png_encode(&resized, to, images::PNG_COMPRESSION))))
        };
        row(&format!("{} — encode ({})", f.name, if is_jpeg { "JPEG q85".to_string() } else { format!("PNG {:?}", images::PNG_COMPRESSION) }), enc, "");
        let out = images::fit_image_bytes(&f.bytes, MAX_IMAGE_EDGE).unwrap().bytes;
        row(&format!("{} — base64 of output", f.name), measure(3, f.iters, || drop(std::hint::black_box(B64.encode(&out)))), "");
    }

    // 3. Resize crate choice.
    header("3. resize: fast_image_resize (shipped filter) vs image::imageops (CatmullRom), then fir filters");
    for f in fx.iter().filter(|f| f.name.starts_with('B') || f.name.starts_with('C')) {
        let img = decode(&f.bytes);
        let dims = (img.width(), img.height());
        let to = target(dims.0, dims.1);
        let rgb = img.to_rgb8();
        let raw = rgb.as_raw().clone();
        let fir = measure(3, f.iters, || drop(std::hint::black_box(fir_resize(&raw, dims, to))));
        let imgops = measure(3, f.iters.min(10), || drop(std::hint::black_box(image::imageops::resize(&rgb, to.0, to.1, image::imageops::FilterType::CatmullRom))));
        row(&format!("{} — fast_image_resize {:?}", f.name, images::RESIZE_FILTER), fir, "");
        row(&format!("{} — image::imageops::resize", f.name), imgops, &format!("{:.1}× slower", imgops.median / fir.median));
        for (label, filter) in [
            ("Box", fr::FilterType::Box),
            ("Bilinear", fr::FilterType::Bilinear),
            ("Hamming", fr::FilterType::Hamming),
            ("CatmullRom", fr::FilterType::CatmullRom),
            ("Lanczos3", fr::FilterType::Lanczos3),
        ] {
            row(&format!("{} — fir {label}", f.name), measure(3, f.iters, || drop(std::hint::black_box(fir_resize_with(&raw, dims, to, filter)))), "");
        }
    }

    // 4. PNG compression level (decides Fast vs Default).
    header("4. PNG compression on B's resized output");
    let b = &fx[1];
    let img = decode(&b.bytes);
    let dims = (img.width(), img.height());
    let to = target(dims.0, dims.1);
    let resized = fir_resize(&img.to_rgb8().into_raw(), dims, to);
    let default_len = png_encode(&resized, to, CompressionType::Default).len();
    for (label, c) in [
        ("Fast", CompressionType::Fast),
        ("Level(1)", CompressionType::Level(1)),
        ("Level(2)", CompressionType::Level(2)),
        ("Level(3)", CompressionType::Level(3)),
        ("Default", CompressionType::Default),
    ] {
        let len = png_encode(&resized, to, c).len();
        row(
            &format!("PNG {label}"),
            measure(3, 20, || drop(std::hint::black_box(png_encode(&resized, to, c)))),
            &format!("{} KB ({:.2}× Default)", len / 1024, len as f64 / default_len as f64),
        );
    }
    let chosen = images::fit_image_bytes(&b.bytes, MAX_IMAGE_EDGE).unwrap().bytes.len();
    println!("chosen encoder: {} KB = {:.2}× Default (rule: ≤ 1.50×)", chosen / 1024, chosen as f64 / default_len as f64);
    budgets.push(("chosen PNG size / Default size".into(), chosen as f64 / default_len as f64, 1.5));

    // 5. Per-request path.
    header("5. fit_request / fit_tool_parts");
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let small: Vec<ChatMessage> = (0..100).map(|_| tool_msg(vec![image_part(&fx[0].bytes)])).collect();
    let noop = measure(3, 20, || {
        let (m, n) = rt.block_on(images::fit_request(small.clone()));
        assert_eq!(n, 0);
        std::hint::black_box(m);
    });
    let clone_only = measure(3, 20, || drop(std::hint::black_box(small.clone())));
    let noop_net = noop.median - clone_only.median;
    row("fit_request, 100 small images (no-op)", noop, &format!("{noop_net:.3} ms net of the bench's own clone"));
    budgets.push(("fit_request no-op, 100 small images".into(), noop_net, 5.0));

    let bs: Vec<ContentBlock> = (0..20).map(|i| image_part(&distinct_b(i))).collect();
    let cold = measure(1, 5, || {
        let mut parts = bs.clone();
        assert_eq!(rt.block_on(images::fit_tool_parts(&mut parts)), 20);
    });
    // Hardware ceiling: images/s with one plain thread per core (no tokio),
    // so mixed performance/efficiency cores are measured, not assumed equal.
    let b_bytes = &fx[1].bytes;
    let mut throughput = 0.0;
    for n in [1usize, 2, 4, cores] {
        let per = 4;
        let t = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..n {
                s.spawn(|| {
                    for _ in 0..per {
                        std::hint::black_box(images::fit_image_bytes(b_bytes, MAX_IMAGE_EDGE));
                    }
                });
            }
        });
        throughput = (n * per) as f64 / t.elapsed().as_secs_f64();
        println!("raw threads={n}: {throughput:.1} B-images/s");
    }
    let ceiling = 20.0 / throughput * 1000.0;
    let naive = (20.0 / cores as f64).ceil() * single_b_new * 1.5;
    row(
        "20 × B cold (parallel, uncached)",
        cold,
        &format!("hardware ceiling {ceiling:.1} ms; {:.1}× faster than sequential; equal-cores formula {naive:.1} ms (not asserted)", 20.0 * single_b_new / cold.median),
    );
    budgets.push(("20 × B cold ≤ 1.25 × measured hardware ceiling".into(), cold.median, ceiling * 1.25));

    let history = vec![tool_msg(bs.clone())];
    let t = Instant::now();
    let (_, n) = rt.block_on(images::fit_request(history.clone()));
    assert_eq!(n, 20);
    println!("fit_request 20 × B first call (fills the cache): {:.1} ms", ms(t.elapsed()));
    let cached = measure(2, 20, || {
        let (m, n) = rt.block_on(images::fit_request(history.clone()));
        assert_eq!(n, 20);
        std::hint::black_box(m);
    });
    row("fit_request 20 × B cached", cached, "");
    budgets.push(("fit_request 20 × B cached".into(), cached.median, 20.0));

    // 6. Header check alone.
    header("6. header check (dimensions_b64)");
    for f in &fx {
        let data = B64.encode(&f.bytes);
        row(
            f.name,
            measure(3, 50, || {
                std::hint::black_box(images::dimensions_b64(&data));
            }),
            "",
        );
    }

    std::fs::remove_dir_all(&dir).ok();
    println!("\n### budgets\n| budget | measured | limit | result |\n|---|---|---|---|");
    let mut failed = vec![];
    for (name, v, limit) in &budgets {
        let ok = v <= limit;
        println!("| {name} | {v:.3} | {limit:.3} | {} |", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failed.push(name.clone());
        }
    }
    assert!(failed.is_empty(), "budgets missed: {failed:?}");
}
