#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! blindmark desktop app — native Win32 (Windows only). On other systems use `blindmark-cli`.

#[cfg(any(windows, feature = "win32-check"))]
mod win;

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(not(windows))]
fn main() {
    #[cfg(feature = "win32-check")]
    let _ = win::run;
    eprintln!("blindmark 的图形界面只支持 Windows。在这个系统上请使用命令行版：blindmark-cli --help");
    std::process::exit(1);
}
