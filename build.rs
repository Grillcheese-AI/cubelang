fn main() {
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().expect("vendored protoc"));
    prost_build::compile_protos(&["proto/reasoning.proto"], &["proto/"]).unwrap();
    println!("cargo:rerun-if-changed=proto/reasoning.proto");
}
