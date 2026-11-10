# Roadmap

sbcast started from sccast's panel format.

## Panel protocol

Done (unverified on a panel):

- [x] Command channel: varint length, protobuf message, JSON payload; connect, cast, heartbeat, stop, key frame request
- [x] Video (H.264, SPS and PPS apart) and sound (AAC with ADTS) over TCP
- [x] Find panels with mDNS (`_sspcast._tcp`)
- [x] Find a panel by the code it shows (`--pin`, `c` in the menu)
- [x] Reconnect after a loss, without limit, each cause logged; a refusal on the panel ends the cast. Checked on a fake panel that cuts every 30 s (`examples/fake_panel.rs`): back in about 4 s, video restarts on a key frame
- [x] Bounded delay over TCP: small send buffer, drop frames until a key frame when the network falls behind, lower the bitrate
- [x] Touchback always on: the panel's touches move the pointer through the RemoteDesktop portal

To do:

- [ ] Test on a real panel
- [ ] Congested school network: the panel's own sender app often drops. At busy hours, count drops and cut lengths for that app and for sbcast on the same panel
- [ ] Touchback on a real panel: which coordinates it sends, the wheel's direction, the UDP beat to the panel
- [ ] SSDP discovery, only if a panel does not answer mDNS
- [ ] Follow the panel's size and frame rate requests, and its bitrate
- [ ] Keep the panel connection between two casts, if a real panel asks for approval again each time

## RTSP panels

The school's panels run another receiver, read from the traffic of their own Linux client.

Done:

- [x] List them with mDNS, the UDP search on port 48689 and a ping of the panels seen before (`~/.local/state/sbcast/panels.tsv`). Green: the panel answers. Dark gray: remembered only
- [x] Forget a panel silent for 3 weeks
- [x] Cast with touch back (Connection: Accept). On COL-A22, image and pointer are right but writing lags. In room C22 (2026-11-04), the cast works very well (pen not tested)
- [x] Cast to the code receiver on port 7385 (Connection: Code): image only

To do:

- [ ] Sound on EShare panels: sent as AAC-LC over RTP/UDP (format read from the official client capture, which uses AAC-ELD), untested on a panel. If silent, encode ELD with libfdk-aac
- [ ] Reconnect after a loss
- [ ] Make writing follow the pen with touch back. Room B22 (panel COL-B22, formerly COL-A22, 10.116.11.20), 2026-11-04, writing in OpenBoard:
  - Writing at normal speed leaves holes, and small curves come out straight. Very slow writing leaves fewer holes. The panel's own writing app has no holes: the sensor is fine
  - Moving the pointer from the PC shows on the panel without lag: the video is not the cause. A performance power profile changes nothing
  - Raw messages on port 52020 (`RTSP touch raw` at debug level): only type 5, 60 per second (one every 16 ms), about 4 steps of the 1920x1080 grid apart, each applied at once (no batching). Buttons: 3 while the pen is down, 1 then 3 at a press, 0 at a lift, rarely 4 or 5
  - The holes are lifts sent by the panel itself: within a word, button 0, then nothing for 80 to 300 ms, then a press 20 to 90 steps further. sbcast drops nothing
  - Tried and reverted: joining a lift and a press within 250 ms. It also joined separate words
  - Not a pointer resolution issue: one grid step is 0.8 px of the 1536x960 desktop
  - Official client capture (2026-11-02, `~/eshare-test/COL-A22-1312`): same messages, same 60 per second. It was not a writing test
  - Next: write with the official client (`~/eshare-test/eshare.sh`, code from `getServerInfo` on port 8121) and compare. It runs in X11 under KDE Wayland and shows a black screen: start OpenBoard with `QT_QPA_PLATFORM=xcb openboard`, or use a Plasma (X11) session. Same holes: the panel's limit. Better writing: capture its traffic and compare
- [ ] Touch back on the code receiver
- [ ] Try Connection: Code from the menu on a panel

## Menu

Done:

- [x] Panels, presets and settings, prefilled from the command line
- [x] Enter asks for the code the panel shows, then counts down while someone allows the cast (90 s)
- [x] Stays during the cast: duration, reconnections, lost frames, CPU, bitrate
- [x] `p` chooses another screen and moves the running cast to it
- [x] Audio delay in 50 ms steps, from -500 to 1500 ms (negative: the image waits), applied at the next cast: changed live, it froze the image in sccast
- [x] Name a missing GStreamer element and its package at start

To do:

- [ ] Tune the alert thresholds on a real panel and the school network

## Tray

Done:

- [x] `sbcast tray`: icon in the notification area, started with the session (.deb). Panels, cast by code, stop, freeze, screen, presets, terminal menu
- [x] The screen is shared only during a cast; errors as KDE notifications (kdialog)

To do:

- [ ] Cast from the tray on a real panel
- [ ] GNOME: the icon needs the AppIndicator extension, and kdialog is a KDE tool

## Packaging

Done:

- [x] .deb for KDE neon and Ubuntu 26.04, releases v0.1.0 to v0.4.0

To do:

- [ ] On Ubuntu 26.04 (GNOME), check the screen and remote control dialogs
- [ ] Cast for one hour without a cut
