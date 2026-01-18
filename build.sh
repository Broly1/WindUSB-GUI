#!/bin/bash
set -e

# Configuration
APP_DIR="WindUSB.AppDir"
BIN_DIR="$APP_DIR/bin-local"
LIB_DIR="$APP_DIR/lib-local"
APPIMAGE_TOOL="./appimagetool-x86_64.appimage"
BASE_URL_7Z="https://sourceforge.net/projects/sevenzip/files/7-Zip/"
URL_APPIMAGETOOL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"

log_error() { echo "❌ ERROR: $1"; exit 1; }

# Handle Clean Flag
if [[ "$1" == "--clean" ]]; then
    echo "🧹 Performing full cleanup..."
    rm -rf "$BIN_DIR" "$LIB_DIR" "$APPIMAGE_TOOL" "*.AppImage"
    echo "✨ Cleanup complete."
    exit 0
fi

download_appimagetool() {
    echo "📥 Downloading appimagetool..."
    curl -Lo "$APPIMAGE_TOOL" "$URL_APPIMAGETOOL" || log_error "Failed to download appimagetool"
    chmod +x "$APPIMAGE_TOOL"
}

get_latest_version_7z() {
    page_content=$(curl -sL "$BASE_URL_7Z") || log_error "Failed to fetch 7zip version"
    latest_version=$(echo "$page_content" | grep -oP '(?<=href="/projects/sevenzip/files/7-Zip/)[0-9]+\.[0-9]+' | sort -V | tail -n 1)
    printf "%s\n" "$latest_version"
}

download_and_extract_7z() {
    latest_version=$(get_latest_version_7z)
    [ -z "$latest_version" ] && log_error "Could not find latest 7zip version."
    
    ver_flat="${latest_version//./}"
    file_url="${BASE_URL_7Z}${latest_version}/7z${ver_flat}-linux-x64.tar.xz/download"
    
    echo "📥 Downloading 7-Zip v$latest_version..."
    curl -Lo "7z-linux.tar.xz" "$file_url" || log_error "Failed to download 7zip"
    
    tar -xJf "7z-linux.tar.xz" 7zzs || log_error "Failed to extract 7zip (7zzs)"
    
    mkdir -p "$BIN_DIR"
    mv 7zzs "$BIN_DIR/7z"
    chmod +x "$BIN_DIR/7z"
    rm "7z-linux.tar.xz"
}

echo "-------------------------------------------------------"
echo "🚀 WindUSB-GUI Automated Build Script"
echo "-------------------------------------------------------"

# 1. Automatic Dependency Checks
[ ! -f "$APPIMAGE_TOOL" ] && download_appimagetool
[ ! -f "$BIN_DIR/7z" ] && download_and_extract_7z

# 2. Decide on Re-bundling
if [ ! -d "$LIB_DIR" ] || [ ! "$(ls -A $LIB_DIR)" ]; then
    REBUNDLE="y"
else
    echo "💡 TIP: If this is your first build, or you just installed new system tools, select 'y'."
    read -p "❓ Re-bundle system dependencies (wipe and re-copy)? [y/N]: " REBUNDLE
fi

if [[ "$REBUNDLE" =~ ^[Yy]$ ]]; then
    echo "🧹 Cleaning and bundling dependencies..."
    mkdir -p "$BIN_DIR" "$LIB_DIR"
    find "$BIN_DIR" -type f ! -name '7z' ! -name '.gitkeep' -delete
    rm -rf "$LIB_DIR" && mkdir -p "$LIB_DIR"
    touch "$BIN_DIR/.gitkeep" "$LIB_DIR/.gitkeep"

    echo "🦀 Building Rust app..."
    cargo build --release
    install -m755 target/release/windusb-gui "$BIN_DIR/windusb-gui"

    echo "📦 Bundling tools (sgdisk, wimlib, etc.)..."
    TOOLS=("sgdisk" "wimlib-imagex" "mkfs.fat" "wipefs")
    for tool in "${TOOLS[@]}"; do
        cp "$(which "$tool")" "$BIN_DIR/"
    done

    echo "📚 Gathering library dependencies..."
    EXCLUDE_LIST="libc.so|libpthread.so|libdl.so|libm.so|librt.so|libgcc_s.so|libstdc++.so|libresolv.so|libcrypt.so|libutil.so|libnsl.so|libGL|libnvidia|libdrm|libX11|libxcb|libasound|libpulse"
    find "$BIN_DIR" -type f -executable | xargs ldd | grep "=> /" | awk '{print $3}' | sort -u | while read -r lib; do
        [[ ! "$(basename "$lib")" =~ $EXCLUDE_LIST ]] && cp -L "$lib" "$LIB_DIR/"
    done
else
    echo "⏭️  Updating Rust binary only..."
    cargo build --release
    install -m755 target/release/windusb-gui "$BIN_DIR/windusb-gui"
fi

# 3. Final Packaging
echo "🚀 Packaging AppImage..."
export VERSION=$(grep '^version' Cargo.toml | awk -F '"' '{print $2}')
$APPIMAGE_TOOL "$APP_DIR" "WindUSB-x86_64.AppImage"

echo "✅ Success: WindUSB-x86_64.AppImage"