# Data sources

Where to find open data, and the command that brings each source into a map. Every source here is one the map opens with its own commands: `evals/data-sources/check.mjs` fetches each address and runs each command on the real engine. This page is written from the same catalogue (`evals/data-sources/catalogue.json`, last checked 2026-11-14) by `cargo xtask data-sources`, and `cargo xtask ci` fails if the two differ, so it says only what that check proves. Nothing is preloaded: a source is downloaded or read in place only when a command asks for it.

Licences were read on the publishers' own pages on the date above, and each source names the pages. A publisher's update rhythm can change: the page named under the source is the authority for the current one.

## How to run a command

Every command is an id with JSON parameters (`syngle-map commands` lists them all). From a shell or an agent:

```sh
syngle-map run --cmd 'id={"param":"value"}' --cmd 'share.exportMap={"path":"map.png"}'
```

Each command answers with one line of JSON that says what it added or why it could not. The same ids and parameters are what the app's bridge takes (see [The Observatory bridge](bridge.md)).

## Basemaps

A vector basemap for the whole world, added as a tile layer. Nothing is downloaded whole: the map reads the tiles it shows.

### Protomaps daily OSM basemap (PMTiles)

- **Licence:** ODbL (OSM data, attribution required); the tiles are a Produced Work
- **Updates:** daily build
- **Size:** 139.4 GB planet, read by range
- **Pages read:** <https://docs.protomaps.com/basemaps/downloads>, <https://docs.protomaps.com/pmtiles/cloud-storage/>

```sh
syngle-map run --cmd 'map.addTiles={"kind":"pmtiles","name":"Protomaps","url":"https://build.protomaps.com/20261113.pmtiles"}'
```

Protomaps discourages hotlinking its dated builds and asks users to copy the tileset to their own storage; a build is kept for a week, so the dated address changes (the current one is listed at <https://maps.protomaps.com/builds>). `pmtiles extract` cuts a region out of one.

### OpenFreeMap Liberty style (vector basemap, no key)

- **Licence:** MIT code, ODbL data (attribution required)
- **Updates:** weekly planet build (survey run, not re-read)
- **Size:** served as tiles
- **Pages read:** <https://openfreemap.org/>

```sh
syngle-map run --cmd 'map.addTiles={"kind":"style","name":"OpenFreeMap","url":"https://tiles.openfreemap.org/styles/liberty"}'
```

## Administrative and statistical boundaries

Country, state and district polygons, as layers. Natural Earth is the small, quick one for a world view; geoBoundaries and Overture go down to districts; the Australian Bureau of Statistics serves its own areas.

### Natural Earth admin 0, 110m

- **Licence:** public domain
- **Updates:** versioned, no fixed cadence
- **Size:** 215 KB (10m: 4.7 MB)
- **Pages read:** <https://www.naturalearthdata.com/about/terms-of-use/>

```sh
syngle-map run --cmd 'layer.addUrl={"name":"Natural Earth countries","url":"https://naciscdn.org/naturalearth/110m/cultural/ne_110m_admin_0_countries.zip"}'
```

### geoBoundaries gbOpen, Australia ADM1

- **Licence:** CC BY 4.0
- **Updates:** release builds, nightly pipeline
- **Size:** per country
- **Pages read:** <https://www.geoboundaries.org/api.html>, <https://www.geoboundaries.org/>

```sh
syngle-map run --cmd 'boundaries.add={"country":"AUS","level":"ADM1"}'
```

`boundaries.add` reads the gbOpen release, which is the CC BY 4.0 set: use the name "geoBoundaries" and a link to <https://www.geoboundaries.org> where the boundaries are shown. Each country layer also names its original source and that source's licence in its metadata.

### Overture divisions, read in place (Sydney window)

- **Licence:** ODbL for OSM-conflated content, per-source licences
- **Updates:** monthly release
- **Size:** full divisions about 6.4 GB; a window is read by range
- **Pages read:** <https://docs.overturemaps.org/guides/divisions/>, <https://docs.overturemaps.org/getting-data/cloud-sources/>

```sh
syngle-map run --cmd 'overture.add={"bbox":[150.9,-34.0,151.3,-33.7],"theme":"divisions","type":"division_area"}'
```

`bbox` can be `"view"` (the default: the map's extent) or four numbers. Overture warns against using its published tiles (`"tiles": true`) in production: they may change in any release, so the features-in-view form above is the stable one.

### ABS ASGS 2021 SA2 (map service)

- **Licence:** CC BY 4.0
- **Updates:** ASGS edition (Edition 4 from July 2026)
- **Size:** served by feature
- **Pages read:** <https://www.abs.gov.au/statistics/standards/australian-statistical-geography-standard-asgs/edition-3-july-2021-june-2026/access-and-downloads/data-services-and-apis>, <https://www.abs.gov.au/website-privacy-copyright-and-disclaimer>

```sh
syngle-map run --cmd 'service.layers={"url":"https://geo.abs.gov.au/arcgis/rest/services/ASGS2021/SA2/MapServer"}'
```

## Elevation for a bounding box

To get elevation for any box, search the Copernicus 30 m collection for the tiles over the box and add the tile (two commands, shown under the STAC source). The map reads the tile in place, by byte range, so a 20.9 MB tile is not downloaded whole. Geoscience Australia's 1 second model is the Australian alternative, served as a map service.

### Copernicus DEM GLO-30, one 1 degree COG tile (Sydney)

- **Licence:** Copernicus WorldDEM-30 free and open licence, notice required
- **Updates:** static (2021 release)
- **Size:** 20.9 MB per tile
- **Pages read:** <https://registry.opendata.aws/copernicus-dem/>, <https://copernicus-dem-30m.s3.amazonaws.com/readme.html>

```sh
syngle-map run --cmd 'layer.addUrl={"name":"GLO-30 Sydney","url":"https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_S34_00_E151_00_DEM/Copernicus_DSM_COG_10_S34_00_E151_00_DEM.tif"}'
```

GLO-30 Public has no tiles over the ocean (read the height as zero there) and leaves out a few countries' tiles that the Copernicus Programme has not released.

### Elevation for any box: Earth Search STAC (cop-dem-glo-30) then stac.addAsset

- **Licence:** as GLO-30
- **Updates:** static
- **Size:** 20.9 MB per tile, read in place by range
- **Pages read:** <https://registry.opendata.aws/copernicus-dem/>, <https://earth-search.aws.element84.com/v1>

```sh
# 1. the items over a box (west, south, east, north); each answer names the item's address
syngle-map run --cmd 'stac.search={"bbox":[150.9,-34.0,151.3,-33.7],"collections":"cop-dem-glo-30","url":"https://earth-search.aws.element84.com/v1"}'
# 2. add the tile of an item (the example is the one over Sydney)
syngle-map run --cmd 'stac.addAsset={"asset":"data","name":"GLO-30 Sydney (STAC)","url":"https://earth-search.aws.element84.com/v1/collections/cop-dem-glo-30/items/Copernicus_DSM_COG_10_S34_00_E151_00_DEM"}'
```

The same coverage limits as the tile above. `bbox` is west, south, east, north in degrees; a box wider than one degree matches several tiles, and each is added with its own `stac.addAsset`.

### Geoscience Australia SRTM-derived 1 second DEM (map service)

- **Licence:** CC BY 4.0
- **Updates:** none established
- **Size:** 23.5 GB product, served by tile
- **Pages read:** <https://ecat.ga.gov.au/geonetwork/srv/api/records/aac46307-fce8-449d-e044-00144fdd4fa6>

```sh
syngle-map run --cmd 'service.layers={"url":"https://services.ga.gov.au/gis/rest/services/DEM_SRTM_1Second_2024/MapServer"}'
```

## Open data catalogues

To find a dataset rather than a known address: `portal.search` searches a catalogue, and its answer names each file's address, which `layer.addUrl` adds.

### data.gov.au catalogue (CKAN)

- **Licence:** per dataset
- **Updates:** per dataset
- **Size:** metadata
- **Pages read:** <https://data.gov.au/data/api/3/action/package_show?id=5bd7fcab-e315-42cb-8daf-50b7efc2027e>

```sh
syngle-map run --cmd 'portal.search={"portal":"ckan","query":"g-naf","url":"https://data.gov.au/data"}'
```

## Australian addresses

`map.findPlace` finds a place by name or address and can go to it. It answers from Nominatim or Photon over the internet.

### Offline Australian address search in the map's own find-place

- **Licence:** CC BY 4.0 with the Open G-NAF EULA
- **Updates:** quarterly (G-NAF)
- **Size:** 1.73 GiB zip, indexed locally
- **Pages read:** <https://data.gov.au/data/dataset/geocoded-national-address-file-g-naf>

Not built yet: the map cannot open this source today.
