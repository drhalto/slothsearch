#![windows_subsystem = "windows"]
//! SlothSearch: a Spotlight-style launcher for Windows. Alt+Space searches apps, files
//! (through Everything), open windows, git repos, commands and quick math.

mod app;
mod apps;
mod calc;
mod commands;
mod config;
mod everything;
mod explorer;
mod favorites;
mod filters;
mod history;
mod icons;
mod launch;
mod model;
mod projects;
mod search;
mod shell;
mod theme;
mod util;
mod webpreview;
mod wins;

use eframe::egui;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::{PCWSTR, w};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = args.first() {
        cli(cmd, &args[1..]);
        return;
    }

    // One copy at a time: a second launch just opens the first one's window.
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\SlothSearch.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(shell::CLASS, PCWSTR::null()) {
                let _ = PostMessageW(Some(h), shell::WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }
        std::mem::forget(_mutex);
    }

    let viewport = egui::ViewportBuilder::default()
        .with_title("SlothSearch")
        .with_inner_size([shell::WIN_W, shell::WIN_H])
        .with_position([-32000.0, -32000.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_active(false)
        .with_has_shadow(false);
    let options = eframe::NativeOptions { viewport, centered: false, ..Default::default() };
    let _ = eframe::run_native("SlothSearch", options, Box::new(|cc| Ok(Box::new(app::SlothSearch::new(cc)))));
}

/// Debugging commands, printed to the console: `slothsearch query devduck`.
fn cli(cmd: &str, rest: &[String]) {
    let text = rest.join(" ");
    match cmd {
        "query" => {
            let t = std::time::Instant::now();
            let (cfg, _) = config::load();
            let mut engine = search::Engine::new(cfg, history::History::load());
            engine.apps = apps::enumerate();
            let mut client = everything::Client::new().expect("everything client");
            engine.projects = projects::scan(&mut client);
            engine.wins = wins::list();
            let loaded = t.elapsed();
            let t = std::time::Instant::now();
            let (filter, text) = filters::parse(&text);
            let files: Vec<_> = engine
                .searches(&text, None, filter.as_ref())
                .iter()
                .map(|s| client.query(&s.text, s.max, s.sort, std::time::Duration::from_secs(2)))
                .collect();
            let sections = engine.sections(&text, None, filter.as_ref(), &files);
            println!("sources {loaded:?}, search {:?}", t.elapsed());
            for s in sections {
                println!("== {}", s.title);
                for i in s.items {
                    println!("  {:6.1}  {:<48} {}", i.score, i.title.chars().take(48).collect::<String>(), i.subtitle);
                }
            }
        }
        "apps" => {
            for a in apps::enumerate().iter().filter(|a| a.name.to_lowercase().contains(&text.to_lowercase())) {
                println!("{:<40} {:<70} {:?}", a.name, a.id, a.target);
            }
        }
        "wins" => {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            for w in wins::list() {
                let scope = explorer::current(w.hwnd);
                println!("{:<10} {:<20} {:<60} {:?} {:?}", w.hwnd, w.process, w.title.chars().take(60).collect::<String>(), w.aumid, scope);
            }
        }
        "calc" => println!("{:?}", calc::eval(&text)),
        // Replays typing through the background worker, as the window does.
        "typing" => {
            let (cfg, _) = config::load();
            let engine = search::Engine::new(cfg, history::History::load());
            let (tx, rx) = std::sync::mpsc::channel();
            let worker = everything::spawn(move |r| {
                if let everything::Reply::Search(r) = r {
                    let _ = tx.send(r);
                }
            });
            let start = std::time::Instant::now();
            let mut id = 0;
            for n in 1..=text.len() {
                id += 1;
                let _ = worker.send(everything::Job::Search(everything::Request { id, searches: engine.searches(&text[..n], None, None) }));
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            while let Ok(r) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
                let summary: Vec<String> = r
                    .results
                    .iter()
                    .map(|x| match x {
                        Ok(x) => format!("{}/{}", x.hits.len(), x.total),
                        Err(e) => format!("{e:?}"),
                    })
                    .collect();
                println!("{:>6?} req {} took {:?}: {summary:?}", start.elapsed(), r.id, r.elapsed);
                if r.id == id {
                    break;
                }
            }
        }
        "ev" => {
            let mut c = everything::Client::new().expect("everything client");
            let t = std::time::Instant::now();
            match c.query(&text, 1500, everything::SORT_DATE_MODIFIED_DESC, std::time::Duration::from_secs(10)) {
                Ok(r) => println!("{:?} total {} first {:?}", t.elapsed(), r.total, r.hits.first().map(|h| h.path())),
                Err(e) => println!("{:?} {e:?}", t.elapsed()),
            }
        }
        _ => println!("usage: slothsearch [query <text> | apps [filter] | wins | calc <expr>]"),
    }
}
