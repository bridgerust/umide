fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Rerun if proto files change
    println!("cargo:rerun-if-changed=proto/emulator_controller.proto");
    println!("cargo:rerun-if-changed=proto/idb.proto");

    // Compile the proto files - tonic-build 0.12 API
    tonic_build::configure()
        .build_server(false) // We only need the clients
        .compile_protos(
            &["proto/emulator_controller.proto", "proto/idb.proto"],
            &["proto/"],
        )?;

    Ok(())
}
