# runtime

Userspace renderer and touch reader for the Waveshare 2.8 inch SPI LCD.

Drives the ST7789V directly over `/dev/spidev0.0` and reads the CST328 over
`/dev/i2c-1`. No X, no DRM, no compositor, no kernel display or input driver.

Independent of `kernel/`. Nothing is shared between them.

## Why not a browser kiosk

Measured on a Pi 3A+ with 512 MB: Chromium reports the configuration as
unsupported and the Volumio UI is not usable at 240x320 regardless. The Touch
Display plugin additionally assumes a KMS device in three places (xorg config,
xrandr, backlight), none of which hold for an SPI panel.

This binary is around 2 MB resident against Chromium's hundreds.

## Mutually exclusive with the kernel display path

One SPI chip select, one owner. An fbtft or mipi-dbi overlay on spi0 cs0
disables the spidev node, and the SPI core refuses a second device on a
claimed chip select.

An overlay in `/boot/userconfig.txt` is merged into the device tree by the
firmware before the kernel starts, so it cannot be removed at runtime and the
renderer can never take the panel on that boot. `waveshare28-config` removes
such lines for this reason.

Requires only:

    dtparam=spi=on

I2C is already enabled on Volumio via `dtparam=i2c_arm=on` in
`volumioconfig.txt`.

## What it shows

Two screens.

Before Volumio's node process answers, the panel shows the hostname and the
host's addresses, updating as they are assigned. If the wireless daemon has
fallen back to an access point and there is no other address, it shows the
SSID and `192.168.211.1` instead, which is an instruction rather than an
address list: join this network, then open this address.

This exists because the renderer starts early and deliberately does not wait
for `volumio.service`. Without it the panel is dark for most of a minute, and
the address is the one thing someone needs before the player is reachable.

After the first successful state poll it shows the player: album art, wrapped
title, artist and album, a volume slider, a progress bar and a transport
strip. It never returns to the status screen. A failed poll during a Volumio
restart is transient, and reverting to an address list mid-listening would be
worse than a slightly stale player.

## Layout

    src/main.rs     process wiring, the poll loop, and the status transition
    src/config.rs   pin map and behaviour, defaults match SKU 27579
    src/display.rs  panel bring-up and the drawing surface
    src/input.rs    touch thread, blocked on a GPIO edge event
    src/touch.rs    CST328 reader
    src/net.rs      host addresses and network state
    src/state.rs    Volumio state polling and commands
    src/art.rs      album art fetch, decode and scale, on its own thread
    src/ui.rs       layouts, drawing, hit regions
    src/http.rs     minimal GET over TCP or TLS, no HTTP crate

## Build

    make            # validate, then the musl targets
    make validate   # fmt, clippy and tests, no build
    make fmt-fix    # apply rustfmt
    make gnu        # glibc targets
    make dist       # musl binaries plus SHA256SUMS into dist/

Targets:

    arm-unknown-linux-musleabihf        ARMv6: Pi Zero, Zero W, Pi 1
    armv7-unknown-linux-musleabihf      ARMv7
    aarch64-unknown-linux-musl          ARM64
    armv7-unknown-linux-gnueabihf       not published
    aarch64-unknown-linux-gnu           not published

ARMv6 and ARMv7 are separate targets and must not be conflated. Volumio builds
a `+` kernel variant from `bcmrpi_defconfig` for the Zero and Pi 1, and an
armv7 binary dies there with SIGILL at the first ARMv7-only instruction.

musl binaries are statically linked and run on Buster, Bookworm and Trixie,
which matters because Volumio 3 is Buster. The glibc pair builds in cross's
stock images (Ubuntu 20.04, glibc 2.31) and runs on Bookworm and Trixie but
not Buster, because glibc compatibility is forward only.

No custom Docker images. This crate has no C dependency: it talks to spidev
and i2cdev through ioctls and the wrapping crates are pure Rust. That is the
difference from evo-device-volumio, which pins trixie images because
`libasound2-dev` must match the runtime symbol version.

`make` runs `validate` first. Cross-compiling three targets to discover a type
error on the third wastes minutes on something `cargo check` reports in
seconds.

## Dependency policy

No HTTP client crate. `ureq` and `reqwest` both pull `url` into `idna` into the
`icu_*` normalisation stack, which also raises the toolchain floor. Carrying a
Unicode library to fetch JSON from loopback is not a trade worth making on a
512 MB board. `src/http.rs` is a small GET instead, with rustls under it for
the https artwork case.

Trust anchors come from `webpki-roots`, compiled in. A statically linked binary
cannot assume a system certificate store exists, and Volumio images do not
necessarily ship `ca-certificates`.

`mipidsi` must be 0.10 or later: `interface::SpiInterface` and the
`Builder::new(model, di)` form do not exist in 0.9.

`resvg` with default features off. Several BBC stations serve their logo as
SVG, which is a vector document to rasterise rather than an image format to
decode. Turning off default features drops `fontdb`, `rustybuzz` and the
unicode crates; station logos are shapes, and an SVG containing text renders
without the text rather than failing.

The toolchain pin in `rust-toolchain.toml` is a build pin, not an MSRV
declaration. This is a standalone binary with no downstream consumers, so the
floor is whatever the dependencies need.

## Configuration

Defaults match the reference wiring, so no config file is needed.

`/etc/waveshare28-panel.toml`, or a path as the first argument. Generated by
`waveshare28-config` from `/boot/waveshare28.conf`; edits are overwritten.

A missing file means defaults. A file that exists but does not parse is an
error: a misspelled key that is quietly ignored produces a panel that does not
do what the file says for reasons nobody can see.

    rotation = 90          # degrees clockwise: 0, 90, 180, 270
    spi_speed_hz = 32000000

Rotation drives both the panel and the touch mapping. The controller always
reports in its native 240x320 frame regardless of what the display driver was
told, so `Layout::map` applies the inverse transform.

## Status

Runs on hardware. Display, touch, artwork including SVG and TLS, both layouts,
the volume slider and the status screen are all exercised on a Pi 3A+.

Open defects are in `../DEFECTS.md`.

## Notes worth keeping

Established by measurement on real hardware. Do not undo them without new
evidence.

The finger count at `0xD005` lags the per-finger status at `0xD000`. A packet
can report a finger present in the count while the status nibble already says
lifted. Gate on the status nibble.

Pre-polling `0xD005` for readiness before reading the packet buys nothing. In
204 captured events it rejected zero samples, and the latency it added lost
short taps outright. Read the packet on the interrupt and validate it.

Reset timing comes from datasheet 10.5: TRST 0.1 ms pulse, TRON 200 ms
re-initialisation. The 50 ms that circulates in example code is a fourfold
shortfall.

Touch is read on the falling edge of the interrupt, from a thread blocked on a
GPIO edge event. Sampling the level from the main loop missed most presses,
because the controller asserts for a few milliseconds and the loop can be busy
for forty.

Anything animated is composed in memory and blitted in one write. Drawing onto
a `clipped()` view of the panel forces `fill_contiguous` to degrade to
`draw_iter`, which sets an address window per pixel, and it flickers visibly.

Real addresses win over hotspot mode. `/data/wlan0status` persists across
boots and can report `hotspot` while the interface is down, so it is not a
reliable statement of current state on its own.
