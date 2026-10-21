fn main() {
    println!("cargo:rerun-if-env-changed=MFMCL_MICROSOFT_CLIENT_ID");
    tauri_build::build()
}
