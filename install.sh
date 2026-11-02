#!/bin/sh
# Install or upgrade Weft — its desktop app, `weft` and `ringframe` — then
# set it up: RingFrame's configuration, and Weft's harness and tool files.
#
#   macOS  Weft.app in /Applications (~/Applications without the rights), as
#          Homebrew's cask puts it; `weft` and `ringframe` on PATH link into
#          the app.
#   Linux  each version in its own folder, ~/.fab7/weft/versions/<version>,
#          so an update never replaces a program a running Weft uses:
#          weft-desktop, weft and ringframe there, linked from ~/.local/bin;
#          the desktop entry and icon in ~/.local/share. git and the
#          libraries the app draws with come from the system's packages.
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
        Linux) platform=unknown-linux-gnu ;;
        *) fail "Unsupported system: $(uname -s). Weft builds for macOS and Linux." ;;
    esac

    case "$(uname -m)" in
        arm64 | aarch64) arch=aarch64 ;;
        x86_64 | amd64) arch=x86_64 ;;
        *) fail "Unsupported architecture: $(uname -m)." ;;
    esac

    # Sense runs on ONNX Runtime, which is built for Apple silicon and for
    # Linux with glibc only.
    if [ "$platform-$arch" = apple-darwin-x86_64 ]; then
        fail 'Weft has no build for an Intel Mac: it needs Apple silicon.'
    fi
    if [ "$platform" = unknown-linux-gnu ]; then
        libc=$(ldd --version 2>&1 | head -n 1)
        case "$libc" in
            *[Mm]usl*) fail 'Weft has no build for a musl system, such as Alpine: it needs glibc.' ;;
        esac
        # ONNX Runtime's Linux build calls glibc 2.38's functions. A version
        # ldd does not say is not refused.
        glibc=${libc##* }
        case "$glibc" in
            [0-9]*.[0-9]*)
                major=${glibc%%.*}
                minor=${glibc#*.}
                minor=${minor%%[!0-9]*}
                if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "${minor:-0}" -lt 38 ]; }; then
                    fail "Weft needs glibc 2.38 or later; this system has $glibc."
                fi
                ;;
        esac
    fi

    target="$arch-$platform"

    command -v curl >/dev/null 2>&1 || fail 'curl is required.'
    command -v tar >/dev/null 2>&1 || fail 'tar is required.'

    # What Weft needs of a Linux system: git, which RingFrame fetches its
    # configuration and keeps a project's record with, and the libraries the
    # desktop app draws with. What is missing comes from the system's package
    # manager. WEFT_SYSTEM_PACKAGES=0 leaves the system to the person.
    system_packages() {
        libs=$({ ldconfig -p || /sbin/ldconfig -p; } 2>/dev/null || true)
        missing=
        command -v git >/dev/null 2>&1 || missing="$missing git"
        for lib in libxcb.so.1 libfreetype.so.6 libxkbcommon.so.0 libxkbcommon-x11.so.0 \
            libvulkan.so.1 libwayland-client.so.0 libwayland-egl.so.1 libEGL.so.1; do
            case "$libs" in *"$lib "*) ;; *) missing="$missing $lib" ;; esac
        done
        # A Vulkan driver: without one the app has nothing to draw with.
        ls /usr/share/vulkan/icd.d/*.json >/dev/null 2>&1 || missing="$missing a-vulkan-driver"
        [ -n "$missing" ] || return 0
        refresh=
        if command -v apt-get >/dev/null 2>&1; then
            # Asking nothing on the way: there is no one to answer a pipe.
            refresh='apt-get update -q'
            install='env DEBIAN_FRONTEND=noninteractive apt-get install -y -q git libxcb1 libfreetype6 libxkbcommon0 libxkbcommon-x11-0 libvulkan1 libwayland-client0 libwayland-egl1 libegl1 mesa-vulkan-drivers'
        elif command -v dnf >/dev/null 2>&1; then
            install='dnf install -y git libxcb freetype libxkbcommon libxkbcommon-x11 vulkan-loader libwayland-client libwayland-egl libglvnd-egl mesa-vulkan-drivers'
        elif command -v pacman >/dev/null 2>&1; then
            install='pacman -Sy --needed --noconfirm git libxcb freetype2 libxkbcommon libxkbcommon-x11 vulkan-icd-loader wayland libglvnd vulkan-swrast'
        else
            printf 'Weft needs%s; install them with your package manager.\n' "$missing"
            return 0
        fi
        if [ "$(id -u)" -eq 0 ]; then
            as_root=
        elif command -v sudo >/dev/null 2>&1; then
            as_root=sudo
        else
            printf 'Weft needs%s. As root, run: %s\n' "$missing" "$install"
            return 0
        fi
        printf 'Weft needs%s: installing them with your package manager\n' "$missing"
        # Each word of a command is its own argument.
        if [ -n "$refresh" ]; then
            $as_root $refresh >/dev/null || fail "Could not run: $as_root $refresh"
        fi
        $as_root $install >/dev/null || fail "Could not run: $as_root $install"
    }
    if [ "$platform" = unknown-linux-gnu ] && [ "${WEFT_SYSTEM_PACKAGES:-1}" != 0 ]; then
        system_packages
    fi

    if [ -n "${WEFT_VERSION:-}" ]; then
        tag=$WEFT_VERSION
    else
        # The latest release, from where its page leads: GitHub's API allows
        # an address 60 calls an hour, which one shared by many soon uses up.
        tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" |
            sed -n 's|.*/releases/tag/\([^/?#]*\)$|\1|p')
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

    # Where the app goes, checked before anything is switched: an earlier
    # Weft is replaced, never another app, nor one Homebrew keeps.
    if [ "$platform" = apple-darwin ]; then
        apps=${WEFT_APPS_DIR:-}
        if [ -z "$apps" ]; then
            if [ -w /Applications ]; then apps=/Applications; else apps="$HOME/Applications"; fi
        fi
        if [ -e "$apps/Weft.app" ] && [ ! -L "$apps/Weft.app" ]; then
            id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
                "$apps/Weft.app/Contents/Info.plist" 2>/dev/null || true)
            [ "$id" = com.getfab7.weft ] ||
                fail "$apps/Weft.app is not one Weft installed; move it away and install again."
            if command -v brew >/dev/null 2>&1 && [ -d "$(brew --prefix)/Caskroom/weft" ]; then
                fail 'Weft is installed with Homebrew: brew upgrade --cask weft updates it.'
            fi
        fi
    fi

    # Each link replaced, never written through.
    link() {
        rm -f "$2.new"
        ln -s "$1" "$2.new"
        mv -f "$2.new" "$2"
    }

    version=${tag#v}
    if [ "$platform" = apple-darwin ]; then
        [ -f "$work/Weft.app/Contents/MacOS/weft-desktop" ] || fail "$archive does not contain Weft.app."
        mkdir -p "$apps"
        # The new app beside the old, then in its place: a running Weft keeps
        # the files it has open.
        rm -rf "$apps/.Weft.app.new"
        mv "$work/Weft.app" "$apps/.Weft.app.new"
        if [ -e "$apps/Weft.app" ] || [ -L "$apps/Weft.app" ]; then
            mv "$apps/Weft.app" "$work/Weft.app.old"
        fi
        mv "$apps/.Weft.app.new" "$apps/Weft.app"
        programs="$apps/Weft.app/Contents/MacOS"
        link "$programs/weft" "$BIN_DIR/weft"
        link "$programs/ringframe" "$BIN_DIR/ringframe"
        bundled="$apps/Weft.app/Contents/Resources/fab7"
        # Finder, the Dock and Spotlight show its icon at once. One put in a
        # folder of the caller's choosing is theirs to register.
        lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
        if [ -z "${WEFT_APPS_DIR:-}" ] && [ -x "$lsregister" ]; then
            "$lsregister" -f "$apps/Weft.app" || true
        fi
        printf 'Installed Weft %s to %s\n' "$tag" "$apps/Weft.app"
    else
        for name in weft-desktop weft ringframe weft.desktop weft.svg; do
            [ -f "$work/$name" ] || fail "$archive does not contain $name."
        done
        # This version in its own folder, beside the others: a running Weft
        # keeps its own, and its daemon starts again on this one once no
        # agent runs.
        folder="$HOME/.fab7/weft/versions/$version"
        rm -rf "$folder.new"
        mkdir -p "$folder.new"
        for name in weft-desktop weft ringframe; do
            mv "$work/$name" "$folder.new/$name"
            chmod 755 "$folder.new/$name"
        done
        # fab7's files beside the programs, as a first start reads them.
        if [ -d "$work/fab7" ]; then mv "$work/fab7" "$folder.new/fab7"; fi
        # The same version again: the one there is moved aside first, so a
        # Weft running from it keeps its files until it ends.
        if [ -e "$folder" ]; then
            rm -rf "$folder.old"
            mv "$folder" "$folder.old"
        fi
        mv "$folder.new" "$folder"
        rm -rf "$folder.old"
        link "$folder/weft" "$BIN_DIR/weft"
        link "$folder/ringframe" "$BIN_DIR/ringframe"
        link "$folder/weft-desktop" "$BIN_DIR/weft-desktop"
        bundled="$folder/fab7"
        share="$HOME/.local/share"
        mkdir -p "$share/applications" "$share/icons/hicolor/scalable/apps"
        # Started by a desktop that may not have BIN_DIR on its PATH.
        sed "s|^Exec=weft-desktop|Exec=$BIN_DIR/weft-desktop|" "$work/weft.desktop" >"$share/applications/weft.desktop"
        cp "$work/weft.svg" "$share/icons/hicolor/scalable/apps/weft.svg"
        # The launcher lists the app at once, where the desktop keeps a database.
        if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "$share/applications" >/dev/null 2>&1 || true
        fi
        printf 'Installed Weft %s to %s, linked from %s\n' "$tag" "$folder" "$BIN_DIR"
    fi

    # RingFrame's configuration at fab7's latest release; offline, the one
    # that came with this Weft. The binaries are in place whatever happens,
    # so a configuration that cannot be installed is said, not aborted on.
    if ! "$BIN_DIR/ringframe" init --global >/dev/null 2>&1 &&
        ! { [ -d "$bundled/ringframe/config" ] &&
            "$BIN_DIR/ringframe" init --global --from "$bundled/ringframe/config" >/dev/null; }; then
        printf '\n%s\n' "The binaries are installed, but RingFrame's configuration could not be."
        printf '%s\n' "Run \`$BIN_DIR/ringframe init --global\` again once that is sorted."
    fi

    # Weft's harness files say how to start, resume and set up each harness,
    # and its tool files how to install a tunnel. They came with this Weft,
    # and replace whatever was there: they are never edited.
    for part in harnesses tools; do
        if [ -d "$bundled/weft/$part" ]; then
            mkdir -p "$HOME/.fab7/weft"
            rm -rf "$HOME/.fab7/weft/$part.new"
            cp -R "$bundled/weft/$part" "$HOME/.fab7/weft/$part.new"
            rm -rf "$HOME/.fab7/weft/$part"
            mv "$HOME/.fab7/weft/$part.new" "$HOME/.fab7/weft/$part"
        fi
    done
    if [ -d "$HOME/.fab7/weft/harnesses" ]; then
        printf 'Installed the harness files to %s\n' "$HOME/.fab7/weft/harnesses"
    else
        printf '\n%s\n' "This release carries no harness files, so Weft offers no harness yet."
    fi

    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) printf '\n%s\n' "Add $BIN_DIR to your PATH." ;;
    esac
}

main "$@"
