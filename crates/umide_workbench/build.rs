fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/browser.m");
        cc::Build::new()
            .file("src/browser.m")
            .flag("-fobjc-arc")
            .compile("umide_web_preview");
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
}
