# WindUSB-GUI 🚀

**WindUSB-GUI** is a modern, Rust-based graphical tool for creating bootable Windows USB installers on Linux. It is a GTK4/Libadwaita wrapper based on the original [WindUSB CLI bash script](https://github.com/Broly1/Windusb/blob/main/windusb.sh).

This tool handles partitioning, formatting (FAT32), and automatically splitting large `install.wim` files to ensure UEFI compatibility. It also features built-in ISO verification to ensure you are flashing a valid Windows image.

## 🛠️ Requirements for Building

If you are building from source on **Arch Linux**, you will need:

### 1. System Tools

```bash
sudo pacman -S gptfdisk wimlib rsync dosfstools util-linux rust

```

### 2. Automated Build Dependencies

The build script will automatically handle the downloading and setup of:

* **appimagetool**: Used to package the final AppImage.
* **7z (Static 64-bit)**: Used for ISO content verification to prevent flashing errors.

## 🚀 How to Build & Bundle

The provided `build.sh` script automates the compilation, dependency gathering, and packaging.

1. **Clone the repo:**

```bash
git clone https://github.com/YourUsername/WindUSB-GUI.git
cd WindUSB-GUI

```

2. **Set permissions:**

```bash
chmod +x build.sh

```

3. **Run the build:**

```bash
./build.sh

```

> [!IMPORTANT]
> **First Build:** When prompted to re-bundle system dependencies, **select `y**`. This ensures all necessary `.so` libraries are gathered into the AppImage for the first time.

* **Subsequent Runs:** You can select `n` to skip re-bundling if you only modified the Rust source code; this makes building much faster.
* **Clean Build:** Run `./build.sh --clean` to wipe all cached tools (7z, appimagetool) and libraries to start from scratch.

## ⚠️ Current Status

* **Tested on:** Arch Linux (x86_64).
* **Portability:** The AppImage bundles its own dependencies to ensure it runs on major distributions like Debian, Fedora, and openSUSE.

## 📦 Download

Check the [Releases](https://github.com/Broly1/WindUSB-GUI/releases) section for the latest pre-built **WindUSB-x86_64.AppImage**.

## ⚖️ License

[GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).
