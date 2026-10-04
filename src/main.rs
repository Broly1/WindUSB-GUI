use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::env;
use std::ffi::CString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{self, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

// ───────────────────────── shared ─────────────────────────

static START: OnceLock<Instant> = OnceLock::new();

/// Live trace to stderr: [seconds] [tag] message
fn log(tag: &str, msg: &str) {
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    eprintln!("[{:8.3}] [{:<8}] {}", t, tag, msg);
}

/// Read a pipe and log every line as it arrives. Splits on \n and \r so
/// progress-style output (mkfs, 7z) shows up live too.
fn stream<R: Read + Send + 'static>(r: R, tag: &'static str) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        for b in BufReader::new(r).bytes() {
            match b {
                Ok(b'\n') | Ok(b'\r') => {
                    if !buf.is_empty() {
                        log(tag, &String::from_utf8_lossy(&buf));
                        buf.clear();
                    }
                }
                Ok(c) => buf.push(c),
                Err(_) => break,
            }
        }
        if !buf.is_empty() {
            log(tag, &String::from_utf8_lossy(&buf));
        }
    })
}

/// For commands whose output we need to read (lsblk, 7z l). Also logs it.
fn capture(cmd: &mut Command) -> Result<String, String> {
    log("RUN", &format!("{:?}", cmd));
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to start {:?}: {}", cmd.get_program(), e))?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    for l in s.lines().take(20) {
        log("out", l);
    }
    if s.lines().count() > 20 {
        log("out", "... (truncated)");
    }
    for l in String::from_utf8_lossy(&out.stderr).lines() {
        log("err", l);
    }
    log("exit", &format!("{}", out.status));
    Ok(s)
}

/// Path of a tool bundled in the AppImage (bin-local), or the bare name
/// to be resolved from the host PATH. Bundled tools NEED the bundled libs.
fn get_local_bin(bin_name: &str) -> String {
    if let Ok(appdir) = env::var("APPDIR") {
        let local_path = format!("{}/bin-local/{}", appdir, bin_name);
        if Path::new(&local_path).exists() {
            return local_path;
        }
    }
    bin_name.to_string()
}

/// Command for a HOST binary (sync, cp, du, ...). Strips the AppImage's
/// library overrides so it links against the system libs, not the
/// Ubuntu-built ones in lib-local.
fn host_cmd(name: &str) -> Command {
    let mut c = Command::new(name);
    c.env_remove("LD_LIBRARY_PATH");
    c.env_remove("LD_PRELOAD");
    c
}

fn get_system_dirty_bytes() -> f64 {
    let mut total_kb = 0.0;
    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
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

// ── free space / work directory selection ──

fn free_bytes(path: &str) -> Option<u64> {
    let c = CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// True for RAM-backed filesystems (tmpfs/ramfs), which must not hold the install image.
fn is_ram_backed(path: &str) -> bool {
    let c = match CString::new(path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    const TMPFS_MAGIC: u64 = 0x0102_1994;
    const RAMFS_MAGIC: u64 = 0x8584_58f6;
    let t = (st.f_type as u64) & 0xFFFF_FFFF;
    t == TMPFS_MAGIC || t == RAMFS_MAGIC
}

/// Parse the exact size of `install_file` from `7z l -slt` output.
fn image_size_from_listing(listing: &str, install_file: &str) -> Option<u64> {
    let mut in_target = false;
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("Path = ") {
            in_target = p.replace('\\', "/").to_lowercase() == install_file;
        } else if in_target {
            if let Some(s) = line.strip_prefix("Size = ") {
                return s.trim().parse().ok();
            }
        }
    }
    None
}

/// First disk-backed candidate with enough free space for the extracted image.
/// /var/tmp comes before /tmp because /tmp is RAM-backed (tmpfs) on Fedora and
/// others; RAM-backed directories are skipped automatically.
fn pick_work_dir(needed: u64, iso_parent: Option<&str>) -> Result<String, String> {
    let mut candidates: Vec<String> = vec!["/var/tmp".into(), "/tmp".into()];
    if let Some(p) = iso_parent {
        candidates.push(p.to_string());
    }
    let mut report = Vec::new();
    for dir in &candidates {
        if is_ram_backed(dir) {
            report.push(format!("{}: RAM-backed, skipped", dir));
            continue;
        }
        match free_bytes(dir) {
            Some(free) if free >= needed => {
                log(
                    "helper",
                    &format!(
                        "work dir {} ({} MB free, need {} MB)",
                        dir,
                        free / 1024 / 1024,
                        needed / 1024 / 1024
                    ),
                );
                return Ok(dir.clone());
            }
            Some(free) => report.push(format!("{}: {} MB free", dir, free / 1024 / 1024)),
            None => report.push(format!("{}: unavailable", dir)),
        }
    }
    Err(format!(
        "Not enough temporary disk space: need {} MB ({}). Free up space and try again.",
        needed / 1024 / 1024,
        report.join(", ")
    ))
}

// ── mount / umount via syscalls (no dependency on host `mount` + libmount) ──

fn sys_mount_vfat(dev: &str, target: &str) -> Result<(), String> {
    let src = CString::new(dev).map_err(|e| e.to_string())?;
    let tgt = CString::new(target).map_err(|e| e.to_string())?;
    let fstype = CString::new("vfat").unwrap();
    let data = CString::new("iocharset=utf8").unwrap();

    // First try with utf8 charset, then fall back to kernel defaults.
    let rc = unsafe {
        libc::mount(
            src.as_ptr(),
            tgt.as_ptr(),
            fstype.as_ptr(),
            libc::MS_NOATIME,
            data.as_ptr() as *const libc::c_void,
        )
    };
    if rc == 0 {
        log("helper", &format!("mounted {} on {}", dev, target));
        return Ok(());
    }
    let first_err = io::Error::last_os_error();
    log(
        "helper",
        &format!(
            "mount with iocharset=utf8 failed ({}), retrying without",
            first_err
        ),
    );
    let rc = unsafe {
        libc::mount(
            src.as_ptr(),
            tgt.as_ptr(),
            fstype.as_ptr(),
            libc::MS_NOATIME,
            std::ptr::null(),
        )
    };
    if rc != 0 {
        return Err(format!("mount failed: {}", io::Error::last_os_error()));
    }
    log("helper", &format!("mounted {} on {}", dev, target));
    Ok(())
}

fn sys_umount(target: &str) -> bool {
    match CString::new(target) {
        Ok(t) => unsafe { libc::umount2(t.as_ptr(), 0) == 0 },
        Err(_) => false,
    }
}

fn sys_umount_lazy(target: &str) -> bool {
    match CString::new(target) {
        Ok(t) => unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) == 0 },
        Err(_) => false,
    }
}

fn main() {
    START.get_or_init(Instant::now);
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 && args[1] == "--flash" {
        if args.len() != 4 {
            eprintln!("usage: {} --flash <device> <iso>", args[0]);
            process::exit(2);
        }
        helper_main(&args[2], &args[3]);
    }
    gui_main();
}

// ───────────────────── privileged helper ─────────────────────
// Runs as root via pkexec. Talks to the GUI over stdout (progress)
// and stdin (EOF == cancel). Diagnostics go to stderr.
//
// Protocol lines (stdout):
//   PROGRESS <fraction> <text>   determinate progress
//   PULSE <text>                 indeterminate (bar bounces)
//   DONE
//   ERROR <text>

static CANCELLED: AtomicBool = AtomicBool::new(false);
static CHILD_PID: AtomicI32 = AtomicI32::new(0);
static DIRS: Mutex<Option<(String, String)>> = Mutex::new(None);

fn emit(line: &str) {
    if line.starts_with("ERROR") || line == "DONE" {
        log("result", line);
    }
    let out = io::stdout();
    let mut l = out.lock();
    let _ = writeln!(l, "{}", line.replace('\n', " "));
    let _ = l.flush();
}

fn emit_progress(text: &str, fraction: f64) {
    log("progress", &format!("{:.0}% {}", fraction * 100.0, text));
    emit(&format!(
        "PROGRESS {:.4} {}",
        fraction.clamp(0.0, 1.0),
        text
    ));
}

/// Run a command, streaming its stdout/stderr live to our stderr trace.
/// Tracks the PID so a cancel can kill it. Returns Ok(success).
fn run(cmd: &mut Command) -> Result<bool, String> {
    if CANCELLED.load(Ordering::SeqCst) {
        return Err("Cancelled".into());
    }
    log("RUN", &format!("{:?}", cmd));
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start {:?}: {}", cmd.get_program(), e))?;
    CHILD_PID.store(child.id() as i32, Ordering::SeqCst);

    let h1 = stream(child.stdout.take().unwrap(), "out");
    let h2 = stream(child.stderr.take().unwrap(), "err");

    let status = child.wait();
    let _ = h1.join();
    let _ = h2.join();
    CHILD_PID.store(0, Ordering::SeqCst);
    log("exit", &format!("{:?}", status));

    if CANCELLED.load(Ordering::SeqCst) {
        return Err("Cancelled".into());
    }
    status
        .map(|s| s.success())
        .map_err(|e| format!("Wait failed: {}", e))
}

fn cleanup_dirs() {
    let dirs = DIRS.lock().unwrap().clone();
    if let Some((usb, iso)) = dirs {
        log(
            "cleanup",
            &format!("umount -l {} ; rm {} {}", usb, usb, iso),
        );
        sys_umount_lazy(&usb);
        let _ = fs::remove_dir(&usb); // non-recursive on purpose
        let _ = fs::remove_dir_all(&iso);
    }
}

fn helper_main(drive: &str, iso: &str) -> ! {
    if unsafe { libc::getuid() } != 0 {
        emit("ERROR Helper must run as root");
        process::exit(1);
    }
    log("helper", &format!("started: drive={} iso={}", drive, iso));

    // Cancel watcher: GUI closing our stdin means "stop".
    thread::spawn(|| {
        let mut buf = [0u8; 64];
        let mut stdin = io::stdin();
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        log("helper", "stdin closed -> cancelling");
        CANCELLED.store(true, Ordering::SeqCst);
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            log("helper", &format!("killing child pid {}", pid));
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        // Failsafe in case the main thread is stuck (e.g. in sync).
        thread::sleep(Duration::from_secs(5));
        cleanup_dirs();
        process::exit(130);
    });

    let result = flash(drive, iso);
    cleanup_dirs();
    match result {
        Ok(()) => {
            emit("DONE");
            process::exit(0);
        }
        Err(e) => {
            emit(&format!("ERROR {}", e));
            process::exit(1);
        }
    }
}

fn validate_drive(drive: &str) -> Result<String, String> {
    let canon = fs::canonicalize(drive).map_err(|e| format!("Device not found: {}", e))?;
    let canon_s = canon.to_string_lossy().to_string();
    if !canon_s.starts_with("/dev/") {
        return Err("Target is not under /dev".into());
    }
    let meta = fs::metadata(&canon).map_err(|e| format!("Cannot stat device: {}", e))?;
    if !meta.file_type().is_block_device() {
        return Err("Target is not a block device".into());
    }
    let text = capture(Command::new(get_local_bin("lsblk")).args(["-dnro", "TYPE,TRAN", &canon_s]))
        .map_err(|e| format!("lsblk failed: {}", e))?;
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.first() != Some(&"disk") || parts.get(1) != Some(&"usb") {
        return Err("Refusing to write: target is not a USB disk".into());
    }
    Ok(canon_s)
}

/// Decode the octal escapes (\040 for space, etc.) used in /proc/mounts.
fn unescape_mount_path(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = ((b[i + 1] - b'0') as u32) * 64
                + ((b[i + 2] - b'0') as u32) * 8
                + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Unmount every mounted partition of `drive`. umount2() takes the MOUNT
/// POINT, so we read it from the second field of /proc/mounts.
fn unmount_device(drive: &str) {
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let mut it = line.split_whitespace();
            let dev = it.next().unwrap_or("");
            let mountpoint = it.next().unwrap_or("");
            if let Some(rest) = dev.strip_prefix(drive) {
                if rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit() || c == 'p') {
                    let mp = unescape_mount_path(mountpoint);
                    log("helper", &format!("unmounting {} from {}", dev, mp));
                    if !sys_umount_lazy(&mp) {
                        log(
                            "helper",
                            &format!("umount {} failed: {}", mp, io::Error::last_os_error()),
                        );
                    }
                }
            }
        }
    }
}

fn flash(drive_arg: &str, iso_arg: &str) -> Result<(), String> {
    let drive = validate_drive(drive_arg)?;
    let iso = fs::canonicalize(iso_arg).map_err(|e| format!("ISO not found: {}", e))?;
    if !iso.is_file() {
        return Err("ISO path is not a regular file".into());
    }
    let iso_s = iso.to_string_lossy().to_string();
    let z_bin = get_local_bin("7z");

    // Find install image inside the ISO (-slt gives exact sizes).
    let listing = capture(Command::new(&z_bin).args(["l", "-slt", &iso_s]))
        .map_err(|e| format!("7z failed: {}", e))?;
    let stdout = listing.to_lowercase();
    let (install_file, is_wim) = if stdout.contains("sources/install.wim") {
        ("sources/install.wim", true)
    } else if stdout.contains("sources/install.esd") {
        ("sources/install.esd", false)
    } else {
        return Err("Invalid ISO: install.wim/esd not found".into());
    };
    log(
        "helper",
        &format!("found {} (wim={})", install_file, is_wim),
    );
    let install_name = install_file.rsplit('/').next().unwrap().to_string();

    // Exact size if we can parse it, otherwise the ISO size (always >= the image).
    let image_size = image_size_from_listing(&listing, install_file)
        .or_else(|| fs::metadata(&iso).ok().map(|m| m.len()))
        .unwrap_or(0);
    log(
        "helper",
        &format!("install image size (listing): {} bytes", image_size),
    );
    let needed = image_size + image_size / 50; // ~2% filesystem slack
    let iso_parent = iso.parent().map(|p| p.to_string_lossy().to_string());
    let work_base = pick_work_dir(needed, iso_parent.as_deref())?;

    let pid = process::id();
    let usb_mt = format!("/tmp/windusb_usb_{}", pid); // tiny mount point, RAM-backed is fine
    let iso_mt = format!("{}/windusb_iso_{}", work_base, pid); // big temp file, disk-backed
    fs::create_dir(&usb_mt).map_err(|e| format!("Cannot create {}: {}", usb_mt, e))?;
    fs::create_dir(&iso_mt).map_err(|e| format!("Cannot create {}: {}", iso_mt, e))?;
    log("helper", &format!("temp dirs: {} {}", usb_mt, iso_mt));
    *DIRS.lock().unwrap() = Some((usb_mt.clone(), iso_mt.clone()));

    // ── format ──
    emit_progress(&format!("Formatting drive {}...", drive), 0.02);
    unmount_device(&drive);
    if !device_exists(&drive) {
        return Err("Drive disconnected before formatting".into());
    }
    let _ = run(Command::new(get_local_bin("blockdev")).args(["--flushbufs", &drive]));
    let _ = run(Command::new(get_local_bin("wipefs")).args(["-af", &drive]));
    let _ = run(Command::new(get_local_bin("sgdisk")).args(["-Z", &drive]));
    if !run(Command::new(get_local_bin("sgdisk")).args(["-n=1:0:0", "-t=1:0700", &drive]))? {
        return Err("Partitioning failed (sgdisk). Drive may have been removed.".into());
    }
    let _ = run(Command::new(get_local_bin("partprobe")).arg(&drive));

    let part = if drive.chars().last().map_or(false, |c| c.is_ascii_digit()) {
        format!("{}p1", drive)
    } else {
        format!("{}1", drive)
    };
    log("helper", &format!("waiting for partition {}", part));
    for _ in 0..20 {
        if Path::new(&part).exists() {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    thread::sleep(Duration::from_secs(1));
    let part_exists = Path::new(&part).exists();
    log("helper", &format!("partition exists: {}", part_exists));
    if !part_exists {
        return Err("Partition did not appear after partitioning.".into());
    }

    if !run(Command::new(get_local_bin("mkfs.fat")).args(["-F32", "-I", "-n", "WINDUSB", &part]))? {
        return Err("Formatting failed. Drive may have been removed.".into());
    }
    if let Err(e) = sys_mount_vfat(&part, &usb_mt) {
        return Err(format!("Failed to mount USB drive: {}", e));
    }

    // ── extract install image to temp ──
    emit_progress("Extracting install image...", 0.04);
    let ok = run(Command::new(&z_bin).args([
        "e",
        "-bd",
        &iso_s,
        &format!("-o{}", iso_mt),
        install_file,
        "-y",
    ]))?;
    if !ok || !device_exists(&drive) {
        return Err("Failed to extract install.wim/esd from ISO".into());
    }
    let install_full_path = format!("{}/{}", iso_mt, install_name);
    let wim_size = fs::metadata(&install_full_path)
        .map(|m| m.len() as f64)
        .unwrap_or(image_size as f64);
    log(
        "helper",
        &format!("install image size: {:.0} bytes", wim_size),
    );

    if !is_wim && wim_size >= 4_294_967_295.0 {
        return Err("install.esd is larger than 4 GB and cannot be stored on FAT32".into());
    }

    // ── progress monitor ──
    let active = Arc::new(AtomicBool::new(true));
    let phase = Arc::new(AtomicU8::new(1));
    {
        let active = active.clone();
        let phase = phase.clone();
        let usb_mt = usb_mt.clone();
        let drive = drive.clone();
        thread::spawn(move || {
            let total_mb = wim_size / 1024.0 / 1024.0;
            let mut baseline = 0.0;
            while active.load(Ordering::SeqCst) {
                if !device_exists(&drive) {
                    break;
                }
                let out = host_cmd("du").args(["-sb", &usb_mt]).output();
                if let Ok(out) = out {
                    let s = String::from_utf8_lossy(&out.stdout);
                    if let Some(Ok(cur)) = s.split_whitespace().next().map(|x| x.parse::<f64>()) {
                        if phase.load(Ordering::SeqCst) == 1 {
                            let dirty = get_system_dirty_bytes();
                            let actual = (cur - dirty).max(0.0);
                            let p = 0.05 + ((actual / 500_000_000.0).min(1.0) * 0.20);
                            emit_progress("Extracting boot files...", p);
                            baseline = cur;
                        } else {
                            let done = (cur - baseline).max(0.0);
                            let p = 0.25 + ((done / wim_size).min(1.0) * 0.55);
                            emit_progress(
                                &format!(
                                    "Writing install image: {:.0} / {:.0} MB",
                                    done / 1024.0 / 1024.0,
                                    total_mb
                                ),
                                p,
                            );
                        }
                    }
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
    }

    // ── extract everything except the install image onto the USB ──
    let ok = run(Command::new(&z_bin).args([
        "x",
        "-bd",
        &iso_s,
        &format!("-o{}", usb_mt),
        &format!("-xr!{}", install_name),
        "-y",
    ]));
    if !matches!(ok, Ok(true)) || !device_exists(&drive) {
        active.store(false, Ordering::SeqCst);
        return Err(match ok {
            Err(e) => e,
            _ => "Drive removed or 7z error during extraction.".into(),
        });
    }
    phase.store(2, Ordering::SeqCst);

    // ── split (wim) or copy (esd) ──
    let ok = if is_wim {
        let dst = format!("{}/sources/install.swm", usb_mt);
        run(Command::new(get_local_bin("wimlib-imagex")).args([
            "split",
            &install_full_path,
            &dst,
            "3400",
        ]))
    } else {
        let dst = format!("{}/sources/install.esd", usb_mt);
        run(host_cmd("cp").args([&install_full_path, &dst]))
    };
    active.store(false, Ordering::SeqCst);
    if !matches!(ok, Ok(true)) || !device_exists(&drive) {
        return Err(match ok {
            Err(e) => e,
            _ => "Drive removed or error while writing install image.".into(),
        });
    }
    let _ = fs::remove_file(&install_full_path);

    // ── flush + unmount (both can block, so run them off-thread) ──
    let initial_dirty = get_system_dirty_bytes().max(1.0);
    log(
        "helper",
        &format!(
            "dirty before sync: {:.1} MB",
            initial_dirty / 1024.0 / 1024.0
        ),
    );
    let finish_done = Arc::new(AtomicBool::new(false));
    {
        let finish_done = finish_done.clone();
        let usb_mt = usb_mt.clone();
        thread::spawn(move || {
            let _ = run(&mut host_cmd("sync"));
            if !sys_umount(&usb_mt) {
                log(
                    "helper",
                    &format!(
                        "umount failed ({}), trying lazy",
                        io::Error::last_os_error()
                    ),
                );
                sys_umount_lazy(&usb_mt);
            }
            finish_done.store(true, Ordering::SeqCst);
        });
    }
    while !finish_done.load(Ordering::SeqCst) {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err("Cancelled".into());
        }
        if !device_exists(&drive) {
            return Err("Drive disconnected during final sync.".into());
        }
        let dirty = get_system_dirty_bytes();
        if dirty > 10.0 * 1024.0 * 1024.0 {
            let p = 0.80 + ((1.0 - (dirty / initial_dirty)) * 0.19);
            emit_progress(
                &format!("Flushing cache: {:.1} MB left", dirty / 1024.0 / 1024.0),
                p.min(0.99),
            );
        } else {
            emit("PULSE Finishing writes, please don't unplug...");
        }
        thread::sleep(Duration::from_millis(200));
    }
    if !device_exists(&drive) {
        return Err("Drive was unplugged during final sync.".into());
    }
    Ok(())
}

// ─────────────────────────── GUI ───────────────────────────
// Runs as the normal user.

struct AppState {
    drive: Option<String>,
    iso: Option<PathBuf>,
}

enum ProgressMsg {
    Update(String, f64),
    Pulse(String),
    Finished,
    Error(String),
}

// Keeping this open keeps the helper alive; dropping it cancels the flash.
static HELPER_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);

fn close_helper_pipe() {
    if let Ok(mut g) = HELPER_STDIN.lock() {
        if g.take().is_some() {
            log("gui", "closed helper stdin (cancel signal)");
        }
    }
}

fn cleanup_processes() {
    log("gui", "exiting");
    close_helper_pipe();
    // Give the helper a moment to see EOF before we vanish.
    thread::sleep(Duration::from_millis(100));
    process::exit(0);
}

fn gui_main() {
    log("gui", "starting");
    ctrlc::set_handler(move || {
        cleanup_processes();
    })
    .expect("Error setting Ctrl-C handler");

    let app = libadwaita::Application::builder()
        .application_id("io.github.windusb")
        .build();
    app.connect_activate(build_ui);
    app.run();
}

fn is_valid_windows_iso(path: &Path) -> bool {
    let z_bin = get_local_bin("7z");
    match capture(Command::new(z_bin).args(["l", &path.to_string_lossy()])) {
        Ok(s) => {
            let l = s.to_lowercase();
            l.contains("sources/install.wim") || l.contains("sources/install.esd")
        }
        Err(_) => false,
    }
}

fn spawn_helper(drive: String, iso: PathBuf, tx: mpsc::Sender<ProgressMsg>) {
    let exe = env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| env::current_exe().ok());
    let exe = match exe {
        Some(e) => e,
        None => {
            let _ = tx.send(ProgressMsg::Error("Cannot locate application path".into()));
            return;
        }
    };

    log(
        "gui",
        &format!(
            "pkexec {} --flash {} {}",
            exe.display(),
            drive,
            iso.display()
        ),
    );
    let mut child = match Command::new("pkexec")
        .arg(&exe)
        .arg("--flash")
        .arg(&drive)
        .arg(&iso)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(ProgressMsg::Error(format!("Cannot start pkexec: {}", e)));
            return;
        }
    };

    if let Some(stdin) = child.stdin.take() {
        *HELPER_STDIN.lock().unwrap() = Some(stdin);
    }
    let stdout = child.stdout.take();

    thread::spawn(move || {
        let mut finished = false;
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                // PULSE lines arrive every 200 ms; don't flood the trace with them.
                if !line.starts_with("PULSE ") {
                    log("gui<-", &line);
                }
                if let Some(rest) = line.strip_prefix("PROGRESS ") {
                    let mut it = rest.splitn(2, ' ');
                    let frac = it.next().and_then(|f| f.parse::<f64>().ok()).unwrap_or(0.0);
                    let text = it.next().unwrap_or("").to_string();
                    let _ = tx.send(ProgressMsg::Update(text, frac));
                } else if let Some(text) = line.strip_prefix("PULSE ") {
                    let _ = tx.send(ProgressMsg::Pulse(text.to_string()));
                } else if line == "DONE" {
                    finished = true;
                    let _ = tx.send(ProgressMsg::Finished);
                } else if let Some(e) = line.strip_prefix("ERROR ") {
                    finished = true;
                    let _ = tx.send(ProgressMsg::Error(e.to_string()));
                }
            }
        }
        let status = child.wait();
        log("gui", &format!("helper exited: {:?}", status));
        close_helper_pipe();
        if !finished {
            let msg = match status.ok().and_then(|s| s.code()) {
                Some(126) | Some(127) => "Authentication was cancelled or failed.",
                _ => "The flashing helper exited unexpectedly.",
            };
            let _ = tx.send(ProgressMsg::Error(msg.into()));
        }
    });
}

fn build_ui(app: &libadwaita::Application) {
    // Icon setup: let GTK find the bundled hicolor icon inside the AppImage.
    if let Some(display) = gtk4::gdk::Display::default() {
        if let Ok(appdir) = env::var("APPDIR") {
            gtk4::IconTheme::for_display(&display)
                .add_search_path(format!("{}/usr/share/icons", appdir));
        }
    }
    gtk4::Window::set_default_icon_name("io.github.windusb");

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(b"
    button.pill-btn { border-radius: 99px; padding-left: 24px; padding-right: 24px; min-height: 38px; }
    .invalid-iso { background-color: alpha(@error_color, 0.15); border: 1px solid @error_color; border-radius: 12px; }
    .invalid-iso label { color: @error_color; }
    .title-4 { margin-bottom: 8px; }

    progressbar progress {
        background-color: @accent_bg_color;
        background-image: none;
        min-height: 12px;
        border-radius: 99px;
    }

    progressbar trough {
        background-color: alpha(@accent_bg_color, 0.15);
        min-height: 12px;
        border-radius: 99px;
    }
    ");
    gtk4::StyleContext::add_provider_for_display(
        &gtk4::gdk::Display::default().expect("Could not connect to a display."),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    // No forced color scheme: libadwaita follows the system light/dark setting.

    let state = Arc::new(Mutex::new(AppState {
        drive: None,
        iso: None,
    }));
    let window = libadwaita::ApplicationWindow::builder()
        .application(app)
        .title("WindUSB Creator")
        .default_width(550)
        .default_height(380)
        .resizable(false)
        .build();
    window.connect_close_request(|_| {
        cleanup_processes();
        gtk4::Inhibit(false)
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
    progress_bar.set_pulse_step(0.15);
    let percent_label = gtk4::Label::builder()
        .label("0%")
        .width_chars(5)
        .valign(gtk4::Align::Center)
        .build();
    percent_label.add_css_class("caption");
    let finish_btn = gtk4::Button::with_label("Finish & Exit");
    finish_btn.add_css_class("suggested-action");
    finish_btn.add_css_class("pill-btn");
    finish_btn.set_visible(false);
    finish_btn.connect_clicked(|_| {
        cleanup_processes();
    });
    let cancel_btn = gtk4::Button::with_label("Cancel");
    cancel_btn.add_css_class("destructive-action");
    cancel_btn.add_css_class("pill-btn");
    cancel_btn.connect_clicked(|_| {
        cleanup_processes();
    });
    let (sender, receiver) = mpsc::channel::<ProgressMsg>();
    let st_c = status_label.clone();
    let pb_c = progress_bar.clone();
    let fb_c = finish_btn.clone();
    let cb_c = cancel_btn.clone();
    let pl_c = percent_label.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        while let Ok(msg) = receiver.try_recv() {
            match msg {
                ProgressMsg::Update(text, fraction) => {
                    st_c.set_text(&text);
                    pb_c.set_fraction(fraction);
                    let p = (fraction * 100.0).floor() as u32;
                    pl_c.set_text(&format!("{}%", p));
                }
                ProgressMsg::Pulse(text) => {
                    st_c.set_text(&text);
                    pb_c.pulse();
                    pl_c.set_text("");
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
        glib::Continue(true)
    });
    let drive_page = build_drive_page(&stack, state.clone());
    let iso_page = build_iso_page(&stack, state.clone(), sender);
    let prog_page = build_progress_page(
        status_label,
        progress_bar,
        percent_label,
        finish_btn,
        cancel_btn,
    );
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
    next_btn.add_css_class("pill-btn");
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
            let title = row_action.title().to_string();
            log("gui", &format!("selected drive {}", title));
            s_c.lock().unwrap().drive = Some(title);
            nb_c.set_sensitive(true);
        }
    });
    let st_c = stack.clone();
    next_btn.connect_clicked(move |_| {
        st_c.set_visible_child_name("iso");
    });
    box_
}

fn build_iso_page(
    stack: &gtk4::Stack,
    state: Arc<Mutex<AppState>>,
    sender: mpsc::Sender<ProgressMsg>,
) -> gtk4::Box {
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
    back_btn.add_css_class("pill-btn");
    let start_btn = gtk4::Button::with_label("Flash USB");
    start_btn.add_css_class("destructive-action");
    start_btn.add_css_class("pill-btn");
    start_btn.set_sensitive(false);
    let st_c = stack.clone();
    back_btn.connect_clicked(move |_| {
        st_c.set_visible_child_name("drive");
    });
    let s_c = state.clone();
    let b_c = start_btn.clone();
    let r_c = iso_row.clone();
    iso_row.connect_activated(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Select Windows ISO"),
            Some(&r_c.root().and_downcast::<gtk4::Window>().unwrap()),
            gtk4::FileChooserAction::Open,
            &[
                ("_Cancel", gtk4::ResponseType::Cancel),
                ("_Open", gtk4::ResponseType::Ok),
            ],
        );

        let home = env::var("HOME").unwrap_or_else(|_| "/home".to_string());
        let downloads = format!("{}/Downloads", home);
        if Path::new(&downloads).exists() {
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
                    log("gui", &format!("selected ISO {}", path.display()));
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
            &format!(
                "WARNING: ALL DATA on {} will be DELETED. Proceed?",
                drive_name
            ),
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
                thread::spawn(move || {
                    spawn_helper(drv, iso, tx);
                });
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

fn build_progress_page(
    status: gtk4::Label,
    bar: gtk4::ProgressBar,
    percent: gtk4::Label,
    finish: gtk4::Button,
    cancel: gtk4::Button,
) -> gtk4::Box {
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
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let lsblk_bin = get_local_bin("lsblk");
    // -d: whole disks only, -p: full /dev paths
    if let Ok(stdout) = capture(Command::new(lsblk_bin).args(["-dpno", "NAME,SIZE,MODEL,TRAN"])) {
        for line in stdout.lines().filter(|l| l.contains("usb")) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let row = libadwaita::ActionRow::builder()
                    .title(parts[0])
                    .subtitle(&parts[1..].join(" "))
                    .activatable(true)
                    .build();
                row.add_prefix(&gtk4::Image::from_icon_name(
                    "drive-removable-media-symbolic",
                ));
                list.append(&row);
            }
        }
    }
}
