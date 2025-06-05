// use prost_build::Config;
use std::io::Result;
// use std::{fs, path::Path, path::PathBuf};

fn main() -> Result<()> {
    // let out_dir = PathBuf::from("src/pb");
    // fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    // // List all .proto files explicitly
    // let protos = vec!["proto/chain.proto", "proto/grc20.proto"];
    // let proto_include = &["proto"];

    // let mut config = Config::new();
    // config.out_dir(&out_dir);

    // // Generate one file per proto
    // config
    //     .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
    //     .compile_protos(&protos, proto_include)
    //     .expect("Failed to compile protos");

    // // Create a mod.rs file that re-exports each generated file
    // let mut mod_file = String::new();
    // for proto_path in &protos {
    //     let filename = Path::new(proto_path).file_stem().unwrap().to_str().unwrap();
    //     mod_file.push_str(&format!("pub mod {};\n", filename));
    // }

    // fs::write(out_dir.join("mod.rs"), mod_file).expect("Failed to write mod.rs");
    Ok(())
}
