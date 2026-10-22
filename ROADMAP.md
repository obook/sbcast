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

## Menu

Done:

- [x] Panels, presets and settings, prefilled from the command line
- [x] Enter asks for the code the panel shows, then counts down while someone allows the cast (90 s)
- [x] Stays during the cast: duration, reconnections, lost frames, CPU, bitrate, live audio delay
- [x] `p` chooses another screen and moves the running cast to it

To do:

- [ ] Tune the alert thresholds on a real panel and the school network

## Packaging

Done:

- [x] First release, v0.1.0: .deb for KDE neon and Ubuntu 26.04

To do:

- [ ] On Ubuntu 26.04 (GNOME), check the screen and remote control dialogs
- [ ] Cast for one hour without a cut
