fn main() {
    prost_build::compile_protos(&["src/ss.proto"], &["src/"]).unwrap();
}
