//! Embeds the application icon in the executable (the file icon in Explorer, the taskbar before the window opens).
//! A failure stops the build: an exe without its icon is not silently accepted.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_resource::compile("assets/app.rc", embed_resource::NONE).manifest_required().expect("the icon resource must compile (needs the Windows SDK rc.exe)");
}