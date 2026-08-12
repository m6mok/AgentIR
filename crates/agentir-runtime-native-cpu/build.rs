fn main() {
    let target = std::env::var("TARGET").expect("Cargo supplies TARGET to build scripts");
    println!("cargo:rustc-env=AGENTIR_NATIVE_PARENT_TARGET={target}");
}
