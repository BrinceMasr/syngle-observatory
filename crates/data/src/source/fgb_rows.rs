//! A dataset's rows as FlatGeobuf, whole and in order (#471): how the engine serves the rows of a dataset to a page
//! that takes the project in parts, and how the page reads them back.
//!
//! Unlike [`super::fgb::write`], which writes a file for other programs (spatially indexed, so features come out in
//! Hilbert order, and those without a geometry left out), this keeps what a row is here: its position (a layer's
//! selection is row numbers), its feature id, a missing geometry and every value. There is no index, so the file is
//! read from its start; the page's cost is one fetch per dataset, not a query per view. The feature id is the first
//! column, `_fid`; the dataset's own fields follow in order.

use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use flatgeobuf::geozero::wkb::Wkb;
use flatgeobuf::geozero::{ColumnValue, CoordDimensions, FeatureProcessor, GeozeroGeometry, PropertyProcessor, ToWkb};
use flatgeobuf::{ColumnType, FallibleStreamingIterator, FeatureProperties, FgbReader, FgbWriter, FgbWriterOptions, GeometryType as G};
use std::io::Cursor;

/// The column that carries a row's feature id.
const ID_COLUMN: &str = "_fid";

fn err(e: impl std::fmt::Display) -> DataError {
    DataError::Format(format!("FlatGeobuf rows: {e}"))
}

/// The rows of `fields`, `features`, as one FlatGeobuf file in row order.
pub fn write_rows(name: &str, geometry_type: GeometryType, fields: &[Field], features: &[Feature]) -> Result<Vec<u8>> {
    let kind = match geometry_type {
        GeometryType::Point | GeometryType::None => G::Unknown,
        GeometryType::Polyline => G::MultiLineString,
        GeometryType::Polygon => G::MultiPolygon,
    };
    let options = FgbWriterOptions { write_index: false, ..Default::default() };
    let mut w = FgbWriter::create_with_options(name, kind, options).map_err(err)?;
    w.add_column(ID_COLUMN, ColumnType::ULong, |_, c| c.nullable = false);
    for f in fields {
        let t = match f.ty {
            FieldType::Integer => ColumnType::Long,
            FieldType::Double => ColumnType::Double,
            FieldType::Boolean => ColumnType::Bool,
            FieldType::Text => ColumnType::String,
        };
        w.add_column(&f.name, t, |_, c| c.nullable = true);
    }
    for (row, feat) in features.iter().enumerate() {
        let row = row as u64;
        w.feature_begin(row).map_err(err)?;
        w.properties_begin().map_err(err)?;
        w.property(0, ID_COLUMN, &ColumnValue::ULong(feat.id)).map_err(err)?;
        for (i, (f, v)) in fields.iter().zip(&feat.attrs).enumerate() {
            let at = i + 1;
            match v {
                Value::Null => continue,
                Value::Int(x) => w.property(at, &f.name, &ColumnValue::Long(*x)),
                Value::Float(x) => w.property(at, &f.name, &ColumnValue::Double(*x)),
                Value::Bool(x) => w.property(at, &f.name, &ColumnValue::Bool(*x)),
                Value::Text(x) => w.property(at, &f.name, &ColumnValue::String(x)),
            }
            .map_err(err)?;
        }
        w.properties_end().map_err(err)?;
        if let Some(g) = &feat.geometry {
            w.geometry_begin().map_err(err)?;
            Wkb(wkb::encode(g)).process_geom(&mut w).map_err(err)?;
            w.geometry_end().map_err(err)?;
        }
        w.feature_end(row).map_err(err)?;
    }
    let mut out = Vec::new();
    w.write(&mut out).map_err(err)?;
    Ok(out)
}

struct RowProps {
    id: u64,
    values: Vec<Value>,
}

impl PropertyProcessor for RowProps {
    fn property(&mut self, idx: usize, name: &str, v: &ColumnValue) -> flatgeobuf::geozero::error::Result<bool> {
        if name == ID_COLUMN {
            if let ColumnValue::ULong(id) = v {
                self.id = *id;
            }
            return Ok(false);
        }
        let value = match v {
            ColumnValue::Long(x) => Value::Int(*x),
            ColumnValue::Double(x) => Value::Float(*x),
            ColumnValue::Bool(x) => Value::Bool(*x),
            ColumnValue::String(s) => Value::Text((*s).to_string()),
            _ => Value::Null,
        };
        let at = idx.saturating_sub(1);
        if let Some(slot) = self.values.get_mut(at) {
            *slot = value;
        }
        Ok(false)
    }
}

/// The rows [`write_rows`] wrote, in order, for a dataset of `fields`. Hostile input: a row that does not read is an
/// error, never a panic, and a file that is not this format fails at its header.
pub fn read_rows(bytes: &[u8], fields: &[Field]) -> Result<Vec<Feature>> {
    let reader = FgbReader::open(Cursor::new(bytes)).map_err(err)?;
    let mut iter = reader.select_all().map_err(err)?;
    let mut out = Vec::new();
    while let Some(feat) = iter.next().map_err(err)? {
        // A row with no geometry has none to read (the writer leaves an empty geometry, and asking for its points would index an empty vector).
        let geometry = if feat
            .fbs_feature()
            .geometry()
            .is_some_and(|g| g.xy().is_some_and(|xy| !xy.is_empty()) || g.parts().is_some_and(|p| !p.is_empty()))
        {
            feat.to_wkb(CoordDimensions::xy()).ok().as_deref().and_then(wkb::decode)
        } else {
            None
        };
        let mut props = RowProps { id: 0, values: vec![Value::Null; fields.len()] };
        feat.process_properties(&mut props).map_err(err)?;
        out.push(Feature { id: props.id, geometry, attrs: props.values });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Geometry;
    use rustgis_geom::Coord;

    fn fields() -> Vec<Field> {
        vec![
            Field::new("name", FieldType::Text),
            Field::new("n", FieldType::Integer),
            Field::new("x", FieldType::Double),
            Field::new("ok", FieldType::Boolean),
        ]
    }

    fn rows() -> Vec<Feature> {
        vec![
            Feature {
                id: 7,
                geometry: Some(Geometry::Point(Coord::new(151.2, -33.9))),
                attrs: vec![Value::Text("a".into()), Value::Int(-3), Value::Float(1.5), Value::Bool(true)],
            },
            // No geometry, and nothing but nulls: still a row, still in its place.
            Feature { id: 2, geometry: None, attrs: vec![Value::Null; 4] },
            Feature {
                id: 90_000_000_000,
                geometry: Some(Geometry::Point(Coord::new(-0.1, 51.5))),
                attrs: vec![Value::Text(String::new()), Value::Null, Value::Float(f64::MAX), Value::Bool(false)],
            },
        ]
    }

    #[test]
    fn rows_come_back_as_they_went_in_order_ids_missing_geometry_and_nulls_included() {
        let bytes = write_rows("pts", GeometryType::Point, &fields(), &rows()).unwrap();
        assert_eq!(read_rows(&bytes, &fields()).unwrap(), rows());
    }

    #[test]
    fn polygons_and_an_empty_dataset_round_trip() {
        let ring = vec![Coord::new(0.0, 0.0), Coord::new(1.0, 0.0), Coord::new(1.0, 1.0), Coord::new(0.0, 0.0)];
        let polygons = vec![Feature { id: 1, geometry: Some(Geometry::Polygon(vec![vec![ring]])), attrs: vec![] }];
        let bytes = write_rows("poly", GeometryType::Polygon, &[], &polygons).unwrap();
        assert_eq!(read_rows(&bytes, &[]).unwrap(), polygons);
        let bytes = write_rows("none", GeometryType::None, &fields(), &[]).unwrap();
        assert_eq!(read_rows(&bytes, &fields()).unwrap(), Vec::<Feature>::new());
    }

    #[test]
    fn what_is_not_rows_is_an_error_not_a_panic() {
        for bad in [&b""[..], b"not a flatgeobuf file at all", &[0u8; 64]] {
            assert!(read_rows(bad, &fields()).is_err());
        }
        let mut bytes = write_rows("pts", GeometryType::Point, &fields(), &rows()).unwrap();
        bytes.truncate(bytes.len() / 2);
        let _ = read_rows(&bytes, &fields());
    }
}
