use super::*;
use crate::{Geometry, Value, wkb};
use rustgis_geom::Extent;
use std::io::BufReader;

/// A file of `n` points, scattered by a fixed generator over most of the world, with a name and a number each.
fn write(path: &Path, n: usize) {
    use flatgeobuf::geozero::{ColumnValue, PropertyProcessor};
    use flatgeobuf::{ColumnType, FgbWriter, GeometryType as G};
    let mut w = FgbWriter::create("scatter", G::Point).unwrap();
    w.add_column("name", ColumnType::String, |_, _| {});
    w.add_column("value", ColumnType::Long, |_, _| {});
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 0..n {
        let c = rustgis_geom::Coord::new(-170.0 + 340.0 * next(), -80.0 + 160.0 * next());
        let g = flatgeobuf::geozero::wkb::Wkb(wkb::encode(&Geometry::Point(c)));
        let name = format!("point number {i}");
        w.add_feature_geom(g, |f| {
            f.property(0, "name", &ColumnValue::String(&name)).unwrap();
            f.property(1, "value", &ColumnValue::Long(i as i64)).unwrap();
        })
        .unwrap();
    }
    w.write(std::fs::File::create(path).unwrap()).unwrap();
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rustgis-fgb-resident-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn crate_way(src: &FgbSource, path: &Path, req: &Request) -> Batch {
    src.read_with(&|| Ok(BufReader::new(File::open(path)?)), req, false).unwrap()
}

fn same(a: &Batch, b: &Batch, what: &str) {
    assert_eq!((a.matched, a.complete, a.skipped), (b.matched, b.complete, b.skipped), "{what}");
    assert_eq!(a.features.len(), b.features.len(), "{what}");
    assert!(a.features == b.features, "{what}: the features differ");
}

fn requests() -> Vec<(&'static str, Request)> {
    let e = |a, b, c, d| Some(Extent::new(a, b, c, d));
    let mut v = Vec::new();
    for limit in [40usize, 700, 3000, 100_000] {
        for (name, extent) in [
            ("no extent", None),
            ("the world", e(-180.0, -90.0, 180.0, 90.0)),
            ("past the data", e(-400.0, -200.0, 400.0, 200.0)),
            ("a quarter", e(-100.0, -40.0, 0.0, 40.0)),
            ("a sliver", e(10.0, 10.0, 10.5, 60.0)),
            ("an empty corner", e(179.5, 89.0, 179.9, 89.9)),
        ] {
            v.push((name, Request { extent, limit }));
        }
    }
    v
}

fn plans() -> Vec<Plan> {
    vec![
        // Threads and runs small enough that 6,000 points make many of both; reads in spans, then one by one.
        Plan { threads: 3, run: 100, dense_gap: u64::MAX, span: 2048, near: 64 },
        Plan { threads: 1, run: 4096, dense_gap: 0, span: 1 << 20, near: 16 },
        Plan { threads: 8, run: 7, dense_gap: 0, span: 4096, near: 1 },
    ]
}

#[test]
fn a_read_from_the_table_is_the_crates_read() {
    let path = scratch("scatter.fgb");
    write(&path, 6000);
    let src = FgbSource::open_path(&path, "scatter").unwrap();
    let table = Table::build(&path, 6000).unwrap();
    for (name, req) in requests() {
        let want = crate_way(&src, &path, &req);
        for (i, plan) in plans().iter().enumerate() {
            let got = src.answer(&table, &path, &req, plan).unwrap();
            same(&got, &want, &format!("{name}, limit {}, plan {i}", req.limit));
        }
    }
    // The sample really is a sample, and the others are the whole.
    let sampled = src.answer(&table, &path, &Request { extent: None, limit: 700 }, &plans()[0]).unwrap();
    assert!(!sampled.complete && sampled.features.len() <= 700 && sampled.matched == 6000);
    let whole = src.answer(&table, &path, &Request { extent: None, limit: 100_000 }, &plans()[0]).unwrap();
    assert!(
        whole.complete && whole.features.len() == 6000 && whole.features[5].id == 6 && matches!(whole.features[5].attrs[1], Value::Int(_))
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn spliced_reads_and_seeks_like_a_file() {
    let head = [1u8, 2, 3];
    let mut s = Spliced { head: &head, gap: 4, body: vec![9, 8, 7, 6], pos: 0 };
    let mut all = Vec::new();
    s.read_to_end(&mut all).unwrap();
    assert_eq!(all, [1, 2, 3, 0, 0, 0, 0, 9, 8, 7, 6]);
    assert_eq!(s.seek(SeekFrom::Start(2)).unwrap(), 2);
    assert_eq!(s.seek(SeekFrom::Current(5)).unwrap(), 7);
    let mut two = [0u8; 2];
    s.read_exact(&mut two).unwrap();
    assert_eq!(two, [9, 8]);
    assert_eq!(s.seek(SeekFrom::End(-1)).unwrap(), 10);
    assert!(s.seek(SeekFrom::Current(-20)).is_err());
    // Past the end reads nothing.
    s.seek(SeekFrom::Start(50)).unwrap();
    assert_eq!(s.read(&mut two).unwrap(), 0);
}

#[test]
fn a_file_that_is_not_what_its_index_says_is_left_to_the_crate() {
    let path = scratch("hostile.fgb");
    write(&path, 3000);
    let good = std::fs::read(&path).unwrap();
    assert!(Table::build(&path, 3000).is_some());
    let table = Table::build(&path, 3000).unwrap();
    let src = FgbSource::open_path(&path, "scatter").unwrap();
    // The leaves of the index: the last `count` nodes of it, after the head.
    let head = table.head.len();
    let leaves = head + table.index_len as usize - 3000 * NODE_BYTES;
    let broken = |name: &str, f: &dyn Fn(&mut Vec<u8>)| {
        let p = scratch(name);
        let mut bytes = good.clone();
        f(&mut bytes);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    // Offsets that do not climb; a header size that runs off the file; a file cut inside the feature section; no magic.
    let p = broken("flat.fgb", &|b| b[leaves + 5 * NODE_BYTES + 32..leaves + 5 * NODE_BYTES + 40].copy_from_slice(&0u64.to_le_bytes()));
    assert!(Table::build(&p, 3000).is_none());
    let p = broken("header.fgb", &|b| b[8..12].copy_from_slice(&u32::MAX.to_le_bytes()));
    assert!(Table::build(&p, 3000).is_none());
    let p = broken("cut.fgb", &|b| b.truncate(table.features_at as usize + 1000));
    assert!(Table::build(&p, 3000).is_none());
    let p = broken("magic.fgb", &|b| b[0] = b'x');
    assert!(Table::build(&p, 3000).is_none());
    // A record whose size prefix is nonsense, or that is not a feature: the read gives up, and does not panic.
    let at = table.features_at as usize + table.nodes[1500].offset as usize;
    let p = broken("size.fgb", &|b| b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes()));
    let req = Request { extent: None, limit: 100_000 };
    assert!(src.answer(&table, &p, &req, &plans()[0]).is_none());
    let p = broken("noise.fgb", &|b| b[at + 4..at + 40].fill(0xff));
    assert!(src.answer(&table, &p, &req, &plans()[1]).is_none());
    // A file that changed under the table is not read through it.
    assert!(table.still_current(&path));
    let mut longer = good.clone();
    longer.push(0);
    std::fs::write(&path, longer).unwrap();
    assert!(!table.still_current(&path));
    for name in ["flat", "header", "cut", "magic", "size", "noise"] {
        let _ = std::fs::remove_file(scratch(&format!("{name}.fgb")));
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_small_file_gets_no_table_and_a_made_one_keeps_its_bytes_until_dropped() {
    let path = scratch("small.fgb");
    write(&path, 1000);
    assert!(Table::load(&path, 1000).is_none());
    let before = HELD.load(Ordering::Relaxed);
    let mut held = Table::build(&path, 1000).unwrap();
    held.held = 123;
    HELD.fetch_add(123, Ordering::Relaxed);
    drop(held);
    // No other test loads a table (the others build theirs outside the budget), so the 123 bytes went back.
    assert_eq!(HELD.load(Ordering::Relaxed), before);
    let _ = std::fs::remove_file(path);
}
