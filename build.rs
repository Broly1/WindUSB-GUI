// Embeds every po/<lang>.po file into the binary, so translators only have
// to drop a new file into po/ (no code changes, nothing extra to bundle).

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=po");
    let po_dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("po");

    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir(&po_dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("po") {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            let lang = path.file_stem().unwrap().to_string_lossy().to_string();
            entries.push((lang, path.to_string_lossy().to_string()));
        }
    }
    entries.sort();

    let mut code = String::from("pub static CATALOGS: &[(&str, &str)] = &[\n");
    for (lang, path) in &entries {
        code.push_str(&format!("    ({:?}, include_str!({:?})),\n", lang, path));
    }
    code.push_str("];\n");

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    fs::write(out, code).unwrap();
}
