# kernel

CST328 touch support for Raspberry Pi kernels: an out-of-tree build of
`hynitron_cstxxx` plus a device tree overlay.

Independent of `runtime/`. Nothing is shared between them.

## Why this exists

The display needs nothing from here. Both kernel display drivers are already
enabled in the stock Raspberry Pi defconfigs:

- `CONFIG_FB_TFT_ST7789V=m`, via the `fbtft` overlay
- `CONFIG_DRM_PANEL_MIPI_DBI=m`, via the `mipi-dbi-spi` overlay, which needs a
  `/lib/firmware/panel.bin` built with `mipi-dbi-cmd`

Touch does not. `drivers/input/touchscreen/hynitron_cstxxx.c` is present in the
Raspberry Pi kernel tree but `CONFIG_TOUCHSCREEN_HYNITRON_CSTXXX` is not set in
any Pi defconfig, on 6.1, 6.6 or 6.12. No module ships. There is also no
hynitron or CST overlay upstream.

Only needed if you want a kernel input device. The userspace renderer in
`runtime/` reads the CST328 over i2cdev and needs none of this.

## Layout

    build-script/   download, patch and cross-compile
    source_files/   overlay sources, per kernel version
    patch/          reserved for a carried driver patch
    output/         packaged tarballs (gitignored)

## Build

Cross-compiles. Requires `gcc-arm-linux-gnueabihf` and `gcc-aarch64-linux-gnu`.
Do not attempt this on the target.

    cd build-script
    ./download_build.sh

Set `KERNEL_VERSION` at the top to match the target. The version to commit map
follows the same convention as volumio-rpi-custom; add entries as needed from
`raspberrypi/rpi-firmware`.

`re-build.sh` skips the download and patch steps and rebuilds from the trees
already on disk. Use it after editing the overlay or driver source, having
copied the change into the trees.

Output lands in `output/` as `cst328-rpi-<version>.tar.gz` with md5 and sha1.

## Install on target

    tar -xzf cst328-rpi-6.12.75.tar.gz -C /
    depmod -a

Then add to `/boot/userconfig.txt`:

    dtoverlay=cst328

## Binding

The in-tree driver declares only `hynitron,cst340`. The overlay binds to that
string.

Per the CST328 datasheet the register map is the same one the driver uses:
`0xD1FC` holds the `0xCACA` firmware verification code the driver tests at
probe, `0xD000` onward is the touch report in the layout the driver parses,
`0xD006` is the fixed `0xAB` marker, `0xD1` command writes select the debug and
normal reporting modes.

That is a paper comparison. If probe fails with `ic mismatch, chkcode is ...`
then the check code differs on real silicon and a `hynitron,cst328` entry has
to be added to the driver. See `patch/README.md`.

## Overlay parameters

    addr        I2C address, default 0x1a
    int_pin     interrupt GPIO, default 4
    rst_pin     reset GPIO, default 17
    sizex       touchscreen-size-x, default 240
    sizey       touchscreen-size-y, default 320
    invx        invert X
    invy        invert Y
    swapxy      swap X and Y

## Orientation

Touch orientation is independent of display rotation. Rotating the framebuffer
with the fbtft `rotate` parameter does not rotate the input device. Use `invx`,
`invy` and `swapxy` to match the two, and confirm with `evtest` rather than by
eye.

## Verify

    modinfo hynitron_cstxxx
    dmesg | grep -i hynitron
    ls /dev/input/event*
    evtest

A successful probe registers an input device. A failed identity check logs
`ic mismatch, chkcode is ...` and returns -ENODEV.

## Licence

Overlay sources under `source_files/` are `GPL-2.0 OR MIT`, following the
upstream device tree convention, so they stay eligible for submission to the
Raspberry Pi kernel tree. Everything else here is Apache-2.0 with the rest of
the repository.
