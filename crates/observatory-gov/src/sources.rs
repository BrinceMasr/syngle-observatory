//! The sources by name: the licence and attribution every answer carries, and the order sources are listed in.

pub struct SourceInfo {
    pub name: &'static str,
    pub licence: &'static str,
    pub attribution: &'static str,
    pub publisher: &'static str,
}

/// In this order everywhere (a source list in an answer is never in map order).
pub const SOURCES: [SourceInfo; 6] = [
    SourceInfo {
        name: "abr",
        licence: "CC BY 3.0 AU",
        attribution: "Australian Business Register, via data.gov.au",
        publisher: "Australian Business Register",
    },
    SourceInfo { name: "asic", licence: "CC BY 3.0 AU", attribution: "ASIC company dataset, via data.gov.au", publisher: "ASIC" },
    SourceInfo { name: "asicBn", licence: "CC BY 3.0 AU", attribution: "ASIC business names dataset, via data.gov.au", publisher: "ASIC" },
    SourceInfo {
        name: "gnaf",
        licence: "CC BY 4.0 (Open G-NAF EULA)",
        attribution: "G-NAF, Geoscape Australia, Open G-NAF EULA: not for compiling mailing addresses unless each is verified",
        publisher: "Geoscape Australia",
    },
    SourceInfo {
        name: "abs",
        licence: "CC BY 4.0",
        attribution: "Australian Bureau of Statistics",
        publisher: "Australian Bureau of Statistics",
    },
    SourceInfo {
        name: "asgs",
        licence: "CC BY 4.0",
        attribution: "Australian Bureau of Statistics (ASGS)",
        publisher: "Australian Bureau of Statistics",
    },
];

pub fn info(name: &str) -> Option<&'static SourceInfo> {
    SOURCES.iter().find(|s| s.name == name)
}
