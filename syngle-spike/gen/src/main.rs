//! Spike A (#43): 1M synthetic points across Australia, as GeoParquet 1.1 (with a bbox covering)
//! and FlatGeobuf.
//!
//! Why clustered: uniform noise draws a flat cost profile; real Observatory data clusters in
//! cities, so the view's culling and the sources' spatial index do real work. 80% of the points
//! cluster around 40 Australian population centres (a Gaussian tail per city), 20% are uniform
//! over the continental box (the "outback" background).
//!
//! Attributes: id, name, category, value, state. Features are emitted in Morton (Z-order) order
//! over the continental box, so consecutive rows are near each other and a 50k-row row group is
//! spatially coherent (a range read fetches one corner of the country, not a stripe of the world).
//!
//! Usage: `cargo run --release -- --count 1000000 --out ../data/points_1m`

use arrow_array::{ArrayRef, BinaryArray, Float32Array, Float64Array, Int64Array, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field as AField, Schema};
use std::sync::Arc;

// --- deterministic PRNG (SplitMix64): the same dataset on every run and machine -------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal, Box-Muller.
    fn normal(&mut self) -> f64 {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// --- the continental box and its population centres -----------------------------------------

const LON0: f64 = 112.0;
const LON1: f64 = 154.0;
const LAT0: f64 = -44.0;
const LAT1: f64 = -10.0;

/// (name, lon, lat, spread in degrees, weight, state) — realistic AU clustering.
const CITIES: [(&str, f64, f64, f64, u32, &str); 40] = [
    ("Sydney", 151.209, -33.868, 0.30, 200, "NSW"),
    ("Melbourne", 144.963, -37.814, 0.28, 190, "VIC"),
    ("Brisbane", 153.028, -27.469, 0.25, 120, "QLD"),
    ("Perth", 115.861, -31.952, 0.24, 110, "WA"),
    ("Adelaide", 138.600, -34.928, 0.20, 80, "SA"),
    ("Gold Coast", 153.400, -28.017, 0.12, 45, "QLD"),
    ("Newcastle", 151.781, -32.928, 0.10, 35, "NSW"),
    ("Canberra", 149.130, -35.280, 0.12, 33, "ACT"),
    ("Sunshine Coast", 153.070, -26.650, 0.10, 28, "QLD"),
    ("Wollongong", 150.893, -34.424, 0.08, 25, "NSW"),
    ("Geelong", 144.360, -38.150, 0.08, 20, "VIC"),
    ("Hobart", 147.327, -42.882, 0.10, 20, "TAS"),
    ("Townsville", 146.817, -19.259, 0.08, 15, "QLD"),
    ("Cairns", 145.771, -16.920, 0.07, 14, "QLD"),
    ("Darwin", 130.841, -12.463, 0.09, 13, "NT"),
    ("Toowoomba", 151.950, -27.560, 0.06, 12, "QLD"),
    ("Ballarat", 143.850, -37.562, 0.06, 11, "VIC"),
    ("Bendigo", 144.280, -36.758, 0.06, 10, "VIC"),
    ("Albury", 146.916, -36.074, 0.05, 9, "NSW"),
    ("Launceston", 147.145, -41.434, 0.05, 8, "TAS"),
    ("Mackay", 149.186, -21.141, 0.05, 8, "QLD"),
    ("Rockhampton", 150.512, -23.379, 0.05, 7, "QLD"),
    ("Bunbury", 115.640, -33.327, 0.04, 6, "WA"),
    ("Coffs Harbour", 153.114, -30.296, 0.05, 6, "NSW"),
    ("Wagga Wagga", 147.370, -35.108, 0.04, 5, "NSW"),
    ("Mildura", 142.162, -34.185, 0.04, 4, "VIC"),
    ("Shepparton", 145.400, -36.380, 0.04, 4, "VIC"),
    ("Port Macquarie", 152.908, -31.430, 0.04, 4, "NSW"),
    ("Geraldton", 114.611, -28.777, 0.04, 3, "WA"),
    ("Kalgoorlie", 121.466, -30.749, 0.04, 3, "WA"),
    ("Alice Springs", 133.880, -23.698, 0.04, 3, "NT"),
    ("Dubbo", 148.601, -32.243, 0.04, 3, "NSW"),
    ("Bathurst", 149.577, -33.419, 0.03, 3, "NSW"),
    ("Mount Gambier", 140.780, -37.829, 0.03, 2, "SA"),
    ("Devonport", 146.351, -41.179, 0.03, 2, "TAS"),
    ("Warrnambool", 142.483, -38.383, 0.03, 2, "VIC"),
    ("Albany", 117.884, -35.023, 0.03, 2, "WA"),
    ("Broome", 122.235, -17.961, 0.03, 2, "WA"),
    ("Armidale", 151.665, -30.514, 0.03, 2, "NSW"),
    ("Whyalla", 137.585, -33.033, 0.03, 1, "SA"),
];

const CATEGORIES: [&str; 12] = [
    "retail", "logistics", "agriculture", "mining", "health", "education", "finance", "tourism", "construction", "utilities", "public", "other",
];

/// Interleave the low 16 bits of two integers (Morton / Z-order code).
fn morton(x: u16, y: u16) -> u32 {
    fn spread(v: u16) -> u32 {
        let mut v = u32::from(v);
        v = (v | (v << 8)) & 0x00FF_00FF;
        v = (v | (v << 4)) & 0x0F0F_0F0F;
        v = (v | (v << 2)) & 0x3333_3333;
        v = (v | (v << 1)) & 0x5555_5555;
        v
    }
    spread(x) | (spread(y) << 1)
}

struct Point {
    lon: f64,
    lat: f64,
    id: i64,
    name: String,
    category: &'static str,
    value: f64,
    state: &'static str,
    key: u32,
}

fn generate(count: usize, seed: u64) -> Vec<Point> {
    let mut rng = Rng::new(seed);
    let total: u32 = CITIES.iter().map(|c| c.4).sum();
    let mut cumulative = Vec::with_capacity(CITIES.len());
    let mut acc = 0;
    for c in CITIES {
        acc += c.4;
        cumulative.push(acc);
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (lon, lat, state) = if rng.f64() < 0.80 {
            let pick = (rng.next_u64() % u64::from(total)) as u32;
            let idx = cumulative.iter().position(|&c| pick < c).unwrap_or(0);
            let (_, clon, clat, spread, _, state) = CITIES[idx];
            // Mostly tight around the centre, with a tail (15% of the points spread 2.5× wider).
            let tail = if rng.f64() < 0.15 { 2.5 } else { 1.0 };
            (clon + rng.normal() * spread * tail, clat + rng.normal() * spread * tail * 0.8, state)
        } else {
            (LON0 + rng.f64() * (LON1 - LON0), LAT0 + rng.f64() * (LAT1 - LAT0), "AU")
        };
        let lon = lon.clamp(LON0 - 0.5, LON1 + 0.5);
        let lat = lat.clamp(LAT0 - 0.5, LAT1 + 0.5);
        let nx = ((lon - LON0) / (LON1 - LON0) * 65_535.0).clamp(0.0, 65_535.0) as u16;
        let ny = ((lat - LAT0) / (LAT1 - LAT0) * 65_535.0).clamp(0.0, 65_535.0) as u16;
        out.push(Point {
            lon,
            lat,
            id: i as i64 + 1,
            name: format!("site-{:07}", i + 1),
            category: CATEGORIES[rng.below(CATEGORIES.len())],
            value: (rng.f64() * 10_000.0 * 100.0).round() / 100.0,
            state,
            key: morton(nx, ny),
        });
    }
    out.sort_unstable_by_key(|p| p.key);
    out
}

fn point_wkb(lon: f64, lat: f64) -> Vec<u8> {
    let mut w = Vec::with_capacity(21);
    w.push(1); // little endian
    w.extend_from_slice(&1u32.to_le_bytes()); // Point
    w.extend_from_slice(&lon.to_le_bytes());
    w.extend_from_slice(&lat.to_le_bytes());
    w
}

fn parquet_schema() -> Schema {
    let bbox_fields: Vec<Arc<AField>> = ["xmin", "ymin", "xmax", "ymax"].map(|n| Arc::new(AField::new(n, DataType::Float32, false))).to_vec();
    Schema::new(vec![
        AField::new("id", DataType::Int64, false),
        AField::new("name", DataType::Utf8, false),
        AField::new("category", DataType::Utf8, false),
        AField::new("value", DataType::Float64, false),
        AField::new("state", DataType::Utf8, false),
        AField::new("geometry", DataType::Binary, false),
        AField::new("geometry_bbox", DataType::Struct(bbox_fields.into_iter().collect()), false),
    ])
}

/// GeoParquet 1.1 metadata: WKB points, and the bbox covering the reader prunes row groups with.
const GEO_METADATA: &str = r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"],"covering":{"bbox":{"xmin":["geometry_bbox","xmin"],"ymin":["geometry_bbox","ymin"],"xmax":["geometry_bbox","xmax"],"ymax":["geometry_bbox","ymax"]}}}}}"#;

fn parquet_batch(points: &[Point], schema: Arc<Schema>) -> Result<RecordBatch, Box<dyn std::error::Error>> {
    let bbox_fields: Vec<Arc<AField>> = ["xmin", "ymin", "xmax", "ymax"].map(|n| Arc::new(AField::new(n, DataType::Float32, false))).to_vec();
    let coord = |f: fn(&Point) -> f64| -> ArrayRef { Arc::new(Float32Array::from(points.iter().map(|p| f(p) as f32).collect::<Vec<_>>())) };
    let bbox = StructArray::new(bbox_fields.into_iter().collect(), vec![coord(|p| p.lon), coord(|p| p.lat), coord(|p| p.lon), coord(|p| p.lat)], None);
    let wkbs: Vec<Vec<u8>> = points.iter().map(|p| point_wkb(p.lon, p.lat)).collect();
    let geometry: BinaryArray = wkbs.iter().map(|w| Some(w.as_slice())).collect();
    let cols: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(points.iter().map(|p| p.id).collect::<Vec<_>>())),
        Arc::new(StringArray::from(points.iter().map(|p| p.name.clone()).collect::<Vec<_>>())),
        Arc::new(StringArray::from(points.iter().map(|p| p.category).collect::<Vec<_>>())),
        Arc::new(Float64Array::from(points.iter().map(|p| p.value).collect::<Vec<_>>())),
        Arc::new(StringArray::from(points.iter().map(|p| p.state).collect::<Vec<_>>())),
        Arc::new(geometry),
        Arc::new(bbox),
    ];
    Ok(RecordBatch::try_new(schema, cols)?)
}

fn write_parquet(points: &[Point], path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let schema = Arc::new(parquet_schema());
    // 50k rows a group: 20 groups for 1M, each a contiguous Morton range (a corner of the map).
    let props = parquet::file::properties::WriterProperties::builder()
        .set_compression(parquet::basic::Compression::SNAPPY)
        .set_max_row_group_row_count(Some(50_000))
        .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue::new("geo".to_string(), GEO_METADATA.to_string())]))
        .build();
    let mut w = parquet::arrow::ArrowWriter::try_new(std::fs::File::create(path)?, schema.clone(), Some(props))?;
    for chunk in points.chunks(50_000) {
        w.write(&parquet_batch(chunk, schema.clone())?)?;
    }
    w.close()?;
    Ok(())
}

fn write_fgb(points: &[Point], path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use geozero::{ColumnValue, PropertyProcessor};
    use flatgeobuf::{ColumnType, FgbWriter, GeometryType as G};
    let mut w = FgbWriter::create_with_options("points_au", G::Point, flatgeobuf::FgbWriterOptions { write_index: true, ..Default::default() })?;
    w.add_column("id", ColumnType::Long, |_, _| {});
    w.add_column("name", ColumnType::String, |_, _| {});
    w.add_column("category", ColumnType::String, |_, _| {});
    w.add_column("value", ColumnType::Double, |_, _| {});
    w.add_column("state", ColumnType::String, |_, _| {});
    for p in points {
        let g = geozero::wkb::Wkb(point_wkb(p.lon, p.lat));
        w.add_feature_geom(g, |f| {
            // Our own generated data: a rejected property would be a bug in this generator.
            f.property(0, "id", &ColumnValue::Long(p.id)).unwrap();
            f.property(1, "name", &ColumnValue::String(&p.name)).unwrap();
            f.property(2, "category", &ColumnValue::String(p.category)).unwrap();
            f.property(3, "value", &ColumnValue::Double(p.value)).unwrap();
            f.property(4, "state", &ColumnValue::String(p.state)).unwrap();
        })?;
    }
    w.write(std::fs::File::create(path)?)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut count = 1_000_000usize;
    let mut out = "../data/points_1m".to_string();
    let mut seed = 43u64;
    let mut skip_fgb = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--count" => {
                count = args.get(i + 1).ok_or("--count needs a number")?.parse()?;
                i += 2;
            }
            "--out" => {
                out = args.get(i + 1).ok_or("--out needs a path prefix")?.clone();
                i += 2;
            }
            "--seed" => {
                seed = args.get(i + 1).ok_or("--seed needs a number")?.parse()?;
                i += 2;
            }
            "--no-fgb" => {
                skip_fgb = true;
                i += 1;
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let t0 = std::time::Instant::now();
    let points = generate(count, seed);
    println!("generated {count} points in {:?}", t0.elapsed());
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let parquet_path = format!("{out}.geoparquet");
    let t = std::time::Instant::now();
    write_parquet(&points, &parquet_path)?;
    let size = std::fs::metadata(&parquet_path)?.len();
    println!("wrote {parquet_path}: {size} bytes ({:.1} MB) in {:?}", size as f64 / 1e6, t.elapsed());
    if !skip_fgb {
        let fgb_path = format!("{out}.fgb");
        let t = std::time::Instant::now();
        write_fgb(&points, &fgb_path)?;
        let size = std::fs::metadata(&fgb_path)?.len();
        println!("wrote {fgb_path}: {size} bytes ({:.1} MB) in {:?}", size as f64 / 1e6, t.elapsed());
    }
    println!("total {:?}", t0.elapsed());
    Ok(())
}
