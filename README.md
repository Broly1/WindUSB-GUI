# WindUSB-GUI 🚀

**WindUSB-GUI** is a modern, Rust-based graphical tool for creating bootable Windows USB installers on Linux. It is a GTK4/Libadwaita wrapper based on the original [WindUSB CLI bash script](https://github.com/Broly1/Windusb/blob/main/windusb.sh).

This tool handles partitioning, formatting (FAT32), and automatically splitting large `install.wim` files to ensure UEFI compatibility.



## 🛠️ Requirements for Building
If you are building from source on **Arch Linux**, you will need:

1.  **System Tools:**
    ```bash
    sudo pacman -S gptfdisk wimlib rsync dosfstools util-linux rust
    ```
2.  **AppImage Tool:**
    You **must** download the `appimagetool-x86_64.appimage` and place it in the root of this project directory.
    👉 [Download it here](https://github.com/AppImage/appimagetool/releases)

## 🚀 How to Build & Bundle
We have provided a smart build script (`build.sh`) that automates the compilation of the Rust code and the gathering of all necessary library dependencies.

1.  **Clone the repo:**
    ```bash
    git clone https://github.com/YourUsername/WindUSB-GUI.git
    cd WindUSB-GUI
    ```
2.  **Set permissions:**
    ```bash
    chmod +x appimagetool-x86_64.appimage build.sh
    ```
3.  **Run the build:**
    ```bash
    ./build.sh
    ```
    * **First Run:** The script will gather all `.so` libraries and tools (`sgdisk`, `wimlib`) from your system into the `WindUSB.AppDir`.
    * **Subsequent Runs:** It will ask if you want to re-bundle. If you only changed the Rust code, you can skip re-bundling to save time!

## ⚠️ Current Status
* **Tested on:** Arch Linux.
* **Portability:** The AppImage is designed to bundle its own dependencies to run on other distros (Debian, Fedora, openSUSE), but it is currently in active development.

## 📦 Download
Check the [Releases](https://github.com/Broly1/WindUSB-GUI/releases) section for the latest pre-built **WindUSB-x86_64.AppImage**.

## ⚖️ License

[GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).
