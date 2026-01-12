use libadwaita::prelude::*;
use gtk4::glib;
use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::path::{PathBuf, Path};
use std::thread;
use std::sync::mpsc;

struct AppState {
    drive: Option<String>,
    iso: Option<PathBuf>,
}

enum ProgressMsg {
    Update(String, f64),
    Finished,
    Error(String),
}

fn cleanup_processes() {
    let pgid = unsafe { libc::getpgrp() };
    thread::spawn(move || {
        let _ = Command::new("pkill").args(["-9", "wimlib-imagex"]).status();
        let _ = Command::new("pkill").args(["-9", "7z"]).status();
        let _ = Command::new("sh")
        .args(["-c", "umount -l /tmp/windusb_* 2>/dev/null"])
        .status();
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    });
    std::process::exit(0);
}

fn main() {
    unsafe {
        libc::setpgid(0, 0);
    }
    env::set_var("GSETTINGS_BACKEND", "memory");
    env::set_var("GTK_USE_PORTAL", "1");
    env::set_var("GIO_USE_VFS", "local");
    ctrlc::set_handler(move || {
        cleanup_processes();
    }).expect("Error setting Ctrl-C handler");
    let app = libadwaita::Application::builder()
    .application_id("io.github.windusb")
    .build();
    app.connect_activate(build_ui);
    app.run();
}

fn is_valid_windows_iso(path: &Path) -> bool {
    let z_bin = if let Ok(appdir) = env::var("APPDIR") {
        format!("{}/bin-local/7z", appdir)
    } else {
        "7z".to_string()
    };
    let output = Command::new(z_bin)
    .args(["l", &path.to_string_lossy()])
    .output();
    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
        stdout.contains("sources/install.wim") || stdout.contains("sources/install.esd")
    } else {
        false
    }
}

fn get_system_dirty_bytes() -> f64 {
    let mut total_kb = 0.0;
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("Dirty:") || line.starts_with("Writeback:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<f64>() {
                        total_kb += kb;
                    }
                }
            }
        }
    }
    total_kb * 1024.0
}

fn device_exists(drive: &str) -> bool {
    Path::new(drive).exists()
}

fn run_flasher(drive: String, iso: PathBuf, tx: mpsc::Sender<ProgressMsg>) {
    let usb_mt = format!("/tmp/windusb_usb_{}", unsafe { libc::rand() });
    let iso_mt = format!("/tmp/windusb_iso_{}", unsafe { libc::rand() });
    let _ = Command::new("mkdir").args(["-p", &usb_mt, &iso_mt]).status();

    let z_bin = if let Ok(appdir) = std::env::var("APPDIR") {
        format!("{}/bin-local/7z", appdir)
    } else {
        "7z".to_string()
    };

    let list_output = Command::new(&z_bin).args(["l", &iso.to_string_lossy()]).output().ok();
    let mut install_file = String::new();
    let mut extension = String::new();
    if let Some(out) = list_output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout.contains("sources/install.wim") {
            install_file = "sources/install.wim".to_string();
            extension = "swm".to_string();
        } else if stdout.contains("sources/install.esd") {
            install_file = "sources/install.esd".to_string();
            extension = "esd".to_string();
        }
    }

    if install_file.is_empty() {
        let _ = tx.send(ProgressMsg::Error("Invalid ISO: install.wim/esd not found".into()));
        return;
    }

    let _ = tx.send(ProgressMsg::Update(format!("Formatting drive {}...", drive), 0.02));
    let _ = Command::new("sh").args(["-c", &format!("umount -l {}* 2>/dev/null", drive)]).status();

    if !device_exists(&drive) {
        let _ = tx.send(ProgressMsg::Error("Drive disconnected before formatting".into()));
        return;
    }

    let _ = Command::new("blockdev").args(["--flushbufs", &drive]).status();
    let _ = Command::new("wipefs").args(["-af", &drive]).status();
    let _ = Command::new("sgdisk").args(["-Z", &drive]).status();
    let _ = Command::new("sgdisk").args(["-n=1:0:0", "-t=1:0700", &drive]).status();
    let _ = Command::new("partprobe").arg(&drive).status();
    thread::sleep(std::time::Duration::from_secs(2));

    let part = if drive.contains("nvme") { format!("{}p1", drive) } else { format!("{}1", drive) };
    if Command::new("mkfs.fat").args(["-F32", "-I", &part]).status().is_err() {
        let _ = tx.send(ProgressMsg::Error("Formatting failed. Drive may have been removed.".into()));
        return;
    }

    if Command::new("mount").args([&part, &usb_mt]).status().is_err() {
        let _ = tx.send(ProgressMsg::Error("Failed to mount USB drive.".into()));
        return;
    }
    let _ = Command::new("mount").args(["-o", "loop,ro", &iso.to_string_lossy(), &iso_mt]).status();

    let install_full_path = format!("{}/{}", iso_mt, install_file);
    let wim_size = match std::fs::metadata(&install_full_path) {
        Ok(m) => m.len() as f64,
        Err(_) => 4_000_000_000.0,
    };

    let is_active = Arc::new(Mutex::new(true));
    let phase = Arc::new(Mutex::new(1));

    let is_active_t = is_active.clone();
    let phase_t = phase.clone();
    let tx_t = tx.clone();
    let usb_mt_t = usb_mt.clone();
    let drive_t = drive.clone();

    thread::spawn(move || {
        let total_wim_mb = wim_size / 1024.0 / 1024.0;
        let mut baseline_size = 0.0;

        while *is_active_t.lock().unwrap() {
            if !device_exists(&drive_t) { break; }

            let current_phase = *phase_t.lock().unwrap();
            let output = Command::new("du").args(["-sb", &usb_mt_t]).output();

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(size_str) = stdout.split_whitespace().next() {
                    if let Ok(current_bytes) = size_str.parse::<f64>() {
                        if current_phase == 1 {
                            let dirty = get_system_dirty_bytes();
                            let actual = (current_bytes - dirty).max(0.0);
                            let progress = 0.05 + ((actual / 500_000_000.0).min(1.0) * 0.20);
                            let _ = tx_t.send(ProgressMsg::Update(
                                "Extracting boot files...".to_string(),
                                                                  progress
                            ));
                            baseline_size = current_bytes;
                        } else {
                            let wim_progress_bytes = (current_bytes - baseline_size).max(0.0);
                            let mb = wim_progress_bytes / 1024.0 / 1024.0;
                            let progress = 0.25 + ((wim_progress_bytes / wim_size).min(1.0) * 0.55);
                            let _ = tx_t.send(ProgressMsg::Update(
                                format!("Splitting install.wim: {:.0} / {:.0} MB", mb, total_wim_mb),
                                    progress
                            ));
                        }
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(500));
        }
    });

    let status_7z = Command::new(&z_bin)
    .args(["x", &iso.to_string_lossy(), &format!("-o{}", usb_mt), &format!("-xr!{}", install_file.split('/').last().unwrap()), "-y"])
    .status();

    if status_7z.is_err() || !status_7z.unwrap().success() || !device_exists(&drive) {
        *is_active.lock().unwrap() = false;
        let _ = tx.send(ProgressMsg::Error("Drive removed or 7z error during extraction.".into()));
        return;
    }

    {
        let mut p = phase.lock().unwrap();
        *p = 2;
    }

    let dst_path = format!("{}/sources/install.{}", usb_mt, extension);
    let status_wim = Command::new("wimlib-imagex")
    .args(["split", &install_full_path, &dst_path, "3400"])
    .status();

    if status_wim.is_err() || !status_wim.unwrap().success() || !device_exists(&drive) {
        *is_active.lock().unwrap() = false;
        let _ = tx.send(ProgressMsg::Error("Drive removed or wimlib error during split.".into()));
        return;
    }

    *is_active.lock().unwrap() = false;

    let initial_dirty = get_system_dirty_bytes().max(1.0);
    let usb_mt_c = usb_mt.clone();
    let iso_mt_c = iso_mt.clone();
    let unmount_done = Arc::new(Mutex::new(false));
    let unmount_error = Arc::new(Mutex::new(None));
    let unmount_done_t = unmount_done.clone();
    let unmount_err_t = unmount_error.clone();
    let drive_check = drive.clone();

    thread::spawn(move || {
        let s1 = Command::new("sync").status();
        let s2 = Command::new("umount").arg("-l").arg(&usb_mt_c).status();
        let _ = Command::new("umount").arg("-l").arg(&iso_mt_c).status();

        if s1.is_err() || s2.is_err() || !Path::new(&drive_check).exists() {
            let mut err = unmount_err_t.lock().unwrap();
            *err = Some("Sync failed. Drive was likely unplugged.".to_string());
        }
        let mut done = unmount_done_t.lock().unwrap();
        *done = true;
    });

    let mut spin_idx = 0;
    let spinners = vec!["-", "\\", "|", "/"];
    loop {
        if *unmount_done.lock().unwrap() { break; }

        if let Some(err_msg) = unmount_error.lock().unwrap().clone() {
            let _ = tx.send(ProgressMsg::Error(err_msg));
            return;
        }

        if !device_exists(&drive) {
            let _ = tx.send(ProgressMsg::Error("Drive disconnected during final sync.".into()));
            return;
        }

        let current_dirty = get_system_dirty_bytes();
        let sync_progress = 0.80 + ((1.0 - (current_dirty / initial_dirty)) * 0.19);

        if current_dirty <= 10.0 * 1024.0 * 1024.0 {
            spin_idx = (spin_idx + 1) % 4;
            let _ = tx.send(ProgressMsg::Update(format!("Finishing writes... {}", spinners[spin_idx]), 0.99));
        } else {
            let mb_left = current_dirty / 1024.0 / 1024.0;
            let _ = tx.send(ProgressMsg::Update(format!("Flushing cache: {:.1} MB left", mb_left), sync_progress.min(0.99)));
        }
        thread::sleep(std::time::Duration::from_millis(200));
    }

    let _ = tx.send(ProgressMsg::Finished);
}

fn build_ui(app: &libadwaita::Application) {
    if unsafe { libc::getuid() } != 0 {
        escalate_privileges();
    }
    let provider = gtk4::CssProvider::new();
    provider.load_from_data("
    button { border-radius: 99px; padding-left: 24px; padding-right: 24px; min-height: 38px; }
    .invalid-iso { background-color: rgba(237, 51, 59, 0.15); border: 1px solid #ed333b; border-radius: 12px; }
    .invalid-iso label { color: #ff7b72; }
    .title-4 { margin-bottom: 8px; }

    progressbar progress {
    background-color: #612a74;
    background-image: none;
    min-height: 12px;
    border-radius: 99px;
}

progressbar trough {
background-color: rgba(145, 65, 172, 0.1);
    min-height: 12px;
    border-radius: 99px;
}
");
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("Could not connect to a display."),
                                                 &provider,
                                                 gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let style_manager = libadwaita::StyleManager::default();
    style_manager.set_color_scheme(libadwaita::ColorScheme::PreferDark);
    let state = Arc::new(Mutex::new(AppState { drive: None, iso: None }));
    let window = libadwaita::ApplicationWindow::builder()
    .application(app)
    .title("WindUSB Creator")
    .default_width(550)
    .default_height(380)
    .resizable(false)
    .build();
    window.connect_close_request(|_| {
        cleanup_processes();
        glib::Propagation::Proceed
    });
    let root_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let header_bar = libadwaita::HeaderBar::new();
    header_bar.set_show_end_title_buttons(true);
    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(30);
    content_box.set_margin_end(30);
    content_box.set_margin_top(30);
    content_box.set_margin_bottom(30);
    let stack = gtk4::Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);
    let status_label = gtk4::Label::builder()
    .label("Waiting...")
    .wrap(true)
    .justify(gtk4::Justification::Center)
    .build();
    let progress_bar = gtk4::ProgressBar::new();
    let percent_label = gtk4::Label::builder()
    .label("0%")
    .width_chars(5)
    .valign(gtk4::Align::Center)
    .build();
    percent_label.add_css_class("caption");
    let finish_btn = gtk4::Button::with_label("Finish & Exit");
    finish_btn.add_css_class("suggested-action");
    finish_btn.set_visible(false);
    finish_btn.connect_clicked(|_| { cleanup_processes(); });
    let cancel_btn = gtk4::Button::with_label("Cancel");
    cancel_btn.add_css_class("destructive-action");
    cancel_btn.connect_clicked(|_| { cleanup_processes(); });
    let (sender, receiver) = mpsc::channel::<ProgressMsg>();
    let st_c = status_label.clone();
    let pb_c = progress_bar.clone();
    let fb_c = finish_btn.clone();
    let cb_c = cancel_btn.clone();
    let pl_c = percent_label.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        while let Ok(msg) = receiver.try_recv() {
            match msg {
                ProgressMsg::Update(text, fraction) => {
                    st_c.set_text(&text);
                    pb_c.set_fraction(fraction);
                    let p = (fraction * 100.0).floor() as u32;
                    pl_c.set_text(&format!("{}%", p));
                }
                ProgressMsg::Finished => {
                    st_c.set_text("Installation Finished! You can now safely unplug the drive.");
                    pb_c.set_visible(false);
                    pl_c.set_visible(false);
                    cb_c.set_visible(false);
                    fb_c.set_visible(true);
                }
                ProgressMsg::Error(err) => {
                    st_c.set_text(&format!("Error: {}", err));
                    pb_c.add_css_class("error");
                    pl_c.set_visible(false);
                    cb_c.set_visible(false);
                    fb_c.set_label("Close");
                    fb_c.set_visible(true);
                }
            }
        }
        glib::ControlFlow::Continue
    });
    let drive_page = build_drive_page(&stack, state.clone());
    let iso_page = build_iso_page(&stack, state.clone(), sender);
    let prog_page = build_progress_page(status_label, progress_bar, percent_label, finish_btn, cancel_btn);
    stack.add_named(&drive_page, Some("drive"));
    stack.add_named(&iso_page, Some("iso"));
    stack.add_named(&prog_page, Some("progress"));
    root_box.append(&header_bar);
    content_box.append(&stack);
    root_box.append(&content_box);
    window.set_content(Some(&root_box));
    window.present();
}

fn build_drive_page(stack: &gtk4::Stack, state: Arc<Mutex<AppState>>) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    let label = gtk4::Label::new(Some("Select USB Drive"));
    label.add_css_class("title-4");
    label.set_hexpand(true);
    label.set_halign(gtk4::Align::Start);
    let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.add_css_class("flat");
    header_box.append(&label);
    header_box.append(&refresh_btn);
    box_.append(&header_box);
    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    box_.append(&list_box);
    let next_btn = gtk4::Button::with_label("Next");
    next_btn.add_css_class("suggested-action");
    next_btn.set_sensitive(false);
    next_btn.set_halign(gtk4::Align::Center);
    next_btn.set_margin_top(12);
    box_.append(&next_btn);
    refresh_drives(&list_box);
    let lb_ref = list_box.clone();
    let nb_ref = next_btn.clone();
    refresh_btn.connect_clicked(move |_| {
        refresh_drives(&lb_ref);
        nb_ref.set_sensitive(false);
    });
    let nb_c = next_btn.clone();
    let s_c = state.clone();
    list_box.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            let row_action = row.downcast_ref::<libadwaita::ActionRow>().unwrap();
            s_c.lock().unwrap().drive = Some(row_action.title().to_string());
            nb_c.set_sensitive(true);
        }
    });
    let st_c = stack.clone();
    next_btn.connect_clicked(move |_| { st_c.set_visible_child_name("iso"); });
    box_
}

fn build_iso_page(stack: &gtk4::Stack, state: Arc<Mutex<AppState>>, sender: mpsc::Sender<ProgressMsg>) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    let label = gtk4::Label::new(Some("Select Windows ISO"));
    label.add_css_class("title-4");
    label.set_halign(gtk4::Align::Start);
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
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
    btn_box.set_halign(gtk4::Align::Center);
    btn_box.set_margin_top(12);
    let back_btn = gtk4::Button::with_label("Back");
    let start_btn = gtk4::Button::with_label("Flash USB");
    start_btn.add_css_class("destructive-action");
    start_btn.set_sensitive(false);
    let st_c = stack.clone();
    back_btn.connect_clicked(move |_| { st_c.set_visible_child_name("drive"); });
    let s_c = state.clone();
    let b_c = start_btn.clone();
    let r_c = iso_row.clone();
    iso_row.connect_activated(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Select Windows ISO"),
                                                  Some(&r_c.root().and_downcast::<gtk4::Window>().unwrap()),
                                                  gtk4::FileChooserAction::Open,
                                                  &[("_Cancel", gtk4::ResponseType::Cancel), ("_Open", gtk4::ResponseType::Ok)],
        );

        let user_home = std::env::var("USER_HOME").unwrap_or_else(|_| "/home".to_string());
        let downloads = format!("{}/Downloads", user_home);
        if std::path::Path::new(&downloads).exists() {
            let _ = dialog.set_current_folder(Some(&gtk4::gio::File::for_path(downloads)));
        }

        let filter = gtk4::FileFilter::new();
        filter.set_name(Some("Windows ISOs (*.iso)"));
        filter.add_pattern("*.iso");
        filter.add_pattern("*.ISO");
        dialog.add_filter(&filter);
        let s_i = s_c.clone();
        let b_i = b_c.clone();
        let r_i = r_c.clone();
        dialog.connect_response(move |d, res| {
            if res == gtk4::ResponseType::Ok {
                if let Some(file) = d.file() {
                    let path = file.path().unwrap();
                    if is_valid_windows_iso(&path) {
                        r_i.remove_css_class("invalid-iso");
                        r_i.set_title("Selected (Valid)");
                        r_i.set_subtitle(&path.file_name().unwrap().to_string_lossy());
                        s_i.lock().unwrap().iso = Some(path);
                        b_i.set_sensitive(true);
                    } else {
                        r_i.add_css_class("invalid-iso");
                        r_i.set_title("Invalid ISO");
                        r_i.set_subtitle("Missing install.wim/esd");
                        b_i.set_sensitive(false);
                    }
                }
            }
            d.destroy();
        });
        dialog.show();
    });
    let st_flash = stack.clone();
    start_btn.connect_clicked(move |btn| {
        let drive_name = state.lock().unwrap().drive.clone().unwrap_or_default();
        let confirm = gtk4::MessageDialog::new(
            Some(&btn.root().and_downcast::<gtk4::Window>().unwrap()),
                                               gtk4::DialogFlags::MODAL,
                                               gtk4::MessageType::Warning,
                                               gtk4::ButtonsType::YesNo,
                                               &format!("WARNING: ALL DATA on {} will be DELETED. Proceed?", drive_name)
        );
        let st_conf = st_flash.clone();
        let s_conf = state.clone();
        let tx_conf = sender.clone();
        confirm.connect_response(move |d, res| {
            if res == gtk4::ResponseType::Yes {
                st_conf.set_visible_child_name("progress");
                let s = s_conf.lock().unwrap();
                let drv = s.drive.clone().unwrap();
                let iso = s.iso.clone().unwrap();
                let tx = tx_conf.clone();
                thread::spawn(move || { run_flasher(drv, iso, tx); });
            }
            d.destroy();
        });
        confirm.show();
    });
    btn_box.append(&back_btn);
    btn_box.append(&start_btn);
    box_.append(&label);
    box_.append(&list_box);
    box_.append(&btn_box);
    box_
}

fn build_progress_page(status: gtk4::Label, bar: gtk4::ProgressBar, percent: gtk4::Label, finish: gtk4::Button, cancel: gtk4::Button) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 20);
    box_.set_valign(gtk4::Align::Center);
    box_.set_margin_top(20);
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    row.set_halign(gtk4::Align::Center);
    bar.set_hexpand(true);
    bar.set_width_request(320);
    bar.set_valign(gtk4::Align::Center);
    row.append(&bar);
    row.append(&percent);
    status.set_margin_bottom(8);
    box_.append(&status);
    box_.append(&row);
    cancel.set_halign(gtk4::Align::Center);
    cancel.set_width_request(160);
    box_.append(&cancel);

    finish.set_halign(gtk4::Align::Center);
    finish.set_width_request(120);
    box_.append(&finish);
    box_
}

fn refresh_drives(list: &gtk4::ListBox) {
    while let Some(child) = list.first_child() { list.remove(&child); }
    if let Ok(out) = Command::new("lsblk").args(["-pno", "NAME,SIZE,MODEL,TRAN"]).output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines().filter(|l| l.contains("usb")) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let row = libadwaita::ActionRow::builder()
                .title(parts[0])
                .subtitle(parts[1..].join(" "))
                .activatable(true)
                .build();
                row.add_prefix(&gtk4::Image::from_icon_name("drive-removable-media-symbolic"));
                list.append(&row);
            }
        }
    }
}

fn escalate_privileges() {
    let args: Vec<String> = env::args().collect();
    let appimage = env::var("APPIMAGE").expect("APPIMAGE env var not found");
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    let vars = ["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "XDG_SESSION_TYPE", "APPDIR", "PATH", "LD_LIBRARY_PATH", "APPIMAGE", "XDG_DATA_DIRS"];
    for var in vars {
        if let Ok(val) = env::var(var) { cmd.arg(format!("{}={}", var, val)); }
    }
    if let Ok(home) = env::var("HOME") { cmd.arg(format!("USER_HOME={}", home)); }
    let _ = cmd.arg(&appimage).args(&args[1..]).exec();
}
