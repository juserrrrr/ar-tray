fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("icons/icon.ico").set("ProductName", "Painel do Ar").set("FileDescription", "Painel do Ar");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=o ícone do exe não foi embutido: {error}");
        }
    }
}
