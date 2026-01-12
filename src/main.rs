use libadwaita::prelude::*;
use libadwaita::gio;
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
    // Kill the specific worker tools
    let _ = Command::new("pkill").args(["-9", "rsync"]).status();
    let _ = Command::new("pkill").args(["-9", "wimlib-imagex"]).status();

    // Only unmount the specific WindUSB temporary mounts, not the whole system
    let _ = Command::new("sh")
    .args(["-c", "umount -l /tmp/windusb_* 2>/dev/null"])
    .status();

    unsafe {
        let pid = libc::getpid();
        // Kill the process group to ensure all children die immediately
        libc::kill(-pid, libc::SIGKILL);
    }
}

fn main() {
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

fn build_ui(app: &libadwaita::Application) {
    if unsafe { libc::getuid() } != 0 {
        escalate_privileges();
    }

    let style_manager = libadwaita::StyleManager::default();
    style_manager.set_color_scheme(libadwaita::ColorScheme::PreferDark);

    let state = Arc::new(Mutex::new(AppState { drive: None, iso: None }));

    let window = libadwaita::ApplicationWindow::builder()
    .application(app)
    .title("WindUSB Creator")
    .default_width(500)
    .default_height(250)
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
    content_box.set_margin_start(20);
    content_box.set_margin_end(20);
    content_box.set_margin_top(20);
    content_box.set_margin_bottom(20);

    let stack = gtk4::Stack::new();
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeftRight);

    let status_label = gtk4::Label::builder()
    .label("Preparing to format...")
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
    finish_btn.connect_clicked(|_| { std::process::exit(0); });

    let (sender, receiver) = mpsc::channel::<ProgressMsg>();

    let st_c = status_label.clone();
    let pb_c = progress_bar.clone();
    let fb_c = finish_btn.clone();
    let pl_c = percent_label.clone();

    let is_finished = Arc::new(Mutex::new(false));
    let last_progress = Arc::new(Mutex::new(0.0));
    let is_fin_c = is_finished.clone();
    let lp_c = last_progress.clone();

    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        while let Ok(msg) = receiver.try_recv() {
            if *is_fin_c.lock().unwrap() { break; }
            match msg {
                ProgressMsg::Update(text, fraction) => {
                    let mut lp = lp_c.lock().unwrap();
                    if fraction >= *lp || fraction == 0.99 || fraction == 0.01 {
                        st_c.set_text(&text);
                        pb_c.set_fraction(fraction);
                        let p = (fraction * 100.0).floor() as u32;
                        pl_c.set_text(&format!("{}%", p));
                        *lp = fraction;
                    }
                }
                ProgressMsg::Finished => {
                    *is_fin_c.lock().unwrap() = true;
                    st_c.set_text("Installation Finished! You can now safely unplug the drive.");
                    pb_c.set_visible(false);
                    pl_c.set_visible(false);
                    fb_c.set_visible(true);
                }
                ProgressMsg::Error(err) => {
                    st_c.set_text(&format!("Error: {}", err));
                    pb_c.add_css_class("error");
                    pl_c.set_visible(false);
                    fb_c.set_label("Close");
                    fb_c.set_visible(true);
                }
            }
        }
        glib::ControlFlow::Continue
    });

    let drive_page = build_drive_page(&stack, state.clone());
    let iso_page = build_iso_page(&stack, state.clone(), sender);
    let prog_page = build_progress_page(status_label, progress_bar, percent_label, finish_btn);

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
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    let header_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    let label = gtk4::Label::new(Some("Select USB Drive"));
    label.add_css_class("title-4");
    label.set_hexpand(true);
    label.set_halign(gtk4::Align::Start);
    let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
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
        if Path::new(&downloads).exists() {
            let _ = dialog.set_current_folder(Some(&gio::File::for_path(downloads)));
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
                        r_i.set_title("Selected (Valid)");
                        r_i.set_subtitle(&path.file_name().unwrap().to_string_lossy());
                        s_i.lock().unwrap().iso = Some(path);
                        b_i.set_sensitive(true);
                    } else {
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

fn build_progress_page(status: gtk4::Label, bar: gtk4::ProgressBar, percent: gtk4::Label, finish: gtk4::Button) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    box_.set_valign(gtk4::Align::Center);
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    row.set_halign(gtk4::Align::Center);
    bar.set_hexpand(true);
    bar.set_width_request(300);
    bar.set_valign(gtk4::Align::Center);
    row.append(&bar);
    row.append(&percent);
    box_.append(&status);
    box_.append(&row);
    box_.append(&finish);
    box_
}

fn run_flasher(drive: String, iso: PathBuf, tx: mpsc::Sender<ProgressMsg>) {
    let iso_size = std::fs::metadata(&iso).unwrap().len() as f64;
    // Use fixed identifiable paths for easier cleanup on exit
    let usb_mt = format!("/tmp/windusb_usb_{}", unsafe { libc::rand() });
    let iso_mt = format!("/tmp/windusb_iso_{}", unsafe { libc::rand() });
    let _ = Command::new("mkdir").args(["-p", &usb_mt, &iso_mt]).status();

    let copy_text = "Copying Windows files... \nThis might take a while, please hang tight.".to_string();

    let _ = Command::new("sh").args(["-c", &format!("umount -l {}* 2>/dev/null", drive)]).status();

    let _ = Command::new("blockdev").args(["--flushbufs", &drive]).status();
    let _ = Command::new("wipefs").args(["-af", &drive]).status();
    let _ = Command::new("sgdisk").args(["-Z", &drive]).status();
    let _ = Command::new("sgdisk").args(["-n=1:0:0", "-t=1:0700", &drive]).status();
    let _ = Command::new("partprobe").arg(&drive).status();
    thread::sleep(std::time::Duration::from_secs(2));

    let part = if drive.contains("nvme") { format!("{}p1", drive) } else { format!("{}1", drive) };

    let _ = tx.send(ProgressMsg::Update("Formatting partitions...".into(), 0.1));
    if !Command::new("mkfs.fat").args(["-F32", "-I", &part]).status().unwrap().success() {
        let _ = tx.send(ProgressMsg::Error("Formatting failed. The drive might be busy.".into()));
        return;
    }

    let m1 = Command::new("mount").args([&part, &usb_mt]).status();
    let m2 = Command::new("mount").args(["-o", "loop,ro", &iso.to_string_lossy(), &iso_mt]).status();

    if m1.is_ok() && m2.is_ok() {
        let tx_monitor = tx.clone();
        let usb_mt_mon = usb_mt.clone();
        let active = Arc::new(Mutex::new(true));
        let active_c = active.clone();
        let status_c = copy_text.clone();

        thread::spawn(move || {
            while *active_c.lock().unwrap() && Path::new(&usb_mt_mon).exists() {
                let output = Command::new("du").args(["-sb", &usb_mt_mon]).output();
                if let Ok(out) = output {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if let Some(size_str) = stdout.split_whitespace().next() {
                        if let Ok(current_bytes) = size_str.parse::<f64>() {
                            let fraction = (current_bytes / iso_size).min(0.98);
                            let _ = tx_monitor.send(ProgressMsg::Update(status_c.clone(), fraction));
                        }
                    }
                }
                thread::sleep(std::time::Duration::from_millis(1000));
            }
        });

        let _ = Command::new("mkdir").args(["-p", &format!("{}/sources", usb_mt)]).status();
        let _ = Command::new("rsync")
        .args(["-rltD", "--exclude=sources/install.wim", "--exclude=sources/install.esd", &format!("{}/", iso_mt), &format!("{}/", usb_mt)])
        .status();

        let wim_src = format!("{}/sources/install.wim", iso_mt);
        let swm_dst = format!("{}/sources/install.swm", usb_mt);
        let _ = Command::new("wimlib-imagex")
        .args(["split", &wim_src, &swm_dst, "3800"])
        .status();

        *active.lock().unwrap() = false;
        let _ = tx.send(ProgressMsg::Update("Syncing Windows files... \nThis might take a while, please hang tight.".into(), 0.99));
        let _ = Command::new("sync").status();

        let _ = Command::new("umount").arg("-l").arg(&usb_mt).status();
        let _ = Command::new("umount").arg("-l").arg(&iso_mt).status();
        let _ = tx.send(ProgressMsg::Finished);
    } else {
        let _ = tx.send(ProgressMsg::Error("Mounting failed".into()));
    }
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
