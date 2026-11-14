//! `cargo xtask data-sources [--check]`: writes `docs/data-sources.md` from `evals/data-sources/catalogue.json`.
//!
//! The catalogue is what `evals/data-sources/check.mjs` fetches and opens on the real engine, so a page written from it says
//! only what that check proves. `--check` (a step of `cargo xtask ci`, and a test of this crate) fails when the committed page
//! is not what the catalogue produces, so the page cannot drift from it. Everything the page says that the catalogue does not
//! (the order of the sections, what each is for, a publisher's warning) is in this file, each with the page it was read on.

use serde_json::Value;
use std::path::Path;

const CATALOGUE: &str = "evals/data-sources/catalogue.json";
const PAGE: &str = "docs/data-sources.md";

/// The catalogue's sources the map did not open when the catalogue was written (`want`) and that it opens now. A source here
/// is written like any other, with its command; one that is not says it is not built yet. The ticket that builds one adds its id.
const BUILT: &[&str] = &[];

/// The sections, in order: the catalogue's `group`, the heading a person or an agent searches for, and what the section is for.
const GROUPS: &[(&str, &str, &str)] = &[
    (
        "basemap",
        "Basemaps",
        "A vector basemap for the whole world, added as a tile layer. Nothing is downloaded whole: the map reads the tiles it shows.",
    ),
    (
        "boundary",
        "Administrative and statistical boundaries",
        "Country, state and district polygons, as layers. Natural Earth is the small, quick one for a world view; geoBoundaries and Overture go down to districts; the Australian Bureau of Statistics serves its own areas.",
    ),
    (
        "elevation",
        "Elevation for a bounding box",
        "To get elevation for any box, search the Copernicus 30 m collection for the tiles over the box and add the tile (two commands, shown under the STAC source). The map reads the tile in place, by byte range, so a 20.9 MB tile is not downloaded whole. Geoscience Australia's 1 second model is the Australian alternative, served as a map service.",
    ),
    (
        "catalogue",
        "Open data catalogues",
        "To find a dataset rather than a known address: `portal.search` searches a catalogue, and its answer names each file's address, which `layer.addUrl` adds.",
    ),
    (
        "places",
        "Australian addresses",
        "`map.findPlace` finds a place by name or address and can go to it. It answers from Nominatim or Photon over the internet.",
    ),
];

/// What a publisher's page says that a person adding the source needs and the catalogue has no field for. Each is from the page
/// read on the catalogue's `checkedOn` date, named in the sentence or in the source's `cite`.
const NOTES: &[(&str, &str)] = &[
    (
        "protomaps-planet",
        "Protomaps discourages hotlinking its dated builds and asks users to copy the tileset to their own storage; a build is kept for a week, so the dated address changes (the current one is listed at <https://maps.protomaps.com/builds>). `pmtiles extract` cuts a region out of one.",
    ),
    (
        "overture-divisions-sydney",
        "`bbox` can be `\"view\"` (the default: the map's extent) or four numbers. Overture warns against using its published tiles (`\"tiles\": true`) in production: they may change in any release, so the features-in-view form above is the stable one.",
    ),
    (
        "geoboundaries-aus-adm1",
        "`boundaries.add` reads the gbOpen release, which is the CC BY 4.0 set: use the name \"geoBoundaries\" and a link to <https://www.geoboundaries.org> where the boundaries are shown. Each country layer also names its original source and that source's licence in its metadata.",
    ),
    (
        "copernicus-glo30-tile",
        "GLO-30 Public has no tiles over the ocean (read the height as zero there) and leaves out a few countries' tiles that the Copernicus Programme has not released.",
    ),
    (
        "copernicus-glo30-stac",
        "The same coverage limits as the tile above. `bbox` is west, south, east, north in degrees; a box wider than one degree matches several tiles, and each is added with its own `stac.addAsset`.",
    ),
];

/// Pages read for a licence that the catalogue's `cite` does not name.
const EXTRA_PAGES: &[(&str, &str)] = &[
    ("abs-asgs-sa2-service", "https://www.abs.gov.au/website-privacy-copyright-and-disclaimer"),
    ("geoboundaries-aus-adm1", "https://www.geoboundaries.org/"),
];

const INTRO: &str = r#"# Data sources

Where to find open data, and the command that brings each source into a map. Every source here is one the map opens with its own commands: `evals/data-sources/check.mjs` fetches each address and runs each command on the real engine. This page is written from the same catalogue (`evals/data-sources/catalogue.json`, last checked __CHECKED__) by `cargo xtask data-sources`, and `cargo xtask ci` fails if the two differ, so it says only what that check proves. Nothing is preloaded: a source is downloaded or read in place only when a command asks for it.

Licences were read on the publishers' own pages on the date above, and each source names the pages. A publisher's update rhythm can change: the page named under the source is the authority for the current one.

## How to run a command

Every command is an id with JSON parameters (`syngle-map commands` lists them all). From a shell or an agent:

```sh
syngle-map run --cmd 'layer.addUrl={"url":"https://naciscdn.org/naturalearth/110m/cultural/ne_110m_admin_0_countries.zip"}' --cmd 'share.exportMap={"path":"map.png"}'
```

Each command answers with one line of JSON that says what it added or why it could not. The same ids and parameters are what the app's bridge takes (see [The Observatory bridge](bridge.md)).
"#;

/// The catalogue's text field `key` of `source`.
fn text<'a>(source: &'a Value, key: &str) -> Result<&'a str, String> {
    source.get(key).and_then(Value::as_str).ok_or_else(|| format!("{CATALOGUE}: a source has no \"{key}\""))
}

/// A command as a shell line: `syngle-map run --cmd 'id={json}'`. The JSON is compact and its keys are in order, so the line
/// is the same on every machine.
fn command_line(id: &str, params: &Value) -> Result<String, String> {
    let json = serde_json::to_string(params).map_err(|e| e.to_string())?;
    if json.contains('\'') {
        return Err(format!("{CATALOGUE}: the parameters of {id} hold a quote, which a shell line cannot carry"));
    }
    Ok(format!("syngle-map run --cmd '{id}={json}'"))
}

/// The search that finds what a STAC `url` names: `<api>/collections/<collection>/items/<item>` is one item of a collection, so
/// the box's items come from searching that collection.
fn stac_search(url: &str) -> Option<(String, String)> {
    let (api, rest) = url.split_once("/collections/")?;
    let (collection, _item) = rest.split_once("/items/")?;
    Some((api.to_string(), collection.to_string()))
}

fn source(out: &mut String, s: &Value, built: bool) -> Result<(), String> {
    let (id, name) = (text(s, "id")?, text(s, "name")?);
    out.push_str(&format!("### {name}\n\n"));
    out.push_str(&format!("- **Licence:** {}\n", text(s, "licence")?));
    out.push_str(&format!("- **Updates:** {}\n", text(s, "cadence")?));
    out.push_str(&format!("- **Size:** {}\n", text(s, "size")?));
    let mut pages: Vec<&str> =
        s.get("cite").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    pages.extend(EXTRA_PAGES.iter().filter(|(i, _)| *i == id).map(|(_, p)| *p));
    if !pages.is_empty() {
        out.push_str(&format!("- **Pages read:** {}\n", pages.iter().map(|p| format!("<{p}>")).collect::<Vec<_>>().join(", ")));
    }
    out.push('\n');
    let wanted = s.get("want").and_then(Value::as_bool).unwrap_or(false);
    if wanted && !built {
        out.push_str("Not built yet: the map cannot open this source today.\n\n");
        return Ok(());
    }
    let open = s.get("open").ok_or_else(|| format!("{CATALOGUE}: {id} has no \"open\""))?;
    let command = text(open, "command")?;
    let params = open.get("params").ok_or_else(|| format!("{CATALOGUE}: {id} has no parameters"))?;
    out.push_str("```sh\n");
    if command == "stac.addAsset"
        && let Some((api, collection)) = params.get("url").and_then(Value::as_str).and_then(stac_search)
    {
        let search = serde_json::json!({"url": api, "collections": collection, "bbox": [151.1, -33.95, 151.3, -33.8]});
        out.push_str("# 1. the items over a box (west, south, east, north); an item's address is its `self` link\n");
        out.push_str(&command_line("stac.search", &search)?);
        out.push_str("\n# 2. add the tile of an item (the example is the one over Sydney)\n");
    }
    out.push_str(&command_line(command, params)?);
    out.push_str("\n```\n\n");
    if let Some((_, note)) = NOTES.iter().find(|(i, _)| *i == id) {
        out.push_str(note);
        out.push_str("\n\n");
    }
    Ok(())
}

/// The page the catalogue text produces.
pub fn page(catalogue: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(catalogue).map_err(|e| format!("{CATALOGUE}: {e}"))?;
    let sources = value.get("sources").and_then(Value::as_array).ok_or_else(|| format!("{CATALOGUE}: no \"sources\""))?;
    let mut out = INTRO
        .replace("__CHECKED__", value.get("checkedOn").and_then(Value::as_str).ok_or_else(|| format!("{CATALOGUE}: no \"checkedOn\""))?);
    out.push('\n');
    for (group, heading, about) in GROUPS {
        let members: Vec<&Value> = sources.iter().filter(|s| s.get("group").and_then(Value::as_str) == Some(group)).collect();
        if members.is_empty() {
            continue;
        }
        out.push_str(&format!("## {heading}\n\n{about}\n\n"));
        for s in members {
            let built = text(s, "id").is_ok_and(|id| BUILT.contains(&id));
            source(&mut out, s, built)?;
        }
    }
    if let Some(stray) =
        sources.iter().filter_map(|s| s.get("group").and_then(Value::as_str)).find(|g| !GROUPS.iter().any(|(k, _, _)| k == g))
    {
        return Err(format!("{CATALOGUE}: the group \"{stray}\" has no section in xtask/src/datasources.rs"));
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    Ok(out)
}

pub fn run(root: &Path, check: bool) -> Result<(), String> {
    let read = |file: &str| std::fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"));
    let want = page(&read(CATALOGUE)?)?;
    if check {
        return if read(PAGE).is_ok_and(|have| have == want) {
            println!("data-sources: {PAGE} matches {CATALOGUE}");
            Ok(())
        } else {
            Err(format!("{PAGE} is not what {CATALOGUE} produces: run `cargo xtask data-sources` and commit the page"))
        };
    }
    std::fs::write(root.join(PAGE), want).map_err(|e| format!("{PAGE}: {e}"))?;
    println!("data-sources: wrote {PAGE}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed page is what the catalogue produces (the gate `cargo xtask ci` also runs).
    #[test]
    fn the_page_matches_the_catalogue() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_default();
        assert_eq!(run(&root, true), Ok(()));
    }

    #[test]
    fn a_source_is_written_with_its_command_and_a_stac_item_with_the_search_that_finds_it() {
        let catalogue = r#"{"checkedOn":"2026-01-02","sources":[
          {"id":"a","group":"basemap","name":"Tiles","licence":"L","cadence":"daily","size":"big","cite":["https://x.test/a"],
           "open":{"command":"map.addTiles","params":{"url":"https://x.test/t.pmtiles","kind":"pmtiles"}},"want":false},
          {"id":"b","group":"elevation","name":"DEM","licence":"L","cadence":"static","size":"20 MB","cite":[],
           "open":{"command":"stac.addAsset","params":{"url":"https://api.test/v1/collections/dem/items/T1","asset":"data"}},"want":false},
          {"id":"c","group":"places","name":"Later","licence":"L","cadence":"never","size":"0","cite":[],
           "open":{"command":"map.findPlace","params":{"provider":"gnaf"}},"want":true}]}"#;
        let page = page(catalogue).unwrap_or_default();
        assert!(page.contains("last checked 2026-01-02"));
        assert!(page.contains("syngle-map run --cmd 'map.addTiles={\"kind\":\"pmtiles\",\"url\":\"https://x.test/t.pmtiles\"}'"));
        assert!(page.contains("stac.search={\"bbox\":[151.1,-33.95,151.3,-33.8],\"collections\":\"dem\",\"url\":\"https://api.test/v1\"}"));
        assert!(page.contains("- **Pages read:** <https://x.test/a>"));
        // A source the map cannot open yet is not given a command that fails.
        assert!(page.contains("Not built yet") && !page.contains("map.findPlace="));
        assert_eq!(page.matches("\n## ").count(), 4);
    }

    #[test]
    fn a_group_with_no_section_and_a_quote_in_the_parameters_are_refused() {
        let bad_group = r#"{"checkedOn":"d","sources":[{"id":"a","group":"zzz","name":"n","licence":"l","cadence":"c","size":"s","open":{"command":"x","params":{}}}]}"#;
        assert!(page(bad_group).is_err());
        let quote = r#"{"checkedOn":"d","sources":[{"id":"a","group":"basemap","name":"n","licence":"l","cadence":"c","size":"s","open":{"command":"x","params":{"q":"it's"}}}]}"#;
        assert!(page(quote).is_err());
    }
}
