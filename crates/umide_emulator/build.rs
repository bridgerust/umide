fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rerun if proto file changes
    println!("cargo:rerun-if-changed=proto/emulator_controller.proto");
    
    // Compile the proto file - tonic-build 0.12 API
    tonic_build::configure()
        .build_server(false)  // We only need the client
        .compile_protos(
            &["proto/emulator_controller.proto"],
            &["proto/"],
        )?;
    
    Ok(())
}
