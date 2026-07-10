// Windows: compile the Slint UI + embed the app icon/version info into the exe.
// Skipped on other platforms (same workspace as mac-sender).
fn main() {
    #[cfg(windows)]
    {
        slint_build::compile("ui/keyboard-it.slint").expect("failed to compile the Slint UI");

        let mut res = winresource::WindowsResource::new();
        res.set_icon("ui/app.ico");
        res.set("FileDescription", "keyboard-it receiver");
        res.set("ProductName", "keyboard-it");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the app icon: {e}");
        }
    }
}
