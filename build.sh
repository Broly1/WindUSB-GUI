#!/bin/bash
set -e

APP_DIR="WindUSB.AppDir"
BIN_DIR="$APP_DIR/bin-local"
LIB_DIR="$APP_DIR/lib-local"
APPIMAGE_TOOL="./appimagetool-x86_64.appimage"

echo "-------------------------------------------------------"
echo "💡 TIP: If you want fresh libraries and a clean binary,"
echo "   simply delete the '$BIN_DIR' or '$LIB_DIR' folders."
echo "   The script will detect they are missing and re-bundle."
echo "-------------------------------------------------------"

# Check if we should skip the heavy lifting
if [ -d "$BIN_DIR" ] && [ -d "$LIB_DIR" ] && [ "$(ls -A $BIN_DIR)" ]; then
    echo "🔍 Existing bundle detected in $APP_DIR."
    read -p "❓ Do you want to RE-BUNDLE everything (wipe folders and copy libs again)? [y/N]: " REBUNDLE
else
    REBUNDLE="y"
fi

if [[ "$REBUNDLE" =~ ^[Yy]$ ]]; then
    echo "🧹 Removing old directories and creating fresh ones..."
    rm -rf "$BIN_DIR" "$LIB_DIR"
    mkdir -p "$BIN_DIR" "$LIB_DIR"
    touch "$BIN_DIR/.gitkeep" "$LIB_DIR/.gitkeep"

    echo "🦀 Building Rust app..."
    cargo build --release
    install -m755 target/release/windusb-gui "$BIN_DIR/windusb-gui"

    echo "📦 Bundling tools (sgdisk, wimlib, etc.)..."
    TOOLS=("sgdisk" "wimlib-imagex" "rsync" "mkfs.fat" "wipefs")
    for tool in "${TOOLS[@]}"; do
        cp "$(which "$tool")" "$BIN_DIR/"
    done

    echo "📚 Gathering library dependencies..."
    # Exclude list to keep the AppImage portable across different Linux distros
    EXCLUDE_LIST="libc.so|libpthread.so|libdl.so|libm.so|librt.so|libgcc_s.so|libstdc++.so|libresolv.so|libcrypt.so|libutil.so|libnsl.so|libGL|libnvidia|libdrm|libX11|libxcb|libasound|libpulse"
    
    find "$BIN_DIR" -type f -executable | xargs ldd | grep "=> /" | awk '{print $3}' | sort -u | while read -r lib; do
        libname=$(basename "$lib")
        if [[ ! "$libname" =~ $EXCLUDE_LIST ]]; then
            cp -L "$lib" "$LIB_DIR/"
        fi
    done
else
    echo "⏭️  Using existing libs. Just updating the Rust binary..."
    cargo build --release
    install -m755 target/release/windusb-gui "$BIN_DIR/windusb-gui"
fi

# Final Packaging
echo "🚀 Packaging AppImage..."
export VERSION=$(grep '^version' Cargo.toml | awk -F '"' '{print $2}')
$APPIMAGE_TOOL "$APP_DIR" "WindUSB-x86_64.AppImage"

echo "✅ Build complete: WindUSB-x86_64.AppImage"