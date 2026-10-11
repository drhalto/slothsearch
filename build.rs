// Embeds the sloth icon (resource 1, also the tray icon) and version info in the exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/slothsearch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("assets/slothsearch.ico", "1");
        res.set("FileDescription", "SlothSearch");
        res.set("ProductName", "SlothSearch");
        let _ = res.compile();
    }
}
