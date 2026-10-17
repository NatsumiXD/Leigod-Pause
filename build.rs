fn main() {
    // Only release launcher binaries request elevation. Tests and development
    // diagnostics remain ordinary processes. Node children inherit the token.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        println!("cargo:rustc-link-arg-bin=leigod-status=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=leigod-status=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'"
        );
    }
    println!("cargo:rerun-if-changed=build.rs");
}
