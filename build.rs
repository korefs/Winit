fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    slint_build::compile_with_config(
        "src/ui/app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("Falha ao compilar a interface Slint");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "Winit")
            .set("InternalName", "Winit")
            .set("OriginalFilename", "Winit.exe")
            .set("FileDescription", "Winit — Central de aplicativos")
            .set_icon("assets/icon.ico")
            .set_manifest_file("assets/app.manifest");
        res.compile()
            .expect("Falha ao compilar os recursos Windows");
    }
}
