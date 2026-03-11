fn main() {
    // Use protox (pure-Rust protobuf compiler) instead of protoc binary
    let file_descriptors = protox::compile(["proto/telemetry.proto"], ["proto/"])
        .expect("failed to compile proto files");

    tonic_build::compile_fds(file_descriptors)
        .expect("failed to generate gRPC code from proto");
}
