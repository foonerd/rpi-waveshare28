//! Userspace renderer and touch reader for the Waveshare 2.8 inch SPI LCD.
//!
//! Two display backends. `spi` owns `/dev/spidev0.0` and cannot coexist with
//! an fbtft overlay. `framebuffer` draws into the live `fb_st7789v` node
//! while fbtft holds
//! the bus, which is how Plymouth can run in the initramfs and this process
//! can take over after `plymouth-quit`. Touch is always `/dev/i2c-1`.

mod art;
mod config;
mod display;
mod fbdev;
mod http;
mod input;
mod net;
mod state;
mod surface;
mod touch;
mod ui;

use anyhow::{Context, Result};
use embedded_graphics::prelude::Point;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use art::{Art, ArtLoader};
use config::{Config, Strip};
use display::Panel;
use net::{HostInfo, NetMonitor};
use state::{poll_system_status, Command, CommandSink, PlayerState, StateSource, SystemStatus};
use surface::{volume_at, SurfHit, Surface};
use ui::{face_hit, face_text_slot, Hotspot, TextPane, TouchEv};

/// How long the main loop waits for a touch before going round again.
///
/// Also the ticker step interval, so text scrolls at one pixel per tick. A
/// press wakes the loop immediately regardless, because touches arrive on a
/// channel from a thread blocked on a GPIO edge event.
const TICK: Duration = Duration::from_millis(40);
/// How often to ask `/status` while still on the address screen.
const STATUS_POLL: Duration = Duration::from_secs(1);
/// Fall through to `getState` if `/status` never becomes `ready`.
const STATUS_WAIT_CAP: Duration = Duration::from_secs(120);
/// Spinner step on the `starting` footer.
const SPIN_INTERVAL: Duration = Duration::from_millis(400);
const SPIN: [char; 4] = ['|', '/', '-', '\\'];
/// How long a surface stays up with no touch.
const SURFACE_HOLD: Duration = Duration::from_secs(10);
const HOLD_DELAY: Duration = Duration::from_millis(400);
const HOLD_REPEAT: Duration = Duration::from_millis(150);
const MUTE_HOLD: Duration = Duration::from_millis(600);
const TAP_SLOP: i32 = 6;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            // rustls logs every handshake at debug, which drowns this crate's
            // own output and fills the journal once this runs as a service.
            // Narrowing it here rather than in the unit file means
            // RUST_LOG=debug stays useful without anyone having to remember.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rustls=warn".into()),
        )
        .init();

    let cfg_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/waveshare28-panel.toml"));

    let cfg = Config::load(&cfg_path).context("loading configuration")?;
    tracing::info!(?cfg, "starting");

    run(cfg)
}

fn run(cfg: Config) -> Result<()> {
    let poll_interval = Duration::from_millis(cfg.poll_interval_ms);
    // Fixed rather than derived from the poll interval. Measured on a Pi 3A+,
    // getState answers in 5 to 11 ms, so this is generous headroom for a
    // loaded board without being long enough to stack requests.
    let net_timeout = Duration::from_secs(2);

    let source = StateSource::new(&cfg.state_url, net_timeout);
    let commands = CommandSink::new(&cfg.command_url, net_timeout);

    let mut panel = Panel::open(&cfg).context("opening panel")?;
    panel.backlight(true)?;
    let layout = *panel.layout();

    let (tx, rx) = mpsc::channel::<TouchEv>();
    input::spawn(&cfg, layout, tx).context("starting touch input")?;

    // Fetching and decoding a cover is slow enough to stall the ticker and
    // delay a touch, so it runs on its own thread and the picture arrives when
    // it arrives.
    let mut loader = ArtLoader::spawn(cfg.art_base.clone(), panel.art_size(), net_timeout)
        .context("starting album art loader")?;
    let mut art: Option<Art> = None;

    let mut shown: Option<PlayerState> = None;
    let mut current = PlayerState::default();
    let mut next_poll = Instant::now();

    // Until the backend is ready and getState succeeds once, the panel
    // shows host addresses. The renderer starts early and deliberately
    // does not wait for volumio.service: getState answers as soon as
    // Express is up, which is well before plugins finish. /status is
    // the gate; the address is the one thing someone needs before the
    // player is reachable.
    let mut player = false;
    let mut backend_ready = false;
    let wait_from = Instant::now();
    let mut next_status = Instant::now();
    let mut next_spin = Instant::now() + SPIN_INTERVAL;
    let mut spin = 0usize;
    let mut last_footer: Option<String> = None;
    let mut netmon = NetMonitor::default();
    let mut host = HostInfo::default();
    let mut open: Option<Surface> = None;
    let mut surface_until: Option<Instant> = None;
    let mut press: Option<Press> = None;
    let mut scrub: Option<f32> = None;
    let mut strip_view = cfg.strip();
    let mut last_step = Instant::now();
    let mut pane = TextPane::default();
    let mut hold_shown: Option<u8> = None;

    loop {
        if !player {
            // Driven by wireless.js touching /tmp/networkstatus, not by a
            // timer over getifaddrs. See src/net.rs.
            let mut dirty = false;
            if let Some(now) = netmon.poll() {
                if now != host {
                    host = now;
                    dirty = true;
                }
            }

            if !backend_ready && Instant::now() >= next_status {
                if poll_system_status(&cfg.status_url, net_timeout) == Some(SystemStatus::Ready) {
                    tracing::info!("backend ready, waiting for player state");
                    backend_ready = true;
                    next_poll = Instant::now();
                }
                next_status = Instant::now() + STATUS_POLL;
                if !backend_ready && wait_from.elapsed() >= STATUS_WAIT_CAP {
                    tracing::warn!(
                        secs = STATUS_WAIT_CAP.as_secs(),
                        "backend still not ready, falling through to getState"
                    );
                    backend_ready = true;
                    next_poll = Instant::now();
                }
            }

            let want_spin = host.has_address() && !backend_ready;
            let spin_due = want_spin && Instant::now() >= next_spin;
            if spin_due {
                spin = (spin + 1) % SPIN.len();
                next_spin = Instant::now() + SPIN_INTERVAL;
            }
            let footer = want_spin.then(|| format!("starting {}", SPIN[spin]));
            if dirty || spin_due || footer != last_footer {
                panel.render_status(&host, footer.as_deref())?;
                last_footer = footer;
            }
        }

        if Instant::now() >= next_poll {
            if backend_ready {
                match source.poll() {
                    Ok(s) => {
                        // First success: leave the status screen. `shown` is
                        // cleared so the next pass draws the player in full,
                        // since none of it is on screen yet.
                        if !player {
                            tracing::info!("player available, switching from status screen");
                            player = true;
                            shown = None;
                        }
                        current = s;
                    }
                    // A failed poll keeps whatever is on screen. Once the
                    // player has been seen once the status screen is never
                    // shown again: a failure during a Volumio restart is
                    // transient, and reverting to an address list mid-
                    // listening would be worse than a slightly stale player.
                    Err(e) => tracing::warn!(error = %e, "state poll failed"),
                }
            }
            next_poll = Instant::now() + poll_interval;
        }

        if !player {
            // Drain touches so the channel cannot fill, and use the same
            // blocking wait as the player path so the loop is not spinning.
            if let Ok(action) = rx.recv_timeout(TICK) {
                tracing::debug!(?action, "touch before player ready, ignored");
            }
            continue;
        }

        if let Some(now) = netmon.poll() {
            if now != host {
                host = now;
                if open == Some(Surface::Status) {
                    shown = None;
                }
            }
        }

        if surface_until.is_some_and(|until| Instant::now() >= until) {
            open = None;
            surface_until = None;
            scrub = None;
            shown = None;
            hold_shown = None;
        }
        let hold_left = surface_until
            .map(|until| surface::hold_secs(until.saturating_duration_since(Instant::now())));

        if panel.layout().strip != strip_view {
            panel.set_strip(strip_view);
        }

        if let Some(path) = current.album_art.as_deref() {
            loader.request(path);
        }
        if let Some(new) = loader.poll() {
            art = new;
            shown = None;
        }

        let portrait = layout.frame.size.height > layout.frame.size.width;
        pane.set(
            current.title.as_deref().unwrap_or(""),
            if portrait {
                ""
            } else {
                current.artist.as_deref().unwrap_or("")
            },
            if portrait {
                ""
            } else {
                current.album.as_deref().unwrap_or("")
            },
            face_text_slot(&layout),
        );

        match shown.as_ref() {
            Some(prev) if prev.same_scene(&current) && scrub.is_none() => {
                if prev.seek != current.seek {
                    match open {
                        None if strip_view == Strip::Progress => {
                            panel.render_progress(&current)?;
                        }
                        Some(Surface::Controls) => {
                            panel.render_controls_seek(&current)?;
                        }
                        _ => {}
                    }
                }
                if open.is_some() && hold_left != hold_shown {
                    if let Some(left) = hold_left {
                        panel.render_hold(left)?;
                    }
                    hold_shown = hold_left;
                }
                shown = Some(current.clone());
            }
            _ => {
                if let Some(kind) = open {
                    panel.render_surface(
                        kind,
                        &current,
                        art.as_ref(),
                        &host,
                        scrub,
                        hold_left.unwrap_or(surface::HOLD_SECS),
                    )?;
                    hold_shown = hold_left;
                } else {
                    panel.render(&current, art.as_ref(), &pane, scrub)?;
                    hold_shown = None;
                }
                shown = Some(current.clone());
            }
        }

        if open.is_none() && pane.step() {
            panel.render_rows(&pane)?;
        }

        if let Some(p) = press.as_mut() {
            if tick_hold(p, &current, &commands, &mut last_step) {
                next_poll = Instant::now();
                shown = None;
            }
        }

        match rx.recv_timeout(TICK) {
            Ok(ev) => {
                handle_touch(
                    ev,
                    &mut FaceTouch {
                        layout: &layout,
                        open: &mut open,
                        surface_until: &mut surface_until,
                        press: &mut press,
                        scrub: &mut scrub,
                        strip_view: &mut strip_view,
                        host: &mut host,
                        netmon: &mut netmon,
                        current: &current,
                        commands: &commands,
                        next_poll: &mut next_poll,
                        shown: &mut shown,
                        last_step: &mut last_step,
                    },
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("touch thread stopped");
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Press {
    at: Point,
    from: Instant,
    face: Option<Hotspot>,
    surf: Option<SurfHit>,
    moved: bool,
    acted: bool,
}

fn tick_hold(
    press: &mut Press,
    state: &PlayerState,
    commands: &CommandSink,
    last_step: &mut Instant,
) -> bool {
    let now = Instant::now();
    match press.surf {
        Some(SurfHit::VolUp) | Some(SurfHit::VolDown) => {
            if now.duration_since(press.from) < HOLD_DELAY {
                return false;
            }
            if now.duration_since(*last_step) < HOLD_REPEAT {
                return false;
            }
            *last_step = now;
            press.acted = true;
            let delta = if press.surf == Some(SurfHit::VolUp) {
                2i16
            } else {
                -2
            };
            let next = (i16::from(state.volume.unwrap_or(0)) + delta).clamp(0, 100) as u8;
            send_cmd(commands, Command::Volume(next));
            true
        }
        Some(SurfHit::Speaker)
            if now.duration_since(press.from) >= MUTE_HOLD && !press.moved && !press.acted =>
        {
            press.acted = true;
            if state.is_muted() {
                send_cmd(commands, Command::Unmute);
            } else {
                send_cmd(commands, Command::Mute);
            }
            true
        }
        _ => false,
    }
}

fn send_cmd(commands: &CommandSink, cmd: Command) {
    if let Err(e) = commands.send(cmd) {
        tracing::warn!(error = %e, ?cmd, "command failed");
    }
}

struct FaceTouch<'a> {
    layout: &'a ui::Layout,
    open: &'a mut Option<Surface>,
    surface_until: &'a mut Option<Instant>,
    press: &'a mut Option<Press>,
    scrub: &'a mut Option<f32>,
    strip_view: &'a mut Strip,
    host: &'a mut HostInfo,
    netmon: &'a mut NetMonitor,
    current: &'a PlayerState,
    commands: &'a CommandSink,
    next_poll: &'a mut Instant,
    shown: &'a mut Option<PlayerState>,
    last_step: &'a mut Instant,
}

fn handle_touch(ev: TouchEv, ctx: &mut FaceTouch<'_>) {
    match ev {
        TouchEv::Down(p) => {
            *ctx.press = Some(if let Some(kind) = *ctx.open {
                *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                Press {
                    at: p,
                    from: Instant::now(),
                    face: None,
                    surf: Some(surface::hit(ctx.layout, kind, p)),
                    moved: false,
                    acted: false,
                }
            } else {
                Press {
                    at: p,
                    from: Instant::now(),
                    face: face_hit(ctx.layout, p),
                    surf: None,
                    moved: false,
                    acted: false,
                }
            });
            *ctx.last_step = Instant::now();
        }
        TouchEv::Move(p) => {
            if let Some(pr) = ctx.press.as_mut() {
                if (p.x - pr.at.x).abs() > TAP_SLOP || (p.y - pr.at.y).abs() > TAP_SLOP {
                    pr.moved = true;
                }
                let seeking = pr.face == Some(Hotspot::Seek) || pr.surf == Some(SurfHit::Seek);
                if seeking && pr.moved {
                    *ctx.scrub = Some(ui::seek_fraction(ctx.layout.progress, p));
                    *ctx.shown = None;
                }
                if pr.surf == Some(SurfHit::VolSlider) {
                    send_cmd(ctx.commands, Command::Volume(volume_at(ctx.layout, p)));
                    *ctx.next_poll = Instant::now();
                    *ctx.shown = None;
                }
            }
        }
        TouchEv::Up(p) => {
            let Some(pr) = ctx.press.take() else {
                return;
            };
            if ctx.open.is_some() {
                *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                match pr.surf {
                    Some(SurfHit::Close) => {
                        *ctx.open = None;
                        *ctx.surface_until = None;
                        *ctx.scrub = None;
                        *ctx.shown = None;
                    }
                    Some(SurfHit::Prev) if !pr.moved => {
                        send_cmd(ctx.commands, Command::Prev);
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::PlayPause) if !pr.moved => {
                        send_cmd(ctx.commands, Command::Toggle);
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::Next) if !pr.moved => {
                        send_cmd(ctx.commands, Command::Next);
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::Seek) => {
                        if ctx.current.duration.unwrap_or(0) > 0 {
                            let frac = ctx
                                .scrub
                                .unwrap_or_else(|| ui::seek_fraction(ctx.layout.progress, p));
                            let secs = (frac * ctx.current.duration.unwrap_or(0) as f32) as u32;
                            send_cmd(ctx.commands, Command::Seek(secs));
                            *ctx.next_poll = Instant::now();
                        }
                        *ctx.scrub = None;
                        *ctx.shown = None;
                    }
                    Some(SurfHit::Shuffle) if !pr.moved => {
                        send_cmd(ctx.commands, Command::Random);
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::Repeat) if !pr.moved => {
                        send_cmd(ctx.commands, Command::Repeat);
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::VolUp) if !pr.moved && !pr.acted => {
                        let next = ctx.current.volume.unwrap_or(0).saturating_add(2).min(100);
                        send_cmd(ctx.commands, Command::Volume(next));
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::VolDown) if !pr.moved && !pr.acted => {
                        let next = ctx.current.volume.unwrap_or(0).saturating_sub(2);
                        send_cmd(ctx.commands, Command::Volume(next));
                        *ctx.next_poll = Instant::now();
                    }
                    Some(SurfHit::VolSlider) => {
                        send_cmd(ctx.commands, Command::Volume(volume_at(ctx.layout, p)));
                        *ctx.next_poll = Instant::now();
                        *ctx.shown = None;
                    }
                    Some(SurfHit::Speaker) | None => {}
                    _ => {}
                }
                return;
            }
            if pr.moved && pr.face == Some(Hotspot::Seek) {
                if ctx.current.duration.unwrap_or(0) > 0 {
                    let frac = ctx
                        .scrub
                        .unwrap_or_else(|| ui::seek_fraction(ctx.layout.progress, p));
                    let secs = (frac * ctx.current.duration.unwrap_or(0) as f32) as u32;
                    send_cmd(ctx.commands, Command::Seek(secs));
                    *ctx.next_poll = Instant::now();
                }
                *ctx.scrub = None;
                *ctx.shown = None;
                return;
            }
            if pr.moved {
                return;
            }
            match pr.face {
                Some(Hotspot::Art) => {
                    *ctx.open = Some(if ctx.current.album_art.is_some() {
                        Surface::Artwork
                    } else {
                        Surface::Metadata
                    });
                    *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                    *ctx.shown = None;
                }
                Some(Hotspot::Info) => {
                    *ctx.host = ctx.netmon.refresh();
                    *ctx.open = Some(Surface::Status);
                    *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                    *ctx.shown = None;
                }
                Some(Hotspot::Title) | Some(Hotspot::DockMeta) => {
                    *ctx.open = Some(Surface::Metadata);
                    *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                    *ctx.shown = None;
                }
                Some(Hotspot::DockControls) => {
                    *ctx.open = Some(Surface::Controls);
                    *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                    *ctx.shown = None;
                }
                Some(Hotspot::DockVolume) => {
                    *ctx.open = Some(Surface::Volume);
                    *ctx.surface_until = Some(Instant::now() + SURFACE_HOLD);
                    *ctx.shown = None;
                }
                Some(Hotspot::Seek) => {
                    *ctx.strip_view = match *ctx.strip_view {
                        Strip::Progress => Strip::Stream,
                        Strip::Stream => Strip::Progress,
                        Strip::Off => Strip::Off,
                    };
                    *ctx.shown = None;
                }
                None => {}
            }
        }
    }
}
