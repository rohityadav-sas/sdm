fn main() {
    // Bake the fixed Chromium extension ID into the binary.
    let id = std::fs::read_to_string("../../extension/EXTENSION_ID").expect("extension/EXTENSION_ID");
    println!("cargo:rustc-env=SDM_EXTENSION_ID={}", id.trim());
    println!("cargo:rerun-if-changed=../../extension");
}
