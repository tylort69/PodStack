//! Embeds the icon and version details into podstack.exe on Windows (packaging/windows/podstack.rc). Nothing to do
//! on Linux, where the desktop file and icon theme carry them.

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
    embed_resource::compile(
        "../../packaging/windows/podstack.rc",
        [format!("PODSTACK_VERSION=\"{version}\""), format!("PODSTACK_VERSION_COMMAS={commas}")],
    )
    .manifest_optional()
    .expect("couldn't embed the Windows resources");
}
