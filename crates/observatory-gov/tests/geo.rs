//! G-NAF, ABS and ASGS beyond the evaluator's ten-row fixtures: the three boundary formats agreeing on the same shapes, a
//! ZIP of tables the same as a folder, the rules the real files need (aliases, retired rows, blank values, quoted cells), and
//! files that are damaged leaving the index as it was.

use observatory_gov::{Env, run};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obs-gov-geo-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn call(data: &Path, args: &[&str]) -> (Value, u8) {
    let env = Env { gov_dir: Some(data.to_path_buf()), local_data: None, sort_run_bytes: 4096 };
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = run(&args, &env);
    (serde_json::from_str(&out.stdout).unwrap_or(Value::Null), out.exit)
}

fn manifest(dir: &Path, source: &str, files: &[&str]) {
    let list: Vec<String> = files.iter().map(|f| format!("\"{f}\"")).collect();
    fs::write(
        dir.join("manifest.json"),
        format!("{{\"format\":1,\"sources\":{{\"{source}\":{{\"asOf\":\"2026-08-17\",\"files\":[{}]}}}}}}", list.join(",")),
    )
    .unwrap();
}

fn refresh(data: &Path, from: &Path) -> (Value, u8) {
    call(data, &["refresh", "--from", from.to_str().unwrap()])
}

// ---- ASGS: one square with a hole, in GeoJSON, Shapefile and GeoPackage -------------------------------------------------

const OUTER: [[f64; 2]; 5] = [[150.0, -34.0], [152.0, -34.0], [152.0, -33.0], [150.0, -33.0], [150.0, -34.0]];
const HOLE: [[f64; 2]; 5] = [[150.5, -33.8], [150.8, -33.8], [150.8, -33.5], [150.5, -33.5], [150.5, -33.8]];
const POINTS: [(&str, &str, usize); 5] =
    [("-33.9", "151", 1), ("-33.6", "150.6", 0), ("-33.6", "151.5", 1), ("-35", "151", 0), ("-33.5", "151.9", 1)];

fn geojson(dir: &Path) {
    let ring = |r: &[[f64; 2]]| serde_json::json!(r);
    let feature = serde_json::json!({"type": "FeatureCollection", "features": [{"type": "Feature",
        "properties": {"SA2_CODE21": "1", "SA2_NAME21": "Square", "STE_NAME21": "New South Wales"},
        "geometry": {"type": "Polygon", "coordinates": [ring(&OUTER), ring(&HOLE)]}}]});
    fs::create_dir_all(dir.join("asgs")).unwrap();
    fs::write(dir.join("asgs/SA2_TEST.geojson"), feature.to_string()).unwrap();
    manifest(dir, "asgs", &["asgs/SA2_TEST.geojson"]);
}

fn shapefile(dir: &Path) {
    use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};
    use shapefile::{Point, Polygon, PolygonRing};
    fs::create_dir_all(dir.join("asgs")).unwrap();
    let pts = |r: &[[f64; 2]]| r.iter().map(|p| Point::new(p[0], p[1])).collect::<Vec<_>>();
    // A shapefile's outer ring runs clockwise and its holes counter-clockwise.
    let (mut outer, hole) = (pts(&OUTER), pts(&HOLE));
    outer.reverse();
    let polygon = Polygon::with_rings(vec![PolygonRing::Outer(outer), PolygonRing::Inner(hole)]);
    let table = TableWriterBuilder::new()
        .add_character_field("SA2_CODE21".try_into().unwrap(), 9)
        .add_character_field("SA2_NAME21".try_into().unwrap(), 50)
        .add_character_field("STE_NAME21".try_into().unwrap(), 50);
    let mut w = shapefile::Writer::from_path(dir.join("asgs/SA2_TEST.shp"), table).unwrap();
    let mut rec = Record::default();
    rec.insert("SA2_CODE21".into(), FieldValue::Character(Some("1".into())));
    rec.insert("SA2_NAME21".into(), FieldValue::Character(Some("Square".into())));
    rec.insert("STE_NAME21".into(), FieldValue::Character(Some("New South Wales".into())));
    w.write_shape_and_record(&polygon, &rec).unwrap();
    drop(w);
    manifest(dir, "asgs", &["asgs/SA2_TEST.shp", "asgs/SA2_TEST.dbf"]);
}

fn geopackage(dir: &Path) {
    fs::create_dir_all(dir.join("asgs")).unwrap();
    let db = rusqlite::Connection::open(dir.join("asgs/test.gpkg")).unwrap();
    db.execute_batch(
        "CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT);
         INSERT INTO gpkg_geometry_columns VALUES ('SA2_2021_AUST_GDA2020', 'geom');
         CREATE TABLE SA2_2021_AUST_GDA2020 (id INTEGER PRIMARY KEY, geom BLOB, SA2_CODE_2021 TEXT, SA2_NAME_2021 TEXT, STATE_NAME_2021 TEXT);",
    )
    .unwrap();
    let mut wkb = vec![1u8];
    wkb.extend_from_slice(&3u32.to_le_bytes());
    wkb.extend_from_slice(&2u32.to_le_bytes());
    for ring in [&OUTER, &HOLE] {
        wkb.extend_from_slice(&(ring.len() as u32).to_le_bytes());
        for p in ring {
            wkb.extend_from_slice(&p[0].to_le_bytes());
            wkb.extend_from_slice(&p[1].to_le_bytes());
        }
    }
    let mut blob = b"GP".to_vec();
    blob.extend_from_slice(&[0, 1, 0, 0, 0, 0]);
    blob.extend_from_slice(&wkb);
    db.execute("INSERT INTO SA2_2021_AUST_GDA2020 (geom, SA2_CODE_2021, SA2_NAME_2021, STATE_NAME_2021) VALUES (?1, '1', 'Square', 'New South Wales')", [blob]).unwrap();
    drop(db);
    manifest(dir, "asgs", &["asgs/test.gpkg"]);
}

#[test]
fn the_three_boundary_formats_answer_alike() {
    let root = temp("asgs");
    for (name, make) in [("json", geojson as fn(&Path)), ("shp", shapefile), ("gpkg", geopackage)] {
        let (src, data) = (root.join(format!("src-{name}")), root.join(format!("data-{name}")));
        fs::create_dir_all(&src).unwrap();
        make(&src);
        let (r, exit) = refresh(&data, &src);
        assert_eq!((exit, r["ok"].as_bool()), (0, Some(true)), "{name}: {r}");
        for (lat, lon, want) in POINTS {
            let (a, exit) = call(&data, &["area", "--lat", lat, "--lon", lon]);
            assert_eq!(exit, 0, "{name} {lat} {lon}: {a}");
            assert_eq!(a["count"], want, "{name} {lat} {lon}: {a}");
            if want == 1 {
                let hit = &a["results"][0];
                assert_eq!(
                    (hit["layer"].as_str(), hit["code"].as_str(), hit["name"].as_str(), hit["state"].as_str()),
                    (Some("SA2"), Some("1"), Some("Square"), Some("New South Wales")),
                    "{name}: {hit}"
                );
            }
        }
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn damaged_boundary_files_are_refused_and_change_nothing() {
    let root = temp("bad-asgs");
    let (src, data) = (root.join("src"), root.join("data"));
    fs::create_dir_all(&src).unwrap();
    geojson(&src);
    let (first, _) = refresh(&data, &src);
    let version = first["dataVersion"].as_str().unwrap().to_string();
    for (file, bytes) in [
        ("asgs/SA2_TEST.geojson", b"{\"features\": [{\"geometry\": {\"type\": \"Polygon\", \"coordinates\": [[[1, \"x\"]]]}}]}".to_vec()),
        ("asgs/SA2_TEST.geojson", b"nope".to_vec()),
    ] {
        fs::write(src.join(file), bytes).unwrap();
        let (r, exit) = refresh(&data, &src);
        assert_eq!((exit, r["error"]["code"].as_str()), (2, Some("refresh-failed")), "{r}");
        assert_eq!(call(&data, &["status"]).0["dataVersion"], version.as_str());
    }
    // A shapefile with no .dbf, a truncated one, and a GeoPackage that is not SQLite.
    fs::write(src.join("asgs/a.shp"), [0u8; 10]).unwrap();
    fs::write(src.join("asgs/b.gpkg"), b"not a database at all, just text").unwrap();
    for file in ["asgs/a.shp", "asgs/b.gpkg"] {
        manifest(&src, "asgs", &[file]);
        let (r, exit) = refresh(&data, &src);
        assert_eq!((exit, r["error"]["code"].as_str()), (2, Some("refresh-failed")), "{file}: {r}");
    }
    assert_eq!(call(&data, &["status"]).0["dataVersion"], version.as_str());
    let _ = fs::remove_dir_all(&root);
}

// ---- ABS --------------------------------------------------------------------------------------------------------------

fn abs_source(dir: &Path) {
    fs::create_dir_all(dir.join("abs/data")).unwrap();
    fs::write(
        dir.join("abs/dataflows.json"),
        r#"{"data":{"dataflows":[{"id":"CPI","version":"2.0.0","name":"Consumer Price Index"},{"id":"LF","version":"1.0","name":"Labour force, monthly"},{"name":"no id"}]}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("abs/data/CPI.csv"),
        "\u{feff}DATAFLOW,MEASURE,REGION,TIME_PERIOD,OBS_VALUE,UNIT_MEASURE,OBS_STATUS,DECIMALS\r\n\
         ABS:CPI(2.0.0),1,\"Sydney, NSW\",2025-Q4,2.5,PCT,,1\r\n\
         ABS:CPI(2.0.0),1,\"Sydney, NSW\",2025-Q3,,PCT,,1\r\n\
         ABS:CPI(2.0.0),1,Perth,2025-Q4,3,PCT,,0\r\n\
         ABS:CPI(2.0.0),1,Perth,2026-Q1,1e21,PCT,,0\r\n",
    )
    .unwrap();
    manifest(dir, "abs", &["abs/dataflows.json", "abs/data/CPI.csv"]);
}

#[test]
fn abs_values_periods_filters_and_the_catalogue() {
    let root = temp("abs");
    let (src, data) = (root.join("src"), root.join("data"));
    abs_source(&src);
    let (r, exit) = refresh(&data, &src);
    assert_eq!((exit, r["sources"][0]["records"].as_u64()), (0, Some(2)), "{r}");
    let get = |args: &[&str]| call(&data, &[&["stats", "get", "CPI"], args].concat());
    let (all, _) = get(&[]);
    let rows: Vec<(String, String, String)> = all["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["period"].to_string(), r["value"].to_string(), r["dims"]["REGION"].to_string()))
        .collect();
    assert_eq!(
        rows,
        [
            ("\"2025-Q3\"", "null", "\"Sydney, NSW\""),
            ("\"2025-Q4\"", "2.5", "\"Sydney, NSW\""),
            ("\"2025-Q4\"", "3", "\"Perth\""),
            ("\"2026-Q1\"", "1e+21", "\"Perth\"")
        ]
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
    );
    assert_eq!(all["results"][0]["dims"].as_object().unwrap().keys().collect::<Vec<_>>(), ["MEASURE", "REGION"]);
    assert_eq!(get(&["--filter", "REGION=Perth", "--last", "1"]).0["results"][0]["period"], "2026-Q1");
    assert_eq!(get(&["--from", "2025-Q4", "--to", "2025-Q4"]).0["count"], 2);
    assert_eq!(get(&["--filter", "REGION=Nowhere"]).0["count"], 0);
    assert_eq!(get(&["--filter", "NOPE=1"]).0["count"], 0);
    for (args, code) in
        [(&["--filter", "REGION"][..], "invalid-filter"), (&["--last", "0"], "invalid-limit"), (&["--last", "abc"], "invalid-limit")]
    {
        let (r, exit) = get(args);
        assert_eq!((exit, r["error"]["code"].as_str()), (2, Some(code)), "{args:?}: {r}");
    }
    assert_eq!(call(&data, &["stats", "find", "price", "index"]).0["results"][0]["id"], "CPI");
    assert_eq!(call(&data, &["stats", "find", "lab"]).0["count"], 1);
    assert_eq!(call(&data, &["stats", "find", "nothing"]).0["count"], 0);
    assert_eq!(call(&data, &["stats", "get", "LF"]).0["error"]["code"], "unknown-dataflow");
    // A catalogue with no flows file is refused.
    fs::remove_file(src.join("abs/dataflows.json")).unwrap();
    manifest(&src, "abs", &["abs/data/CPI.csv"]);
    assert_eq!(refresh(&data, &src).0["error"]["code"], "refresh-failed");
    let _ = fs::remove_dir_all(&root);
}

// ---- G-NAF ------------------------------------------------------------------------------------------------------------

const DETAIL: &str = "ADDRESS_DETAIL_PID|DATE_CREATED|DATE_LAST_MODIFIED|DATE_RETIRED|BUILDING_NAME|LOT_NUMBER_PREFIX|LOT_NUMBER|LOT_NUMBER_SUFFIX|FLAT_TYPE_CODE|FLAT_NUMBER_PREFIX|FLAT_NUMBER|FLAT_NUMBER_SUFFIX|LEVEL_TYPE_CODE|LEVEL_NUMBER_PREFIX|LEVEL_NUMBER|LEVEL_NUMBER_SUFFIX|NUMBER_FIRST_PREFIX|NUMBER_FIRST|NUMBER_FIRST_SUFFIX|NUMBER_LAST_PREFIX|NUMBER_LAST|NUMBER_LAST_SUFFIX|STREET_LOCALITY_PID|LOCATION_DESCRIPTION|LOCALITY_PID|ALIAS_PRINCIPAL|POSTCODE|PRIVATE_STREET|LEGAL_PARCEL_ID|CONFIDENCE|ADDRESS_SITE_PID|LEVEL_GEOCODED_CODE|PROPERTY_PID|GNAF_PROPERTY_PID|PRIMARY_SECONDARY\n";

fn detail_row(pid: &str, retired: &str, flat: &str, first: &str, last: &str, street: &str, alias: &str, post: &str) -> String {
    let columns: Vec<&str> = DETAIL.trim_end().split('|').collect();
    let mut cells = vec![""; columns.len()];
    let mut put = |name: &str, value: &str| cells[columns.iter().position(|c| *c == name).unwrap()] = value.to_string().leak();
    put("ADDRESS_DETAIL_PID", pid);
    put("DATE_CREATED", "2016-08-01");
    put("DATE_RETIRED", retired);
    put("FLAT_TYPE_CODE", if flat.is_empty() { "" } else { "UNIT" });
    put("FLAT_NUMBER", flat);
    put("NUMBER_FIRST", first);
    put("NUMBER_LAST", last);
    put("STREET_LOCALITY_PID", street);
    put("LOCALITY_PID", "L1");
    put("ALIAS_PRINCIPAL", alias);
    put("POSTCODE", post);
    format!("{}\n", cells.join("|"))
}

fn gnaf_tables(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("NSW_LOCALITY_psv.psv"), "LOCALITY_PID|DATE_CREATED|DATE_RETIRED|LOCALITY_NAME|PRIMARY_POSTCODE\nL1|2016-08-01||SYDNEY|2000\r\nL2|2016-08-01|2020-01-01|OLDTOWN|2999\n").unwrap();
    fs::write(
        dir.join("NSW_STREET_LOCALITY_psv.psv"),
        "STREET_LOCALITY_PID|DATE_CREATED|DATE_RETIRED|STREET_CLASS_CODE|STREET_NAME|STREET_TYPE_CODE|STREET_SUFFIX_CODE|LOCALITY_PID\nS1|2016-08-01||C|GEORGE|STREET||L1\nS2|2016-08-01||C|MAIN|ROAD|N|L1\n",
    )
    .unwrap();
    let mut detail = String::from(DETAIL);
    detail.push_str(&detail_row("GA1", "", "", "100", "", "S1", "P", "2000"));
    detail.push_str(&detail_row("GA2", "", "5", "100", "", "S1", "P", "2000"));
    detail.push_str(&detail_row("GA3", "2021-01-01", "", "101", "", "S1", "P", "2000"));
    detail.push_str(&detail_row("GA4", "", "", "100", "", "S1", "A", "2000"));
    detail.push_str(&detail_row("GA5", "", "", "10", "14", "S2", "P", ""));
    detail.push_str(&detail_row("GA6", "", "", "7", "", "S9", "P", "2000"));
    detail.push_str(&detail_row("GA7", "", "", "8", "", "S1", "P", "2000"));
    fs::write(dir.join("NSW_ADDRESS_DETAIL_psv.psv"), detail).unwrap();
    let geo = |pid: &str, lat: &str| format!("G{pid}|2016-08-01||{pid}|PC|151.2|{lat}\n");
    let mut geocodes =
        String::from("ADDRESS_DEFAULT_GEOCODE_PID|DATE_CREATED|DATE_RETIRED|ADDRESS_DETAIL_PID|GEOCODE_TYPE_CODE|LONGITUDE|LATITUDE\n");
    for (pid, lat) in
        [("GA1", "-33.5"), ("GA2", "-33.6"), ("GA3", "-33.7"), ("GA4", "-33.8"), ("GA5", "-33.9"), ("GA6", "-34"), ("GA9", "-1")]
    {
        geocodes.push_str(&geo(pid, lat));
    }
    fs::write(dir.join("NSW_ADDRESS_DEFAULT_GEOCODE_psv.psv"), geocodes).unwrap();
}

fn pids(v: &Value) -> Vec<String> {
    v["results"].as_array().unwrap().iter().map(|r| r["addressId"].as_str().unwrap().to_string()).collect()
}

#[test]
fn g_naf_rules_and_a_zip_the_same_as_a_folder() {
    let root = temp("gnaf");
    let (loose, zipped) = (root.join("loose"), root.join("zipped"));
    gnaf_tables(&loose.join("Standard"));
    manifest(&loose, "gnaf", &["Standard"]);
    fs::create_dir_all(&zipped).unwrap();
    let mut zip = zip::ZipWriter::new(File::create(zipped.join("gnaf.zip")).unwrap());
    for entry in fs::read_dir(loose.join("Standard")).unwrap().flatten() {
        zip.start_file(
            format!("G-NAF/G-NAF AUGUST 2026/Standard/{}", entry.file_name().to_string_lossy()),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&fs::read(entry.path()).unwrap()).unwrap();
    }
    zip.start_file("G-NAF/Documents/readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(b"not a table").unwrap();
    zip.finish().unwrap();
    manifest(&zipped, "gnaf", &["gnaf.zip"]);

    let answers: Vec<Vec<Value>> = [(&loose, "a"), (&zipped, "b")]
        .iter()
        .map(|(src, tag)| {
            let data = root.join(format!("data-{tag}"));
            let (r, exit) = refresh(&data, src);
            // GA3 is retired, GA4 an alias, GA6 has no street, GA7 has no geocode: three addresses remain.
            assert_eq!((exit, r["sources"][0]["records"].as_u64()), (0, Some(3)), "{r}");
            [
                "100 george st sydney",
                "unit 5 100 george street",
                "5/100 George St",
                "george",
                "14 main rd",
                "10 main road n",
                "sydney 2000 --limit 1",
                "7 george",
            ]
            .iter()
            .map(|q| {
                let text = q.split(" --limit").next().unwrap();
                let mut args = vec!["address", text];
                if q.contains("--limit") {
                    args.extend(["--limit", "1"]);
                }
                call(&data, &args).0
            })
            .collect()
        })
        .collect();
    let (a, b) = (&answers[0], &answers[1]);
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x["results"], y["results"]);
    }
    // The closest address first (fewest words beyond the query's), the PID breaking ties.
    assert_eq!(pids(&a[0]), ["GA1", "GA2"]);
    assert_eq!(pids(&a[1]), ["GA2"]);
    assert_eq!(pids(&a[2]), ["GA2"]);
    assert_eq!(pids(&a[3]), ["GA1", "GA2"]);
    assert_eq!(a[4]["count"], 1);
    assert_eq!(a[4]["results"][0]["address"], "10-14 MAIN ROAD N, SYDNEY NSW 2000");
    assert_eq!(pids(&a[5]), ["GA5"]);
    assert_eq!(a[6]["count"], 1);
    assert_eq!(a[7]["count"], 0);
    assert_eq!(a[2]["results"][0]["address"], "UNIT 5/100 GEORGE STREET, SYDNEY NSW 2000");
    assert_eq!(a[2]["results"][0]["lat"], -33.6);
    let data = root.join("data-a");
    assert_eq!(pids(&call(&data, &["address", "--id", "GA5"]).0), ["GA5"]);
    for id in ["GA3", "GA4", "GA7", "ZZZ", ""] {
        assert_eq!(call(&data, &["address", "--id", id]).0["count"], 0, "{id}");
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn g_naf_tables_that_are_not_tables_are_refused() {
    let root = temp("gnaf-bad");
    let (src, data) = (root.join("src"), root.join("data"));
    gnaf_tables(&src.join("Standard"));
    manifest(&src, "gnaf", &["Standard"]);
    let (first, _) = refresh(&data, &src);
    fs::write(src.join("Standard/NSW_ADDRESS_DETAIL_psv.psv"), "NOT|A|G-NAF|TABLE\n1|2|3|4\n").unwrap();
    let (r, exit) = refresh(&data, &src);
    assert_eq!((exit, r["error"]["code"].as_str()), (2, Some("refresh-failed")), "{r}");
    assert_eq!(call(&data, &["status"]).0["dataVersion"], first["dataVersion"]);
    let _ = fs::remove_dir_all(&root);
}
