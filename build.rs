fn main() {
    println!("cargo:rerun-if-changed=assets/lingoleaf.rc");
    println!("cargo:rerun-if-changed=assets/lingoleaf.manifest");
    println!("cargo:rerun-if-changed=assets/lingoleaf.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let assets = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR")
                .expect("Cargo must provide a manifest directory"),
        )
        .join("assets");
        embed_resource::compile(
            "assets/lingoleaf.rc",
            embed_resource::ParamsIncludeDirs([assets]),
        )
        .manifest_required()
        .expect("Windows resources and application manifest must compile");
    }
}
