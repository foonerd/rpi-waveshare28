#!/bin/bash
#
# Installer for the Waveshare 2.8 inch SPI LCD (SKU 27579) on Raspberry Pi.
#
# Fetches published artefacts from GitHub Releases. Two independent streams,
# because the two halves have different clocks: a new Volumio kernel needs a
# new module with no code change, and a code change needs new binaries with no
# kernel change.
#
#   kernel-<kver>     cst328-rpi-<kver>.tar.gz   touch module and overlay
#   runtime-v<x.y.z>  waveshare28-panel-<arch>   userspace renderer
#
# Usage:
#   ./install.sh runtime          userspace renderer only (default)
#   ./install.sh kernel           touch module and overlay only
#   ./install.sh both
#
# The two are mutually exclusive at run time on spi0 cs0. Installing both is
# fine; the boot configuration decides which one owns the panel.

set -euo pipefail

REPO="foonerd/rpi-waveshare28"
RELEASE_BASE="https://github.com/${REPO}/releases/download"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/main"
RUNTIME_TAG="runtime-v1.0.0"

BIN_DIR="/usr/local/bin"
USERCONFIG="/boot/userconfig.txt"

MODE="${1:-runtime}"

# Working directory for downloads, created in main and removed on exit.
#
# One directory for the whole run rather than a RETURN trap per function.
# A RETURN trap in a function that uses a `local` fires after that local has
# gone out of scope, so it either references an unset variable or has to bake
# the path in at declaration time, which shellcheck then flags. Neither is
# worth the argument for something a single EXIT trap handles correctly.
TMPDIR_RUN=""

cleanup() {
    [[ -n "$TMPDIR_RUN" ]] && rm -rf "$TMPDIR_RUN"
}
trap cleanup EXIT

log()  { printf '[install] %s\n' "$*"; }
warn() { printf '[install] WARNING: %s\n' "$*" >&2; }
die()  { printf '[install] ERROR: %s\n' "$*" >&2; exit 1; }

require_root() {
    [[ $EUID -eq 0 ]] || die "run as root (sudo $0 $MODE)"
}

require() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"
}

# Full kernel release string, e.g. 6.12.75-v7+. The module must match this
# exactly or it will not load.
kernel_release() {
    uname -r
}

# Semantic version only, e.g. 6.12.75. This keys the release tag, because one
# tarball carries the module for every variant of that version.
kernel_version() {
    local r
    r="$(kernel_release)"
    r="${r%%-*}"
    printf '%s' "${r%%+*}"
}

# Rust target triple for this machine.
#
# Only musl builds are published: statically linked, so they run on Buster,
# Bookworm and Trixie alike, which matters because Volumio 3 is Buster and
# Volumio 4 is Bookworm.
#
# ARMv6 and ARMv7 are separate targets and must not be conflated. Volumio
# builds a `+` kernel variant from bcmrpi_defconfig for the Pi Zero, Zero W
# and Pi 1, which are ARMv6. An armv7 binary on those boards dies with SIGILL
# at the first ARMv7-only instruction, which is a considerably worse failure
# than refusing to install.
#
# Note that uname -m reports the kernel architecture, not the userland. That
# is the right question here: these binaries are static, so the userland ABI
# is irrelevant and only the kernel's ability to execute them matters. A
# 64-bit kernel with a 32-bit userland, which Volumio does not currently
# ship, would still run the aarch64 binary.
runtime_arch() {
    case "$(uname -m)" in
        armv6l)  printf 'arm-unknown-linux-musleabihf' ;;
        armv7l)  printf 'armv7-unknown-linux-musleabihf' ;;
        aarch64) printf 'aarch64-unknown-linux-musl' ;;
        *)       die "unsupported architecture: $(uname -m)" ;;
    esac
}

# Download a URL to a path, failing loudly rather than leaving a truncated
# file behind.
fetch() {
    local url="$1" dest="$2"
    log "fetching ${url}"
    if ! curl -fsSL --retry 3 --connect-timeout 15 -o "$dest" "$url"; then
        rm -f "$dest"
        die "download failed: ${url}"
    fi
}

# Verify a file against a published .sha256 sidecar. A missing sidecar is an
# error, not a reason to skip: an unverified binary from the network is not
# something to install on someone else's player.
verify() {
    local file="$1" sums="$2"
    local want have
    want="$(awk '{print $1}' "$sums")"
    have="$(sha256sum "$file" | awk '{print $1}')"
    [[ "$want" == "$have" ]] || die "checksum mismatch for $(basename "$file")"
    log "checksum ok: $(basename "$file")"
}

# Add a line to userconfig.txt if it is not already present.
#
# Lines are kept short deliberately. The firmware silently truncates config
# lines past 100 characters, dropping the tail of the last parameter with no
# warning anywhere.
ensure_config_line() {
    local line="$1"
    [[ ${#line} -lt 100 ]] || die "config line is ${#line} chars, must be under 100: ${line}"

    if [[ ! -f "$USERCONFIG" ]]; then
        die "${USERCONFIG} not found. This does not look like a Volumio Pi image."
    fi

    if grep -qxF "$line" "$USERCONFIG"; then
        log "already present in userconfig.txt: ${line}"
        return
    fi

    log "adding to userconfig.txt: ${line}"
    printf '%s\n' "$line" >> "$USERCONFIG"
}

install_kernel() {
    local kver tag url
    kver="$(kernel_version)"
    tag="kernel-${kver}"
    url="${RELEASE_BASE}/${tag}/cst328-rpi-${kver}.tar.gz"

    log "running kernel $(kernel_release), looking for release ${tag}"

    local tmp="${TMPDIR_RUN}/kernel"
    mkdir -p "$tmp"

    if ! curl -fsIL --connect-timeout 15 -o /dev/null "$url" 2>/dev/null; then
        die "no published module for kernel ${kver}.
    A module built for a different kernel will not load, so this cannot be
    substituted. Either build it yourself from kernel/ in this repository, or
    open an issue at https://github.com/${REPO}/issues asking for ${kver}."
    fi

    fetch "$url" "${tmp}/module.tar.gz"
    fetch "${url}.sha256" "${tmp}/module.sha256"
    verify "${tmp}/module.tar.gz" "${tmp}/module.sha256"

    log "unpacking module and overlay"
    tar -xzf "${tmp}/module.tar.gz" -C "$tmp"

    # The tarball mirrors / with a single top-level directory.
    local root
    root="$(find "$tmp" -maxdepth 1 -type d -name 'cst328-rpi-*' | head -n1)"
    [[ -n "$root" ]] || die "unexpected tarball layout"

    cp -a "${root}/lib/." /lib/
    cp -a "${root}/boot/." /boot/

    log "running depmod for $(kernel_release)"
    depmod -a "$(kernel_release)"

    ensure_config_line "dtoverlay=cst328"

    log "kernel side installed. Reboot, then check:"
    log "  dmesg | grep -i hynitron"
    log "  ls /dev/input/event*"
}

install_runtime() {
    local arch url
    arch="$(runtime_arch)"
    url="${RELEASE_BASE}/${RUNTIME_TAG}/waveshare28-panel-${arch}"

    local tmp="${TMPDIR_RUN}/runtime"
    mkdir -p "$tmp"

    fetch "$url" "${tmp}/waveshare28-panel"
    fetch "${url}.sha256" "${tmp}/waveshare28-panel.sha256"
    verify "${tmp}/waveshare28-panel" "${tmp}/waveshare28-panel.sha256"

    log "installing to ${BIN_DIR}"
    install -m 0755 "${tmp}/waveshare28-panel" "${BIN_DIR}/waveshare28-panel"

    log "installing configurator"
    fetch "${RAW_BASE}/scripts/waveshare28-config" "${tmp}/waveshare28-config"
    install -m 0755 "${tmp}/waveshare28-config" "${BIN_DIR}/waveshare28-config"

    # Boot configuration, the systemd unit and its safe enablement are all
    # derived from /boot/waveshare28.conf, so the configurator owns them
    # rather than this script writing a second copy that can drift.
    log "applying configuration"
    "${BIN_DIR}/waveshare28-config" apply

    log "runtime installed."
    log "Rotation:  waveshare28-config set rotation=90"
    log "Status:    systemctl status waveshare28-panel"
    log "Recovery:  waveshare28-config recover"
}

check_conflict() {
    if grep -qE '^dtoverlay=(fbtft|mipi-dbi-spi)' "$USERCONFIG" 2>/dev/null; then
        warn "a display overlay is configured on spi0 cs0 in ${USERCONFIG}."
        warn "An overlay applied by the firmware is merged into the device tree"
        warn "before the kernel starts and cannot be removed at runtime, so the"
        warn "renderer could never open /dev/spidev0.0. The configurator will"
        warn "remove those lines."
    fi
}

main() {
    require_root
    require curl
    require tar
    require sha256sum

    TMPDIR_RUN="$(mktemp -d)"

    case "$MODE" in
        kernel)
            install_kernel
            ;;
        runtime)
            check_conflict
            install_runtime
            ;;
        both)
            install_kernel
            check_conflict
            install_runtime
            ;;
        *)
            die "usage: $0 [runtime|kernel|both]"
            ;;
    esac

    log "done."
}

main
