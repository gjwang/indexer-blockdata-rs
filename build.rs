use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=wal.fbs");

    // Use the OUT_DIR environment variable provided by Cargo
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not defined");

    flatc_rust::run(flatc_rust::Args {
        inputs: &[Path::new("wal.fbs")],
        out_dir: Path::new(&out_dir), // Write to target/debug/build/...
        ..Default::default()
    })
    .expect("flatc failed - Do you have 'flatc' installed and in your PATH?");
}
