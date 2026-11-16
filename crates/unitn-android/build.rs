// See shim/hash_memory.cpp: fills in a libc++ function that Skia needs but
// the NDK's libc++ lacks.
fn main() {
    println!("cargo:rerun-if-changed=shim/hash_memory.cpp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        cc::Build::new()
            .cpp(true)
            .cpp_link_stdlib(None)
            // Keep it even though nothing before Skia on the link line uses it.
            .link_lib_modifier("+whole-archive")
            .file("shim/hash_memory.cpp")
            .compile("unitn_libcxx_shim");
    }
}
