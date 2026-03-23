fn main() {
    // Read version from src-tauri/tauri.conf.json (single source of truth)
    let conf = std::fs::read_to_string("src-tauri/tauri.conf.json")
        .expect("Failed to read src-tauri/tauri.conf.json");
    let version = conf
        .split("\"version\"")
        .nth(1)
        .expect("No version field in tauri.conf.json")
        .split('"')
        .nth(1)
        .expect("No version value in tauri.conf.json");
    println!("cargo:rustc-env=APP_VERSION={}", version);
    println!("cargo:rerun-if-changed=src-tauri/tauri.conf.json");
}
