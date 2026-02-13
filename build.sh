#!/bin/bash
set -e
clear

APP_DIR="WindUSB.AppDir"
BIN_DIR="$(pwd)/$APP_DIR/bin-local"
LIB_DIR="$(pwd)/$APP_DIR/lib-local"
BUILD_ROOT="$(pwd)/build_temp"
APPIMAGE_TOOL="./appimagetool-x86_64.appimage"

URL_APPIMAGETOOL=$(curl -s https://api.github.com/repos/AppImage/appimagetool/releases/latest | grep "browser_download_url.*x86_64.AppImage\"" | cut -d '"' -f 4 | head -n 1)
LATEST_7Z_VER=$(curl -s https://www.7-zip.org/download.html | grep -oP '7z\d{4}-linux-x64.tar.xz' | head -n 1 | grep -oP '\d{4}')
URL_7Z="https://www.7-zip.org/a/7z${LATEST_7Z_VER}-linux-x64.tar.xz"
URL_WIMLIB="https://wimlib.net/downloads/wimlib-1.14.5.tar.gz"
URL_DOSFSTOOLS="https://github.com/dosfstools/dosfstools/releases/download/v4.2/dosfstools-4.2.tar.gz"
URL_UTIL_LINUX="https://mirrors.edge.kernel.org/pub/linux/utils/util-linux/v2.41/util-linux-2.41.3.tar.gz"
URL_POPT="https://ftp.osuosl.org/pub/blfs/conglomeration/popt/popt-1.19.tar.gz"
URL_GPTFDISK="https://downloads.sourceforge.net/project/gptfdisk/gptfdisk/1.0.10/gptfdisk-1.0.10.tar.gz"
URL_PARTED="https://ftp.gnu.org/gnu/parted/parted-3.6.tar.xz"

export CC="gcc"
export CXX="g++"

cleanup() {
    if [ -d "$BUILD_ROOT" ]; then
        echo "🧹 Auto-cleaning temporary build files..."
        rm -rf "$BUILD_ROOT"
    fi
}
trap cleanup EXIT INT TERM

echo "-------------------------------------------------------"
echo "🚀 WindUSB-GUI Automated Build Script"
echo "-------------------------------------------------------"

while true; do
    read -p "❓ Perform clean start? (Builds all latest binaries) [y/n]: " yn
    case $yn in
        [Yy]* ) CLEAN_START=true; break;;
        [Nn]* ) CLEAN_START=false; break;;
        * ) echo "Please answer y or n.";;
    esac
done

if [ "$CLEAN_START" = true ]; then
    echo "🧹 Performing Deep Build (Full Clean)..."
    mkdir -p "$BIN_DIR" "$LIB_DIR" "$BUILD_ROOT"
    find "$BIN_DIR" -mindepth 1 ! -name ".gitkeep" -delete 2>/dev/null || true
    find "$LIB_DIR" -mindepth 1 ! -name ".gitkeep" -delete 2>/dev/null || true
    touch "$BIN_DIR/.gitkeep"
    touch "$LIB_DIR/.gitkeep"
    rm -rf "$APPIMAGE_TOOL" "$BUILD_ROOT"
    mkdir -p "$BUILD_ROOT"

    echo "📥 Downloading tools..."
    curl -Lo "$APPIMAGE_TOOL" "$URL_APPIMAGETOOL"
    chmod +x "$APPIMAGE_TOOL"

    curl -Lo "7z-linux.tar.xz" "$URL_7Z"
    tar -xJf "7z-linux.tar.xz" 7zzs || true
    [ -f 7zzs ] && mv 7zzs "$BIN_DIR/7z"
    rm -f "7z-linux.tar.xz"

    ROOT_DIR=$(pwd)
    cd "$BUILD_ROOT"

    echo "📦 Building wimlib..."
    wget -qN "$URL_WIMLIB"
    tar -xf wimlib-1.14.5.tar.gz && cd wimlib-1.14.5
    ./configure --enable-static --disable-shared --without-ntfs-3g --without-fuse
    make -j$(nproc) -k || true
    gcc -static -no-pie -O2 $(find programs -name "*imagex.o") $(find programs -name "*common_utils.o") \
        -I. -I./include .libs/libwim.a -lpthread -o "$BIN_DIR/wimlib-imagex"
    cd ..

    echo "📦 Building dosfstools..."
    wget -qN "$URL_DOSFSTOOLS"
    tar -xf dosfstools-4.2.tar.gz && cd dosfstools-4.2
    ./configure --enable-compat-symlinks
    make -j$(nproc)
    cp src/mkfs.fat "$BIN_DIR/" && cd ..

    echo "📦 Building util-linux..."
    wget -qN "$URL_UTIL_LINUX"
    tar -xf util-linux-2.41.3.tar.gz && cd util-linux-2.41.3
    ./configure --disable-all-programs --enable-wipefs --enable-lsblk --enable-blockdev \
                --enable-libuuid --enable-libblkid --enable-libsmartcols --enable-libmount \
                --disable-bash-completion --disable-nls --without-python --without-systemd --without-udev
    make -j$(nproc)
    find misc-utils -name wipefs -type f -executable -exec cp {} "$BIN_DIR/" \;
    find misc-utils -name lsblk -type f -executable -exec cp {} "$BIN_DIR/" \;
    find sys-utils -name blockdev -type f -executable -exec cp {} "$BIN_DIR/" \;
    LOCAL_UUID_DIR=$(pwd)
    cd ..

    echo "📦 Building sgdisk..."
    wget -qN "$URL_POPT"
    tar -xf popt-1.19.tar.gz && cd popt-1.19
    ./configure --enable-static --disable-shared
    make -j$(nproc)
    POPT_LIB=$(find $(pwd) -name libpopt.a | head -n 1)
    POPT_INC=$(pwd)
    cd ..
    wget -qN "$URL_GPTFDISK"
    tar -xf gptfdisk-1.0.10.tar.gz && cd gptfdisk-1.0.10
    SOURCES=$(ls *.cc | grep -vE '^(gdisk|cgdisk|fixparts|diskio-windows|gptcurses)\.cc$')
    g++ -o "$BIN_DIR/sgdisk" $SOURCES -I"$POPT_INC" -I"$POPT_INC/src" -I"$LOCAL_UUID_DIR/libuuid/src" \
        "$POPT_LIB" "$LOCAL_UUID_DIR/.libs/libuuid.a" -static -static-libgcc -static-libstdc++ -lpthread -no-pie
    cd ..

    echo "📦 Building partprobe..."
    wget -qN "$URL_PARTED"
    tar -xf parted-3.6.tar.xz && cd parted-3.6
    sed -i 's/do_version ()/do_version (PedDevice** dev, PedDisk** diskp)/g' parted/parted.c
    ./configure --enable-static --disable-shared --without-readline --disable-device-mapper --disable-nls \
                UUID_LIBS="-L$LOCAL_UUID_DIR/.libs -luuid" \
                UUID_CFLAGS="-I$LOCAL_UUID_DIR/libuuid/src"
    make -j$(nproc)
    find parted -name partprobe -type f -executable -exec cp {} "$BIN_DIR/" \;
    cd ..

    cd "$ROOT_DIR"
    chmod 755 "$BIN_DIR"/* || true
    for f in "$BIN_DIR"/*; do
        if file "$f" | grep -q "ELF"; then
            strip "$f"
        fi
    done
else
    echo "⏭️  Fast Build: Skipping tools and library scan..."
fi

echo "🦀 Compiling Rust source..."
touch src/main.rs
cargo build --release

TARGET_BINARY=$(find target/release -maxdepth 1 -type f -executable ! -name "*.so" ! -name "*.dylib" | head -n 1)
cp "$TARGET_BINARY" "$BIN_DIR/windusb-gui"
strip "$BIN_DIR/windusb-gui"

if [ "$CLEAN_START" = true ]; then
    echo "📚 Gathering libraries recursively for maximum portability..."
    EXCLUDE_LIST="libc.so|libpthread.so|libdl.so|libm.so|librt.so|libgcc_s.so|libstdc++.so|libresolv.so|libcrypt.so|libutil.so|libnsl.so|libGL|libnvidia|libdrm|libX11|libxcb|libasound|libpulse|ld-linux"
    TEMP_LIBS="all_libs.txt"
    > "$TEMP_LIBS"

    get_deps() { 
        ldd "$1" 2>/dev/null | grep "=> /" | awk '{print $3}'; 
    }

    echo -n "🔍 Analyzing dependencies: "
    for f in "$BIN_DIR"/*; do
        if file "$f" | grep -q "ELF" && ldd "$f" 2>&1 | grep -qv "not a dynamic executable"; then
            get_deps "$f" >> "$TEMP_LIBS"
        fi
    done

    while read -r lib; do
        get_deps "$lib" >> "$TEMP_LIBS"
        count=$(wc -l < "$TEMP_LIBS")
        echo -ne "\r🔍 Analyzing dependencies: $count found"
    done < "$TEMP_LIBS"
    echo -e "\n✅ Analysis complete."

    echo "🚚 Copying libraries..."
    sort -u "$TEMP_LIBS" | while read -r lib; do
        if [[ ! "$(basename "$lib")" =~ $EXCLUDE_LIST ]]; then
            cp -L -n "$lib" "$LIB_DIR/" 2>/dev/null || true
        fi
    done
    rm "$TEMP_LIBS"
fi

echo "📊 Binary Status Check:"
for bin in "$BIN_DIR"/*; do
    [ -e "$bin" ] || continue
    if [[ "$(basename "$bin")" == "windusb-gui" ]]; then continue; fi
    if file "$bin" | grep -q "ELF"; then
        if ldd "$bin" 2>&1 | grep -q "not a dynamic executable"; then
            echo "  $(basename "$bin") fully static"
        else
            echo "  $(basename "$bin") not fully static"
        fi
    fi
done

echo "🚀 Packaging AppImage..."
[ -f "$APP_DIR/AppRun" ] && chmod +x "$APP_DIR/AppRun"
FINAL_FILENAME="WindUSB-x86_64.AppImage"
$APPIMAGE_TOOL "$APP_DIR" "$FINAL_FILENAME"
APP_SIZE=$(du -h "$FINAL_FILENAME" | cut -f1)

echo "-------------------------------------------------------"
echo "✅ Build Complete!"
echo "📦 File: $FINAL_FILENAME"
echo "📏 Size: $APP_SIZE"
echo "-------------------------------------------------------"