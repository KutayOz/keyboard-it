// Windows: Slint arayüzünü derle + uygulama ikonunu/sürüm bilgisini exe'ye göm.
// Diğer platformlarda (mac-sender ile aynı workspace) atlanır.
fn main() {
    #[cfg(windows)]
    {
        slint_build::compile("ui/keyboard-it.slint").expect("slint arayüzü derlenemedi");

        let mut res = winresource::WindowsResource::new();
        res.set_icon("ui/app.ico");
        res.set("FileDescription", "keyboard-it receiver");
        res.set("ProductName", "keyboard-it");
        if let Err(e) = res.compile() {
            println!("cargo:warning=uygulama ikonu gömülemedi: {e}");
        }
    }
}
