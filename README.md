# rpi-waveshare28

Raspberry Pi support for the Waveshare 2.8 inch SPI LCD (SKU 27579): ST7789V
display, CST328 touch, kernel module and overlay builds, and a lightweight
Rust renderer.

Standalone. Not part of any Volumio or evo release stream.

## Two independent halves

    kernel/     CST328 kernel module and device tree overlay
    runtime/    userspace renderer and touch reader, Rust
    scripts/    installer and configurator
    docs/       CONFIG.md, BOOT-FLOW.md, design notes

They share nothing but the hardware. Different toolchains, different build
systems, different outputs. Each has its own README and builds on its own.

The SPI backend and an fbtft overlay cannot share spi0 cs0. A firmware-applied
overlay in `userconfig.txt` cannot be removed later, which is why Plymouth
can bind the `fb_st7789v` framebuffer in the initramfs. `backend=framebuffer`
draws into that device after `plymouth-quit`. `backend=spi` owns the bus and
the configurator strips the fbtft lines.

    make validate   fmt, clippy and tests for the runtime, no build
    make runtime    validate then cross-build the renderer
    make kernel     build the CST328 module and overlay

or work in either subtree directly.

## Install

On the target:

    curl -fsSL https://raw.githubusercontent.com/foonerd/rpi-waveshare28/main/scripts/install.sh | sudo bash -s runtime

`runtime`, `kernel` or `both`. Artefacts are fetched from GitHub Releases and
verified against their published sha256 before installation.

## Configuration

All of it is `waveshare28-config`. The full reference is
[`docs/CONFIG.md`](docs/CONFIG.md).

    waveshare28-config show
    sudo waveshare28-config set rotation=270 backend=framebuffer console=release
    waveshare28-config verify
    sudo waveshare28-config recover

`/boot/waveshare28.conf` is the durable copy. `set` and `apply` regenerate
everything else from it. `set`, `apply` and `recover` need root.

## What the panel shows

Before Volumio's node process answers, the hostname and the host's addresses,
updating as they are assigned, and the hotspot SSID if the AP is up. After
`/status` is `ready` and the first `getState` succeeds, the player. Tap
the cover to see the addresses again for ten seconds.

The renderer starts early and does not wait for `volumio.service`. Without
that the panel is dark for most of a minute, and the address is the one thing
someone needs before the player is reachable.

Plymouth on this panel needs fbtft in `userconfig.txt` so `fb_st7789v`
exists before the initramfs hook starts `plymouthd`. That overlay owns the
SPI bus for the life of the boot. `backend=framebuffer` is the renderer
path that shares it: the process mmaps that node after `plymouth-quit` and
does not open `/dev/spidev0.0`. The node is found by sysfs name, because
HDMI or the firmware KMS framebuffer can already occupy `/dev/fb0`.
`backend=spi` is the original path and removes the fbtft lines. See
`docs/BOOT-FLOW.md`.

## Releases

Two tag streams, because the two halves have different clocks. A new Volumio
kernel needs a new module with no source change; a source change needs new
binaries with no kernel change. Tying both to one version number would mean
lying about one of them on every release.

    kernel-<x.y.z>      cst328-rpi-<x.y.z>.tar.gz
    runtime-v<x.y.z>    waveshare28-panel-<target>

Both are built and published by GitHub Actions on tag push, so a published
artefact is reproducible from the tag rather than from whatever was on a
workstation that day.

The kernel module must match the running kernel exactly. If there is no
release for your kernel version the installer says so and stops, rather than
installing something that will not load.

Only musl runtime binaries are published, for three targets: ARMv6, ARMv7 and
ARM64. They are statically linked and run on Buster, Bookworm and Trixie
alike, which covers Volumio 3 and Volumio 4. The glibc targets build but are
not published: cross's stock images are glibc 2.31, which excludes Buster.

ARMv6 is a separate target from ARMv7, not a synonym. Volumio builds a `+`
kernel variant from `bcmrpi_defconfig` for the Zero and Pi 1, and an armv7
binary dies there with SIGILL.

## Hardware

Waveshare SKU 27579. ST7789V (ST7789T3) display over 4-wire SPI, Hynitron
CST328 capacitive touch over I2C, 240(H) x 320(V) native portrait.

Wiring per the Waveshare wiki, BCM numbering:

    MOSI     GPIO10
    SCLK     GPIO11
    LCD_CS   GPIO8    (spi0 cs0)
    LCD_DC   GPIO25
    LCD_RST  GPIO27
    LCD_BL   GPIO18
    TP_SDA   GPIO2    (i2c1)
    TP_SCL   GPIO3    (i2c1)
    TP_INT   GPIO4
    TP_RST   GPIO17

MISO is not brought out on either the 13 pin connector or the 18 pin FPC.

Touch I2C address 0x1A (0x34/0x35 as the 8-bit write/read pair). The address
is customisable in chip firmware, so a clone module may differ.

## Boot configuration

`waveshare28-config apply` writes `/boot/userconfig.txt` and the two `fbcon=`
tokens on `cmdline.txt`. Never edit `config.txt` or `volumioconfig.txt`; they
are system managed and an OTA overwrites them. The lines, the 100-character
firmware limit, and clockwise vs fbtft `rotate` are in
[`docs/CONFIG.md`](docs/CONFIG.md).

## Known constraints

These apply to the kernel display path and to `backend=framebuffer`, which
shares that framebuffer. They do not apply to `backend=spi`.

The fbtft backlight device at `/sys/class/backlight/fb_st7789v/` reports
`max_brightness` of 0. It is a GPIO backlight with no usable range: the only
writable value is 0 and anything else returns EINVAL. Control GPIO18 directly
if you need the backlight.

There is no DRM device when driving this panel via fbtft, so X requires the
`fbdev` driver with explicit `Device`, `Screen` and `ServerLayout` sections.
Without a `Screen` section naming the device, autoconfiguration discards it
with `Screen 0 deleted because of no matching config section`.

TTY1, `fbcon=`, and `console=release` are owned by the configurator; see
[`docs/CONFIG.md`](docs/CONFIG.md).

## Licence

Apache-2.0, except `kernel/source_files/*/overlays/*.dts`, which is
`GPL-2.0 OR MIT` following the upstream device tree convention so the overlay
stays eligible for submission to the Raspberry Pi kernel tree.
