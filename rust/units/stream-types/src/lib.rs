//! Rust layout counterparts for UxPlay's internal decoded-frame callback structs.
//!
//! Adapted from `lib/stream.h`. These raw-pointer structs preserve field layout; application
//! code should use owned or borrowed Rust frame types instead of dereferencing them here.

// Copyright (c) 2019 dsafa22. Licensed under LGPL-2.1-or-later.

use std::os::raw::{c_int, c_uchar, c_ushort};
use std::ptr;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoDecodeStruct {
    pub is_h265: bool,
    pub nal_count: c_int,
    pub data: *mut c_uchar,
    pub data_len: c_int,
    pub ntp_time_local: u64,
    pub ntp_time_remote: u64,
}

impl Default for VideoDecodeStruct {
    fn default() -> Self {
        Self {
            is_h265: false,
            nal_count: 0,
            data: ptr::null_mut(),
            data_len: 0,
            ntp_time_local: 0,
            ntp_time_remote: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioDecodeStruct {
    pub data: *mut c_uchar,
    pub ct: c_uchar,
    pub data_len: c_int,
    pub sync_status: c_int,
    pub ntp_time_local: u64,
    pub ntp_time_remote: u64,
    pub rtp_time: u32,
    pub seqnum: c_ushort,
}

impl Default for AudioDecodeStruct {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            ct: 0,
            data_len: 0,
            sync_status: 0,
            ntp_time_local: 0,
            ntp_time_remote: 0,
            rtp_time: 0,
            seqnum: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioDecodeStruct, VideoDecodeStruct};

    #[test]
    fn default_values_are_empty_and_null() {
        let video = VideoDecodeStruct::default();
        assert!(!video.is_h265);
        assert_eq!(video.nal_count, 0);
        assert!(video.data.is_null());
        assert_eq!(video.data_len, 0);
        assert_eq!(video.ntp_time_local, 0);
        assert_eq!(video.ntp_time_remote, 0);

        let audio = AudioDecodeStruct::default();
        assert!(audio.data.is_null());
        assert_eq!(audio.ct, 0);
        assert_eq!(audio.data_len, 0);
        assert_eq!(audio.sync_status, 0);
        assert_eq!(audio.ntp_time_local, 0);
        assert_eq!(audio.ntp_time_remote, 0);
        assert_eq!(audio.rtp_time, 0);
        assert_eq!(audio.seqnum, 0);
    }
}
