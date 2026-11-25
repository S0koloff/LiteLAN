fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("Windows icon resource");
        println!("cargo:rustc-link-arg-bin=litelan=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=litelan=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'");
    }
}
