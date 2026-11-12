---
title: "Forgetting panels in the tray drops all silent ones at once, after a confirmation box"
author: Olivier
date: 2026-11-12
branch: feat/tray-rescan-forget
cost: { minutes: 8, tokens: 75387 }
id: l-72ddd7ac
scope: repo
status: active
src: agent
touches:
  - media/menu/forget.png
  - media/menu/forget.svg
  - media/menu/rescan.png
  - media/menu/rescan.svg
  - src/device/discovery.rs
  - src/menu/mod.rs
  - src/session/tray_loop.rs
  - src/tray/mod.rs
---

## Decided
The tray forgets every panel that did not answer since the last scan in one go, behind a yes/no box, rather than one panel at a time. A per-panel item would double the list and the remembered panels already expire after three weeks; this is for a panel that moved or was removed. A forgotten panel comes back as soon as it answers again, so a wrong click costs nothing.

## Tried and ruled out
Theme icon names (view-refresh, edit-delete) for the two new items: the other items ship their own PNG, and a theme name shows nothing when the theme lacks it, so the new ones ship PNGs too.

## Watch out
Inkscape is not installed on the dev machine; the menu PNGs were rendered with ImageMagick instead. Force 8-bit RGBA, or convert produces a 16-bit or colormap PNG unlike the others.

In the code:
- discovery::forget(ids) rewrites panels.tsv; menu.forget_silent() is the caller.
- convert x.svg -background none -density 384 -resize 32x32 -depth 8 -define png:color-type=6 x.png
