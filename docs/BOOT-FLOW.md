# Boot and status flow

Design proposal. Not implemented. Written before code because the previous
attempt bricked the device twice, and the failures were in sequencing rather
than in logic.

---

## Requirement

From power on, the panel shows the host's addresses, updating as they change,
until Volumio's node process answers. Then it shows the player.

If there is no LAN, it shows the hotspot SSID and address instead.

All addresses are shown, IPv4 and IPv6, excluding those nobody can use.

None of this may block or delay system startup, and a failure in any part of
it must not prevent the device from booting.

Everything is rotation aware.

---

## What is established, and how

| Fact | Evidence |
|---|---|
| `192.168.211.1` on a wireless interface means hotspot mode | `volumio3-backend/.../network/network_monitor.sh` treats it as not connected |
| A firmware-applied overlay cannot be removed at runtime | `/sys/kernel/config/device-tree/overlays/` empty on a clean boot; `dtoverlay -l` reports none |
| A runtime-applied overlay round-trips cleanly | observed: apply creates `fb1` and removes `spidev0.0`; remove reverses it |
| The recreated spidev node needs a udev re-trigger | udev logs `Preserve permissions`; `udevadm trigger --subsystem-match=spidev` restores `root:spi 0660` |
| Plymouth will not adopt a later framebuffer | `plymouth-start` at 20:56:36, `fb1` at 20:56:37, nothing drawn until quit at 20:57:13 |
| Volumio's API answers in 5 to 11 ms once up | measured with `curl -w '%{time_total}'`, ten samples |
| Boot to `graphical.target` is ~16 s userspace | `systemd-analyze` |

Not established, needs one check on a device in hotspot mode:

- Nothing. All three previously open questions were answered from the source
  trees; see below.

### Network state is already published, before node starts

`volumio-os/volumio/bin/wireless.js` maintains state files specifically as a
notifier, and `wireless.service` is ordered `Before=volumio.service`. So these
are written during exactly the window this display has to fill.

| File | Content | Written by |
|---|---|---|
| `/tmp/networkstatus` | `ap`, `hotspot` or `offline` | `wstatus()`, plus `utimesSync` on every update to trigger watchers |
| `/data/wlan0status` | `connected`, `hotspot` or `disconnected` | `updateNetworkState()` |
| `/data/eth0status` | `connected` or `disconnected` | watched and written by the same daemon |

The comment on `refreshNetworkStatusFile()` is explicit: the mtime is touched
to trigger a watch. This is a push notification channel that already exists,
is maintained by the same author as this repository, and is authoritative.

Polling `getifaddrs` on a timer would be second-guessing it.

### The hotspot SSID comes from the file hostapd actually reads

`volumio3-backend/.../network/index.js`, `rebuildHotspotConfig()` writes
`ssid=<hotspot_name>` into `/etc/hostapd/hostapd.conf`, defaulting to
`Volumio`. That file is what hostapd loads, so reading `ssid=` from it matches
what is being broadcast regardless of who set it or when.

Note it is the backend that writes it, and the backend is not running during
the window this display covers. That does not matter: hostapd is already
running with whatever the file says, so the file and the broadcast agree.

---

## Not blocking startup

This is the part that matters most, because it is the part that failed.

### The service is a leaf

Nothing in the system depends on it, and it depends only on things that are
guaranteed to exist.

    [Unit]
    After=local-fs.target

    [Install]
    WantedBy=multi-user.target

No `Before=`. No `RequiresMountsFor=`. No `DefaultDependencies=no`. No `Wants=`
on any other unit of ours.

Rationale, from the two failures:

- `DefaultDependencies=no` + `Before=basic.target` + `RequiresMountsFor=/boot`
  is a cycle, because the mount is ordered as part of `basic.target`. systemd
  broke it by deleting the job and the unit silently never ran.
- `After=local-fs.target` on a service with `Wants=` on a unit that is
  `WantedBy=sysinit.target` wedged the boot entirely: ping responded, nothing
  else did.

The pattern to copy is Volumio's own: `allo_relay_attenuator` uses
`After=local-fs.target` and nothing else, and waits for readiness in its own
loop.

### Bounded failure

    TimeoutStartSec=30
    Restart=on-failure
    RestartSec=5
    StartLimitIntervalSec=120
    StartLimitBurst=5

A crash loop stops after five attempts in two minutes rather than restarting
forever. `TimeoutStartSec` bounds the handover, which shells out to
`dtoverlay` and `udevadm` and could in principle hang.

The handover script additionally wraps each external call in `timeout 10`, so
a hung `udevadm settle` cannot consume the whole start timeout.

### Never persist an unverified unit

The configurator does not `systemctl enable` and hope. It:

1. writes the unit
2. runs `systemd-analyze verify` on it, and refuses to continue on failure
3. `systemctl start` and waits for the service to reach `active (running)`
4. only then `systemctl enable`

A unit that cannot start in the current boot is never made persistent, so the
failure is visible immediately on a machine you still have a shell on, rather
than on the next boot when you may not.

This is the single change that would have prevented tonight's brick.

### Recovery path

`waveshare28-config recover` disables and removes both units and reloads
systemd, for the case where the device still boots but the panel service is
misbehaving.

For the case where it does not boot, `DEFECTS.md` documents the SD card
procedure: the units live on the data partition under `/dyn/etc/systemd/`.

---

## Address model

### What is shown

All addresses a person could actually use, ordered by how likely they are to
want them.

Included:

- IPv4 global unicast
- IPv6 global unicast
- IPv6 unique local, `fc00::/7`

Excluded, with reasons:

- IPv4 loopback `127.0.0.0/8` and IPv6 `::1` - not reachable from elsewhere
- IPv4 link-local `169.254.0.0/16` - means DHCP failed; showing it implies a
  working address when there is not one
- IPv6 link-local `fe80::/10` - requires a scope identifier to be usable and
  is meaningless typed into a browser
- Interfaces that are not operationally up, per RFC 2863 state

`std::net::Ipv4Addr` and `Ipv6Addr` provide `is_loopback`, `is_link_local`,
`is_unique_local` and `is_unicast_link_local`, so the classification is
standard library rather than hand-rolled prefix matching.

### Ordering

1. Wired interfaces before wireless before anything else
2. Within an interface, IPv4 before IPv6

Because someone squinting at a 2.8 inch panel wants the address they will
type, and that is the wired IPv4 one if it exists.

### Hostname

Shown as `hostname.local`, because avahi is running and that is what most
people will actually type. The bare hostname is not useful off the box.

### Fitting IPv6

A full IPv6 address is up to 39 characters. At 6 pixels per character that is
234 pixels.

- Landscape frame is 320 wide: fits on one line with room to spare
- Portrait frame is 240 wide: 40 characters, so it just fits

So the status screen uses the full frame rather than the player's text column,
which is only 132 wide. This is why `Layout` gains an explicit `frame`
rectangle rather than the status screen deriving one from the transport strip,
which is what the discarded implementation did.

---

## Hotspot

Detected from `/data/wlan0status` containing `hotspot`, which is what
`wireless.js` writes in AP mode. `192.168.211.1` on a wireless interface is a
corroborating signal and is what Volumio's own `network_monitor.sh` uses, but
the status file is the daemon's own statement of intent and is preferred.

When in hotspot mode the screen shows:

    <hostname>
    Wi-Fi setup
    <SSID>
    192.168.211.1

rather than the address list, because in that state the address is not
something to connect to over an existing network, it is an instruction: join
this network, then open this address.

SSID from `ssid=` in `/etc/hostapd/hostapd.conf`, falling back to `Volumio` if
the file is unreadable.

---

## Readiness and the transition

The renderer polls `getState` at the configured interval from the moment it
starts. It does not wait for `volumio.service`.

- Before the first successful poll: status screen
- On the first success: full player draw, and address polling stops
- After that: the player screen stays, even if polls start failing

The asymmetry is deliberate. A failing poll during a Volumio restart is
transient, and reverting to an address screen mid-listening would be worse
than showing a slightly stale player.

Address polling stops after readiness because it is only there to serve the
pre-setup case, and there is no reason to pay for it forever.

---

## Change detection

Watch `/tmp/networkstatus` for modification, and re-read addresses when it
changes.

The file is maintained by `wireless.js` precisely as a notifier: every state
change writes the content and touches the mtime, and the source comments say
so. `wireless.service` is `Before=volumio.service`, so it is running and
publishing throughout the window this display exists to fill.

This is better than polling on three counts. It is push rather than poll, so
an address change appears immediately rather than up to a second later. It is
authoritative, because it reflects what the daemon that manages the network
believes rather than what a snapshot of the kernel happens to show mid
transition. And it costs nothing while idle.

Implementation: poll the mtime of `/tmp/networkstatus` on the existing loop
tick rather than using inotify. A `stat` is cheaper than the inotify plumbing,
the loop already wakes every 40 ms for touch, and it avoids a dependency. The
distinction that matters is watching a state signal rather than enumerating
addresses on a timer; the mechanism for noticing the signal is incidental.

Addresses themselves still come from `if-addrs`, but only when the signal
fires or on first draw, not on a timer.

Fallback: if `/tmp/networkstatus` does not exist, fall back to re-reading
addresses every two seconds. An older Volumio, or a rebuilt image without the
notifier, should degrade rather than show nothing.

---

## Rotation

`Layout` gains `frame: Rectangle` covering the whole rotated frame. The status
screen composes into it via the existing `RowBuf` and blits once, the same
path the text pane uses, for the same reason: drawing onto a clipped view of
the panel forces per-pixel addressing and flickers.

Touch is already mapped back through rotation in `Layout::map`. The status
screen has no touch targets, so nothing further is needed.

---

## Honest answers to the quality questions

**Engineering excellence?** In two places. The start-then-enable sequence with
a `systemd-analyze verify` gate is a real improvement over the usual practice
of enabling and hoping. Using the standard library's address classification
rather than hand-written prefix checks is the correct instinct. The rest is
competent, not exceptional.

**Versatile and robust?** The address model handles dual stack, multiple
interfaces, hotspot fallback and the no-address case without special-casing.
The failure model bounds every external call and every restart.

**Enterprise grade?** The parts that matter for that are the bounded failure,
the refusal to persist an unverified unit, and a documented recovery path.
Those are present.

**Outside the box?** One idea: never make a unit persistent until it has been
observed to start. That is not novel in principle but it is rarely done, and
it directly addresses the failure mode that cost a reflash tonight.

**Next generation, never done before?** No. This is careful application of
known practice. Claiming otherwise would be dishonest; a status screen and a
systemd unit are not new ground. What is unusual is the constraint set: one
SPI chip select shared with a boot splash that cannot share it, on a board
where a bad unit costs a reflash.

**Facts not guesswork?** Mostly. The table above separates what is measured
from what is assumed, and there is exactly one open assumption, flagged.
Everything about overlay lifetime, Plymouth binding, udev permissions and API
timing was measured on the device tonight rather than reasoned about.

---

## Open questions before implementation

1. Whether the splash unit is kept at all. It cannot work, because Plymouth
   binds before the framebuffer exists. Keeping it costs an fbtft load and
   unload for nothing. Removing it means no branding until the initramfs
   question is answered in `volumio-os`.
2. Whether the panel should show anything between the renderer starting and
   the first address appearing. Proposed: hostname plus "waiting for network",
   which is at least evidence the board is alive.