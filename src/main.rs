#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    lingoleaf::platform::app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("轻译 LingoLeaf 是 Windows 桌面应用，请在 Windows 上运行发布的 EXE。");
    std::process::exit(1);
}
