# runtime

Userspace renderer and touch reader for the Waveshare 2.8 inch SPI LCD.

Drives the ST7789V directly over `/dev/spidev0.0` and reads the CST328 over
`/dev/i2c-1`. No X, no DRM, no compositor, no kernel display or input driver.

Independent of `kernel/`. Nothing is shared between them.

## Why not a browser kiosk

Measured on a Pi 3A+ with 512 MB: Chromium reports the configuration as
unsupported and the Volumio UI is not usable at 240x320 regardless. The
Touch Display plugin additionally assumes a KMS device in three places (xorg
config, xrandr, backlight), none of which hold for an SPI panel.

This binary is single-digit MB resident against Chromium's hundreds.

## Mutually exclusive with the kernel display path

Loading an fbtft or mipi-dbi overlay on spi0 cs0 disables the spidev node.
`/dev/spidev0.0` will not exist and this process cannot start. Pick one per
boot.

Requires only:

    dtparam=spi=on

in `/boot/userconfig.txt`. I2C is already enabled on Volumio via
`dtparam=i2c_arm=on` in `volumioconfig.txt`.

## Layout

    src/main.rs     process wiring and the poll loop
    src/config.rs   pin map and behaviour, defaults match SKU 27579
    src/display.rs  panel bring-up (pending)
    src/touch.rs    CST328 reader
    src/state.rs    Volumio state polling
    src/ui.rs       layout, drawing, hit regions
    src/http.rs     minimal GET, no HTTP crate

## Build

    make            # musl targets, the default
    make gnu        # glibc targets
    make dist       # musl binaries plus SHA256SUMS into dist/

Targets:

    armv7-unknown-linux-musleabihf
    aarch64-unknown-linux-musl
    armv7-unknown-linux-gnueabihf
    aarch64-unknown-linux-gnu

musl binaries are statically linked and run on Buster, Bookworm and Trixie,
which matters because Volumio 3 is Buster. The glibc pair is built in cross's
stock images (Ubuntu 20.04, glibc 2.31) and runs on Bookworm and Trixie but
not Buster, because glibc compatibility is forward only.

No custom Docker images. This crate has no C dependency: it talks to spidev
and i2cdev through ioctls and the wrapping crates are pure Rust. That is the
difference from evo-device-volumio, which pins trixie images because
`libasound2-dev` must match the runtime symbol version.

## Dependency policy

No HTTP client crate. `ureq` and `reqwest` both pull `url` into `idna` into the
`icu_*` normalisation stack, which also raises the MSRV floor. Carrying a
Unicode library to fetch JSON from loopback is not a trade worth making on a
512 MB board. `src/http.rs` is a ~60 line GET instead.

`mipidsi` must be 0.10 or later: `interface::SpiInterface` and the
`Builder::new(model, di)` form do not exist in 0.9.

## Configuration

Defaults match the reference wiring, so no config file is needed. A TOML file
may be passed as the first argument, default `/etc/waveshare28-panel.toml`.
File loading is not implemented yet; the struct and defaults are.

## Status

Unfinished. `touch.rs`, `state.rs`, `ui.rs` and `http.rs` are complete.
`main.rs` runs the poll loop; SPI and display bring-up is still a commented
block. Nothing here has been run on hardware.

## Notes worth keeping

Two behaviours were established by measurement on real hardware and are
enforced in `touch.rs`. Do not undo them without new evidence.

The finger count at `0xD005` lags the per-finger status at `0xD000`. A packet
can report a finger present in the count while the status nibble already says
lifted. Gate on the status nibble.

Pre-polling `0xD005` for readiness before reading the packet buys nothing. In
204 captured events it rejected zero samples, and the latency it added lost
short taps outright. Read the packet on the interrupt and validate it.

Reset timing comes from datasheet 10.5: TRST 0.1 ms pulse, TRON 200 ms
re-initialisation. The 50 ms that circulates in example code is a fourfold
shortfall.
