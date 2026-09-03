//! `xcal` — drive the engine without a window.
//!
//! The engine has no Tauri dependency precisely so this can exist: bringing up
//! a protocol backend, or reproducing a listing bug, should not require
//! building a GUI. Today it speaks to the mock; each backend joins as it lands.

use std::process::ExitCode;

use excalibur_engine::backend::mock::MockBackend;
use excalibur_engine::ShareBackend;

const USAGE: &str = "\
xcal — headless Excalibur engine driver

USAGE:
    xcal ls [PATH]        list a directory of the mock share
    xcal stat PATH        metadata for one path
    xcal mount            the mount facts a status bar would show
    xcal df               capacity

Every command takes --read-only to serve the mock share read-only, and
--json to print machine-readable output.
";

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let json = arguments.iter().any(|argument| argument == "--json");
    let read_only = arguments.iter().any(|argument| argument == "--read-only");
    let positional: Vec<&String> = arguments
        .iter()
        .filter(|argument| !argument.starts_with("--"))
        .collect();

    let backend = if read_only {
        MockBackend::media_share().read_only("export is read-only")
    } else {
        MockBackend::media_share()
    };

    let Some(command) = positional.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };

    let outcome = match command.as_str() {
        "ls" => {
            let path = positional.get(1).map_or("", |path| path.as_str());
            match backend.list_dir(path).await {
                Ok(listing) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&listing).expect("listing serialises")
                        );
                    } else {
                        for entry in &listing.entries {
                            println!(
                                "{:<28} {:>14} {}",
                                entry.name, entry.attrs.size, entry.kind_label
                            );
                        }
                        let mount = &listing.mount;
                        println!(
                            "\n{} · {} items · {}",
                            if mount.writable { "RW" } else { "RO" },
                            listing.entries.len(),
                            if mount.writable {
                                "read & write".to_owned()
                            } else {
                                mount.reason.clone()
                            }
                        );
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        "stat" => {
            let Some(path) = positional.get(1) else {
                eprintln!("stat needs a path");
                return ExitCode::from(2);
            };
            backend.stat(path).await.map(|attrs| {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&attrs).expect("attrs serialise")
                );
            })
        }
        "mount" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&backend.mount_info()).expect("mount serialises")
            );
            Ok(())
        }
        "df" => backend.capacity().await.map(|capacity| {
            println!("{} free of {}", capacity.free, capacity.total);
        }),
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xcal: {error}");
            ExitCode::FAILURE
        }
    }
}
