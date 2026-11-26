# AAC-ELD mux fixture

`aac-eld-silence-44100-stereo.bin` is one raw stereo AAC-ELD access unit at
44.1 kHz. It was generated from 480 silent PCM samples per channel with the
Fraunhofer FDK AAC encoder installed in the Windows UCRT64 development image,
using Audio Object Type 39, raw MP4 transport, SBR disabled, and the 480-sample
granule length. Its generated AudioSpecificConfig is `f8e85000`, matching the
AAC-ELD caps in the unchanged UxPlay renderer and Rust muxer.

SHA-256: `85d785a75068ab269310c01f4834afaf9bf0eb4fabdf1088a2d036ee0acfa3d5`

The fixture is used only to exercise the application's real AAC-ELD input and
mixed H.264/AAC MP4 finalization path. It is not a recording from an Apple
device and does not establish playback interoperability.
