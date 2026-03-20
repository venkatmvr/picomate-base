use std::fs;

fn main() {
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");

    // Load WiFi credentials from .env (never committed — see .gitignore)
    // Each line must be KEY=VALUE. Whitespace and blank lines are ignored.
    println!("cargo:rerun-if-changed=.env");
    if let Ok(contents) = fs::read_to_string(".env") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            if let Some((key, val)) = line.split_once('=') {
                println!("cargo:rustc-env={}={}", key.trim(), val.trim());
            }
        }
    } else {
        // Allow builds without .env (CI, contributors) — they'll get a compile
        // error only if the code actually calls env!("WIFI_SSID") etc.
        println!("cargo:warning=.env not found — WiFi credentials not set");
    }
}
