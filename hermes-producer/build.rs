fn main() {
    // Use central schema location as source of truth
    prost_build::Config::new()
        .extern_path(".grc20", "::wire::pb::grc20")
        .compile_protos(
            &[
                "../hermes-schema/proto/user_event.proto",
                "../hermes-schema/proto/knowledge.proto"
            ],
            &["../hermes-schema/proto/", "../wire/proto/"]
        )
        .unwrap();
}
