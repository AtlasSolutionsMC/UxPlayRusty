# App replay

Build: `'D:\Save\Uxplay\UxPlayRusty\rust\app\target\release\uxplay.exe'`

Passed: 5/8

Fallback rate: 0.0% (0/8 cases answered by the original)

## Failing cases

### hls-master-missing-current-video

Arguments: `GET /master.m3u8`

response:

```diff
no response: An existing connection was forcibly closed by the remote host. (os error 10054)
```

### play-invalid-body-with-session

Arguments: `POST /play`

response:

```diff
no response: An existing connection was forcibly closed by the remote host. (os error 10054)
```

### playback-info

Arguments: `GET /playback-info`

response:

```diff
no response: An existing connection was forcibly closed by the remote host. (os error 10054)
```
