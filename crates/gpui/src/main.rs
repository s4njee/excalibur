//! Excalibur's native GPUI application.
//!
//! The GPUI shell is an alternative desktop front end to the existing Tauri
//! shell.  It starts with the same visual vocabulary and keeps protocol/model
//! work behind modules so those pieces can be developed and tested separately.

use std::borrow::Cow;
use std::sync::Arc;

use excalibur_engine::backend::ConnectionSpec;
use excalibur_engine::{Connector, KeyringStore, MessageLog, SecretStore};
use excalibur_gpui::{connector, model, services, settings, shell};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::{px, size, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions};

#[cfg(target_os = "macos")]
mod app_icon;

fn bundled_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
    ]
}

fn main() {
    let startup = service_for_args();
    let settings_store = settings::SettingsStore::new(settings_path());
    let settings_document = settings_store.load().unwrap_or_default();
    let settings = settings_document.settings;
    let bookmarks = settings_document.bookmarks;
    gpui_kit::application()
        .with_assets(excalibur_gpui::assets::ExcaliburAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            #[cfg(target_os = "macos")]
            app_icon::install();

            cx.text_system()
                .add_fonts(bundled_fonts())
                .expect("bundled IBM Plex fonts load");

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1060.), px(680.)),
                    cx,
                ))),
                window_min_size: Some(size(px(840.), px(520.))),
                // Client decorations let the shell own title-bar geometry while
                // preserving native resize, focus, and accessibility behaviour.
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };

            cx.open_window(options, move |window, cx| {
                let browser = cx.new(|_| model::BrowserModel::new(Arc::clone(&startup.services)));
                if let Some(spec) = startup.initial_spec {
                    let request = browser.update(cx, |model, _| model.mount(spec));
                    let token = request.token();
                    let browser_for_task = browser.clone();
                    cx.spawn(async move |cx| {
                        let result = request.into_task().await;
                        let listing = browser_for_task.update(cx, |model, cx| {
                            let mounted =
                                model.apply_mount(token, result) && model.mount_info().is_some();
                            cx.notify();
                            mounted.then(|| model.navigate(""))
                        });
                        let Some(listing) = listing else {
                            return;
                        };
                        let listing_token = listing.token();
                        let listing_result = listing.into_task().await;
                        browser_for_task.update(cx, |model, cx| {
                            model.apply_listing(listing_token, listing_result);
                            cx.notify();
                        });
                    })
                    .detach();
                }
                let shell = cx.new(|cx| {
                    shell::Shell::new_with_model_and_settings(
                        window,
                        cx,
                        browser,
                        settings,
                        bookmarks,
                        settings_store.clone(),
                        Arc::clone(&startup.secrets),
                    )
                });
                cx.new(|cx| Root::new(shell, window, cx))
            })
            .expect("the Excalibur GPUI window opens");
            cx.activate(true);
        });
}

fn settings_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Excalibur")
        .join("settings.json")
}

/// Select the service source before the GPUI event loop starts. Production
/// launches use the native connector, while `--demo` remains an explicit and
/// deterministic fixture mode. `--local PATH` is useful for smoke-testing the
/// native shell against a real local folder without an SSH/NFS server.
struct Startup {
    services: Arc<services::Services>,
    initial_spec: Option<ConnectionSpec>,
    secrets: Arc<dyn SecretStore>,
}

fn service_for_args() -> Startup {
    let mut args = std::env::args().skip(1);
    let mut local = None;
    let mut demo = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => demo = true,
            "--local" => local = args.next(),
            _ => {}
        }
    }
    if demo {
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore);
        return Startup {
            services: Arc::new(services::Services::mock().expect("native service runtime")),
            initial_spec: Some(services::Services::mock_spec()),
            secrets,
        };
    }
    if let Some(path) = local {
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore);
        return Startup {
            services: Arc::new(services::Services::local(false).unwrap_or_else(|error| {
                panic!("cannot start local GPUI services for {path}: {error}")
            })),
            initial_spec: Some(services::Services::local_spec(path, false)),
            secrets,
        };
    }

    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore);
    let log = Arc::new(MessageLog::new());
    let connector: Arc<dyn Connector> = Arc::new(
        connector::NativeConnector::app_default(Arc::clone(&secrets), log)
            .expect("the native known-hosts path is available"),
    );
    Startup {
        services: Arc::new(
            services::Services::from_connector_with_secrets(connector, Some(Arc::clone(&secrets)))
                .expect("native GPUI service runtime starts"),
        ),
        initial_spec: dirs::home_dir()
            .map(|path| services::Services::local_spec(path.to_string_lossy().into_owned(), false)),
        secrets,
    }
}
