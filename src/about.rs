// Main menu (hamburger button in the header bar) with About and License.

use gtk4::gio;
use gtk4::prelude::*;

use crate::i18n::tr;
use crate::{donate, links, APP_ID};

const WEBSITE: &str = "https://github.com/Broly1/WindUSB-GUI";
const LICENSE_TEXT: &str = include_str!("../LICENSE");

/// Hamburger button for the header bar. Its items run the window actions
/// registered by `install_actions`.
pub fn menu_button() -> gtk4::MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some(&tr("About WindUSB")), Some("win.about"));
    menu.append(Some(&tr("License")), Some("win.license"));
    gtk4::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text(&tr("Main Menu"))
        .menu_model(&menu)
        .build()
}

/// Register the "win.about" and "win.license" actions on the main window.
pub fn install_actions(window: &libadwaita::ApplicationWindow) {
    let about = gio::SimpleAction::new("about", None);
    let w = window.clone();
    about.connect_activate(move |_, _| show_about(&w));
    window.add_action(&about);

    let license = gio::SimpleAction::new("license", None);
    let w = window.clone();
    license.connect_activate(move |_, _| show_license(&w));
    window.add_action(&license);
}

fn show_about(parent: &libadwaita::ApplicationWindow) {
    let about = libadwaita::AboutWindow::builder()
        .transient_for(parent)
        .modal(true)
        .application_name("WindUSB")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Broly")
        .developers(vec!["Broly https://github.com/Broly1".to_string()])
        .comments(&tr("Create bootable Windows USB drives on Linux"))
        .website(WEBSITE)
        .issue_url(&format!("{}/issues", WEBSITE))
        .copyright("© 2026 Broly")
        .license_type(gtk4::License::Gpl30)
        .build();
    about.add_link(&tr("Donate"), donate::DIALOG_URI);
    about.add_acknowledgement_section(
        Some(&tr("Bundled tools")),
        &[
            "wimlib https://wimlib.net/",
            "GNU Parted https://www.gnu.org/software/parted/",
            "GPT fdisk https://www.rodsbooks.com/gdisk/",
            "util-linux https://github.com/util-linux/util-linux",
            "dosfstools https://github.com/dosfstools/dosfstools",
            "7-Zip https://www.7-zip.org/",
        ],
    );
    // Open links ourselves: GTK's default handler can pick the wrong app (see links.rs).
    about.connect_activate_link(|w, uri| {
        if uri == donate::DIALOG_URI {
            donate::show_dialog(w);
        } else {
            links::open_url(w, uri);
        }
        true
    });
    about.present();
}

/// The full GPL text (the repo's LICENSE file, embedded at build time).
fn show_license(parent: &libadwaita::ApplicationWindow) {
    let text = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .left_margin(18)
        .right_margin(18)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    text.buffer().set_text(LICENSE_TEXT);

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&text)
        .build();

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&libadwaita::HeaderBar::new());
    content.append(&scrolled);

    let window = libadwaita::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(&tr("License"))
        .default_width(640)
        .default_height(560)
        .content(&content)
        .build();
    window.present();
}
