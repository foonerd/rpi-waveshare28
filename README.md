# rpi-waveshare28

Raspberry Pi support for the Waveshare 2.8 inch SPI LCD (SKU 27579): ST7789V
display, CST328 touch, kernel module and overlay builds, and a lightweight
Rust renderer.

Standalone. Not part of any Volumio or evo release stream.

## Two independent halves

    kernel/     CST328 kernel module and device tree overlay
    runtime/    userspace renderer and touch reader, Rust
    scripts/    installer

They share nothing but the hardware. Different toolchains, different build
systems, different outputs. Each has its own README and builds on its own.

They are also mutually exclusive at runtime. Loading a display overlay on
spi0 cs0 disables the spidev node, so `/dev/spidev0.0` disappears and the
runtime cannot open it. Either the kernel owns the panel, or userspace does.
Building both is fine; running both on the same boot is not.

    make kernel
    make runtime

or work in either subtree directly.

## Install

On the target:

    curl -fsSL https://raw.githubusercontent.com/foonerd/rpi-waveshare28/main/scripts/install.sh | sudo bash -s runtime

`runtime`, `kernel` or `both`. Artefacts are fetched from GitHub Releases and
verified against their published sha256 before installation.

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

Only musl runtime binaries are published. They are statically linked and run
on Buster, Bookworm and Trixie alike, which covers Volumio 3 and Volumio 4.
The glibc targets build but are not published: cross's stock images are glibc
2.31, which excludes Buster.

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

All `dtparam` and `dtoverlay` lines go in `/boot/userconfig.txt`. Never edit
`config.txt` or `volumioconfig.txt`; they are system managed and OTA
overwrites them.

Keep every line under 100 characters. The firmware silently truncates longer
lines, dropping the tail of the last parameter, with no warning anywhere. A
`speed=32000000` at the end of a 100 character line arrives as 3200000 and the
panel runs at a tenth of the intended clock. Split parameters across following
`dtparam=` lines instead:

    dtparam=spi=on
    dtoverlay=fbtft,spi0-0,st7789v
    dtparam=width=240,height=320
    dtparam=reset_pin=27,dc_pin=25
    dtparam=led_pin=18,speed=32000000

I2C is already enabled on Volumio. SPI is not.

## Known constraints

The fbtft backlight device at `/sys/class/backlight/fb_st7789v/` reports
`max_brightness` of 0. It is a GPIO backlight with no usable range: the only
writable value is 0 and anything else returns EINVAL. Control GPIO18 directly
if you need the backlight.

There is no DRM device when driving this panel via fbtft, so X requires the
`fbdev` driver with explicit `Device`, `Screen` and `ServerLayout` sections.
Without a `Screen` section naming the device, autoconfiguration discards it
with `Screen 0 deleted because of no matching config section`.

Console does not follow the panel automatically. `con2fbmap 1 1` moves it at
runtime; `fbcon=map:1` on the kernel command line makes it stick, but
`cmdline.txt` is build managed on Volumio and OTA will overwrite it.

## Licence

Apache-2.0, except `kernel/source_files/*/overlays/*.dts`, which is
`GPL-2.0 OR MIT` following the upstream device tree convention so the overlay
stays eligible for submission to the Raspberry Pi kernel tree.
