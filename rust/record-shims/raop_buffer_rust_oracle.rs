#[path = "../app/src/rtp_wire.rs"]
mod rtp_wire;
#[path = "../app/src/raop_buffer.rs"]
mod raop_buffer;

use raop_buffer::{DequeueResult, EnqueueResult, RaopBuffer};

const MARKER: [u8; 4] = [0x00, 0x68, 0x34, 0x00];

fn make_packet(sequence: u16, timestamp: u32) -> Vec<u8> {
    let mut bytes = vec![0x80, 0x60];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&timestamp.to_be_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&MARKER);
    bytes
}

fn enqueue(buffer: &mut RaopBuffer, label: &str, sequence: u16, timestamp: u32, use_sequence: bool) {
    let packet = make_packet(sequence, timestamp);
    let result = match buffer.enqueue(&packet, use_sequence, |_| panic!("marker must bypass AES")) {
        Ok(EnqueueResult::Added) => 1,
        Ok(EnqueueResult::Ignored) => 0,
        Err(_) => -1,
    };
    println!("ENQ {label} rc={result}");
}

fn enqueue_length(buffer: &mut RaopBuffer, label: &str, length: usize) {
    let result = match buffer.enqueue(&vec![0; length], true, |_| panic!("invalid packet must not decrypt")) {
        Ok(EnqueueResult::Added) => 1,
        Ok(EnqueueResult::Ignored) => 0,
        Err(_) => -1,
    };
    println!("ENQ {label} rc={result}");
}

fn dequeue(buffer: &mut RaopBuffer, label: &str, no_resend: bool) {
    match buffer.dequeue(no_resend) {
        DequeueResult::Packet(packet) => {
            let data = packet.payload.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            println!("DEQ {label} seq={} ts={} len={} data={data}",
                     packet.sequence_number, packet.timestamp, packet.payload.len());
        }
        DequeueResult::Empty
        | DequeueResult::WaitingForResend
        | DequeueResult::MissingPacketSkipped => println!("DEQ {label} empty"),
    }
}

fn report_resend(buffer: &RaopBuffer, label: &str) {
    if let Some(range) = buffer.pending_resend() {
        println!("RESEND {label} seq={} count={}", range.sequence_number, range.count);
    }
}

fn main() {
    let mut buffer = RaopBuffer::new();

    dequeue(&mut buffer, "initial", false);
    enqueue_length(&mut buffer, "short", 11);
    enqueue_length(&mut buffer, "header-only", 12);

    enqueue(&mut buffer, "gap-first", 10, 0xabcdef01, true);
    enqueue(&mut buffer, "duplicate", 10, 0x11111111, true);
    enqueue(&mut buffer, "gap-third", 12, 0xabcdef03, true);
    dequeue(&mut buffer, "gap-first", false);
    report_resend(&buffer, "gap");
    dequeue(&mut buffer, "gap-wait", false);
    dequeue(&mut buffer, "gap-skip", true);
    dequeue(&mut buffer, "gap-last", false);
    enqueue(&mut buffer, "late-after-drain", 11, 0xabcdef02, true);

    buffer.flush(None);
    enqueue(&mut buffer, "wrap-last", u16::MAX, 0xffff0001, true);
    enqueue(&mut buffer, "wrap-zero", 0, 0x00000002, true);
    dequeue(&mut buffer, "wrap-last", false);
    dequeue(&mut buffer, "wrap-zero", false);

    buffer.flush(None);
    enqueue(&mut buffer, "window-old", 1, 1, true);
    enqueue(&mut buffer, "window-boundary", 257, 257, true);
    dequeue(&mut buffer, "window-boundary", false);

    buffer.flush(None);
    enqueue(&mut buffer, "sequence-fallback", 42, 42, false);
    dequeue(&mut buffer, "sequence-fallback", false);

    buffer.flush(None);
    enqueue(&mut buffer, "full-prime", 100, 100, true);
    dequeue(&mut buffer, "full-prime", false);
    let mut added = 0;
    for sequence in 102..=356 {
        let packet = make_packet(sequence, u32::from(sequence));
        if matches!(
            buffer.enqueue(&packet, true, |_| panic!("marker must bypass AES")),
            Ok(EnqueueResult::Added)
        ) {
            added += 1;
        }
    }
    println!("ENQ full-range count={added}");
    report_resend(&buffer, "full");
    dequeue(&mut buffer, "full-window-head", false);
    dequeue(&mut buffer, "full-window-next", false);

    println!("AES decrypt calls=0");
}
