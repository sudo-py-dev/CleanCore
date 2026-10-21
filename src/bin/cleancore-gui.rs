// No console window behind the GUI on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> eframe::Result {
    // A windowed app has no console for panic messages: also keep them in the data folder.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = cleancore::config::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let text = format!("[{}] {info}\n{}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), std::backtrace::Backtrace::force_capture());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("gui-crash.log")).and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
        default(info);
    }));
    cleancore::migrate::from_old_name();
    cleancore::update::cleanup_after_update();
    cleancore::gui::run()
}
