//! Deterministic NTP sample math from `lib/raop_ntp.c`.
//!
//! This is a partial subunit: it covers the four-timestamp sample equations,
//! sample aging/selection, root-distance calculation, and offset smoothing.
//! Packet validation, the UDP timing thread, kernel timestamps, locks, and
//! lifecycle remain part of the unported `raop_ntp.c` session.

use crate::ntp_math::diff_to_seconds;
use std::cmp::Ordering;

pub const NTP_SAMPLE_COUNT: usize = 8;
pub const NTP_MAX_DISPERSION_SECONDS: f64 = 16.0;
pub const NTP_MAX_ROOT_DISTANCE_SECONDS: f64 = 1.5;
const NTP_SERVER_PRECISION_SECONDS: f64 = 0.001;
const NTP_CLIENT_PRECISION_SECONDS: f64 = 0.001;
const NTP_CLOCK_PHI: f64 = 15e-6;
const NTP_OFFSET_ALPHA: f64 = 0.05;
const NANOS_PER_SECOND: f64 = 1e9;

/// One completed NTP timing exchange, stored in the same form as the C ring
/// buffer's `raop_ntp_data_t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NtpSample {
    /// T4, local packet-arrival time, as Q32.32.
    pub time_q32_32: u64,
    pub dispersion_seconds: f64,
    pub delay_seconds: f64,
    /// Remote/client minus local/server offset, in seconds.
    pub offset_seconds: f64,
}

impl NtpSample {
    /// C initializes each ring-buffer slot with zero time and a max-dispersion
    /// delay. Such slots are skipped by the filter.
    pub const fn empty() -> Self {
        Self {
            time_q32_32: 0,
            dispersion_seconds: NTP_MAX_DISPERSION_SECONDS,
            delay_seconds: NTP_MAX_DISPERSION_SECONDS,
            offset_seconds: 0.0,
        }
    }

    /// Apply the source equations to a valid `(T1, T2, T3, T4)` exchange.
    /// Timestamp inputs are the source's unsigned Q32.32 values.
    pub fn from_exchange(t1: u64, t2: u64, t3: u64, t4: u64) -> Self {
        let total_time = diff_to_seconds(t4, t1);
        let client_processing_time = diff_to_seconds(t3, t2);
        let client_to_server = diff_to_seconds(t2, t1);
        let server_to_client = diff_to_seconds(t3, t4);
        let mut delay = total_time - client_processing_time;
        if delay < NTP_SERVER_PRECISION_SECONDS {
            delay = NTP_SERVER_PRECISION_SECONDS;
        }

        Self {
            time_q32_32: t4,
            dispersion_seconds: NTP_SERVER_PRECISION_SECONDS
                + NTP_CLIENT_PRECISION_SECONDS
                + NTP_CLOCK_PHI * total_time,
            delay_seconds: delay,
            offset_seconds: (client_to_server + server_to_client) / 2.0,
        }
    }
}

impl Default for NtpSample {
    fn default() -> Self {
        Self::empty()
    }
}

/// The synchronized estimate the C thread publishes under its sync mutex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NtpFilterEstimate {
    pub valid_sample_count: usize,
    pub best_sample: NtpSample,
    pub root_distance_seconds: f64,
    pub root_distance_nanoseconds: u64,
    pub sync_delay_seconds: f64,
    pub sync_offset_nanoseconds: i64,
    pub is_synced: bool,
}

/// Age, order, and combine the fixed eight-slot sample buffer.
///
/// `previous_offset_nanoseconds` models the thread's `have_offset` state. The
/// source uses the previous published offset for 95% of a synchronized update
/// and the newly selected sample for 5%. When root distance is above 1.5s it
/// publishes the best sample directly and marks the clock unsynchronized.
///
/// The C code's `qsort` does not specify the order of equal-delay samples;
/// this implementation keeps their original ring-buffer order. Ordinary
/// finite, distinct-delay samples have the same selection as the C comparator.
pub fn estimate(
    samples: &mut [NtpSample; NTP_SAMPLE_COUNT],
    now_q32_32: u64,
    previous_offset_nanoseconds: Option<i64>,
) -> Option<NtpFilterEstimate> {
    let mut valid = Vec::with_capacity(NTP_SAMPLE_COUNT);

    for sample in samples.iter_mut() {
        if sample.delay_seconds == NTP_MAX_DISPERSION_SECONDS || sample.time_q32_32 == 0 {
            continue;
        }

        let elapsed_seconds = diff_to_seconds(now_q32_32, sample.time_q32_32);
        let aged_dispersion = sample.dispersion_seconds + elapsed_seconds * NTP_CLOCK_PHI;
        if aged_dispersion >= NTP_MAX_DISPERSION_SECONDS {
            sample.dispersion_seconds = NTP_MAX_DISPERSION_SECONDS;
            continue;
        }

        sample.dispersion_seconds = aged_dispersion;
        valid.push(*sample);
    }

    // Match `raop_ntp_compare`: ascending delay; comparisons that are neither
    // less nor greater (including NaN) are treated as equal by the C callback.
    valid.sort_by(|a, b| {
        a.delay_seconds
            .partial_cmp(&b.delay_seconds)
            .unwrap_or(Ordering::Equal)
    });
    let best_sample = *valid.first()?;

    // Match the source implementation exactly: `double inverse_two_pow_n[8] = { 1.0 }`
    // zero-initializes the remaining seven entries, and the following loop only divides
    // those zeros. The C filter therefore weights the best sample by 1/2 and every other
    // sample by zero. Although the apparent intent is geometric rank weighting, changing
    // this changes root_distance and can change the externally visible is_synced flag.
    let inverse_two_pow_n = [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let total_filter_dispersion = valid
        .iter()
        .zip(inverse_two_pow_n)
        .map(|(sample, weight)| sample.dispersion_seconds * weight)
        .sum::<f64>();
    let root_distance_seconds = total_filter_dispersion + best_sample.delay_seconds * 0.5;
    let is_synced = root_distance_seconds <= NTP_MAX_ROOT_DISTANCE_SECONDS;

    let selected_offset_seconds = if is_synced {
        if let Some(previous_offset_nanoseconds) = previous_offset_nanoseconds {
            let previous_seconds = previous_offset_nanoseconds as f64 * 1e-9;
            (1.0 - NTP_OFFSET_ALPHA) * previous_seconds
                + NTP_OFFSET_ALPHA * best_sample.offset_seconds
        } else {
            best_sample.offset_seconds
        }
    } else {
        best_sample.offset_seconds
    };

    // The source converts finite, representable values with C casts, which
    // truncate toward zero. Rust's float casts do the same in that domain.
    let sync_offset_nanoseconds = (selected_offset_seconds * NANOS_PER_SECOND) as i64;
    let root_distance_nanoseconds = (root_distance_seconds * NANOS_PER_SECOND) as u64;

    Some(NtpFilterEstimate {
        valid_sample_count: valid.len(),
        best_sample,
        root_distance_seconds,
        root_distance_nanoseconds,
        sync_delay_seconds: best_sample.delay_seconds,
        sync_offset_nanoseconds,
        is_synced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn q32(seconds: u32) -> u64 {
        (seconds as u64) << 32
    }

    fn sample(time: u64, dispersion: f64, delay: f64, offset: f64) -> NtpSample {
        NtpSample {
            time_q32_32: time,
            dispersion_seconds: dispersion,
            delay_seconds: delay,
            offset_seconds: offset,
        }
    }

    #[test]
    fn exchange_equations_match_source_fixture() {
        // Source-derived from raop_ntp.c's T1..T4 equations:
        // total=.75s, client processing=.25s, delay=.5s,
        // offset=(.25 + -.25)/2=0, dispersion=.002 + 15e-6*.75.
        let actual = NtpSample::from_exchange(
            q32(10),
            q32(10) + (1 << 30),
            q32(10) + (1 << 31),
            q32(10) + 3 * (1 << 30),
        );
        assert_eq!(actual.time_q32_32, q32(10) + 3 * (1 << 30));
        assert_eq!(actual.delay_seconds, 0.5);
        assert_eq!(actual.offset_seconds, 0.0);
        assert!((actual.dispersion_seconds - 0.00201125).abs() < 1e-15);
    }

    #[test]
    fn minimum_delay_clamps_to_one_millisecond() {
        // A short/nonpositive measured delay follows the source's RFC floor.
        let actual = NtpSample::from_exchange(q32(5), q32(5), q32(5), q32(5));
        assert_eq!(actual.delay_seconds, 0.001);
        assert_eq!(actual.dispersion_seconds, 0.002);
    }

    #[test]
    fn filter_preserves_c_zero_initialized_dispersion_weights() {
        let now = q32(100);
        let mut samples = [NtpSample::empty(); NTP_SAMPLE_COUNT];
        samples[0] = sample(now, 0.4, 0.3, 0.1);
        samples[1] = sample(now, 0.2, 0.1, 0.2);
        samples[2] = sample(now, 0.8, 0.2, -0.4);

        let result = estimate(&mut samples, now, None).expect("three valid samples");
        // Delay order is .1, .2, .3. In C the {1.0} initializer leaves the
        // remaining weights zero; total dispersion is .2/2 and root distance .15.
        assert_eq!(result.valid_sample_count, 3);
        assert_eq!(result.best_sample.offset_seconds, 0.2);
        assert!((result.root_distance_seconds - 0.15).abs() < 1e-15);
        assert_eq!(result.root_distance_nanoseconds, 150_000_000);
        assert_eq!(result.sync_offset_nanoseconds, 200_000_000);
        assert!(result.is_synced);
    }

    #[test]
    fn synchronized_offset_uses_five_percent_new_sample() {
        let now = q32(20);
        let mut samples = [NtpSample::empty(); NTP_SAMPLE_COUNT];
        samples[0] = sample(now, 0.01, 0.02, 0.2);

        let result = estimate(&mut samples, now, Some(100_000_000)).expect("valid sample");
        assert_eq!(result.sync_offset_nanoseconds, 105_000_000);
        assert!(result.is_synced);
    }

    #[test]
    fn excessive_root_distance_marks_unsynced_without_smoothing() {
        let now = q32(20);
        let mut samples = [NtpSample::empty(); NTP_SAMPLE_COUNT];
        samples[0] = sample(now, 0.5, 3.0, -0.25);

        let result = estimate(&mut samples, now, Some(100_000_000)).expect("valid sample");
        assert_eq!(result.root_distance_seconds, 1.75);
        assert_eq!(result.root_distance_nanoseconds, 1_750_000_000);
        assert_eq!(result.sync_offset_nanoseconds, -250_000_000);
        assert!(!result.is_synced);
    }

    #[test]
    fn dispersion_ages_and_expired_samples_are_capped_and_excluded() {
        let now = q32(1_000);
        let mut samples = [NtpSample::empty(); NTP_SAMPLE_COUNT];
        samples[0] = sample(q32(800), 15.999, 0.001, 9.0);
        samples[1] = sample(q32(999), 0.01, 0.1, 0.125);

        let result = estimate(&mut samples, now, None).expect("one non-expired sample");
        assert_eq!(samples[0].dispersion_seconds, NTP_MAX_DISPERSION_SECONDS);
        assert_eq!(result.valid_sample_count, 1);
        assert_eq!(result.best_sample.offset_seconds, 0.125);
        assert!((samples[1].dispersion_seconds - 0.010015).abs() < 1e-15);
    }

    #[test]
    fn no_valid_sample_yields_no_new_estimate() {
        let mut samples = [NtpSample::empty(); NTP_SAMPLE_COUNT];
        samples[0] = sample(0, 0.0, 0.01, 0.0);
        assert_eq!(estimate(&mut samples, q32(1), None), None);
    }
}
