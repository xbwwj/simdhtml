use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{criterion_dir, meta, root};

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

const CELL_TOP: f64 = 240.0;
const CELL_MARGIN: f64 = 80.0;
const CELL_GAP: f64 = 80.0;
const CELL_COLS: usize = 2;
const CELL_LABEL_W: f64 = 176.0;
const CELL_PLOT_W: f64 = 470.0;
const CELL_PITCH: f64 = 32.0;
const CELL_BAR_H: f64 = 19.0;
const CELL_HEADER: f64 = 40.0;
const CELL_FOOTER: f64 = 46.0;

const FONT: &str = "Inter, 'Segoe UI', 'Helvetica Neue', 'DejaVu Sans', Arial, sans-serif";

struct Engine {
    /// Criterion benchmark id inside the group.
    id: &'static str,
    label: &'static str,
    lang: &'static str,
    python: bool,
}

const ENGINES: [Engine; 7] = [
    Engine {
        id: "simdhtml-None",
        label: "simdsel",
        lang: "Rust",
        python: false,
    },
    Engine {
        id: "simdhtml-Spec",
        label: "simdsel (spec)",
        lang: "Rust",
        python: false,
    },
    Engine {
        id: "dom_query",
        label: "dom_query",
        lang: "Rust",
        python: false,
    },
    Engine {
        id: "python-selectolax",
        label: "selectolax",
        lang: "Python",
        python: true,
    },
    Engine {
        id: "scraper",
        label: "scraper",
        lang: "Rust",
        python: false,
    },
    Engine {
        id: "python-parsel",
        label: "parsel",
        lang: "Python",
        python: true,
    },
    Engine {
        id: "python-beautifulsoup4",
        label: "beautifulsoup4",
        lang: "Python",
        python: true,
    },
];

#[derive(Clone, Copy)]
enum Metric {
    /// Bytes per second, drawn in GB/s.
    Bytes,
    /// Documents per second.
    Pages,
}

enum Layout {
    /// One group of engines over the whole corpus.
    Whole {
        rust: &'static str,
        python: &'static str,
    },
    /// One section per document-size bucket, ordered small to large.
    Buckets {
        rust_prefix: &'static str,
        python_prefix: &'static str,
    },
}

struct ChartSpec {
    out: &'static str,
    title: &'static str,
    subtitle: &'static str,
    axis: &'static str,
    metric: Metric,
    layout: Layout,
}

const CHARTS: [ChartSpec; 3] = [
    ChartSpec {
        out: "cc",
        title: "Main Content & Link Extraction",
        subtitle: "Common Crawl dataset stratified (CC-MAIN-2026-43, October 2026)",
        axis: "Extraction speed (GB/s per core, higher is better)",
        metric: Metric::Bytes,
        layout: Layout::Whole {
            rust: "extract/commoncrawl",
            python: "python/commoncrawl/common",
        },
    },
    ChartSpec {
        out: "cc-size",
        title: "Main Content & Link Extraction",
        subtitle: "Common Crawl dataset grouped by document size (CC-MAIN-2026-43, October 2026)",
        axis: "Extraction speed (pages/s per core, higher is better)",
        metric: Metric::Pages,
        layout: Layout::Buckets {
            rust_prefix: "extract/commoncrawl",
            python_prefix: "python/commoncrawl",
        },
    },
    ChartSpec {
        out: "swde",
        title: "Structured Data Extraction",
        subtitle: "SWDE dataset, tag heavy documents",
        axis: "Extraction speed (GB/s per core, higher is better)",
        metric: Metric::Bytes,
        layout: Layout::Whole {
            rust: "extract/swde",
            python: "python/swde/per-site",
        },
    },
];

struct Section {
    bucket: Option<String>,
    docs: f64,
    bytes: f64,
    rust_group: String,
    python_group: String,
}

struct Row {
    label: &'static str,
    lang: &'static str,
    value: f64,
    pages: f64,
    accent: bool,
}

struct Results {
    times: BTreeMap<String, f64>,
    inputs: Value,
    env: Value,
}

pub fn run(allow_missing: bool) -> Result<()> {
    let dir = criterion_dir();
    let results = Results::load(&dir)?;
    let out = root().join("figures");
    fs::create_dir_all(&out)?;
    for spec in &CHARTS {
        let sections = sections(spec, &results)?;
        let rows: Vec<Vec<Row>> = sections
            .iter()
            .map(|section| rows(section, &results, spec.metric, allow_missing))
            .collect::<Result<_>>()?;
        let svg = match &spec.layout {
            Layout::Whole { .. } => whole_chart(spec, &rows[0], &results.env),
            Layout::Buckets { .. } => bucket_chart(spec, &sections, &rows, &results.env),
        };
        let path = out.join(format!("{}.svg", spec.out));
        fs::write(&path, svg).with_context(|| format!("write {}", path.display()))?;
        println!("wrote {}", path.display());
    }
    Ok(())
}

impl Results {
    fn load(dir: &Path) -> Result<Self> {
        let mut times = BTreeMap::new();
        scan(dir, &mut times)?;
        if times.is_empty() {
            bail!(
                "no criterion results under {}; run `cargo xtask bench --compare`",
                dir.display()
            );
        }
        Ok(Self {
            times,
            inputs: read_json(&dir.join("corpora.json")).unwrap_or(Value::Null),
            env: meta::load(&dir.join("env.json")),
        })
    }

    fn input(&self, group: &str) -> Result<&Value> {
        self.inputs.get(group).with_context(|| {
            format!("no recorded input for `{group}`; re-run `cargo xtask bench --compare`")
        })
    }
}

/// Every result criterion wrote, keyed by its full benchmark id.
fn scan(dir: &Path, times: &mut BTreeMap<String, f64>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let new = path.join("new");
        if !new.is_dir() {
            scan(&path, times)?;
            continue;
        }
        let (Some(estimates), Some(benchmark)) = (
            read_json(&new.join("estimates.json")),
            read_json(&new.join("benchmark.json")),
        ) else {
            continue;
        };
        let id = benchmark.get("full_id").and_then(Value::as_str);
        let median = estimates
            .pointer("/median/point_estimate")
            .and_then(Value::as_f64);
        if let (Some(id), Some(median)) = (id, median) {
            times.insert(id.to_string(), median / 1e9);
        }
    }
    Ok(())
}

fn sections(spec: &ChartSpec, results: &Results) -> Result<Vec<Section>> {
    match &spec.layout {
        Layout::Whole { rust, python } => {
            let input = results.input(rust)?;
            Ok(vec![Section {
                bucket: None,
                docs: number(input, "docs")?,
                bytes: number(input, "bytes")?,
                rust_group: (*rust).to_string(),
                python_group: (*python).to_string(),
            }])
        }
        Layout::Buckets {
            rust_prefix,
            python_prefix,
        } => {
            let Some(entries) = results.inputs.as_object() else {
                bail!("{}/{rust_prefix} missing from corpora.json", rust_prefix);
            };
            let prefix = format!("{rust_prefix}/");
            let mut buckets: Vec<(u64, &str, &Value)> = Vec::new();
            for (group, input) in entries {
                if !group.starts_with(&prefix) || !input["bucket"].is_string() {
                    continue;
                }
                let index = input["bucket_index"].as_u64().unwrap_or(u64::MAX);
                buckets.push((index, group.as_str(), input));
            }
            buckets.sort_by_key(|(index, _, _)| *index);
            if buckets.is_empty() {
                bail!(
                    "no size buckets recorded for `{rust_prefix}/`; run `cargo xtask bench --compare`"
                );
            }
            buckets
                .into_iter()
                .map(|(_, group, input)| {
                    let bucket = string(input, "bucket");
                    Ok(Section {
                        bucket: Some(bucket.clone()),
                        docs: number(input, "docs")?,
                        bytes: number(input, "bytes")?,
                        rust_group: group.to_string(),
                        python_group: format!("{python_prefix}/{bucket}"),
                    })
                })
                .collect()
        }
    }
}

fn rows(
    section: &Section,
    results: &Results,
    metric: Metric,
    allow_missing: bool,
) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for engine in &ENGINES {
        let group = if engine.python {
            &section.python_group
        } else {
            &section.rust_group
        };
        let id = format!("{group}/{}", engine.id);
        match results.times.get(&id) {
            Some(seconds) if *seconds > 0.0 => rows.push(Row {
                label: engine.label,
                lang: engine.lang,
                value: match metric {
                    Metric::Bytes => section.bytes / seconds / 1e9,
                    Metric::Pages => section.docs / seconds,
                },
                pages: section.docs / seconds,
                accent: engine.label.starts_with("simdsel"),
            }),
            _ => missing.push(id),
        }
    }
    if !missing.is_empty() && !allow_missing {
        bail!(
            "no criterion result for {} benchmark(s):\n  {}\nrun `cargo xtask bench --compare` or pass --allow-missing",
            missing.len(),
            missing.join("\n  ")
        );
    }
    rows.sort_by(|a, b| b.value.total_cmp(&a.value));
    Ok(rows)
}

fn whole_chart(spec: &ChartSpec, rows: &[Row], env: &Value) -> String {
    let (top, ticks) = nice_axis(rows[0].value);
    let height = H.max(ROW_TOP + rows.len() as f64 * ROW_PITCH + 130.0);
    let axis_y = ROW_TOP + rows.len() as f64 * ROW_PITCH - (ROW_PITCH - BAR_H) / 2.0 + 8.0;

    let mut out = frame(spec, &meta::line(env), height);

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
        let width = row.value / top * PLOT_W;
        out.push(format!(
            "<rect x=\"{PLOT_LEFT:.1}\" y=\"{:.1}\" width=\"{width:.1}\" height=\"{BAR_H:.0}\" fill=\"{}\"/>",
            cy - BAR_H / 2.0,
            if row.accent { BAR_ACCENT } else { BAR }
        ));
        out.push(text(LABEL_X, cy - 6.0, row.label, 30.0, FG, 400, "end"));
        out.push(text(LABEL_X, cy + 25.0, row.lang, 20.0, FG_DIM, 400, "end"));
        let label_x = PLOT_LEFT + width + 20.0;
        let value = value_label(spec.metric, row);
        if row.accent {
            out.push(text(label_x, cy - 4.0, &value, 25.0, FG, 400, "start"));
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
            out.push(text(label_x, cy + 9.0, &value, 25.0, FG, 400, "start"));
        }
    }
    out.push(text(
        PLOT_LEFT + PLOT_W / 2.0,
        axis_y + 86.0,
        spec.axis,
        22.0,
        FG_SOFT,
        400,
        "middle",
    ));
    out.push("</svg>".to_string());
    out.join("\n") + "\n"
}

fn bucket_chart(spec: &ChartSpec, sections: &[Section], rows: &[Vec<Row>], env: &Value) -> String {
    let widest = rows.iter().map(Vec::len).max().unwrap_or(1);
    let per_col = sections.len().div_ceil(CELL_COLS).max(1);
    let cell_w = (W - 2.0 * CELL_MARGIN - CELL_GAP * (CELL_COLS as f64 - 1.0)) / CELL_COLS as f64;
    let cell_h = CELL_HEADER + widest as f64 * CELL_PITCH + CELL_FOOTER;
    let height = CELL_TOP + per_col as f64 * cell_h + 110.0;

    let mut out = frame(spec, &meta::line(env), height);

    for (index, (section, rows)) in sections.iter().zip(rows).enumerate() {
        let col = index / per_col;
        let line_index = index % per_col;
        let x0 = CELL_MARGIN + col as f64 * (cell_w + CELL_GAP);
        let y0 = CELL_TOP + line_index as f64 * cell_h;
        let plot_left = x0 + CELL_LABEL_W;
        let bars_top = y0 + CELL_HEADER;
        let axis_y = bars_top + rows.len() as f64 * CELL_PITCH + 6.0;

        let bucket = section.bucket.as_deref().unwrap_or("all");
        out.push(text(x0, y0, bucket, 23.0, FG, 600, "start"));

        // Each section gets its own linear scale, so the parsers stay
        // comparable inside a size class.
        let maximum = rows.iter().map(|row| row.value).fold(0.0, f64::max);
        let (top, ticks) = nice_axis(maximum);
        for tick in &ticks {
            let x = plot_left + tick / top * CELL_PLOT_W;
            out.push(format!(
                "<line x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{axis_y:.1}\" stroke=\"{GRID}\" stroke-width=\"1\"/>",
                bars_top - 6.0
            ));
            out.push(text(
                x,
                axis_y + 24.0,
                &fmt_axis_tick(*tick),
                15.0,
                FG_DIM,
                400,
                "middle",
            ));
        }
        out.push(format!(
            "<line x1=\"{plot_left:.1}\" y1=\"{axis_y:.1}\" x2=\"{:.1}\" y2=\"{axis_y:.1}\" stroke=\"{AXIS}\" stroke-width=\"1.5\"/>",
            plot_left + CELL_PLOT_W
        ));

        for (i, row) in rows.iter().enumerate() {
            let cy = bars_top + i as f64 * CELL_PITCH + CELL_PITCH / 2.0;
            let width = (row.value / top * CELL_PLOT_W).max(1.5);
            out.push(format!(
                "<rect x=\"{plot_left:.1}\" y=\"{:.1}\" width=\"{width:.1}\" height=\"{CELL_BAR_H:.0}\" fill=\"{}\"/>",
                cy - CELL_BAR_H / 2.0,
                if row.accent { BAR_ACCENT } else { BAR }
            ));
            out.push(text(
                x0 + CELL_LABEL_W - 12.0,
                cy + 6.0,
                row.label,
                16.0,
                FG,
                400,
                "end",
            ));
            out.push(text(
                plot_left + width + 8.0,
                cy + 6.0,
                &fmt_pages_compact(row.value),
                16.0,
                if row.accent { FG } else { FG_SOFT },
                400,
                "start",
            ));
        }
    }

    out.push(text(
        CELL_MARGIN + (W - 2.0 * CELL_MARGIN) / 2.0,
        height - 34.0,
        spec.axis,
        21.0,
        FG_SOFT,
        400,
        "middle",
    ));
    out.push("</svg>".to_string());
    out.join("\n") + "\n"
}

fn frame(spec: &ChartSpec, meta_line: &str, height: f64) -> Vec<String> {
    vec![
        format!(
            concat!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" ",
                "viewBox=\"0 0 {:.0} {:.0}\" role=\"img\" aria-label=\"{}\">"
            ),
            W,
            height,
            W,
            height,
            escape(spec.title)
        ),
        format!("<title>{}</title>", escape(spec.title)),
        format!("<rect width=\"{W:.0}\" height=\"{height:.0}\" fill=\"{BG}\"/>"),
        text(80.0, 84.0, spec.title, 42.0, FG, 600, "start"),
        text(80.0, 132.0, spec.subtitle, 24.0, FG_SOFT, 400, "start"),
        text(80.0, 172.0, meta_line, 19.0, FG_DIM, 400, "start"),
    ]
}

fn value_label(metric: Metric, row: &Row) -> String {
    match metric {
        Metric::Bytes => fmt_speed(row.value),
        Metric::Pages => fmt_pages(row.value),
    }
}

fn nice_axis(maximum: f64) -> (f64, Vec<f64>) {
    let mut steps = Vec::new();
    for k in -4..8 {
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
    let step = 10f64.powf((maximum / 6.0).log10().ceil());
    (step * 6.0, (0..7).map(|i| step * i as f64).collect())
}

fn places(value: f64) -> usize {
    if value > 0.0 {
        (1 - value.log10().floor() as i32).max(2) as usize
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

fn fmt_speed(gb: f64) -> String {
    if gb < 0.01 {
        format!("{:.1} MB/s", gb * 1000.0)
    } else {
        format!("{gb:.places$} GB/s", places = places(gb))
    }
}

fn fmt_axis_tick(value: f64) -> String {
    if value >= 1_000_000.0 {
        scaled(value, 1e6, "M")
    } else if value >= 1_000.0 {
        scaled(value, 1e3, "k")
    } else {
        fmt_tick(value)
    }
}

fn scaled(value: f64, unit: f64, suffix: &str) -> String {
    let value = value / unit;
    if value.fract().abs() < 0.05 {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}")
    }
}

fn fmt_pages_compact(pages: f64) -> String {
    if pages >= 1_000.0 {
        format!("{} page/s", scaled(pages, 1e3, "k"))
    } else {
        fmt_pages(pages)
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

fn number(value: &Value, key: &str) -> Result<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("`{key}` missing in corpora.json"))
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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
