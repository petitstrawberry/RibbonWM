fn main() {
    for file in ["demo.m", "query.m", "events.m", "bridge.h", "skylight.h"] {
        println!("cargo:rerun-if-changed=../../native/{file}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    cc::Build::new()
        .no_default_flags(true)
        .flag("-O2")
        .flag("-fPIC")
        .flag("-arch")
        .flag(arch)
        .files([
            "../../native/demo.m",
            "../../native/query.m",
            "../../native/events.m",
        ])
        .flag("-fobjc-arc")
        .flag("-Wno-deprecated-declarations")
        .flag("-mmacosx-version-min=14.0")
        .warnings(true)
        .warnings_into_errors(true)
        .compile("ribbon_native");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
}
