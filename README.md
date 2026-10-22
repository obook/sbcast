# sbcast

`sbcast` casts a Linux desktop's screen and sound to an interactive
classroom panel, through the panel's own screen sharing receiver. The
panel's touches come back to the PC and drive its pointer (touchback).

## Requirements

- A Wayland session with `xdg-desktop-portal` (tested on KDE Plasma).
- PipeWire, with `pipewire-pulse` for sound.
- GStreamer (tested with 1.24).
- A Rust toolchain (`cargo`) 1.92 or newer, only to build from source.
- The PC and the panel on the same local network. mDNS (UDP 5353) lets
  sbcast find the panels; without it, use the code shown on the panel.

## Build

Debian, Ubuntu and derivatives (tested on KDE neon 24.04):

```sh
sudo apt install pkg-config libgstreamer1.0-dev gstreamer1.0-tools \
  gstreamer1.0-plugins-{base,good,bad,ugly} gstreamer1.0-libav \
  gstreamer1.0-pipewire
```

Check the GStreamer elements, then build:

```sh
gst-inspect-1.0 pipewiresrc pulsesrc x264enc avenc_aac appsink > /dev/null
cargo build --release
```

The binary is `target/release/sbcast`. `packaging/build-deb.sh neon` or
`packaging/build-deb.sh ubuntu26.04` builds a `.deb` in Docker.

## Usage

```sh
sbcast                            # menu: panels, presets and settings
sbcast list                       # panels on the network
sbcast cast --device "Room 104"   # name or id from `sbcast list`
sbcast cast --pin 0A1B2C          # panel by the code it shows
sbcast cast --host 10.0.0.7:7385  # panel by its address and command port
sbcast pin 0A1B2C                 # the address a code gives, without connecting
```

Each cast first opens the screen dialog, then a second dialog that allows
remote control: the panel's touches move the pointer on the shared screen.
Share a whole screen: with a single window, touchback stays off. Outside
the menu, Ctrl+C stops the cast.

In a terminal, `sbcast` alone, or `sbcast cast` without `--device`, `--pin`
or `--host`, opens a menu: the panels appear as they answer, and the command
line options prefill the settings. Arrows move and change, Tab switches columns, Enter
asks for the code the panel shows (empty if none), then casts. Someone may
have to allow the cast on the panel: the menu counts down 90 s. `c` casts
to a panel by its code alone, `s` stops, `p` chooses another screen and
moves the running cast to it, `q` or Ctrl+C quits.

| Option | Default | Effect |
|---|---|---|
| `--resolution WxH` | `1920x1080` | Output size, letterboxed to keep the screen shape |
| `--fps N` | `30` | Frame rate |
| `--bitrate KBPS` | `2500` | Video bitrate in kbit/s, lowered automatically when the network falls behind |
| `--no-audio` | | Video only |
| `--audio-delay MS` | `0` | Delays the sound on the panel. The menu changes it during a cast |
| `--device NAME` | | Panel name or id, found with mDNS |
| `--pin CODE` | | The code shown on the panel: sent to the panel, and gives its address |
| `--host IP:PORT` | | The panel's address and command port, when the code cannot give it |

## Troubleshooting

No panel found: the PC may block mDNS (UDP 5353). Use `--pin` or `c` in the
menu.

The panel's image stays still while you work elsewhere: you shared one
window, not the whole screen. Press `p` in the menu to choose again.

The panel is lost during a cast: sbcast connects again, up to 3 times in 5
minutes, without limit in the menu.

Logs: `RUST_LOG=sbcast=debug sbcast cast ...`. The menu always writes its
log to `$XDG_RUNTIME_DIR/sbcast.log`, emptied at each start. Each cast also
writes a protocol trace to `~/.local/state/sbcast/screenshare.log`.

## Roadmap

See [ROADMAP.md](ROADMAP.md).

## Author

Olivier Booklage <olivier.booklage@ac-bordeaux.fr>

## License

MIT, see [LICENSE](LICENSE).

The Rust dependencies use MIT, Apache-2.0, ISC or BSD licenses. At run
time, `sbcast` loads GStreamer (LGPL-2.1 or later) and its `x264enc`
plugin, which wraps x264 (GPL-2.0 or later).
