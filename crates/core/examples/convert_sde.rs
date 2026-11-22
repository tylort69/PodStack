//! Refreshes the static data snapshot bundled in `data/` from an unzipped copy of CCP's static data export (JSONL):
//! https://developers.eveonline.com/static-data/eve-online-static-data-latest-jsonl.zip
//!
//! cargo run -p podstack-core --example convert_sde -- <unzipped dir>
//!
//! PodStack also downloads newer builds by itself while running (see `static_data`); the snapshot only covers
//! offline first starts.

fn main() {
    let src = std::env::args().nth(1).expect("usage: convert_sde <unzipped SDE dir>");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let info = podstack_core::static_data::convert_dir(std::path::Path::new(&src), &out).expect("conversion");
    // The bundled build number lives in its own file: `build.json` belongs to the cache layout.
    std::fs::rename(out.join("build.json"), out.join("sde_build.json")).expect("sde_build.json");
    println!("converted build {}", info.build);
}
