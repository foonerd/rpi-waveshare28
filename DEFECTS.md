# Defects and platform constraints

Working notes. Findings that cost real time to establish, and open defects
that are not yet fixed. Each entry says what is established by evidence and
what is still assumption.

---

## Platform constraints

These are properties of the hardware and of Volumio, not of this code. They
constrain any design and are expensive to rediscover.

### One SPI chip select, one owner

The panel is wired to spi0 cs0. fbtft and spidev cannot both have it.

The SPI core refuses a second device on a claimed chip select:
`drivers/spi/spi.c`, `spi_dev_check_cs`, logs `chipselect %u already in use`.
So an overlay that leaves `spidev@0` enabled while adding an fbtft device
does not help; one of the two fails to register.

Established: read from kernel source, and observed as `/dev/spidev0.0`
disappearing whenever the fbtft overlay is loaded.

### A firmware-applied overlay cannot be removed at runtime

An overlay in `/boot/userconfig.txt` is merged into the device tree by the
firmware before the kernel starts. Nothing remains at runtime to remove.

Observed on a clean boot with fbtft in `userconfig.txt`:

    $ ls /sys/kernel/config/device-tree/overlays/
    (empty)
    $ dtoverlay -l
    No overlays loaded
    $ sudo dtoverlay -r
    * No overlays loaded

`spidev@0` stays `status = "disabled"` for the life of the boot.

Consequence: if fbtft is in `userconfig.txt`, the userspace renderer can
never open `/dev/spidev0.0`, on that boot or any other.

### An overlay applied at runtime IS removable

The same overlay applied with the tool round-trips cleanly:

    $ sudo dtoverlay fbtft spi0-0 st7789v width=240 height=320 \
        reset_pin=27 dc_pin=25 led_pin=18 speed=32000000 rotate=90
    -> /dev/fb1 appears, /dev/spidev0.0 disappears
    $ sudo dtoverlay -r fbtft
    -> /dev/fb1 disappears, /dev/spidev0.0 reappears

The kernel logs `OF: overlay: WARNING: memory leak will occur if overlay
removed` for the two `status` properties it modifies. That is a few dozen
bytes per cycle and is not a problem for a once-per-boot handover.

### The recreated spidev node has wrong permissions

After removing the overlay, `/dev/spidev0.0` comes back as `root:root 0600`
rather than `root:spi 0660`, so a process running as `volumio` cannot open
it.

The udev rule is correct (`/etc/udev/rules.d/99-com.rules:3`,
`SUBSYSTEM=="spidev", GROUP="spi", MODE="0660"`) and `udevadm test` shows it
matching. The message is `Preserve permissions of /dev/spidev0.0`: udev keeps
existing permissions on a device it has seen before.

Fix: `udevadm trigger --subsystem-match=spidev` after the removal, then
`udevadm settle`. Verified to restore `root:spi 0660`.

### Plymouth does not adopt a framebuffer created after it starts

plymouthd starts in the initramfs. A framebuffer that appears later is
invisible to it.

Observed: `plymouth-start.service` at 20:56:36, splash unit created `fb1` at
20:56:37, `plymouth-quit` at 20:57:13. Nothing was drawn on the panel for
those 37 seconds.

Consequence: a boot splash on this panel requires fbtft to exist before the
initramfs plymouth hook runs. That means either `userconfig.txt`, which makes
the overlay unremovable and rules out the renderer, or applying it from
inside the initramfs, which means a change to `volumio-os` and its
`volumio.initrd`, not something this repository or an installer can do.

Open question, untested: whether `dtoverlay` or a direct configfs write works
from an init-premount hook. `volumio-plymouth-adaptive` already ships such a
hook, so the mechanism exists.

### Config lines are silently truncated past 100 characters

The firmware truncates long lines in `config.txt` and its includes, dropping
the tail with no warning anywhere.

Observed: `speed=40000000` at the end of a 100 character `dtoverlay=fbtft,...`
line arrived as `4000000`, and `speed=16000000` as `1600000`. The device tree
node held the truncated value; every earlier parameter on the line applied
correctly.

Fix: keep lines under 100 characters, splitting parameters across following
`dtparam=` lines.

### Kernel modules must match the buildbot toolchain

`bcm2709_defconfig` sets `CONFIG_MODVERSIONS=y`, so symbol CRCs are checked at
load. Building the module means building the whole kernel for `Module.symvers`,
and Kconfig evaluates its `CC_HAS_*` symbols against the actual compiler, so a
different GCC silently produces a different `.config` and different CRCs.

The compiler is not in the vermagic string, so this presents as
`disagrees about version of symbol module_layout`, not as a vermagic mismatch.

`rpi-firmware/uname_string` records the buildbot toolchain per commit. For
every 6.6 and 6.12 kernel checked it is GCC 11.4.0 from Ubuntu 22.04 with
binutils 2.38. `kernel/build-script/download_build.sh` verifies this and
refuses to start on mismatch.

### Console font at 320x240

fbcon picks 8x8 at this size, giving 40x30. The QR in `/etc/issue` uses
half-block characters, so one cell carries two vertical modules and one
horizontal; at 8x8 each module is 8 wide by 4 tall and the code is stretched
two to one. `fbcon=font:VGA8x16` forces 8x16, giving 40x15 and square modules.

`kbd` is not available in the Volumio repositories, so `setfont` is not an
option. The font is built into the kernel.

Both console settings live in `cmdline.txt`:

    fbcon=map:1 fbcon=font:VGA8x16

`cmdline.txt` is inside the OTA kernel tar, so it is replaced when an update
ships one. Not every update does.

### Volumio's /albumart does not proxy remote art

`http://localhost:3000/albumart?url=<encoded>` looked like a proxy and is not.
It returns the default artwork regardless of the `url` parameter, encoded or
not, and returns JPEG even when the source is PNG. Both encoded and unencoded
requests returned byte-identical files.

The browser UI shows station logos because the browser loads the absolute URL
itself. Stream art is served over https, which is why the renderer carries
TLS.

---

## Open defects

### Volumio's network notifier is not always present

`docs/BOOT-FLOW.md` treats `/tmp/networkstatus` as a reliable push channel.
On a running Volumio 4.194 box it did not exist at all:

    $ cat /tmp/networkstatus
    cat: /tmp/networkstatus: No such file or directory

`wstatus()` in `wireless.js` is only called from `updateNetworkState()`, so if
that path has not run this boot the file is never created. The renderer falls
back to a two second timer, which is why the symptom was invisible, but the
change detection is running on the fallback rather than the intended path.

Open: whether it appears on a box that goes through a wireless state change,
or whether the code path is unreachable in normal operation.

### /data/wlan0status persists across boots and goes stale

Observed: `hotspot` while `wlan0` was DOWN with no address and `eth0` held a
normal LAN address.

It is on the data partition, so it survives reboots, and nothing clears it
when the mode it describes ends. It is a record of the last state the daemon
set, not a statement about now.

The renderer therefore treats real addresses as authoritative and uses the
status file only when there are none.

### Repeated album art fetches

rustls handshake logs appeared roughly twice a second for a station whose
`albumart` URL was not changing. `ArtLoader::request` deduplicates on the path
and the logic reads as correct, so the cause is not identified.

Not reproduced since. Needs `RUST_LOG=info,waveshare28_panel=debug` output
while a stream with a remote logo is playing.

### Intermittent state poll failures

`Resource temporarily unavailable (os error 11)` still appears after the
`Content-Length` fix, which was expected to eliminate it. An strace of a
working period showed clean single-`recvfrom` exchanges at 2 to 8 ms, so the
failing case was not captured.

Needs an strace that covers a period where the warnings actually occur.

### Volume slider and progress bar touch zones overlap

`hit` gives the volume slider 10 pixels of vertical slop either side. In
landscape the slider is at y 178 and the progress bar at y 190, so the zones
overlap by four pixels and the slider is tested first.

Harmless today because the progress bar is display-only. Becomes a defect the
moment seeking is added.

---

## Things that broke the device

Recorded so they are not repeated.

### systemd ordering cycles in the early boot targets

Two separate attempts wedged the boot.

`DefaultDependencies=no` with `Before=basic.target` and
`RequiresMountsFor=/boot`: the mount is ordered as part of `basic.target`, so
the unit must run both before and after it. systemd broke the cycle by
deleting the job and the unit never ran.

`After=local-fs.target` on a service with `Wants=` on a unit that is
`WantedBy=sysinit.target`: the box booted far enough to answer ping and no
further. No SSH, no console, no UI. Recovered by reflashing.

Before rebooting after any unit change:

    systemd-analyze verify /etc/systemd/system/<unit>.service

and do not combine an ordering change with an enablement change in the same
reboot.
