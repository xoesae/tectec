//! TecTec, a native LaTeX editor: file tree on the left, editor in the
//! middle, PDF preview compiling on its own as you type, with SyncTeX both
//! ways. Rust + egui port of the former Go/Wails + React app.

mod app;
mod backend;
mod dialogs;
mod editor;
mod fuzzy;
mod i18n;
mod icons;
mod inserts;
mod kinetic;
mod mathpreview;
mod pdfview;
mod preferences;
mod settings;
mod spell;
mod textmatch;
mod theme;
mod tray;
mod widgets;

fn main() -> eframe::Result<()> {
    // `tectec --build-tex-seed DIR`: the LaTeX package cache shipped with
    // the app (see backend/embedded.rs and scripts/release-assets.sh).
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, path] = args.as_slice() {
        match flag.as_str() {
            "--build-tex-seed" => {
                if let Err(e) = backend::embedded::build_seed(std::path::Path::new(path)) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            // `tectec --compile main.tex`: typesets with the built-in LaTeX,
            // without the window.
            "--compile" => {
                let main = std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
                match backend::embedded::compile(&main) {
                    Ok(r) => {
                        for e in &r.errors {
                            eprintln!("{}:{}: {}{}", e.file.as_deref().unwrap_or(&main).display(), e.line, if e.warning { "warning: " } else { "" }, e.message.lines().next().unwrap_or(""));
                        }
                        match r.pdf_path {
                            Some(pdf) => println!("{}", pdf.display()),
                            None => std::process::exit(1),
                        }
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            _ => {}
        }
    }
    // The app was called latex-editor: its settings and recent projects move
    // over once.
    if let (Some(old), Some(new)) = (eframe::storage_dir("latex-editor"), eframe::storage_dir("tectec")) {
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(old, new);
        }
    }
    // Next to eframe's saved state: `tectec.log` is rewritten each run
    // (what goes into a bug report), `panic.log` gets one line per panic.
    let mut logging = fastframe_log::Logging::new("tectec", env!("CARGO_PKG_VERSION")).filter("warn,tectec=info");
    if let Some(dir) = eframe::storage_dir("tectec").filter(|dir| std::fs::create_dir_all(dir).is_ok()) {
        logging = logging.file(dir.join("tectec.log")).panic_log(dir.join("panic.log"));
    }
    let _ = logging.init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TecTec")
            .with_app_id("tectec")
            // The menu bar has the window's buttons (app/window.rs).
            .with_decorations(false)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(tray::window_icon()),
        ..Default::default()
    };
    eframe::run_native("TecTec", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
