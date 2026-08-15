#!/bin/sh
set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP_NAME=${APP_NAME:-GoosyRenderer}
DIST_DIR=${DIST_DIR:-"$ROOT_DIR/dist"}
APP_DIR="$DIST_DIR/$APP_NAME.app"
BINARY_PATH="$ROOT_DIR/target/release/goosy"
VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | sed -n '1p')

if [ "${1:-}" = "--open" ]; then
    OPEN_APP=1
else
    OPEN_APP=0
fi

case "$(uname -s)" in
    Darwin) ;;
    *)
        printf '%s\n' "This script must run on macOS." >&2
        exit 1
        ;;
esac

printf '%s\n' "Building GoosyRenderer release binary..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" --bin goosy

if [ ! -x "$BINARY_PATH" ]; then
    printf '%s\n' "Release binary not found: $BINARY_PATH" >&2
    exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$BINARY_PATH" "$APP_DIR/Contents/MacOS/$APP_NAME"
chmod 755 "$APP_DIR/Contents/MacOS/$APP_NAME"

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleExecutable</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>com.goosy.renderer</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSEnvironment</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
EOF

/usr/bin/plutil -lint "$APP_DIR/Contents/Info.plist"
if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --sign - "$APP_DIR"
    codesign --verify --deep --strict "$APP_DIR"
fi
printf '%s\n' "Created: $APP_DIR"
printf '%s\n' "Launch: open \"$APP_DIR\""

if [ "$OPEN_APP" -eq 1 ]; then
    open "$APP_DIR"
fi
