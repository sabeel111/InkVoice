// Embeds the app icon and version info into the Windows .exe (no-op elsewhere).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/inkvoice.ico");
        res.set("ProductName", "InkVoice");
        res.set("FileDescription", "InkVoice — ebook reader & audiobook maker");
        res.compile().expect("embedding Windows resources");
    }
}
