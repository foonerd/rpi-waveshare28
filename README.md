# rpi-waveshare28

Raspberry Pi support for the Waveshare 2.8 inch SPI LCD (SKU 27579): ST7789V
display, CST328 touch, kernel module and overlay builds, and a lightweight
Rust renderer.

Standalone. Not part of any Volumio or evo release stream.

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

## Two paths, mutually exclusive

The panel can be driven by the kernel or by userspace, not both. Loading a
display overlay on spi0 cs0 disables the spidev node, so `/dev/spidev0.0`
disappears and the userspace renderer cannot open it. Pick one per boot.

### Kernel path

Display works with no build from this repository. Both drivers are already
enabled in the stock Raspberry Pi defconfigs:

- `CONFIG_FB_TFT_ST7789V=m`, via the `fbtft` overlay
- `CONFIG_DRM_PANEL_MIPI_DBI=m`, via the `mipi-dbi-spi` overlay, which needs
  a `/lib/firmware/panel.bin` built with `mipi-dbi-cmd`

Touch does not. `drivers/input/touchscreen/hynitron_cstxxx.c` exists in the
Raspberry Pi kernel tree but `CONFIG_TOUCHSCREEN_HYNITRON_CSTXXX` is not set
in any Pi defconfig, on 6.1, 6.6 or 6.12. No module ships. There is also no
hynitron or CST overlay upstream. That gap is what `build-script/` fills.

### Userspace path

The Rust renderer drives the ST7789V over `/dev/spidev0.0` and reads the
CST328 over i2cdev in the same process. No display overlay, no kernel touch
module, no X, no compositor. Needs only `dtparam=spi=on`.

This is the path that suits memory-constrained boards. A browser kiosk is not
viable at 240x320 on 512 MB.

## Kernel build

Cross-compiles. Requires `gcc-arm-linux-gnueabihf` and `gcc-aarch64-linux-gnu`.
Do not attempt this on the target.

    cd build-script
    ./download_build.sh

Set `KERNEL_VERSION` at the top to match the target. The version to commit map
follows the same convention as volumio-rpi-custom; add entries as needed from
`raspberrypi/rpi-firmware`.

`re-build.sh` skips the download and patch steps and rebuilds from the trees
already on disk.

Output lands in `output/` as `cst328-rpi-<version>.tar.gz` with md5 and sha1.

Install on target:

    tar -xzf cst328-rpi-6.12.75.tar.gz -C /
    depmod -a

## Binding

The in-tree driver declares only `hynitron,cst340`. The overlay binds to that
string.

Per the CST328 datasheet the register map is the same one the driver uses:
`0xD1FC` holds the `0xCACA` firmware verification code the driver tests at
probe, `0xD000` onward is the touch report in the layout the driver parses,
`0xD006` is the fixed `0xAB` marker, `0xD1` command writes select the debug
and normal reporting modes.

That is a paper comparison. If probe fails with `ic mismatch, chkcode is ...`
then the check code differs on real silicon and a `hynitron,cst328` entry has
to be added to the driver. See `patch/README.md`.

## Boot configuration

All `dtparam` and `dtoverlay` lines go in `/boot/userconfig.txt`. Never edit
`config.txt` or `volumioconfig.txt`; they are system managed and OTA
overwrites them.

Keep every line under 100 characters. The firmware silently truncates longer
lines, dropping the tail of the last parameter, with no warning anywhere.
Split parameters across following `dtparam=` lines instead:

    dtparam=spi=on
    dtoverlay=fbtft,spi0-0,st7789v
    dtparam=width=240,height=320
    dtparam=reset_pin=27,dc_pin=25
    dtparam=led_pin=18,speed=32000000

I2C is already enabled on Volumio via `dtparam=i2c_arm=on` in
`volumioconfig.txt`. SPI is not.

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

Touch orientation is independent of display rotation. Rotating the
framebuffer with the fbtft `rotate` parameter does not rotate the input
device. Use `invx`, `invy` and `swapxy` to match the two, and confirm with
`evtest` rather than by eye.

## Verify

    modinfo hynitron_cstxxx
    dmesg | grep -i hynitron
    ls /dev/input/event*
    evtest

A successful probe registers an input device. A failed identity check logs
`ic mismatch, chkcode is ...` and returns -ENODEV.

## Known constraints

The fbtft backlight device at `/sys/class/backlight/fb_st7789v/` reports
`max_brightness` of 0. It is a GPIO backlight with no usable range; the only
writable value is 0 and anything else returns EINVAL. Control the backlight
via GPIO18 directly if needed.

There is no DRM device when driving this panel via fbtft, so X requires the
`fbdev` driver with an explicit `Device`, `Screen` and `ServerLayout` section.
Without a `Screen` section naming the device, autoconfiguration discards it
with `Screen 0 deleted because of no matching config section`.

## Licence

Apache-2.0, except `source_files/*/overlays/*.dts`, which is
`GPL-2.0 OR MIT` following the upstream device tree convention so the overlay
stays eligible for submission to the Raspberry Pi kernel tree.
