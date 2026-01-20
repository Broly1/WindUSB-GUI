# WindUSB-GUI 🚀

**WindUSB-GUI** is a modern, Rust-based graphical tool for creating bootable Windows USB installers on Linux. It is a GTK4/Libadwaita wrapper based on the original [WindUSB CLI bash script](https://github.com/Broly1/Windusb/blob/main/windusb.sh).

This tool handles partitioning, formatting (FAT32), and automatically splitting large `install.wim` files to ensure UEFI compatibility.

## 🛠️ Requirements for Building

The new build system is designed to be highly independent. You only need the basic development headers on your host machine; the script handles the complex system tools by compiling them from source.

### 1. Host Build Dependencies

On **Arch Linux**, ensure you have the base development tools:

```bash
sudo pacman -S base-devel rust git

```

### 2. Automated Build Engine

The `build.sh` script is a "Portable Build Engine" that automatically downloads, compiles, and bundles:

* **Static System Tools:** `wimlib`, `parted`, `sgdisk`, `util-linux`, and `dosfstools`.
* **Packaging Tools:** `appimagetool` and a standalone `7-Zip` binary.
* **Recursive Libraries:** A deep-scan trace of the GTK4/Libadwaita stack to ensure the AppImage runs on any distribution.

## 🚀 How to Build & Bundle

1. **Clone the repo:**
```bash
git clone https://github.com/YourUsername/WindUSB-GUI.git
cd WindUSB-GUI

```


2. **Run the build script:**
```bash
chmod +x build.sh
./build.sh

```



### Build Options

* **Clean Start (`y`):** Re-compiles all C tools from source and performs a **Recursive Library Scan**. Use this for your first build or when moving to a different OS.
* **Rust Only (`n`):** Skips tool compilation and library gathering, only updating the Rust binary. Use this for fast iteration during development.
* **Git Preservation:** The script automatically preserves `.gitkeep` files in `bin-local` and `lib-local` to maintain repository structure.

## 🤝 Credits & Appreciation

WindUSB-GUI is only possible thanks to the incredible work of the open-source community. We rely on and extend our gratitude to the following projects:

| Project | Purpose | Link |
| --- | --- | --- |
| **wimlib** | Handling Windows Imaging files (.wim) | [wimlib.net](https://wimlib.net/) |
| **GNU Parted** | Partition manipulation and partprobe | [gnu.org/s/parted](https://www.gnu.org/software/parted/) |
| **GPT Fdisk** | GPT partitioning (sgdisk) | [rodsbooks.com/gdisk](https://www.rodsbooks.com/gdisk/) |
| **util-linux** | wipefs and block device management | [kernel.org](https://github.com/util-linux/util-linux) |
| **dosfstools** | FAT32 filesystem creation | [github.com/dosfstools](https://github.com/dosfstools/dosfstools) |
| **7-Zip** | ISO verification and extraction | [7-zip.org](https://www.7-zip.org/) |
| **AppImageTool** | Packaging and portability | [appimage.org](https://appimage.org/) |

## ⚠️ Current Status

* **Host OS:** Built and tested on Arch Linux.
* **Portability:** The AppImage uses a **Recursive Dependency Trace** to bundle its own graphics and GUI stack, ensuring compatibility with Pop!_OS, Fedora, Ubuntu, and Debian.

## ⚖️ License

[GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).
