#!/bin/sh
# whatsapp-tui installer for Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/leonavas/whatsapp-tui/master/install.sh | sh
#
# Downloads the latest release (or WHATSAPP_TUI_VERSION=v0.1.0) for this
# machine, checks its SHA-256, puts `whatsapp-tui` in
# ${WHATSAPP_TUI_INSTALL_DIR:-$HOME/.local/bin} and its desktop entry and icon
# in ~/.local/share. No sudo. Run it again to update.

set -eu

REPO="leonavas/whatsapp-tui"

say() { printf 'whatsapp-tui: %s\n' "$*"; }
err() { printf 'whatsapp-tui: error: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

# fetch URL FILE, or fetch URL to stdout
fetch() {
    if has curl; then
        if [ $# -gt 1 ]; then curl -fsSL --proto '=https' -o "$2" "$1"; else curl -fsSL --proto '=https' "$1"; fi
    elif has wget; then
        if [ $# -gt 1 ]; then wget -q -O "$2" "$1"; else wget -q -O - "$1"; fi
    else
        err "needs curl or wget"
    fi
}

sha256() {
    if has sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    else
        err "needs sha256sum or shasum to check the download"
    fi
}

target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux) ;;
        *) err "unsupported OS: $os (only Linux for now; build from source: cargo install --locked --git https://github.com/$REPO)" ;;
    esac
    case "$arch" in
        x86_64 | amd64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64 | arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) err "unsupported architecture: $arch" ;;
    esac
}

latest() {
    fetch "https://api.github.com/repos/$REPO/releases/latest" |
        sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

main() {
    tgt=$(target)
    dir="${WHATSAPP_TUI_INSTALL_DIR:-$HOME/.local/bin}"
    share="${XDG_DATA_HOME:-$HOME/.local/share}"

    tag="${WHATSAPP_TUI_VERSION:-}"
    if [ -z "$tag" ]; then
        tag=$(latest) || true
        [ -n "$tag" ] || err "could not find the latest release on GitHub"
    fi
    case "$tag" in v*) ;; *) tag="v$tag" ;; esac
    ver="${tag#v}"

    name="whatsapp-tui-$ver-$tgt.tar.gz"
    url="https://github.com/$REPO/releases/download/$tag/$name"

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM

    say "downloading whatsapp-tui $ver ($tgt)"
    fetch "$url" "$tmp/$name" || err "download failed: $url"
    fetch "$url.sha256" "$tmp/$name.sha256" || err "download failed: $url.sha256"

    want=$(cut -d' ' -f1 <"$tmp/$name.sha256")
    got=$(sha256 "$tmp/$name")
    [ "$want" = "$got" ] || err "SHA-256 mismatch ($got, expected $want)"

    mkdir "$tmp/pkg"
    tar -xzf "$tmp/$name" -C "$tmp/pkg" || err "could not unpack $name"
    mkdir -p "$dir"
    # Replaced by a rename, so a running whatsapp-tui keeps its own copy.
    cp "$tmp/pkg/whatsapp-tui" "$dir/.whatsapp-tui.new"
    chmod 755 "$dir/.whatsapp-tui.new"
    mv -f "$dir/.whatsapp-tui.new" "$dir/whatsapp-tui"
    say "installed $dir/whatsapp-tui"

    # The launcher entry and its icon.
    mkdir -p "$share/applications" "$share/icons/hicolor/scalable/apps" "$share/icons/hicolor/256x256/apps"
    cp "$tmp/pkg/whatsapp-tui.desktop" "$share/applications/"
    cp "$tmp/pkg/whatsapp-tui.svg" "$share/icons/hicolor/scalable/apps/"
    cp "$tmp/pkg/whatsapp-tui-256.png" "$share/icons/hicolor/256x256/apps/whatsapp-tui.png"
    has update-desktop-database && update-desktop-database "$share/applications" >/dev/null 2>&1 || true
    has gtk-update-icon-cache && gtk-update-icon-cache -q "$share/icons/hicolor" >/dev/null 2>&1 || true

    case ":${PATH:-}:" in
        *":$dir:"*) ;;
        *)
            say "warning: $dir is not on your PATH. Add it with:"
            say "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile   # then open a new shell"
            ;;
    esac

    say "done. Run: whatsapp-tui"
}

main "$@"
