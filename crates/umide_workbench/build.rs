fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/browser.m");
        println!("cargo:rerun-if-changed=src/file_editor/app_icons.m");
        cc::Build::new()
            .file("src/browser.m")
            .file("src/file_editor/app_icons.m")
            .flag("-fobjc-arc")
            .compile("umide_web_preview");
        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
}
