//! Rust packet processor for the RAOP mirroring TCP stream.
//!
//! This ports the stateful packet interpretation from `raop_rtp_mirror.c`:
//! timestamp conversion, H.264/H.265 codec setup, SPS/PPS/VPS buffering,
//! encrypted NAL reconstruction, stream suspend/resume markers, and client
//! streaming reports. The socket/thread owner can supply a clock and consume
//! the typed events without crossing the C callback ABI.

use uxplay_crypto::port::CryptoError;
use uxplay_mirror_buffer::MirrorBuffer;

pub mod runtime;

pub const PACKET_HEADER_LEN: usize = 128;
pub const MAX_PARAMETER_SET_BYTES: usize = 1024;
pub const FPS_TRAILER_BYTES: usize = 25_000;
const NAL_START_CODE: &[u8; 4] = &[0, 0, 0, 1];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoCodec {
    H264,
    H265,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoDimensions {
    pub width_source: u16,
    pub height_source: u16,
    pub x_offset: u16,
    pub y_offset: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    pub codec: VideoCodec,
    pub data: Vec<u8>,
    pub nal_count: usize,
    pub ntp_time_local: u64,
    pub ntp_time_remote: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MirrorEvent {
    CodecConfigured {
        codec: VideoCodec,
        dimensions: VideoDimensions,
    },
    VideoFrame(VideoFrame),
    VideoPaused,
    VideoResumed,
    OldProtocolHeartbeat,
    ClientStreamingReport(Vec<u8>),
    UnexpectedPacketType(u8),
    UnsupportedCodec,
    MalformedParameterSets(&'static str),
    CodecChanged {
        previous: VideoCodec,
        requested: VideoCodec,
    },
    InvalidVideoPayload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorError {
    InvalidHeader,
    PayloadLengthMismatch { declared: usize, actual: usize },
    NegativePayloadLength,
    PayloadTooLarge(usize),
    Crypto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirrorDiagnostic {
    DiscardedParameterSetsTimestampMismatch {
        video_timestamp: u64,
        parameter_set_timestamp: u64,
    },
}

impl From<CryptoError> for MirrorError {
    fn from(_: CryptoError) -> Self {
        Self::Crypto
    }
}

/// Timestamp operations are supplied by the RAOP NTP session.
pub trait NtpClock {
    fn adjust_remote_timestamp_offset(&mut self, wire_ntp: u64, include_epoch: bool) -> u64;
    fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64;
    fn local_time_nanoseconds(&self) -> u64;
    fn convert_remote_time(&self, remote_nanoseconds: u64) -> u64;
    fn set_video_arrival_offset(&mut self, offset_nanoseconds: u64);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodecState {
    Unknown,
    H264,
    H265,
}

impl CodecState {
    fn codec(self) -> Option<VideoCodec> {
        match self {
            Self::Unknown => None,
            Self::H264 => Some(VideoCodec::H264),
            Self::H265 => Some(VideoCodec::H265),
        }
    }

    fn from_codec(codec: VideoCodec) -> Self {
        match codec {
            VideoCodec::H264 => Self::H264,
            VideoCodec::H265 => Self::H265,
        }
    }
}

/// Per-mirroring-session packet state, equivalent to the C worker's locals.
pub struct MirrorPacketProcessor {
    cipher: MirrorBuffer,
    codec: CodecState,
    parameter_sets: Vec<u8>,
    parameter_set_timestamp: u64,
    video_stream_suspended: bool,
    first_packet: bool,
    diagnostics: Vec<MirrorDiagnostic>,
}

impl MirrorPacketProcessor {
    pub fn new(audio_key: &[u8]) -> Result<Self, MirrorError> {
        Ok(Self {
            cipher: MirrorBuffer::new(audio_key)?,
            codec: CodecState::Unknown,
            parameter_sets: Vec::new(),
            parameter_set_timestamp: 0,
            video_stream_suspended: false,
            first_packet: true,
            diagnostics: Vec::new(),
        })
    }

    pub fn init_aes(&mut self, stream_connection_id: u64) -> Result<(), MirrorError> {
        self.cipher.init_aes(stream_connection_id)?;
        Ok(())
    }

    pub fn codec(&self) -> Option<VideoCodec> {
        self.codec.codec()
    }

    pub fn take_diagnostics(&mut self) -> Vec<MirrorDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Process one completed 128-byte AirPlay video header and its payload.
    ///
    /// The source uses native-endian integer/float reads. UxPlay's supported
    /// little-endian targets read the fields in little-endian order here too.
    pub fn process_packet(
        &mut self,
        header: &[u8],
        payload: &mut [u8],
        clock: &mut dyn NtpClock,
        show_client_fps: bool,
    ) -> Result<Vec<MirrorEvent>, MirrorError> {
        if header.len() != PACKET_HEADER_LEN {
            return Err(MirrorError::InvalidHeader);
        }
        let declared = u32::from_le_bytes(header[0..4].try_into().unwrap()) as usize;
        if declared != payload.len() {
            return Err(MirrorError::PayloadLengthMismatch {
                declared,
                actual: payload.len(),
            });
        }
        if declared > i32::MAX as usize {
            return Err(MirrorError::NegativePayloadLength);
        }
        // The source allocates the declared size. Keep that behavior for ordinary
        // packets, while refusing unreasonable sizes before allocating output.
        if declared > 16 * 1024 * 1024 {
            return Err(MirrorError::PayloadTooLarge(declared));
        }

        let wire_ntp = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let raw_ntp = clock.adjust_remote_timestamp_offset(wire_ntp, true);
        let remote_ns = clock.timestamp_to_nanoseconds(raw_ntp);
        if self.first_packet {
            let offset = clock.local_time_nanoseconds().wrapping_sub(remote_ns);
            clock.set_video_arrival_offset(offset);
            self.first_packet = false;
        }

        match header[4] {
            0x00 => self.process_video_data(header, payload, raw_ntp, remote_ns, clock),
            0x01 => self.process_codec_data(header, payload, raw_ntp),
            0x02 => Ok(vec![MirrorEvent::OldProtocolHeartbeat]),
            0x05 => {
                if !show_client_fps || payload.is_empty() {
                    return Ok(Vec::new());
                }
                let report_len = if payload.len() > FPS_TRAILER_BYTES {
                    payload.len() - FPS_TRAILER_BYTES
                } else {
                    payload.len()
                };
                if report_len == 0 {
                    Ok(Vec::new())
                } else {
                    Ok(vec![MirrorEvent::ClientStreamingReport(
                        payload[..report_len].to_vec(),
                    )])
                }
            }
            other => Ok(vec![MirrorEvent::UnexpectedPacketType(other)]),
        }
    }

    fn process_video_data(
        &mut self,
        _header: &[u8],
        payload: &mut [u8],
        raw_ntp: u64,
        remote_ns: u64,
        clock: &dyn NtpClock,
    ) -> Result<Vec<MirrorEvent>, MirrorError> {
        let local_ns = clock.convert_remote_time(remote_ns);
        if !self.parameter_sets.is_empty() && raw_ntp != self.parameter_set_timestamp {
            self.diagnostics
                .push(MirrorDiagnostic::DiscardedParameterSetsTimestampMismatch {
                    video_timestamp: raw_ntp,
                    parameter_set_timestamp: self.parameter_set_timestamp,
                });
            self.parameter_sets.clear();
        }

        let prefix_len = self.parameter_sets.len();
        let mut data = Vec::with_capacity(prefix_len + payload.len());
        data.extend_from_slice(&self.parameter_sets);
        let mut decrypted = self.cipher.decrypt(payload)?;
        let (nal_count, valid) = convert_length_prefixed_nalus(&mut decrypted, self.codec);
        data.extend_from_slice(&decrypted);
        let mut nal_count = nal_count;
        if !valid && !data.is_empty() {
            // This marker matches raop_rtp_mirror.c's failed-decryption signal.
            data[0] = 1;
        }
        if prefix_len != 0 {
            nal_count += if self.codec == CodecState::H265 { 3 } else { 2 };
            self.parameter_sets.clear();
        }
        let codec = self.codec.codec().unwrap_or(VideoCodec::H264);
        Ok(vec![MirrorEvent::VideoFrame(VideoFrame {
            codec,
            data,
            nal_count,
            ntp_time_local: local_ns,
            ntp_time_remote: remote_ns,
        })])
    }

    fn process_codec_data(
        &mut self,
        header: &[u8],
        payload: &[u8],
        raw_ntp: u64,
    ) -> Result<Vec<MirrorEvent>, MirrorError> {
        let mut events = Vec::new();
        match header[6] {
            0x56 | 0x5e if !self.video_stream_suspended => {
                self.video_stream_suspended = true;
                events.push(MirrorEvent::VideoPaused);
            }
            0x16 | 0x1e if self.video_stream_suspended => {
                self.video_stream_suspended = false;
                events.push(MirrorEvent::VideoResumed);
            }
            _ => {}
        }

        if payload.is_empty() {
            events.push(MirrorEvent::UnsupportedCodec);
            return Ok(events);
        }

        let dimensions = dimensions_from_header(header);
        let codec = if payload.get(4..8) == Some(b"hvc1") {
            VideoCodec::H265
        } else {
            VideoCodec::H264
        };
        if let Some(previous) = self.codec.codec()
            && previous != codec
        {
            events.push(MirrorEvent::CodecChanged {
                previous,
                requested: codec,
            });
            return Ok(events);
        }
        if self.codec == CodecState::Unknown {
            self.codec = CodecState::from_codec(codec);
            events.push(MirrorEvent::CodecConfigured { codec, dimensions });
        }
        self.parameter_set_timestamp = raw_ntp;

        match codec {
            VideoCodec::H264 => match h264_parameter_sets(payload) {
                Some(bytes) => self.parameter_sets = bytes,
                None => events.push(MirrorEvent::MalformedParameterSets("H.264 SPS/PPS")),
            },
            VideoCodec::H265 => match h265_parameter_sets(payload) {
                Some(bytes) => self.parameter_sets = bytes,
                None => {
                    events.push(MirrorEvent::VideoPaused);
                    events.push(MirrorEvent::MalformedParameterSets("H.265 VPS/SPS/PPS"));
                }
            },
        }
        if self.parameter_sets.len() > MAX_PARAMETER_SET_BYTES {
            self.parameter_sets.clear();
            events.push(MirrorEvent::MalformedParameterSets(
                "parameter-set buffer limit",
            ));
        }
        Ok(events)
    }
}

fn dimensions_from_header(header: &[u8]) -> VideoDimensions {
    let float = |offset: usize| f32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    VideoDimensions {
        width_source: float(40) as u16,
        height_source: float(44) as u16,
        x_offset: float(48) as u16,
        y_offset: float(52) as u16,
        width: float(56) as u16,
        height: float(60) as u16,
    }
}

fn h264_parameter_sets(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() < 8 {
        return None;
    }
    let sps_len = usize::from(u16::from_be_bytes(payload[6..8].try_into().ok()?));
    let pps_len_offset = sps_len.checked_add(9)?;
    let pps_data_offset = sps_len.checked_add(11)?;
    if pps_len_offset.checked_add(2)? > payload.len() || pps_data_offset > payload.len() {
        return None;
    }
    let pps_len = usize::from(u16::from_be_bytes(
        payload[pps_len_offset..pps_len_offset + 2]
            .try_into()
            .ok()?,
    ));
    let pps_end = pps_data_offset.checked_add(pps_len)?;
    let total = sps_len.checked_add(pps_len)?.checked_add(8)?;
    if pps_end > payload.len() || total > MAX_PARAMETER_SET_BYTES {
        return None;
    }

    let mut output = Vec::with_capacity(total);
    output.extend_from_slice(NAL_START_CODE);
    output.extend_from_slice(&payload[8..8 + sps_len]);
    output.extend_from_slice(NAL_START_CODE);
    output.extend_from_slice(&payload[pps_data_offset..pps_end]);
    Some(output)
}

fn h265_parameter_sets(payload: &[u8]) -> Option<Vec<u8>> {
    // The source's HEVC container starts its parameter sets at byte 0x75.
    let mut cursor = 0x75;
    let (vps, next) = h265_parameter(payload, cursor, &[0xa0, 0x00, 0x01, 0x00])?;
    cursor = next;
    let (sps, next) = h265_parameter(payload, cursor, &[0xa1, 0x00, 0x01, 0x00])?;
    cursor = next;
    let (pps, _) = h265_parameter(payload, cursor, &[0xa2, 0x00, 0x01, 0x00])?;
    let total = vps
        .len()
        .checked_add(sps.len())?
        .checked_add(pps.len())?
        .checked_add(12)?;
    if total > MAX_PARAMETER_SET_BYTES {
        return None;
    }
    let mut output = Vec::with_capacity(total);
    for set in [vps, sps, pps] {
        output.extend_from_slice(NAL_START_CODE);
        output.extend_from_slice(set);
    }
    Some(output)
}

fn h265_parameter<'a>(
    payload: &'a [u8],
    offset: usize,
    marker: &[u8; 4],
) -> Option<(&'a [u8], usize)> {
    let marker_end = offset.checked_add(marker.len())?;
    if payload.get(offset..marker_end)? != marker {
        return None;
    }
    // Keep the reference's overlapping length read at offset + 3.
    let length_end = offset.checked_add(5)?;
    let encoded_length = u16::from_be_bytes(payload.get(offset + 3..length_end)?.try_into().ok()?);
    let data_start = length_end;
    let data_end = data_start.checked_add(usize::from(encoded_length))?;
    Some((payload.get(data_start..data_end)?, data_end))
}

fn convert_length_prefixed_nalus(payload: &mut [u8], codec: CodecState) -> (usize, bool) {
    let mut offset = 0usize;
    let mut count = 0usize;
    // raop_rtp_mirror.c checks the decrypted body length, not the parameter-set prefix.
    while offset < payload.len() {
        let Some(length_bytes) = payload.get(offset..offset.saturating_add(4)) else {
            return (count, false);
        };
        let raw_len = i32::from_be_bytes(length_bytes.try_into().unwrap());
        let Some(nalu_len) = usize::try_from(raw_len).ok() else {
            return (count, false);
        };
        let nalu_start = offset.saturating_add(4);
        let Some(nalu_end) = nalu_start.checked_add(nalu_len) else {
            return (count, false);
        };
        if payload.get(nalu_start..nalu_end).is_none() {
            return (count, false);
        }
        let Some(nalu_first) = payload.get(nalu_start).copied() else {
            return (count, false);
        };
        payload[offset..offset + 4].copy_from_slice(NAL_START_CODE);
        count += 1;
        if nalu_first & 0x80 != 0 {
            return (count, false);
        }
        let _nal_type = match codec {
            CodecState::H264 => nalu_first & 0x1f,
            CodecState::H265 => (nalu_first >> 1) & 0x3f,
            CodecState::Unknown => nalu_first & 0x1f,
        };
        if nalu_end == payload.len() {
            return (count, true);
        }
        // nalu body starts after the replacement marker, so advance by its length.
        offset = nalu_start.saturating_add(nalu_len);
    }
    (count, offset == payload.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uxplay_crypto::port::{AesContext, AesMode, Direction, sha512};

    const AUDIO_KEY: [u8; 16] = *b"audio-key-fixtur";
    const STREAM_ID: u64 = 0x0123_4567_89ab_cdef;

    #[derive(Default)]
    struct TestClock {
        arrival_offset: Option<u64>,
    }

    impl NtpClock for TestClock {
        fn adjust_remote_timestamp_offset(&mut self, wire_ntp: u64, include_epoch: bool) -> u64 {
            assert!(include_epoch);
            wire_ntp
        }
        fn timestamp_to_nanoseconds(&self, timestamp: u64) -> u64 {
            timestamp
        }
        fn local_time_nanoseconds(&self) -> u64 {
            10_000
        }
        fn convert_remote_time(&self, remote_nanoseconds: u64) -> u64 {
            remote_nanoseconds + 5
        }
        fn set_video_arrival_offset(&mut self, offset_nanoseconds: u64) {
            self.arrival_offset = Some(offset_nanoseconds);
        }
    }

    fn header(kind: u8, payload_len: usize, timestamp: u64) -> [u8; PACKET_HEADER_LEN] {
        let mut output = [0; PACKET_HEADER_LEN];
        output[..4].copy_from_slice(&(payload_len as u32).to_le_bytes());
        output[4] = kind;
        output[8..16].copy_from_slice(&timestamp.to_le_bytes());
        for (offset, value) in [
            (40, 1920.0f32),
            (44, 1080.0),
            (48, 0.0),
            (52, 0.0),
            (56, 1920.0),
            (60, 1080.0),
        ] {
            output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        output
    }

    fn h264_codec_payload() -> Vec<u8> {
        let sps = [0x67, 0x42, 0x00, 0x1e];
        let pps = [0x68, 0xce, 0x06, 0xe2];
        let mut data = vec![0, 0, 0, 0, 0, 0];
        data.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        data.extend_from_slice(&sps);
        data.push(1);
        data.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        data.extend_from_slice(&pps);
        data
    }

    fn h265_codec_payload() -> Vec<u8> {
        let mut data = vec![0; 0x75];
        data.extend_from_slice(&[0xa0, 0x00, 0x01, 0x00, 2, 0x40, 0x01]);
        data.extend_from_slice(&[0xa1, 0x00, 0x01, 0x00, 2, 0x42, 0x01]);
        data.extend_from_slice(&[0xa2, 0x00, 0x01, 0x00, 2, 0x44, 0x01]);
        data[4..8].copy_from_slice(b"hvc1");
        data
    }

    fn encrypt_payload(stream_id: u64, plaintext: &[u8]) -> Vec<u8> {
        let key_label = format!("AirPlayStreamKey{stream_id}");
        let iv_label = format!("AirPlayStreamIV{stream_id}");
        let mut key_digest = sha512().unwrap();
        key_digest.update(key_label.as_bytes()).unwrap();
        key_digest.update(&AUDIO_KEY).unwrap();
        let key = key_digest.finish().unwrap();
        let mut iv_digest = sha512().unwrap();
        iv_digest.update(iv_label.as_bytes()).unwrap();
        iv_digest.update(&AUDIO_KEY).unwrap();
        let iv = iv_digest.finish().unwrap();
        AesContext::new(AesMode::Ctr, Direction::Encrypt, &key[..16], &iv[..16])
            .unwrap()
            .ctr_encrypt(plaintext)
            .unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for &byte in bytes {
            output.push(DIGITS[(byte >> 4) as usize] as char);
            output.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        output
    }

    #[test]
    fn h264_parameter_sets_prepend_to_matching_video_frame() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        processor.init_aes(STREAM_ID).unwrap();
        let mut clock = TestClock::default();
        let mut codec_payload = h264_codec_payload();
        let codec_header = header(0x01, codec_payload.len(), 42);
        let codec_events = processor
            .process_packet(&codec_header, &mut codec_payload, &mut clock, false)
            .unwrap();
        let [MirrorEvent::CodecConfigured { codec, dimensions }] = codec_events.as_slice() else {
            panic!("C fixture begins with its codec callback")
        };
        let mut oracle_rows = vec![format!(
            "codec,{},{},{},{},{},{},{}",
            match codec {
                VideoCodec::H264 => 1,
                VideoCodec::H265 => 2,
            },
            dimensions.width_source,
            dimensions.height_source,
            dimensions.x_offset,
            dimensions.y_offset,
            dimensions.width,
            dimensions.height
        )];
        assert_eq!(
            codec_events,
            vec![MirrorEvent::CodecConfigured {
                codec: VideoCodec::H264,
                dimensions: VideoDimensions {
                    width_source: 1920,
                    height_source: 1080,
                    x_offset: 0,
                    y_offset: 0,
                    width: 1920,
                    height: 1080,
                },
            }]
        );

        let body = [0, 0, 0, 2, 0x65, 0xaa];
        let mut encrypted = encrypt_payload(STREAM_ID, &body);
        let frame_header = header(0x00, encrypted.len(), 42);
        let events = processor
            .process_packet(&frame_header, &mut encrypted, &mut clock, false)
            .unwrap();
        let [MirrorEvent::VideoFrame(frame)] = events.as_slice() else {
            panic!("one decoded video frame is expected")
        };
        assert_eq!(frame.codec, VideoCodec::H264);
        assert_eq!(frame.nal_count, 3);
        assert_eq!(
            frame.data,
            [
                &[0, 0, 0, 1, 0x67, 0x42, 0, 0x1e][..],
                &[0, 0, 0, 1, 0x68, 0xce, 0x06, 0xe2][..],
                &[0, 0, 0, 1, 0x65, 0xaa][..]
            ]
            .concat()
        );
        assert_eq!(frame.ntp_time_local, 47);
        assert_eq!(frame.ntp_time_remote, 42);
        assert_eq!(clock.arrival_offset, Some(9_958));
        oracle_rows.push(format!(
            "frame,{},{},{},{}",
            u8::from(frame.codec == VideoCodec::H265),
            frame.nal_count,
            frame.data.len(),
            hex(&frame.data)
        ));
        assert_eq!(
            oracle_rows,
            include_str!("../reference/raop_rtp_mirror.expected.txt")
                .lines()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stale_parameter_sets_emit_the_original_timestamp_diagnostic() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        processor.init_aes(STREAM_ID).unwrap();
        let mut clock = TestClock::default();
        let mut codec_payload = h264_codec_payload();
        let codec_header = header(0x01, codec_payload.len(), 42);
        processor
            .process_packet(&codec_header, &mut codec_payload, &mut clock, false)
            .unwrap();
        assert!(processor.take_diagnostics().is_empty());

        let body = [0, 0, 0, 2, 0x65, 0xaa];
        let mut encrypted = encrypt_payload(STREAM_ID, &body);
        let frame_header = header(0x00, encrypted.len(), 43);
        let events = processor
            .process_packet(&frame_header, &mut encrypted, &mut clock, false)
            .unwrap();
        assert!(matches!(
            events.as_slice(),
            [MirrorEvent::VideoFrame(VideoFrame { nal_count: 1, data, .. })] if data.len() == 6
        ));
        assert_eq!(
            processor.take_diagnostics(),
            [MirrorDiagnostic::DiscardedParameterSetsTimestampMismatch {
                video_timestamp: 43,
                parameter_set_timestamp: 42,
            }]
        );
        assert!(processor.take_diagnostics().is_empty());
    }

    #[test]
    fn codec_markers_pause_and_resume_once_until_state_changes() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        let mut clock = TestClock::default();
        let mut payload = h264_codec_payload();
        let mut start = header(0x01, payload.len(), 1);
        start[6] = 0x56;
        assert_eq!(
            processor
                .process_packet(&start, &mut payload, &mut clock, false)
                .unwrap()[0],
            MirrorEvent::VideoPaused
        );
        let mut resume = header(0x01, payload.len(), 2);
        resume[6] = 0x16;
        assert_eq!(
            processor
                .process_packet(&resume, &mut payload, &mut clock, false)
                .unwrap()[0],
            MirrorEvent::VideoResumed
        );
    }

    #[test]
    fn hevc_parameter_sets_prepend_vps_sps_pps_and_count_three_headers() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        processor.init_aes(STREAM_ID).unwrap();
        let mut clock = TestClock::default();
        let mut codec_payload = h265_codec_payload();
        let codec_header = header(0x01, codec_payload.len(), 77);
        let codec_events = processor
            .process_packet(&codec_header, &mut codec_payload, &mut clock, false)
            .unwrap();
        assert!(matches!(
            codec_events.as_slice(),
            [MirrorEvent::CodecConfigured {
                codec: VideoCodec::H265,
                ..
            }]
        ));

        let body = [0, 0, 0, 2, 0x26, 0xbb];
        let mut encrypted = encrypt_payload(STREAM_ID, &body);
        let frame_header = header(0x00, encrypted.len(), 77);
        let events = processor
            .process_packet(&frame_header, &mut encrypted, &mut clock, false)
            .unwrap();
        let [MirrorEvent::VideoFrame(frame)] = events.as_slice() else {
            panic!("one decoded HEVC video frame is expected")
        };
        assert_eq!(frame.codec, VideoCodec::H265);
        assert_eq!(frame.nal_count, 4);
        assert_eq!(
            frame.data,
            [
                &[0, 0, 0, 1, 0x40, 0x01][..],
                &[0, 0, 0, 1, 0x42, 0x01][..],
                &[0, 0, 0, 1, 0x44, 0x01][..],
                &[0, 0, 0, 1, 0x26, 0xbb][..]
            ]
            .concat()
        );
    }

    #[test]
    fn reports_strip_exact_25000_byte_trailer() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        let mut clock = TestClock::default();
        let mut payload = vec![0x5a; FPS_TRAILER_BYTES + 3];
        let header = header(0x05, payload.len(), 99);
        assert_eq!(
            processor
                .process_packet(&header, &mut payload, &mut clock, true)
                .unwrap(),
            vec![MirrorEvent::ClientStreamingReport(vec![0x5a; 3])]
        );
    }

    #[test]
    fn malformed_nalu_length_becomes_the_source_invalid_marker() {
        let mut processor = MirrorPacketProcessor::new(&AUDIO_KEY).unwrap();
        processor.init_aes(STREAM_ID).unwrap();
        processor.codec = CodecState::H264;
        let mut clock = TestClock::default();
        let plain = [0, 0, 0, 8, 0x65, 0xaa];
        let mut encrypted = encrypt_payload(STREAM_ID, &plain);
        let header = header(0, encrypted.len(), 12);
        let events = processor
            .process_packet(&header, &mut encrypted, &mut clock, false)
            .unwrap();
        let [MirrorEvent::VideoFrame(frame)] = events.as_slice() else {
            panic!("invalid NAL framing is forwarded with source marker")
        };
        assert_eq!(frame.data[0], 1);
        assert_eq!(frame.nal_count, 0);
    }
}
