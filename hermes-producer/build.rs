fn main() {
    // Use central schema location as source of truth
    prost_build::Config::new()
        .compile_protos(&["../schemas/proto/user_event.proto"], &["../schemas/proto/"])
        .unwrap();
}
