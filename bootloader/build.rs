use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Copy memory.x into OUT_DIR so it takes link-search priority over the
    // workspace-root memory.x (which targets 0x10009000 for the app).
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());

    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}
