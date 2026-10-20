//! Windows builds (MinGW target): embed the app icon in DataTired.exe so Explorer shows it.
//! Other targets: nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=assets/DataTired.ico");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("windows-gnu") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/DataTired.ico");
    let rc = out.join("icon.rc");
    std::fs::write(&rc, format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"))).unwrap();
    let obj = out.join("icon.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| format!("{}-w64-mingw32-windres", target.split('-').next().unwrap_or("x86_64")));
    match std::process::Command::new(&windres).args(["-O", "coff", "-o"]).arg(&obj).arg(&rc).status() {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", obj.display()),
        _ => println!("cargo:warning={windres} not available: DataTired.exe has no icon"),
    }
}
