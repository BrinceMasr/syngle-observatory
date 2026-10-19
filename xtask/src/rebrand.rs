//! The rebrand gate: the fork ships as **Syngle Observatory**, so the upstream project's name,
//! addresses and application id must not creep back into the product, its packaging or its docs.
//!
//! `cargo xtask rebrand` (a step of `cargo xtask ci`) reads every tracked text file and fails on
//!
//! - the upstream product name `RustGIS` outside the files that exist to name and credit it,
//! - the upstream addresses (`rustgis.app` and its subdomains),
//! - the upstream project's repositories (`opengeos/RustGIS`, `opengeos/rustgis*`) outside the
//!   files that exist to credit it,
//! - the upstream application id (`org.rustgis…`),
//! - the upstream publisher (`OpenGeos`) and its store listing (`apps.microsoft.com`).
//!
//! What legitimately keeps the upstream name, and why:
//!
//! | What | Why it keeps the upstream name |
//! |---|---|
//! | `rustgis-model`, `rustgis_engine`, `RUSTGIS_*`, `rustgis-cli` | the Rust package, module, environment-variable and binary names of the ~15 crates; renaming them is a separate refactor with no user-visible effect |
//! | `.rgis`, `.rgisx` | the document format's extensions, so projects people already have still open |
//! | `RustGISError` | the exception class of the inherited `rustgis` Python package: not user-visible |
//! | `b"RustGIS parts 1\n"` (`crates/data/src/raster/cog.rs`) | the parts container's on-disk magic: renaming it would stop the fork reading containers the upstream app wrote |
//! | `NOTICE`, `ATTRIBUTION.md`, `LICENSE`, `README.md` | the Apache-2.0 notices must name the work they came from, including its repository, and the README's provenance section says once where the fork came from |
//! | `crates/render/build.rs`'s `assets.rustgis.app/data` | the public-domain Natural Earth sample data the build fetches; the fork hosts no data mirror |
//!
//! This file is skipped, since it names the banned strings to ban them. Binary files are skipped
//! (the check is a text check).

use std::path::Path;
use std::process::Command;

/// The files that may name the upstream project: the licence-relevant notices, the README that
/// states the provenance, and this gate.
const PROVENANCE: &[&str] = &["NOTICE", "ATTRIBUTION.md", "LICENSE", "README.md", "xtask/src/rebrand.rs"];

/// The one upstream address the fork still reads: the build-time fetch of the public-domain
/// Natural Earth sample data. The exact host and path, not the domain.
const SAMPLE_DATA: &str = "assets.rustgis.app/data";

/// The strings that legitimately carry the upstream name inside code: the Python package's
/// exception class (an inherited API identifier), and the parts container's on-disk magic, which
/// must keep its bytes so a container the upstream app wrote still opens here.
const IDENTIFIERS: &[&str] = &["RustGIS parts 1", "RustGISError"];

/// Every finding in one line (or one file path): the offending token, with why.
fn offences(file: &str, line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let provenance = PROVENANCE.contains(&file);
    // The sample-data address is removed first, so its host alone does not trip the domain check,
    // and the Python package's exception class is an identifier, not a product string.
    let mut scan = line.replace(SAMPLE_DATA, "");
    for identifier in IDENTIFIERS {
        scan = scan.replace(identifier, "");
    }
    if !provenance && scan.contains("RustGIS") {
        out.push("RustGIS (upstream product name)".into());
    }
    for address in ["rustgis.app", "rustgis.com", "apps.microsoft.com"] {
        if scan.contains(address) {
            out.push(format!("{address} (upstream address)"));
        }
    }
    if !provenance {
        for repo in ["opengeos/RustGIS", "opengeos/rustgis"] {
            if scan.contains(repo) {
                out.push(format!("{repo} (upstream repository)"));
            }
        }
    }
    if scan.contains("OpenGeos") {
        out.push("OpenGeos (upstream publisher)".into());
    }
    if scan.contains("org.rustgis") {
        out.push("org.rustgis (upstream application id)".into());
    }
    out
}

/// Every tracked file, `/`-separated, as `git ls-files` reports them.
fn tracked_files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git").arg("-C").arg(root).args(["ls-files", "-z"]).output().map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files failed".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect())
}

pub fn run(root: &Path) -> Result<(), String> {
    let files = tracked_files(root)?;
    let mut findings: Vec<String> = Vec::new();
    for file in &files {
        if file == "xtask/src/rebrand.rs" {
            continue;
        }
        for token in offences(file, file) {
            findings.push(format!("{file} (path): {token}"));
        }
        let Ok(bytes) = std::fs::read(root.join(file)) else { continue };
        let Ok(text) = String::from_utf8(bytes) else { continue }; // binary: not a text check
        for (n, line) in text.lines().enumerate() {
            for token in offences(file, line) {
                findings.push(format!("{file}:{}: {token}", n + 1));
            }
        }
    }
    if findings.is_empty() {
        println!("rebrand: {} tracked files, no upstream branding", files.len());
        return Ok(());
    }
    println!("rebrand: the fork still carries the upstream branding");
    for f in &findings {
        println!("  {f}");
    }
    Err(format!("{} findings", findings.len()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn allowed(file: &str, text: &str) -> bool {
        offences(file, text).is_empty()
    }

    fn banned(file: &str, text: &str) -> bool {
        !offences(file, text).is_empty()
    }

    #[test]
    fn identifiers_that_keep_the_upstream_name_are_allowed() {
        assert!(allowed("crates/x/src/lib.rs", "use rustgis_engine::Session;"));
        assert!(allowed("Cargo.toml", "rustgis-ui-egui = { workspace = true }"));
        assert!(allowed("crates/x/src/lib.rs", "if std::env::var_os(\"RUSTGIS_GPU\").is_some()"));
        assert!(allowed("apps/rustgis-cli/src/main.rs", "rustgis-cli: ready"));
        assert!(allowed("docs/x.md", "a project.rgis file and a .rgisx package"));
        // The notices and the README's provenance line must be able to name the upstream work.
        assert!(allowed("NOTICE", "a hard fork of RustGIS (https://github.com/opengeos/RustGIS)"));
        assert!(allowed("ATTRIBUTION.md", "the RustGIS sample data"));
        assert!(allowed("README.md", "A hard fork of RustGIS"));
        // The Python package's exception class is an identifier of the inherited package.
        assert!(allowed("apps/rustgis-python/src/lib.rs", "create_exception!(_rustgis, RustGISError, PyException);"));
        // The sample data's real source, in the table that credits every asset.
        assert!(allowed("ATTRIBUTION.md", "from [rustgis-assets](https://github.com/opengeos/rustgis-assets)"));
        // The parts container's on-disk magic keeps the upstream bytes.
        assert!(allowed("crates/data/src/raster/cog.rs", "const CONTAINER_MAGIC: &[u8; 16] = b\"RustGIS parts 1\\n\";"));
        // The build-time sample data fetch is the one upstream address left.
        assert!(allowed("crates/render/build.rs", "const BASE: &str = \"https://assets.rustgis.app/data\";"));
    }

    #[test]
    fn user_visible_branding_is_banned() {
        assert!(banned("apps/rustgis-web/index.html", "<title>RustGIS</title>"));
        assert!(banned("crates/x/src/lib.rs", "About RustGIS"));
        assert!(banned("docs/x.md", "see https://rustgis.app/user-guide/"));
        assert!(banned("docs/x.md", "see https://web.rustgis.app"));
        assert!(banned("Cargo.toml", "repository = \"https://github.com/opengeos/RustGIS\""));
        assert!(banned("packaging/x.sh", "appid=org.rustgis.RustGIS"));
        assert!(banned("apps/rustgis-web/manifest.webmanifest", "\"name\": \"RustGIS\""));
        assert!(banned("docs/installation.md", "`winget install OpenGeos.Syngle Observatory`"));
        assert!(banned("docs/index.md", "(https://apps.microsoft.com/detail/9ppjw8h561lq)"));
    }

    #[test]
    fn the_working_tree_passes() {
        run(&crate::root()).expect("the fork carries the upstream branding");
    }
}
