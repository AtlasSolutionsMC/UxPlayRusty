use std::mem::{align_of, offset_of, size_of};
use uxplay_stream_types::{AudioDecodeStruct, VideoDecodeStruct};

fn main() {
    println!(
        "video:{},{},{},{},{},{},{},{}",
        size_of::<VideoDecodeStruct>(),
        align_of::<VideoDecodeStruct>(),
        offset_of!(VideoDecodeStruct, is_h265),
        offset_of!(VideoDecodeStruct, nal_count),
        offset_of!(VideoDecodeStruct, data),
        offset_of!(VideoDecodeStruct, data_len),
        offset_of!(VideoDecodeStruct, ntp_time_local),
        offset_of!(VideoDecodeStruct, ntp_time_remote),
    );
    println!(
        "audio:{},{},{},{},{},{},{},{},{},{}",
        size_of::<AudioDecodeStruct>(),
        align_of::<AudioDecodeStruct>(),
        offset_of!(AudioDecodeStruct, data),
        offset_of!(AudioDecodeStruct, ct),
        offset_of!(AudioDecodeStruct, data_len),
        offset_of!(AudioDecodeStruct, sync_status),
        offset_of!(AudioDecodeStruct, ntp_time_local),
        offset_of!(AudioDecodeStruct, ntp_time_remote),
        offset_of!(AudioDecodeStruct, rtp_time),
        offset_of!(AudioDecodeStruct, seqnum),
    );
}
