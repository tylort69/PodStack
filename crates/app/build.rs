//! Embeds the icon and version details into podstack.exe on Windows (packaging/windows/podstack.rc). Nothing to do
//! on Linux, where the desktop file and icon theme carry them.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/podstack.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/podstack.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut parts: Vec<&str> = version.split(['.', '-']).take(3).collect();
    parts.resize(3, "0");
    let commas = format!("{},0", parts.join(","));
    // The values are written into a copy of the script: windres's preprocessor loses the quotes of string macros.
    let windows = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows");
    let icon = windows.join("podstack.ico").canonicalize().expect("packaging/windows/podstack.ico");
    let icon = icon.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    let script = std::fs::read_to_string(windows.join("podstack.rc"))
        .expect("packaging/windows/podstack.rc")
        .replace("@VERSION_COMMAS@", &commas)
        .replace("@VERSION@", version)
        .replace("@ICON@", &icon);
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("podstack.rc");
    std::fs::write(&out, script).expect("write podstack.rc");
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().expect("couldn't embed the Windows resources");
}
