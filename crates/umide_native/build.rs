fn main() {
    // `#[cfg(target_os = ...)]` inside a build script evaluates against the
    // HOST (where the script itself runs), so cross-compiling to a non-macOS
    // target from macOS would still try to build the Objective-C++ sources.
    // Use cargo's CARGO_CFG_TARGET_OS instead — that is the actual target.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rerun-if-changed=cpp/");

        cc::Build::new()
            .cpp(true)
            .file("cpp/macos_surface.mm") // Keep old one for now if needed, or remove if replacing
            .file("cpp/src/emulator_api.cpp")
            .file("cpp/src/macos/macos_emulator.mm")
            .flag("-std=c++17")
            .flag("-fobjc-arc")
            .include("cpp/include")
            .include("cpp") // Add root cpp dir to include path for "src/emulator.h" resolution
            // Link against required macOS frameworks
            .compile("umide_native_cpp");

        // Link macOS frameworks
        println!("cargo:rustc-link-lib=framework=IOSurface");
        println!("cargo:rustc-link-lib=framework=Metal");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=QuartzCore");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
    }
}
