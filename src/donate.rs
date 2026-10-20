// Donations: the discreet footer link and the dialog it opens (PayPal and Pix).

use gtk4::glib;
use gtk4::prelude::*;
use qrcode::QrCode;
use std::time::Duration;

use crate::i18n::tr;
use crate::links;

const PAYPAL_URL: &str = "https://www.paypal.com/paypalme/bmacffe";

/// Static Pix code ("Pix copia e cola"): random key, no fixed amount.
const PIX_CODE: &str = "00020126990014BR.GOV.BCB.PIX0136db63b95a-3189-4f4f-b8b1-944775a630d80237Muito obrigado por apoiar o projeto!!5204000053039865802BR5920ELIERMI CUNHA JUNIOR6008SOBRALIA62070503***6304F396";

/// Pseudo link used by the About window to open the donate dialog instead of a web page.
pub const DIALOG_URI: &str = "windusb:donate";

/// Styles for the footer button; loaded together with the app's other CSS.
pub const CSS: &str = "
    button.donate-btn { min-height: 0; padding: 2px 8px; font-size: 0.85em; font-weight: normal; opacity: 0.6; }
    button.donate-btn:hover { opacity: 1; }
    .donate-heart { color: #e01b24; }
";

/// Footer row with the "♥ Donate" link, to append below the page content.
pub fn footer(window: &libadwaita::ApplicationWindow) -> gtk4::Box {
    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    // A text heart instead of an icon name: always renders, whatever icon theme the host has.
    let heart = gtk4::Label::new(Some("♥"));
    heart.add_css_class("donate-heart");
    content.append(&heart);
    content.append(&gtk4::Label::new(Some(&tr("Donate"))));

    let button = gtk4::Button::builder()
        .child(&content)
        .halign(gtk4::Align::Start)
        .build();
    button.add_css_class("flat");
    button.add_css_class("donate-btn");
    let w = window.clone();
    button.connect_clicked(move |_| show_dialog(&w));

    let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    footer.set_margin_start(8);
    footer.set_margin_bottom(8);
    footer.append(&button);
    footer
}

/// "Support WindUSB" dialog: PayPal (opens the browser) and Pix (QR code + copy button).
pub fn show_dialog(parent: &impl IsA<gtk4::Window>) {
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    body.set_margin_start(24);
    body.set_margin_end(24);
    body.set_margin_top(12);
    body.set_margin_bottom(24);

    body.append(
        &gtk4::Label::builder()
            .label(&tr("WindUSB is free and open source. If it helped you, consider supporting its development:"))
            .wrap(true)
            .max_width_chars(40)
            .justify(gtk4::Justification::Center)
            .build(),
    );

    let paypal_btn = gtk4::Button::with_label(&tr("Donate with PayPal"));
    paypal_btn.add_css_class("suggested-action");
    paypal_btn.add_css_class("pill-btn");
    paypal_btn.set_halign(gtk4::Align::Center);
    body.append(&paypal_btn);

    let separator = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    separator.set_margin_top(6);
    separator.set_margin_bottom(6);
    body.append(&separator);

    let pix_title = gtk4::Label::new(Some(&tr("Pix (Brazil)")));
    pix_title.add_css_class("heading");
    body.append(&pix_title);
    let pix_hint = gtk4::Label::builder()
        .label(&tr("Scan with your bank app, or copy the Pix code:"))
        .wrap(true)
        .max_width_chars(40)
        .justify(gtk4::Justification::Center)
        .build();
    pix_hint.add_css_class("dim-label");
    body.append(&pix_hint);

    let code = QrCode::new(PIX_CODE).ok();
    let qr = gtk4::DrawingArea::builder()
        .content_width(200)
        .content_height(200)
        .halign(gtk4::Align::Center)
        .build();
    qr.set_draw_func(move |_, cr, w, h| draw_qr(cr, w, h, code.as_ref()));
    body.append(&qr);

    let copy_btn = gtk4::Button::with_label(&tr("Copy Pix Code"));
    copy_btn.add_css_class("pill-btn");
    copy_btn.set_halign(gtk4::Align::Center);
    copy_btn.connect_clicked(|btn| {
        btn.clipboard().set_text(PIX_CODE);
        btn.set_label(&tr("Copied!"));
        let b = btn.clone();
        glib::timeout_add_local_once(Duration::from_secs(2), move || {
            b.set_label(&tr("Copy Pix Code"));
        });
    });
    body.append(&copy_btn);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&libadwaita::HeaderBar::new());
    content.append(&body);

    let window = libadwaita::Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .title(&tr("Support WindUSB"))
        .content(&content)
        .build();
    let w = window.clone();
    paypal_btn.connect_clicked(move |_| links::open_url(&w, PAYPAL_URL));
    window.present();
}

/// Dark modules on white with a quiet zone, whatever the theme: bank apps
/// need the usual contrast to scan it.
fn draw_qr(cr: &gtk4::cairo::Context, width: i32, height: i32, code: Option<&QrCode>) {
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.paint();
    let Some(code) = code else { return };

    const QUIET_ZONE: usize = 3;
    let modules = code.width();
    let total = (modules + 2 * QUIET_ZONE) as f64;
    let scale = (f64::from(width.min(height)) / total).floor().max(1.0);
    let offset_x = ((f64::from(width) - scale * total) / 2.0).floor();
    let offset_y = ((f64::from(height) - scale * total) / 2.0).floor();

    cr.set_source_rgb(0.0, 0.0, 0.0);
    for (i, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            let x = (i % modules + QUIET_ZONE) as f64;
            let y = (i / modules + QUIET_ZONE) as f64;
            cr.rectangle(offset_x + x * scale, offset_y + y * scale, scale, scale);
        }
    }
    let _ = cr.fill();
}
