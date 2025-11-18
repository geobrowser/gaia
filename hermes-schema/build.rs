use std::io::Result;
// use std::{fs, path::PathBuf};

fn main() -> Result<()> {
    // let out_dir = PathBuf::from("src/pb");
    // fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    // let protos = vec![
    //     "proto/blockchain_metadata.proto",
    //     "proto/knowledge.proto",
    //     "proto/space.proto",
    // ];
    // let proto_include = &["proto/", "../wire/proto/"];

    // let mut config = prost_build::Config::new();
    // config.out_dir(&out_dir);

    // config
    //     .extern_path(".grc20", "::wire::pb::grc20")
    //     .compile_protos(&protos, proto_include)
    //     .expect("Failed to compile protos");

    // // Create a mod.rs file that re-exports each generated file
    // let mod_file = "pub mod blockchain_metadata;\npub mod knowledge;\npub mod space;";

    // fs::write(out_dir.join("mod.rs"), mod_file).expect("Failed to write mod.rs");
    Ok(())
}
