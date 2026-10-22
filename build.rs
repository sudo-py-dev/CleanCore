//! Embeds every interface translation: each `locales/<code>.po` becomes a language, so adding
//! a language is adding its catalog (see `locales/README.md`).

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=locales");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    let mut catalogs: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("locales/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "po"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), p.to_string_lossy().into_owned()))
        .collect();
    catalogs.sort();
    let mut out = String::from("&[\n");
    for (code, path) in catalogs {
        println!("cargo:rerun-if-changed={path}");
        out += &format!("    ({code:?}, include_str!({path:?})),\n");
    }
    out += "]\n";
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    std::fs::write(dest, out).unwrap();
}
