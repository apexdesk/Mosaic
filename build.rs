fn main() {
    // icono del ejecutable de Windows: recurso precompilado (assets/icon.res, generado desde
    // assets/icon.ico) que link.exe enlaza tal cual; sólo con el enlazador de MSVC
    println!("cargo:rerun-if-changed=assets/icon.res");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let res = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets/icon.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
