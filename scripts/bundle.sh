#!/usr/bin/env bash
#
# Assembles Optra.app from a release build.
# A macOS .app is just a directory with a required layout -- there is no
# installer format. "Installing" is copying the result into /Applications.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Optra"
BIN_NAME="optra"                      # must match [package] name in Cargo.toml
BUNDLE_ID="com.jstanhope.optra"
ICON_SRC="logo.png"                   # square png, ideally >= 1024x1024
# Keep the bundle version in sync with Cargo.toml rather than duplicating it.
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"

APP_DIR="dist/${APP_NAME}.app"

# The runtime Dock icon is compiled into the binary via include_bytes!, so it
# must be generated BEFORE the build, not after.
if [[ -f "${ICON_SRC}" ]]; then
    echo "==> Generating rounded icons from ${ICON_SRC}"
    mkdir -p assets
    cargo run --release --quiet --example make_icon -- "${ICON_SRC}" assets/icon.png 512
fi

echo "==> Building release binary"
cargo build --release

echo "==> Assembling ${APP_DIR}"
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"
mkdir -p "${APP_DIR}/Contents/Resources"

cp "target/release/${BIN_NAME}" "${APP_DIR}/Contents/MacOS/${BIN_NAME}"

# --- Icon ----------------------------------------------------------------
# .icns is a container holding every size macOS might need (Dock, Finder list
# view, Get Info, Cmd-Tab). iconutil builds it from a directory of exactly
# these filenames -- 6 sizes, each also at @2x for Retina.
if [[ -f "${ICON_SRC}" ]]; then
    echo "==> Building ${BIN_NAME}.icns"
    WORK="$(mktemp -d)"
    ICONSET="${WORK}/${APP_NAME}.iconset"
    mkdir -p "${ICONSET}"

    # Round once at full size, then downsample, so every size shares the same
    # corner geometry.
    ROUNDED="${WORK}/rounded.png"
    cargo run --release --quiet --example make_icon -- "${ICON_SRC}" "${ROUNDED}" 1024

    for size in 16 32 128 256 512; do
        sips -z $size    $size    "${ROUNDED}" --out "${ICONSET}/icon_${size}x${size}.png"    >/dev/null
        sips -z $((size*2)) $((size*2)) "${ROUNDED}" --out "${ICONSET}/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "${ICONSET}" -o "${APP_DIR}/Contents/Resources/${BIN_NAME}.icns"
    ICON_KEY="<key>CFBundleIconFile</key><string>${BIN_NAME}</string>"
else
    echo "==> No ${ICON_SRC}, skipping icon"
    ICON_KEY=""
fi

cat > "${APP_DIR}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>

    <key>CFBundlePackageType</key>
    <string>APPL</string>

    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>

    <key>CFBundleName</key>
    <string>${APP_NAME}</string>

    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>

    <key>CFBundleExecutable</key>
    <string>${BIN_NAME}</string>

    ${ICON_KEY}

    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>

    <key>CFBundleVersion</key>
    <string>${VERSION}</string>

    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>

    <key>NSHighResolutionCapable</key>
    <true/>

    <!-- Which file types Optra claims, and how strongly. -->
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>OpenEXR Image</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>com.ilm.openexr-image</string>
                <string>public.radiance</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Owner</string>
        </dict>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Image</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.png</string>
                <string>public.jpeg</string>
                <string>public.tiff</string>
                <string>com.compuserve.gif</string>
                <string>com.microsoft.bmp</string>
                <string>org.webmproject.webp</string>
                <string>com.truevision.tga-image</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
        </dict>
    </array>
</dict>
</plist>
PLIST

# Ad-hoc signature. Required for the bundle to launch cleanly on Apple Silicon.
echo "==> Signing (ad-hoc)"
codesign --force --sign - "${APP_DIR}"

echo "==> Built ${APP_DIR}"
