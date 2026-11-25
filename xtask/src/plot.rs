use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::root;

const W: f64 = 1900.0;
const H: f64 = 980.0;
const BG: &str = "#0d0d0d";
const FG: &str = "#f2f2f2";
const FG_SOFT: &str = "#c9c9c9";
const FG_DIM: &str = "#8b8b8b";
const GRID: &str = "#2b2b2b";
const AXIS: &str = "#3a3a3a";
const BAR: &str = "#454545";
const BAR_ACCENT: &str = "#3b82f6";

const PLOT_LEFT: f64 = 382.0;
const PLOT_RIGHT: f64 = 1540.0;
const PLOT_W: f64 = PLOT_RIGHT - PLOT_LEFT;
const LABEL_X: f64 = 352.0;
const ROW_TOP: f64 = 240.0;
const ROW_PITCH: f64 = 100.0;
const BAR_H: f64 = 50.0;

const FONT: &str = "Inter, 'Segoe UI', 'Helvetica Neue', 'DejaVu Sans', Arial, sans-serif";
const META: &str =
    "simdsel 0.1 · Rust v1.99.1 release · AMD Ryzen 5 5600 (Zen 3, AVX2, single core)";
const AXIS_TITLE: &str = "Extraction speed (GB/s per core, higher is better)";

const ENGINES: [(&str, &str, &str); 7] = [
    ("none", "simdsel", "Rust"),
    ("spec", "simdsel (spec)", "Rust"),
    ("dom_query", "dom_query", "Rust"),
    ("selectolax", "selectolax", "Python"),
    ("scraper", "scraper", "Rust"),
    ("parsel", "parsel", "Python"),
    ("beautifulsoup4", "beautifulsoup4", "Python"),
];

struct CorpusData {
    key: &'static str,
    bytes: f64,
    docs: f64,
    times: Vec<Time>,
}

struct Time {
    key: &'static str,
    seconds: f64,
    group: &'static str,
    bench: &'static str,
}

struct ChartSpec {
    corpus: &'static str,
    out: &'static str,
    title: &'static str,
    subtitle: &'static str,
}

const CHARTS: [ChartSpec; 2] = [
    ChartSpec {
        corpus: "cc",
        out: "cc",
        title: "Main Content & Link Extraction",
        subtitle: "Common Crawl dataset stratified (CC-MAIN-2026-43, October 2026)",
    },
    ChartSpec {
        corpus: "swde",
        out: "swde",
        title: "Structured Data Extraction",
        subtitle: "SWDE dataset, tag heavy documents",
    },
];

fn table() -> Vec<CorpusData> {
    vec![
        CorpusData {
            key: "cc",
            bytes: 15_898_182.0,
            docs: 56.0,
            times: vec![
                Time {
                    key: "none",
                    seconds: 0.012713,
                    group: "extract_commoncrawl",
                    bench: "simdhtml-None",
                },
                Time {
                    key: "spec",
                    seconds: 0.034555,
                    group: "extract_commoncrawl",
                    bench: "simdhtml-Spec",
                },
                Time {
                    key: "dom_query",
                    seconds: 0.093123,
                    group: "extract_commoncrawl",
                    bench: "dom_query",
                },
                Time {
                    key: "selectolax",
                    seconds: 0.090699,
                    group: "python_commoncrawl_common",
                    bench: "python-selectolax",
                },
                Time {
                    key: "scraper",
                    seconds: 0.095067,
                    group: "extract_commoncrawl",
                    bench: "scraper",
                },
                Time {
                    key: "parsel",
                    seconds: 0.148228,
                    group: "python_commoncrawl_common",
                    bench: "python-parsel",
                },
                Time {
                    key: "beautifulsoup4",
                    seconds: 2.100715,
                    group: "python_commoncrawl_common",
                    bench: "python-beautifulsoup4",
                },
            ],
        },
        CorpusData {
            key: "swde",
            bytes: 5_017_058.0,
            docs: 64.0,
            times: vec![
                Time {
                    key: "none",
                    seconds: 0.010684,
                    group: "extract_swde",
                    bench: "simdhtml-None",
                },
                Time {
                    key: "spec",
                    seconds: 0.026690,
                    group: "extract_swde",
                    bench: "simdhtml-Spec",
                },
                Time {
                    key: "dom_query",
                    seconds: 0.087097,
                    group: "extract_swde",
                    bench: "dom_query",
                },
                Time {
                    key: "selectolax",
                    seconds: 0.037652,
                    group: "python_swde_per-site",
                    bench: "python-selectolax",
                },
                Time {
                    key: "scraper",
                    seconds: 0.086466,
                    group: "extract_swde",
                    bench: "scraper",
                },
                Time {
                    key: "parsel",
                    seconds: 0.112277,
                    group: "python_swde_per-site",
                    bench: "python-parsel",
                },
                Time {
                    key: "beautifulsoup4",
                    seconds: 1.889298,
                    group: "python_swde_per-site",
                    bench: "python-beautifulsoup4",
                },
            ],
        },
    ]
}

pub fn run(from_table: bool, allow_missing: bool) -> Result<()> {
    let root = root();
    let mut data = table();
    if !from_table {
        let missing = load_from_criterion(&root.join("target/criterion"), &mut data)?;
        if !missing.is_empty() && !allow_missing {
            let list: Vec<String> = missing
                .iter()
                .map(|(corpus, key)| format!("{corpus}/{key}"))
                .collect();
            bail!(
                "no criterion result for {} benchmark(s):\n  {}\nrun `cargo xtask bench --compare` or pass --from-table",
                list.len(),
                list.join("\n  ")
            );
        }
    }

    let out = root.join("figures");
    fs::create_dir_all(&out)?;
    for spec in &CHARTS {
        let corpus = data.iter().find(|d| d.key == spec.corpus).expect("corpus");
        let path = out.join(format!("{}.svg", spec.out));
        fs::write(&path, chart(spec, corpus))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn load_from_criterion(
    criterion: &Path,
    data: &mut [CorpusData],
) -> Result<Vec<(&'static str, &'static str)>> {
    let mut missing = Vec::new();
    for corpus in data.iter_mut() {
        for time in &mut corpus.times {
            let base = criterion.join(time.group).join(time.bench).join("new");
            let estimates = base.join("estimates.json");
            if !estimates.exists() {
                missing.push((corpus.key, time.key));
                continue;
            }
            let json: Value = serde_json::from_str(
                &fs::read_to_string(&estimates)
                    .with_context(|| format!("read {}", estimates.display()))?,
            )?;
            time.seconds = json["median"]["point_estimate"]
                .as_f64()
                .with_context(|| format!("median in {}", estimates.display()))?
                / 1e9;
            let benchmark = base.join("benchmark.json");
            if benchmark.exists() {
                let json: Value = serde_json::from_str(&fs::read_to_string(&benchmark)?)?;
                if let Some(bytes) = json["throughput"]["Bytes"].as_f64() {
                    corpus.bytes = bytes;
                }
            }
        }
    }
    Ok(missing)
}

fn chart(spec: &ChartSpec, data: &CorpusData) -> String {
    let mut rows: Vec<Row> = ENGINES
        .iter()
        .map(|(key, label, lang)| {
            let seconds = data
                .times
                .iter()
                .find(|t| t.key == *key)
                .expect("engine")
                .seconds;
            Row {
                key,
                label,
                lang,
                gb: data.bytes / seconds / 1e9,
                pages: data.docs / seconds,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.gb.total_cmp(&a.gb));

    let (top, ticks) = nice_axis(rows[0].gb);
    let mb_below = if spec.corpus == "swde" { 0.1 } else { 0.01 };
    let height = H.max(ROW_TOP + rows.len() as f64 * ROW_PITCH + 130.0);

    let mut out = Vec::new();
    out.push(format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" ",
            "viewBox=\"0 0 {:.0} {:.0}\" role=\"img\" aria-label=\"{}\">"
        ),
        W,
        height,
        W,
        height,
        escape(spec.title)
    ));
    out.push(format!("<title>{}</title>", escape(spec.title)));
    out.push(format!(
        "<rect width=\"{W:.0}\" height=\"{height:.0}\" fill=\"{BG}\"/>"
    ));
    out.push(text(80.0, 84.0, spec.title, 42.0, FG, 600, "start"));
    out.push(text(
        80.0,
        132.0,
        spec.subtitle,
        24.0,
        FG_SOFT,
        400,
        "start",
    ));
    out.push(text(80.0, 172.0, META, 19.0, FG_DIM, 400, "start"));

    let axis_y = ROW_TOP + rows.len() as f64 * ROW_PITCH - (ROW_PITCH - BAR_H) / 2.0 + 8.0;
    for tick in &ticks {
        let x = PLOT_LEFT + tick / top * PLOT_W;
        out.push(format!(
            "<line x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{axis_y:.1}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
            ROW_TOP - 10.0
        ));
    }
    out.push(format!(
        "<line x1=\"{PLOT_LEFT:.1}\" y1=\"{axis_y:.1}\" x2=\"{PLOT_RIGHT:.1}\" y2=\"{axis_y:.1}\" stroke=\"{AXIS}\" stroke-width=\"1.5\"/>"
    ));
    for tick in &ticks {
        let x = PLOT_LEFT + tick / top * PLOT_W;
        out.push(text(
            x,
            axis_y + 36.0,
            &fmt_tick(*tick),
            21.0,
            FG_DIM,
            400,
            "middle",
        ));
    }

    for (i, row) in rows.iter().enumerate() {
        let cy = ROW_TOP + i as f64 * ROW_PITCH + ROW_PITCH / 2.0;
        let width = row.gb / top * PLOT_W;
        let accent = row.key == "none" || row.key == "spec";
        out.push(format!(
            "<rect x=\"{PLOT_LEFT:.1}\" y=\"{:.1}\" width=\"{width:.1}\" height=\"{BAR_H:.0}\" fill=\"{}\"/>",
            cy - BAR_H / 2.0,
            if accent { BAR_ACCENT } else { BAR }
        ));
        out.push(text(LABEL_X, cy - 6.0, row.label, 30.0, FG, 400, "end"));
        out.push(text(LABEL_X, cy + 25.0, row.lang, 20.0, FG_DIM, 400, "end"));
        let label_x = PLOT_LEFT + width + 20.0;
        if accent {
            out.push(text(
                label_x,
                cy - 4.0,
                &fmt_speed(row.gb, places(row.gb), mb_below),
                25.0,
                FG,
                400,
                "start",
            ));
            out.push(text(
                label_x,
                cy + 22.0,
                &fmt_pages(row.pages),
                20.0,
                FG_DIM,
                400,
                "start",
            ));
        } else {
            out.push(text(
                label_x,
                cy + 9.0,
                &fmt_speed(row.gb, places(row.gb), mb_below),
                25.0,
                FG,
                400,
                "start",
            ));
        }
    }
    out.push(text(
        PLOT_LEFT + PLOT_W / 2.0,
        axis_y + 86.0,
        AXIS_TITLE,
        22.0,
        FG_SOFT,
        400,
        "middle",
    ));
    out.push("</svg>".to_string());
    out.join("\n") + "\n"
}

struct Row<'a> {
    key: &'a str,
    label: &'a str,
    lang: &'a str,
    gb: f64,
    pages: f64,
}

fn nice_axis(maximum: f64) -> (f64, Vec<f64>) {
    let mut steps = Vec::new();
    for k in -4..2 {
        for m in [1.0, 2.0, 2.5, 5.0] {
            steps.push(m * 10f64.powi(k));
        }
    }
    steps.sort_by(|a, b| a.total_cmp(b));
    for step in steps {
        let labels = (maximum / step).ceil() as i64 + 1;
        if labels <= 6 {
            let top = step * (maximum / step).ceil();
            return (top, (0..labels).map(|i| step * i as f64).collect());
        }
    }
    let step = 5.0 * 10f64.powi(1);
    (step * 6.0, (0..7).map(|i| step * i as f64).collect())
}

fn places(value: f64) -> i32 {
    if value > 0.0 {
        (1 - value.log10().floor() as i32).max(2)
    } else {
        2
    }
}

fn fmt_tick(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let s = format!("{value:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn fmt_speed(gb: f64, places: i32, mb_below: f64) -> String {
    if gb < mb_below {
        format!("{:.1} MB/s", gb * 1000.0)
    } else {
        format!("{gb:.places$} GB/s", places = places as usize)
    }
}

fn fmt_pages(pages: f64) -> String {
    if pages >= 100.0 {
        format!("{pages:.0} page/s")
    } else if pages >= 10.0 {
        format!("{pages:.1} page/s")
    } else {
        format!("{pages:.2} page/s")
    }
}

fn text(x: f64, y: f64, s: &str, size: f64, fill: &str, weight: i32, anchor: &str) -> String {
    let mut attrs = format!(
        "x=\"{x:.1}\" y=\"{y:.1}\" font-family=\"{FONT}\" font-size=\"{size:.0}\" fill=\"{fill}\""
    );
    if weight != 400 {
        attrs.push_str(&format!(" font-weight=\"{weight}\""));
    }
    if anchor != "start" {
        attrs.push_str(&format!(" text-anchor=\"{anchor}\""));
    }
    format!("<text {attrs}>{}</text>", escape(s))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
