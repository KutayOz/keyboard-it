// Generate Slint for the target OS, including cross-checks from macOS.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        slint_build::compile("ui/keyboard-it.slint").expect("failed to compile the Slint UI");
        #[cfg(windows)]
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("ui/app.ico");
            res.set("FileDescription", "keyboard-it receiver");
            res.set("ProductName", "keyboard-it");
            if let Err(e) = res.compile() {
                println!("cargo:warning=could not embed the app icon: {e}");
            }
        }
    }
}
