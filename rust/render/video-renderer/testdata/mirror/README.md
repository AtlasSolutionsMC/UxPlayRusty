# HEVC renderer fixture

`black-frame.hevc` is a single black 64x64 HEVC access unit encoded as an Annex-B stream with GStreamer `videotestsrc`, `x265enc` and `h265parse`. It is used to verify appsrc decode/output and H.265 MP4 finalization; it is not captured from an Apple device.

The checked-in stream was validated with `filesrc ! h265parse ! avdec_h265 ! fakesink` on the Windows MSYS2 GStreamer installation. To regenerate it where those elements are installed:

```sh
gst-launch-1.0 -q videotestsrc num-buffers=1 pattern=black ! video/x-raw,width=64,height=64,framerate=1/1 ! x265enc tune=zerolatency key-int-max=1 option-string='log-level=error:pools=1:frame-threads=1' ! h265parse config-interval=-1 ! filesink location=black-frame.hevc
```