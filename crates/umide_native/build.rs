fn main() {
    // Only compile C++ on macOS for now
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=cpp/");
        
        cc::Build::new()
            .cpp(true)
            .file("cpp/macos_surface.mm")
            .flag("-std=c++17")
            .flag("-fobjc-arc")
            .include("cpp/include")
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
