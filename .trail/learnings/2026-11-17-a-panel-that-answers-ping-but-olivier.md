---
title: "A panel that answers ping but refuses every EShare port needs a reboot, even if its home page shows"
author: Olivier
date: 2026-11-17
branch: main
cost: { minutes: 1, tokens: 1658 }
id: l-f749400b
scope: repo
status: active
replaces: l-455ca42c
src: agent
touches:
  - src/rtsp/mod.rs
---

## Decided
- When a panel cannot be reached, check one IP only: a ping, then a bare TCP connect on its 5 EShare ports. Ping OK with all 5 ports refused means the panel is on the network but its EShare service is not listening. Ask the user to restart the panel.

## Tried and ruled out
- Wi-Fi or routing problem: ruled out, the panel answered 4 pings out of 4 on the same subnet and ARP showed it reachable.
- Standby or another input (HDMI): ruled out, the user confirmed the panel was on and showing its home page while it refused every port.

## Watch out
- A panel that looks ready (on, home page shown) can still have its EShare service down. Do not trust the screen, test the ports.
- Refused at once (bash /dev/tcp exit code 1) is not the same as filtered (timeout, code 124). Refused means the host is up and nothing listens.
- Both known cases are the panel in room B22 (COL-B22), so it may be prone to this:
  - Thursday 2026-11-12, 08:35: ping OK, all ports refused; a restart fixed it. The earlier version of this note said C22, which was wrong; the IP recorded then was 10.116.18.195.
  - 2026-11-17: the panel refused the connection; the user restarted it, then ping and all 5 ports answered at 10.116.10.179. Ports were not tested before the restart.
  - The address changed between the two cases (DHCP): find the panel's current IP (ARP table, discovery) before testing.
- The exact cause (service crashed or stuck) was not established.
- In the code: ports are INFO_PORT 8121, CONTROL_PORT 57395, STATUS_PORT 52030, RTSP_PORT 51040, TOUCH_PORT 52020 in src/rtsp/mod.rs. The last two may only open once a cast is accepted, so 8121 and 57395 are the ones to test first.
