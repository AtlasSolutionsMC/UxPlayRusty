# App replay

Build: `'D:\Save\Uxplay\UxPlayRusty\rust\app\target\debug\uxplay.exe'`

Passed: 43/118

Fallback rate: 63.6% (75/118 cases answered by the original)

## Failing cases

### trap-001-rc

Arguments: `-rc 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
binary content differs (75 bytes expected, 0 bytes got)
```

### trap-002-rc

Arguments: `-rc '👍🏽 thumbs'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
- *** ERROR: startup file ???? thumbs specified by option -rc was not found
```

### trap-003-rc

Arguments: `-rc 日本語`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
- *** ERROR: startup file ??? specified by option -rc was not found
```

### trap-005-rc

Arguments: `-rc 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
binary content differs (66 bytes expected, 0 bytes got)
```

### trap-006-rc

Arguments: `-rc İstanbul`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
- *** ERROR: startup file Istanbul specified by option -rc was not found
```

### trap-007-rc

Arguments: `-rc straße`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
binary content differs (70 bytes expected, 0 bytes got)
```

### trap-008-rc

Arguments: `-rc 'ılık ırmak'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
- *** ERROR: startup file ilik irmak specified by option -rc was not found
```

### trap-009-rc

Arguments: `-rc '👨‍👩‍👧 family'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
- *** ERROR: startup file ???????? family specified by option -rc was not found
```

### trap-010-rc

Arguments: `-rc 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stdout:

```diff
binary content differs (67 bytes expected, 0 bytes got)
```

### trap-011-s

Arguments: `-s 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-012-s

Arguments: `-s '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???? thumbs"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-013-s

Arguments: `-s 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-015-s

Arguments: `-s 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-016-s

Arguments: `-s İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s Istanbul"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-017-s

Arguments: `-s straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-018-s

Arguments: `-s 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ilik irmak"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-019-s

Arguments: `-s '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???????? family"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-020-s

Arguments: `-s 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-021-fps

Arguments: `-fps 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-022-fps

Arguments: `-fps '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-fps ???? thumbs"; -fps n : max n=255, default n=30
```

### trap-023-fps

Arguments: `-fps 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-fps ???"; -fps n : max n=255, default n=30
```

### trap-025-fps

Arguments: `-fps 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-026-fps

Arguments: `-fps İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-fps Istanbul"; -fps n : max n=255, default n=30
```

### trap-027-fps

Arguments: `-fps straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-028-fps

Arguments: `-fps 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-fps ilik irmak"; -fps n : max n=255, default n=30
```

### trap-029-fps

Arguments: `-fps '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-fps ???????? family"; -fps n : max n=255, default n=30
```

### trap-030-fps

Arguments: `-fps 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-031-f

Arguments: `-f 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-032-f

Arguments: `-f '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-f ???? thumbs" , unknown flip type, choices are H, V, I
```

### trap-033-f

Arguments: `-f 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-f ???" , unknown flip type, choices are H, V, I
```

### trap-035-f

Arguments: `-f 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-036-f

Arguments: `-f İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-f Istanbul" , unknown flip type, choices are H, V, I
```

### trap-037-f

Arguments: `-f straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-038-f

Arguments: `-f 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-f ilik irmak" , unknown flip type, choices are H, V, I
```

### trap-039-f

Arguments: `-f '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-f ???????? family" , unknown flip type, choices are H, V, I
```

### trap-040-f

Arguments: `-f 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-041-r

Arguments: `-r 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-042-r

Arguments: `-r '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-r ???? thumbs" , unknown rotation  type, choices are R, L
```

### trap-043-r

Arguments: `-r 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-r ???" , unknown rotation  type, choices are R, L
```

### trap-045-r

Arguments: `-r 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-046-r

Arguments: `-r İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-r Istanbul" , unknown rotation  type, choices are R, L
```

### trap-047-r

Arguments: `-r straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-048-r

Arguments: `-r 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-r ilik irmak" , unknown rotation  type, choices are R, L
```

### trap-049-r

Arguments: `-r '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-r ???????? family" , unknown rotation  type, choices are R, L
```

### trap-050-r

Arguments: `-r 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-051-async

Arguments: `-async -7`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- UxPlay 1.74: An Open-Source AirPlay mirroring and audio-streaming server.
- Required gstreamer plugin 'libav' not found
- Missing plugin is contained in  '[GStreamer 1.x]-libav'
- Required gstreamer plugin 'autodetect' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-good'
- Required gstreamer plugin 'videoparsersbad' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-bad'
- 
- if the plugin is installed, but not found, your gstreamer registry may have been corrupted.
- to rebuild it when gstreamer next starts, clear your gstreamer cache with:
- "rm -rf ~/.cache/gstreamer-1.0"
- 
- *** ERROR: stopping
```

### trap-052-async

Arguments: `-async 0`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- UxPlay 1.74: An Open-Source AirPlay mirroring and audio-streaming server.
- Required gstreamer plugin 'libav' not found
- Missing plugin is contained in  '[GStreamer 1.x]-libav'
- Required gstreamer plugin 'autodetect' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-good'
- Required gstreamer plugin 'videoparsersbad' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-bad'
- 
- if the plugin is installed, but not found, your gstreamer registry may have been corrupted.
- to rebuild it when gstreamer next starts, clear your gstreamer cache with:
- "rm -rf ~/.cache/gstreamer-1.0"
- 
- *** ERROR: stopping
```

### trap-053-async

Arguments: `-async -1`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- UxPlay 1.74: An Open-Source AirPlay mirroring and audio-streaming server.
- Required gstreamer plugin 'libav' not found
- Missing plugin is contained in  '[GStreamer 1.x]-libav'
- Required gstreamer plugin 'autodetect' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-good'
- Required gstreamer plugin 'videoparsersbad' not found
- Missing plugin is contained in  '[GStreamer 1.x]-plugins-bad'
- 
- if the plugin is installed, but not found, your gstreamer registry may have been corrupted.
- to rebuild it when gstreamer next starts, clear your gstreamer cache with:
- "rm -rf ~/.cache/gstreamer-1.0"
- 
- *** ERROR: stopping
```

### trap-059-p

Arguments: `-p 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-060-p

Arguments: `-p '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- *** ERROR: invalid "-p ???? thumbs", all 3 ports must be in range [1024,65535]
```

### trap-061-p

Arguments: `-p 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- *** ERROR: invalid "-p ???", all 3 ports must be in range [1024,65535]
```

### trap-063-p

Arguments: `-p 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-064-p

Arguments: `-p İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- *** ERROR: invalid "-p Istanbul", all 3 ports must be in range [1024,65535]
```

### trap-065-p

Arguments: `-p straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-066-p

Arguments: `-p 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- *** ERROR: invalid "-p ilik irmak", all 3 ports must be in range [1024,65535]
```

### trap-067-p

Arguments: `-p '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stdout:

```diff
- *** ERROR: invalid "-p ???????? family", all 3 ports must be in range [1024,65535]
```

### trap-068-p

Arguments: `-p 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-077-s

Arguments: `-s 'héllo wörld' -fps bad`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-078-s

Arguments: `-s '👍🏽 thumbs' -fps bad`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???? thumbs"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-079-s

Arguments: `-s 日本語 -fps bad`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-081-s

Arguments: `-s 'é' -fps bad`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-082-s

Arguments: `-s İstanbul -fps bad`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s Istanbul"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-083-s

Arguments: `-s straße -fps bad`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-084-s

Arguments: `-s 'ılık ırmak' -fps bad`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ilik irmak"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-085-s

Arguments: `-s '👨‍👩‍👧 family' -fps bad`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s ???????? family"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-086-s

Arguments: `-s 'a b' -fps bad`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```

### trap-087-s

Arguments: `-s bad -fps 'héllo wörld'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (90 bytes expected, 0 bytes got)
```

### trap-088-s

Arguments: `-s bad -fps '👍🏽 thumbs'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s bad"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-089-s

Arguments: `-s bad -fps 日本語`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s bad"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-091-s

Arguments: `-s bad -fps 'é'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (81 bytes expected, 0 bytes got)
```

### trap-092-s

Arguments: `-s bad -fps İstanbul`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s bad"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-093-s

Arguments: `-s bad -fps straße`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (85 bytes expected, 0 bytes got)
```

### trap-094-s

Arguments: `-s bad -fps 'ılık ırmak'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s bad"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-095-s

Arguments: `-s bad -fps '👨‍👩‍👧 family'`

status:

```diff
expected exit code 1, got exit code 53
```

stderr:

```diff
- invalid "-s bad"; -s wxh : max w,h=9999; -s wxh@r : max r=255
```

### trap-096-s

Arguments: `-s bad -fps 'a b'`

status:

```diff
expected exit code 0, got exit code 53
```

stderr:

```diff
binary content differs (82 bytes expected, 0 bytes got)
```
