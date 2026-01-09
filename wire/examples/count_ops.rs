use prost::Message;
use std::fs;
use wire::pb::grc20::Edit;

fn main() {
    let data = fs::read("data/proto").expect("Failed to read proto file");
    let edit = Edit::decode(&data[..]).expect("Failed to decode");

    println!("Ops count: {}", edit.ops.len());
    println!("Proto size: {} bytes ({:.2} MB)", data.len(), data.len() as f64 / 1_000_000.0);
    println!("Bytes per op: {:.1}", data.len() as f64 / edit.ops.len() as f64);
}
