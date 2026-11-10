---
title: EShare panel sound is AAC-ELD
author: Olivier
date: 2026-11-10
branch: main
cost: { minutes: 1, tokens: 1104 }
id: l-a5b17f4f
scope: repo
status: active
src: agent
touches:
  - src/rtsp/wire.rs
  - src/capture/encode.rs
---

RTSP (EShare) panels crackled on AAC-LC and gave noise on raw PCM (audioFormat 0x800), room B22, 2026-11-10: the panel decodes AAC-ELD whatever audioFormat says. The official client's capture is ELD, 44.1 kHz, stereo, 480-sample frames, raw, RTP pt 96, SSRC field 0x01000000; it decodes with fdkaacdec codec_data=f8e85000 (the 512-sample config f8e84000 gives noise, which is how the frame size was found). GStreamer's fdkaacenc is LC only, so src/rtsp/eld.rs dlopens libfdk-aac.so.2 and copies the official client's aacEncoder_SetParam calls (AOT 39, granule 480, 44100, stereo, WAV order, raw transport, 192 kbit/s = the capture's mean frame of 261 bytes). Ruled out: AAC-LC, PCM big-endian. Not tried: PCM little-endian. Checked offline by decoding our frames (clean 440 Hz); heard clean on COL-B22 (room B22, 2026-11-10, user: "J'ai du son"). The Bytello/sccast path keeps AAC-LC with ADTS (Stream.raw_audio false).
