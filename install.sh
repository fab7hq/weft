#!/bin/sh
# Install or upgrade Weft — its desktop app, `weft` and `ringframe` — then
# initialize the global configuration.
#
# Each version goes in its own folder, ~/.fab7/weft/versions/<version>, so an
# update never replaces a program a running Weft uses:
#   macOS  Weft.app there, linked from /Applications (~/Applications without
#          the rights); `weft` and `ringframe` on PATH link into the app.
#   Linux  weft-desktop, weft and ringframe there, linked from ~/.local/bin;
#          the desktop entry and icon in ~/.local/share.
#
# There is no toolchain here. CI builds a binary per platform and publishes it
# with a checksum; this detects the platform, downloads, verifies and installs.
set -eu

# All in one function, called on the last line: a download cut short runs
# nothing rather than half of this.
main() {
    REPO=fab7hq/weft
    BIN_DIR=${WEFT_BIN_DIR:-$HOME/.local/bin}

    fail() {
        printf '%s\n' "$1" >&2
        exit 1
    }

    case "$(uname -s)" in
        Darwin) platform=apple-darwin ;;
        Linux) platform=unknown-linux-musl ;;
        *) fail "Unsupported system: $(uname -s). Weft builds for macOS and Linux." ;;
    esac

    case "$(uname -m)" in
        arm64 | aarch64) arch=aarch64 ;;
        x86_64 | amd64) arch=x86_64 ;;
        *) fail "Unsupported architecture: $(uname -m)." ;;
    esac

    target="$arch-$platform"

    command -v curl >/dev/null 2>&1 || fail 'curl is required.'
    command -v tar >/dev/null 2>&1 || fail 'tar is required.'

    if [ -n "${WEFT_VERSION:-}" ]; then
        tag=$WEFT_VERSION
    else
        # The latest release, without needing a token or jq.
        tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
            sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)
        [ -n "$tag" ] || fail "Could not find the latest release of $REPO."
    fi

    archive="fab7-$tag-$target.tar.gz"
    base="https://github.com/$REPO/releases/download/$tag"

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT INT TERM

    printf 'Downloading %s %s for %s\n' "$REPO" "$tag" "$target"
    curl -fsSL -o "$work/$archive" "$base/$archive" ||
        fail "No build for $target in $tag."
    curl -fsSL -o "$work/$archive.sha256" "$base/$archive.sha256" ||
        fail "$tag publishes no checksum for $target; refusing to install."

    # Verify before anything is unpacked, with whichever tool this system has.
    expected=$(cut -d' ' -f1 <"$work/$archive.sha256")
    if command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$work/$archive" | cut -d' ' -f1)
    elif command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$work/$archive" | cut -d' ' -f1)
    else
        fail 'Neither shasum nor sha256sum is available; cannot verify the download.'
    fi
    [ "$expected" = "$actual" ] ||
        fail "Checksum mismatch for $archive. Expected $expected, got $actual."

    tar -xzf "$work/$archive" -C "$work"
    mkdir -p "$BIN_DIR"

    # Where the app is linked from, checked before anything is switched.
    if [ "$platform" = apple-darwin ]; then
        apps=${WEFT_APPS_DIR:-}
        if [ -z "$apps" ]; then
            if [ -w /Applications ]; then apps=/Applications; else apps="$HOME/Applications"; fi
        fi
        if [ -e "$apps/Weft.app" ] && [ ! -L "$apps/Weft.app" ]; then
            fail "$apps/Weft.app is not one Weft installed; move it away and install again."
        fi
    fi

    # This version in its own folder, beside the others: a running Weft
    # keeps its own, and its daemon starts again on this one once no agent runs.
    version=${tag#v}
    versions="$HOME/.fab7/weft/versions"
    folder="$versions/$version"
    rm -rf "$folder.new"
    mkdir -p "$folder.new"
    if [ "$platform" = apple-darwin ]; then
        programs="Weft.app/Contents/MacOS"
        [ -f "$work/$programs/weft-desktop" ] || fail "$archive does not contain Weft.app."
        mv "$work/Weft.app" "$folder.new/Weft.app"
        ln -s "$programs/weft" "$folder.new/weft"
    else
        programs=.
        for name in weft-desktop weft ringframe weft.desktop weft.svg; do
            [ -f "$work/$name" ] || fail "$archive does not contain $name."
        done
        for name in weft-desktop weft ringframe; do
            mv "$work/$name" "$folder.new/$name"
            chmod 755 "$folder.new/$name"
        done
        # fab7's files beside the programs, as a first start reads them.
        if [ -d "$work/fab7" ]; then mv "$work/fab7" "$folder.new/fab7"; fi
    fi
    # The same version again: the one there is moved aside first, so a Weft
    # running from it keeps its files until it ends.
    if [ -e "$folder" ]; then
        rm -rf "$folder.old"
        mv "$folder" "$folder.old"
    fi
    mv "$folder.new" "$folder"
    rm -rf "$folder.old"

    # Each link replaced, never written through.
    link() {
        rm -f "$2.new"
        ln -s "$1" "$2.new"
        mv -f "$2.new" "$2"
    }
    link "$folder/weft" "$BIN_DIR/weft"
    link "$folder/$programs/ringframe" "$BIN_DIR/ringframe"

    if [ "$platform" = apple-darwin ]; then
        mkdir -p "$apps"
        # -h: the link itself is replaced, not the app it points at.
        ln -sfh "$folder/Weft.app" "$apps/Weft.app"
        printf 'Installed Weft %s to %s, linked from %s\n' "$tag" "$folder" "$apps/Weft.app"
    else
        link "$folder/weft-desktop" "$BIN_DIR/weft-desktop"
        share="$HOME/.local/share"
        mkdir -p "$share/applications" "$share/icons/hicolor/scalable/apps"
        # Started by a desktop that may not have BIN_DIR on its PATH.
        sed "s|^Exec=weft-desktop|Exec=$BIN_DIR/weft-desktop|" "$work/weft.desktop" >"$share/applications/weft.desktop"
        cp "$work/weft.svg" "$share/icons/hicolor/scalable/apps/weft.svg"
        printf 'Installed Weft %s to %s, linked from %s\n' "$tag" "$folder" "$BIN_DIR"
    fi

    # The binaries are in place whatever happens next, so a configuration that
    # cannot be read is worth saying plainly rather than aborting on.
    if ! "$BIN_DIR/ringframe" init --global; then
        printf '\n%s\n' "The binaries are installed, but the configuration could not be."
        printf '%s\n' "Run \`$BIN_DIR/ringframe init --global\` again once that is sorted."
    fi

    # Weft's harness files say how to start, resume and set up each harness. They
    # come with fab7 at the tag the configuration above came from, and replace
    # whatever was there: they are never edited.
    harnesses="$HOME/.fab7/weft/harnesses"
    revision=$(cat "$HOME/.fab7/rf/config/.revision" 2>/dev/null || true)
    if [ -n "$revision" ] && [ "$revision" != local ] &&
        curl -fsSL -o "$work/fab7.tar.gz" "https://codeload.github.com/fab7hq/fab7/tar.gz/refs/tags/$revision" &&
        mkdir -p "$work/fab7" && tar -xzf "$work/fab7.tar.gz" -C "$work/fab7" &&
        [ -d "$work"/fab7/*/products/weft/harnesses ]; then
        mkdir -p "$HOME/.fab7/weft"
        rm -rf "$harnesses.new"
        cp -R "$work"/fab7/*/products/weft/harnesses "$harnesses.new"
        rm -rf "$harnesses"
        mv "$harnesses.new" "$harnesses"
        printf 'Installed the harness files of fab7 %s to %s\n' "$revision" "$harnesses"
        # Its tool files too: how Weft installs cloudflared when asked.
        tools="$HOME/.fab7/weft/tools"
        if [ -d "$work"/fab7/*/products/weft/tools ]; then
            rm -rf "$tools.new"
            cp -R "$work"/fab7/*/products/weft/tools "$tools.new"
            rm -rf "$tools"
            mv "$tools.new" "$tools"
        fi
    else
        printf '\n%s\n' "Weft's harness files could not be installed, so Weft offers no harness yet."
        printf '%s\n' 'Run `weft update` again once the configuration is in place.'
    fi

    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) printf '\n%s\n' "Add $BIN_DIR to your PATH." ;;
    esac
}

main "$@"
