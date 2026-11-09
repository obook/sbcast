//! wire.rs
//! The bytes of the RTSP panels' protocol, read from the traffic of their Linux client
//! (2026-11-02, a COL-A22 panel at school) and checked with a prototype: text requests on
//! 8121, RTSP with binary property lists, video packets with a 128-byte header, 8-byte touches.
//!
//! Author: O. Booklage
//! Date: November 2026
//! Licence: MIT

use std::time::Duration;

use plist::{Dictionary, Value};

/// Video packet types: SPS and PPS together, then each picture.
const CONFIG: u16 = 256;
const PICTURE: u16 = 257;
/// The official client's clock read about 9,996 s; 0 is untested.
const CLOCK_START: f64 = 10_000.0;

/// Text request on port 8121: lines, then an empty line.
pub fn text_request(lines: &[&str]) -> Vec<u8> {
    let mut request = lines.join("\r\n");
    request.push_str("\r\n\r\n");
    request.into_bytes()
}

/// The code a panel asks for, from its server info: it gives it to anyone who asks.
pub fn pin(server_info: &str) -> Option<String> {
    let info: serde_json::Value = serde_json::from_str(server_info.trim()).ok()?;
    info["pin"].as_str().map(str::to_string)
}

/// User-Agent of the panels' own client: kept as is, in case the panels check it.
const USER_AGENT: &str = "EShare (EShare 1.0)";

/// One RTSP request, as the panels' own client writes it.
pub fn rtsp_request(method: &str, host: &str, cseq: u32, body: &[u8]) -> Vec<u8> {
    let head = format!(
        "{method} rtsp://{host} RTSP/1.0\r\nCSeq: {cseq}\r\nUser-Agent: {USER_AGENT}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    [head.as_bytes(), body].concat()
}

/// Length of the body announced in an RTSP reply header.
pub fn content_length(header: &str) -> usize {
    header
        .lines()
        .find_map(|line| line.split_once(':').filter(|(key, _)| key.trim().eq_ignore_ascii_case("content-length")))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

fn binary(value: Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    value.to_writer_binary(&mut bytes).expect("a plist in memory always writes");
    bytes
}

fn streams(stream: Dictionary) -> Value {
    Value::Dictionary(Dictionary::from_iter([("streams", Value::Array(vec![Value::Dictionary(stream)]))]))
}

/// SETUP body of the video stream (type 110). `name` shows on the panel's accept dialog.
pub fn video_setup(name: &str) -> Vec<u8> {
    let data = |text: &str| Value::Data(text.as_bytes().to_vec());
    let stream = Dictionary::from_iter([
        ("type", Value::from(110)),
        ("windowStream", Value::Boolean(true)),
        ("UbuntuStream", Value::from(1)),
        ("machine_name", data(name)),
        ("machine_ip", data("0.0.0.0")),
        ("machine_mac_address", data("000000000000")),
        ("mouse_style", Value::from(1)),
        ("log_connectId", data("0")),
        ("device_info", data("{}")),
    ]);
    binary(streams(stream))
}

/// AirPlay audio format asked of the panel: AAC-LC, 44.1 kHz, stereo. The official client
/// sends AAC-ELD (16_777_216), which GStreamer's fdkaacenc cannot encode.
// ponytail: AAC-LC untested on a panel; if it stays silent, try ELD through libfdk-aac.
const AUDIO_FORMAT: u32 = 0x40_0000;

/// SETUP body of the sound stream (type 96).
pub fn audio_setup() -> Vec<u8> {
    binary(streams(Dictionary::from_iter([("type", Value::from(96)), ("audioFormat", Value::from(AUDIO_FORMAT))])))
}

/// One sound packet, as captured from the official client: RTP header (version 2, payload
/// type 96, sequence, clock in ms) with the audio format where the SSRC goes, then one raw
/// AAC frame.
pub fn audio_packet(seq: u16, au: &[u8], elapsed: Duration) -> Vec<u8> {
    let clock = ((CLOCK_START + elapsed.as_secs_f64()) * 1000.0) as u32;
    let mut packet = vec![0x80, 96];
    packet.extend(seq.to_be_bytes());
    packet.extend(clock.to_be_bytes());
    packet.extend(AUDIO_FORMAT.to_be_bytes());
    packet.extend_from_slice(au);
    packet
}

/// The raw AAC frame of an ADTS one: the header takes 7 bytes, 9 with a CRC.
pub fn raw_aac(adts: &[u8]) -> Option<&[u8]> {
    let header = if *adts.get(1)? & 1 == 1 { 7 } else { 9 };
    adts.get(header..)
}

/// Port the panel gives for the video, in its answer to the video SETUP.
pub fn data_port(body: &[u8]) -> Option<u16> {
    let reply = Value::from_reader(std::io::Cursor::new(body)).ok()?;
    let stream = reply.as_dictionary()?.get("streams")?.as_array()?.first()?.as_dictionary()?;
    stream.get("dataPort")?.as_unsigned_integer()?.try_into().ok()
}

/// One video packet: size (u32), type (u16) and clock (u64, seconds in 32.32 fixed point),
/// little-endian, in a 128-byte header, then the H.264 data with its start codes.
fn packet(kind: u16, data: &[u8], elapsed: Duration) -> Vec<u8> {
    let clock = ((CLOCK_START + elapsed.as_secs_f64()) * 2f64.powi(32)) as u64;
    let mut packet = Vec::with_capacity(128 + data.len());
    packet.extend((data.len() as u32).to_le_bytes());
    packet.extend(kind.to_le_bytes());
    packet.extend([0, 0]);
    packet.extend(clock.to_le_bytes());
    packet.resize(128, 0);
    packet.extend_from_slice(data);
    packet
}

/// NAL unit type of an Annex B part (after its start code).
fn nal_type(part: &[u8]) -> Option<u8> {
    let one = part.iter().position(|&b| b == 1)?;
    part.get(one + 1).map(|header| header & 0x1f)
}

/// Video packets of one access unit, split by `split_config`: SPS and PPS in one config
/// packet, the rest in a picture packet.
pub fn video_packets(parts: &[&[u8]], elapsed: Duration) -> Vec<Vec<u8>> {
    let (config, pictures): (Vec<&[u8]>, Vec<&[u8]>) = parts.iter().partition(|part| matches!(nal_type(part), Some(7 | 8)));
    let mut packets = Vec::new();
    if !config.is_empty() {
        packets.push(packet(CONFIG, &config.concat(), elapsed));
    }
    packets.extend(pictures.into_iter().map(|part| packet(PICTURE, part, elapsed)));
    packets
}

/// A touch from the panel: 8 bytes, big-endian. Type 5 is the pointer: buttons (bit 0 the
/// finger, 8 and 16 the wheel), x and y on a 1920x1080 screen.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Touch {
    pub buttons: u8,
    pub x: u16,
    pub y: u16,
}

pub fn touch(message: [u8; 8]) -> Option<Touch> {
    (message[0] == 5).then(|| Touch {
        buttons: message[1],
        x: u16::from_be_bytes([message[2], message[3]]),
        y: u16::from_be_bytes([message[4], message[5]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_requests_as_the_official_client() {
        assert_eq!(text_request(&["CHECKPASSWORD", "026361"]), b"CHECKPASSWORD\r\n026361\r\n\r\n");
        let info = r#"{"feature":"0x103001ff","name":"COL-A22","pin":"026361","webPort":8000}"#;
        assert_eq!(pin(info).as_deref(), Some("026361"));
        let request = String::from_utf8(rtsp_request("OPTIONS", "10.116.11.20", 3, b"")).unwrap();
        assert!(request.starts_with("OPTIONS rtsp://10.116.11.20 RTSP/1.0\r\nCSeq: 3\r\n"));
        assert_eq!(content_length("RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 197"), 197);
    }

    #[test]
    fn reads_the_video_port_and_writes_setups() {
        let reply = streams(Dictionary::from_iter([("type", Value::from(110)), ("dataPort", Value::from(51030))]));
        assert_eq!(data_port(&binary(reply)), Some(51030));
        assert_eq!(data_port(b"not a plist"), None);
        let setup = Value::from_reader(std::io::Cursor::new(video_setup("PC guest"))).unwrap();
        let stream = &setup.as_dictionary().unwrap()["streams"].as_array().unwrap()[0];
        assert_eq!(stream.as_dictionary().unwrap()["machine_name"].as_data(), Some(&b"PC guest"[..]));
    }

    #[test]
    fn packs_config_and_picture_apart() {
        let (sps, pps, picture) = (&[0, 0, 0, 1, 0x67, 0x64][..], &[0, 0, 0, 1, 0x68, 0xee][..], &[0, 0, 0, 1, 0x65, 0x88][..]);
        let packets = video_packets(&[sps, pps, picture], Duration::from_millis(500));
        assert_eq!(packets.len(), 2);
        // Captured header of the official client: size 35, type 256 (00 01), then the clock.
        assert_eq!(&packets[0][..8], &[12, 0, 0, 0, 0x00, 0x01, 0, 0]);
        assert_eq!(&packets[0][128..], [sps, pps].concat());
        assert_eq!(&packets[1][4..6], &[0x01, 0x01]);
        assert_eq!(u64::from_le_bytes(packets[1][8..16].try_into().unwrap()) >> 31, 20_001);
        assert_eq!(video_packets(&[picture], Duration::ZERO).len(), 1);
    }

    #[test]
    fn wraps_sound_as_captured() {
        // Captured header: seq 0, clock 9,996,983 ms, format 01 00 00 00 (ELD there).
        let packet = audio_packet(1, &[0x21, 0x10], Duration::from_millis(500));
        assert_eq!(&packet[..4], &[0x80, 0x60, 0, 1]);
        assert_eq!(u32::from_be_bytes(packet[4..8].try_into().unwrap()), 10_000_500);
        assert_eq!(&packet[12..], &[0x21, 0x10]);
        assert_eq!(raw_aac(&[0xff, 0xf1, 0, 0, 0, 0, 0, 0x21]), Some(&[0x21][..]));
        assert_eq!(raw_aac(&[0xff, 0xf0, 0, 0, 0, 0, 0, 0, 0, 0x21]), Some(&[0x21][..]));
    }

    #[test]
    fn reads_touches_as_captured() {
        let pressed = touch(0x050106b401c70000u64.to_be_bytes()).unwrap();
        assert_eq!(pressed, Touch { buttons: 1, x: 1716, y: 455 });
        assert_eq!(touch([4, 1, 0, 0, 0, 0, 0, 0x20]), None);
    }
}
