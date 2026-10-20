// Opening web links (donate, About window) in the user's browser.

use gtk4::glib;
use gtk4::prelude::*;
use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::i18n::tr;
use crate::log;

/// Open a link in the user's default browser. Asks the desktop portal first:
/// it runs in the real desktop session, so it finds the browser no matter how
/// we were started (GTK's own lookup only sees our env's XDG_DATA_DIRS and can
/// end up opening the page in a text editor). Falls back to the host's
/// xdg-open, and as a last resort shows the link so it can be copied.
pub fn open_url(parent: &impl IsA<gtk4::Window>, url: &str) {
    log("gui", &format!("opening {}", url));
    let parent: gtk4::Window = parent.clone().upcast();
    let url = url.to_string();
    let bus = match gtk4::gio::bus_get_sync(gtk4::gio::BusType::Session, None::<&gtk4::gio::Cancellable>) {
        Ok(b) => b,
        Err(e) => {
            log("gui", &format!("no session bus: {}; trying xdg-open", e));
            open_url_xdg(parent, url);
            return;
        }
    };
    // OpenURI(s parent_window, s uri, a{sv} options)
    let params = glib::Variant::tuple_from_iter([
        "".to_variant(),
        url.to_variant(),
        glib::VariantDict::new(None).end(),
    ]);
    bus.call(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
        "OpenURI",
        Some(&params),
        None,
        gtk4::gio::DBusCallFlags::NONE,
        10_000,
        None::<&gtk4::gio::Cancellable>,
        move |res| match res {
            Ok(_) => log("gui", "opened via desktop portal"),
            Err(e) => {
                log("gui", &format!("portal failed: {}; trying xdg-open", e));
                open_url_xdg(parent, url);
            }
        },
    );
}

fn open_url_xdg(parent: gtk4::Window, url: String) {
    let spawned = Command::new("xdg-open")
        .arg(&url)
        // Host binary: drop the AppImage's bundled lib path and give it
        // the system data dirs, where the browser's .desktop file lives.
        .env_remove("LD_LIBRARY_PATH")
        .env("XDG_DATA_DIRS", host_data_dirs())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(c) => Some(c),
        Err(e) => {
            log("gui", &format!("xdg-open failed: {}", e));
            show_link_dialog(&parent, &url);
            return;
        }
    };
    // xdg-open exits quickly on failure, but may keep running while the
    // browser is open, so poll for a few seconds instead of blocking.
    let started = Instant::now();
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(c) = child.as_mut() else { return glib::Continue(false) };
        match c.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    log("gui", &format!("xdg-open failed: {}", status));
                    show_link_dialog(&parent, &url);
                }
            }
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                return glib::Continue(true);
            }
            Ok(None) => {
                // Still running: assume the browser opened; reap it later.
                if let Some(mut c) = child.take() {
                    thread::spawn(move || {
                        let _ = c.wait();
                    });
                }
            }
            Err(e) => log("gui", &format!("xdg-open wait failed: {}", e)),
        }
        glib::Continue(false)
    });
}

/// XDG_DATA_DIRS for host programs: ours minus the AppImage's own dir, plus
/// the spec defaults and the snap/flatpak export dirs (where browsers often live).
fn host_data_dirs() -> String {
    let appdir = env::var("APPDIR").ok();
    let home = env::var("HOME").unwrap_or_default();
    let mut dirs: Vec<String> = env::var("XDG_DATA_DIRS")
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .filter(|d| appdir.as_deref().map_or(true, |a| !d.starts_with(a)))
        .map(String::from)
        .collect();
    for d in [
        "/usr/local/share".to_string(),
        "/usr/share".to_string(),
        "/var/lib/snapd/desktop".to_string(),
        "/var/lib/flatpak/exports/share".to_string(),
        format!("{}/.local/share/flatpak/exports/share", home),
    ] {
        let norm = |s: &str| s.trim_end_matches('/').to_string();
        if !dirs.iter().any(|x| norm(x) == norm(&d)) {
            dirs.push(d);
        }
    }
    dirs.join(":")
}

/// Last resort when no browser could be launched: show the link, selectable.
fn show_link_dialog(parent: &gtk4::Window, url: &str) {
    let dialog = gtk4::MessageDialog::new(
        Some(parent),
        gtk4::DialogFlags::MODAL,
        gtk4::MessageType::Info,
        gtk4::ButtonsType::None,
        &tr("Could not open your web browser. You can copy the link below:"),
    );
    dialog.set_secondary_text(Some(url));
    if let Some(area) = dialog.message_area().downcast_ref::<gtk4::Box>() {
        let mut child = area.first_child();
        while let Some(c) = child {
            if let Some(l) = c.downcast_ref::<gtk4::Label>() {
                l.set_selectable(true);
            }
            child = c.next_sibling();
        }
    }
    dialog.add_button(&tr("Close"), gtk4::ResponseType::Close);
    dialog.connect_response(|d, _| d.destroy());
    dialog.show();
}
