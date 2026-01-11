use libadwaita::prelude::*;
use libadwaita::gio;
use gtk4::glib;
use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::thread;

struct AppState {
    drive: Option<String>,
    iso: Option<PathBuf>,
}

enum ProgressMsg {
    Update(String, f64),
    Finished,
    Error(String),
}

fn main() {
    if unsafe { libc::getuid() } == 0 {
        env::set_var("GIO_USE_VFS", "local");
        env::set_var("GSETTINGS_BACKEND", "memory");

        if let Ok(appdir) = env::var("APPDIR") {
            let schema_path = format!("{}/usr/share/glib-2.0/schemas", appdir);
            env::set_var("GSETTINGS_SCHEMA_DIR", schema_path);
        }
    }

    let app = libadwaita::Application::builder()
    .application_id("io.github.windusb")
    .build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &libadwaita::Application) {
    if unsafe { libc::getuid() } != 0 {
        escalate_privileges();
    }

    let state = Arc::new(Mutex::new(AppState { drive: None, iso: None }));

    let window = libadwaita::ApplicationWindow::builder()
    .application(app)
    .title("WindUSB Creator")
    .default_width(500)
    .default_height(250)
    .resizable(false)
    .build();

    window.connect_close_request(|_| {
        unsafe {
            libc::kill(0, libc::SIGTERM);
        }
        std::process::exit(0);
    });

    let style_manager = libadwaita::StyleManager::default();
    style_manager.set_color_scheme(libadwaita::ColorScheme::PreferDark);

    let root_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let header_bar = libadwaita::HeaderBar::new();

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(20);
    content_box.set_margin_end(20);
    content_box.set_margin_top(20);
    content_box.set_margin_bottom(20);

    let stack = gtk4::Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);

    let status_label = gtk4::Label::builder()
    .label("Ready to start...")
    .wrap(true)
    .justify(gtk4::Justification::Center)
    .build();
    let progress_bar = gtk4::ProgressBar::new();

    let finish_btn = gtk4::Button::with_label("Finish & Exit");
    finish_btn.add_css_class("suggested-action");
    finish_btn.set_visible(false);
    finish_btn.connect_clicked(|_| { std::process::exit(0); });

    let (sender, receiver) = glib::MainContext::channel::<ProgressMsg>(glib::Priority::default());

    let st_c = status_label.clone();
    let pb_c = progress_bar.clone();
    let fb_c = finish_btn.clone();

    receiver.attach(None, move |msg| {
        match msg {
            ProgressMsg::Update(text, fraction) => {
                st_c.set_text(&text);
                pb_c.set_fraction(fraction);
            }
            ProgressMsg::Finished => {
                st_c.set_text("Installation Finished! Please reboot and select the USB drive to begin Windows installation.");
                pb_c.set_visible(false);
                fb_c.set_visible(true);
            }
            ProgressMsg::Error(err) => {
                st_c.set_text(&format!("Error: {}", err));
                pb_c.add_css_class("error");
                fb_c.set_label("Close");
                fb_c.set_visible(true);
            }
        }
        glib::ControlFlow::Continue
    });

    let drive_page = build_drive_page(&stack, state.clone());
    let iso_page = build_iso_page(&stack, state.clone(), sender);
    let prog_page = build_progress_page(status_label, progress_bar, finish_btn);

    stack.add_named(&drive_page, Some("drive"));
    stack.add_named(&iso_page, Some("iso"));
    stack.add_named(&prog_page, Some("progress"));

    root_box.append(&header_bar);
    content_box.append(&stack);
    root_box.append(&content_box);

    window.set_content(Some(&root_box));
    window.present();
}

fn escalate_privileges() {
    let args: Vec<String> = env::args().collect();
    let appimage_path = env::var("APPIMAGE").expect("APPIMAGE env var not found");

    let mut cmd = Command::new("pkexec");
    cmd.arg("env");

    let vars_to_pass = [
        "DISPLAY",
        "XAUTHORITY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "HERE",
        "APPDIR",
        "PATH",
        "LD_LIBRARY_PATH",
        "APPIMAGE",
        "XDG_DATA_DIRS",
    ];

    for var in vars_to_pass {
        if let Ok(val) = env::var(var) {
            cmd.arg(format!("{}={}", var, val));
        }
    }

    if let Ok(home) = env::var("HOME") {
        cmd.arg(format!("USER_HOME={}", home));
    }

    let _ = cmd.arg(&appimage_path).args(&args[1..]).exec();
}

fn build_drive_page(stack: &gtk4::Stack, state: Arc<Mutex<AppState>>) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 8);

    // Header for the page
    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);

    let label = gtk4::Label::new(Some("Select USB Drive"));
    label.add_css_class("title-4");
    label.set_hexpand(true);
    label.set_halign(gtk4::Align::Start);

    let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh Drive List"));

    header_box.append(&label);
    header_box.append(&refresh_btn);
    box_.append(&header_box);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    box_.append(&list_box);

    let next_btn = gtk4::Button::with_label("Next");
    next_btn.add_css_class("suggested-action");
    next_btn.set_sensitive(false);
    box_.append(&next_btn);

    // Initial drive scan
    refresh_drives(&list_box);

    // Refresh Logic
    let list_box_refresh = list_box.clone();
    let next_btn_refresh = next_btn.clone();
    refresh_btn.connect_clicked(move |_| {
        refresh_drives(&list_box_refresh);
        next_btn_refresh.set_sensitive(false);
    });

    let next_btn_clone = next_btn.clone();
    let state_clone = state.clone();
    list_box.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let row_action = row.downcast_ref::<libadwaita::ActionRow>().unwrap();
            state_clone.lock().unwrap().drive = Some(row_action.title().to_string());
            next_btn_clone.set_sensitive(true);
        }
    });

    let stack_clone = stack.clone();
    next_btn.connect_clicked(move |_| { stack_clone.set_visible_child_name("iso"); });

    box_
}

fn build_iso_page(stack: &gtk4::Stack, state: Arc<Mutex<AppState>>, sender: glib::Sender<ProgressMsg>) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let label = gtk4::Label::new(Some("Select Windows ISO"));
    label.add_css_class("title-4");

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");

    let iso_row = libadwaita::ActionRow::builder()
    .title("Select ISO File")
    .subtitle("Click to browse")
    .activatable(true)
    .build();

    let folder_icon = gtk4::Image::from_icon_name("folder-open-symbolic");
    iso_row.add_prefix(&folder_icon);
    list_box.append(&iso_row);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    btn_box.set_homogeneous(true);

    let back_btn = gtk4::Button::with_label("Back");
    let start_btn = gtk4::Button::with_label("Flash USB");
    start_btn.add_css_class("destructive-action");
    start_btn.set_sensitive(false);

    let stack_clone_back = stack.clone();
    back_btn.connect_clicked(move |_| {
        stack_clone_back.set_visible_child_name("drive");
    });

    let state_clone = state.clone();
    let start_btn_clone = start_btn.clone();
    let iso_row_clone = iso_row.clone();

    iso_row.connect_activated(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Select Windows ISO"),
                                                  Some(&iso_row_clone.root().and_downcast::<gtk4::Window>().unwrap()),
                                                  gtk4::FileChooserAction::Open,
                                                  &[("_Cancel", gtk4::ResponseType::Cancel), ("_Open", gtk4::ResponseType::Ok)],
        );

        let user_home = std::env::var("USER_HOME").unwrap_or_else(|_| "/home".to_string());
        let downloads_path = format!("{}/Downloads", user_home);
        let downloads_dir = std::path::Path::new(&downloads_path);

        if downloads_dir.exists() {
            let _ = dialog.set_current_folder(Some(&gio::File::for_path(downloads_dir)));
        } else {
            let _ = dialog.set_current_folder(Some(&gio::File::for_path(user_home)));
        }

        let win_filter = gtk4::FileFilter::new();
        win_filter.set_name(Some("Windows ISOs (Win*.iso)"));
        win_filter.add_pattern("Win*.iso");
        win_filter.add_pattern("WIN*.iso");
        dialog.add_filter(&win_filter);

        let s_inner = state_clone.clone();
        let b_inner = start_btn_clone.clone();
        let r_inner = iso_row_clone.clone();

        dialog.connect_response(move |d, res| {
            if res == gtk4::ResponseType::Ok {
                if let Some(file) = d.file() {
                    let path = file.path().unwrap();
                    r_inner.set_title("Selected");
                    r_inner.set_subtitle(&path.file_name().unwrap().to_string_lossy());
                    s_inner.lock().unwrap().iso = Some(path);
                    b_inner.set_sensitive(true);
                }
            }
            d.destroy();
        });
        dialog.show();
    });

    let stack_clone_start = stack.clone();
    start_btn.connect_clicked(move |btn| {
        let drive_name = state.lock().unwrap().drive.clone().unwrap_or_else(|| "Unknown".to_string());

        let confirm_dialog = gtk4::MessageDialog::new(
            Some(&btn.root().and_downcast::<gtk4::Window>().unwrap()),
                                                      gtk4::DialogFlags::MODAL,
                                                      gtk4::MessageType::Warning,
                                                      gtk4::ButtonsType::YesNo,
                                                      &format!("FINAL WARNING: ALL DATA on {} will be PERMANENTLY DELETED.\n\nAre you sure you want to proceed?", drive_name)
        );

        let stack_confirm = stack_clone_start.clone();
        let state_confirm = state.clone();
        let sender_confirm = sender.clone();

        confirm_dialog.connect_response(move |d, res| {
            if res == gtk4::ResponseType::Yes {
                stack_confirm.set_visible_child_name("progress");
                let s = state_confirm.lock().unwrap();
                let drive = s.drive.clone().unwrap();
                let iso = s.iso.clone().unwrap();
                let tx = sender_confirm.clone();
                thread::spawn(move || { run_flasher(drive, iso, tx); });
            }
            d.destroy();
        });
        confirm_dialog.show();
    });

    btn_box.append(&back_btn);
    btn_box.append(&start_btn);

    box_.append(&label);
    box_.append(&list_box);
    box_.append(&btn_box);
    box_
}

fn build_progress_page(status: gtk4::Label, bar: gtk4::ProgressBar, finish: gtk4::Button) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    box_.set_valign(gtk4::Align::Center);
    box_.append(&status);
    box_.append(&bar);
    box_.append(&finish);
    box_
}

fn run_flasher(drive: String, iso: PathBuf, tx: glib::Sender<ProgressMsg>) {
    let usb_mt = "/tmp/windusb_usb_mount";
    let iso_mt = "/tmp/windusb_iso_mount";

    let _ = Command::new("mkdir").args(["-p", usb_mt, iso_mt]).status();

    let _ = tx.send(ProgressMsg::Update(format!("Preparing {}...", drive), 0.05));
    let _ = Command::new("sh").args(["-c", &format!("umount {}*", drive)]).status();
    let _ = Command::new("wipefs").args(["-af", &drive]).status();

    let _ = tx.send(ProgressMsg::Update("Creating Partition Table...".into(), 0.1));
    let _ = Command::new("sgdisk").args(["-Z", &drive]).status();
    let _ = Command::new("sgdisk").args(["-n=1:0:0", "-t=1:0700", &drive]).status();
    let _ = Command::new("partprobe").arg(&drive).status();
    thread::sleep(std::time::Duration::from_secs(3));

    let part = if drive.contains("nvme") { format!("{}p1", drive) } else { format!("{}1", drive) };

    let _ = tx.send(ProgressMsg::Update("Formatting as FAT32...".into(), 0.2));
    if !Command::new("mkfs.fat").args(["-F32", "-I", &part]).status().unwrap().success() {
        let _ = tx.send(ProgressMsg::Error("Formatting failed".into()));
        return;
    }

    let _ = tx.send(ProgressMsg::Update("Mounting ISO and USB...".into(), 0.3));
    let m1 = Command::new("mount").args([&part, usb_mt]).status();
    let m2 = Command::new("mount").args(["-o", "loop,ro", &iso.to_string_lossy(), iso_mt]).status();

    if m1.is_ok() && m2.is_ok() {
        let _ = tx.send(ProgressMsg::Update("Copying files, this can take a long time...".into(), 0.4));
        let _ = Command::new("mkdir").args(["-p", &format!("{}/sources", usb_mt)]).status();

        let wim_path = format!("{}/sources/install.wim", iso_mt);
        let swm_path = format!("{}/sources/install.swm", usb_mt);

        let split_status = Command::new("wimlib-imagex")
        .args(["split", &wim_path, &swm_path, "3500"])
        .status();

        if split_status.is_ok() && split_status.unwrap().success() {
            let _ = tx.send(ProgressMsg::Update("Copying remaining system files...".into(), 0.8));
            let rsync_status = Command::new("rsync")
            .args(["-rltD", "--exclude=sources/install.wim", "--exclude=sources/install.esd", &format!("{}/", iso_mt), &format!("{}/", usb_mt)])
            .status();

            if rsync_status.is_ok() && rsync_status.unwrap().success() {
                let _ = tx.send(ProgressMsg::Update("Syncing changes...".into(), 0.95));
                let _ = Command::new("sync").status();
                let _ = Command::new("umount").arg("-l").arg(usb_mt).status();
                let _ = Command::new("umount").arg("-l").arg(iso_mt).status();
                let _ = tx.send(ProgressMsg::Finished);
            } else {
                let _ = tx.send(ProgressMsg::Error("Rsync failed".into()));
            }
        } else {
            let _ = tx.send(ProgressMsg::Error("Splitting WIM failed".into()));
        }
    } else {
        let _ = tx.send(ProgressMsg::Error("Mounting failed".into()));
    }
}

fn refresh_drives(list: &gtk4::ListBox) {
    // IMPORTANT: Clear the list first
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if let Ok(output) = Command::new("lsblk").args(["-pno", "NAME,SIZE,MODEL,TRAN"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().filter(|l| l.contains("usb")) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let row = libadwaita::ActionRow::builder()
                .title(parts[0])
                .subtitle(parts[1..].join(" "))
                .activatable(true)
                .build();
                let icon = gtk4::Image::from_icon_name("drive-removable-media-symbolic");
                row.add_prefix(&icon);
                list.append(&row);
            }
        }
    }
}
