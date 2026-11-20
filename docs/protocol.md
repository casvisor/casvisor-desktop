# Protocol

Casvisor Desktop connects to Casvisor the same way as the network gateway agent (`casvisor agent`):

1. A WebSocket to `wss://<casvisor>/api/connect-agent` (or a TCP connection to Casvisor's SSH gateway port).
2. SSH over it, user `casvisor-agent`, public key authentication with an Ed25519 key derived from the device token: `seed = SHA-256("casvisor-agent:" + token)`. Casvisor only keeps the SHA-256 fingerprint of the public key, the token never goes over the wire.
3. The device checks Casvisor's SSH host key against the fingerprint it was given (`SHA256:...`).

The device never opens channels and refuses `dial@casvisor.org`: it is not a network gateway and doesn't reach other machines for Casvisor. Casvisor opens a `desktop@casvisor.org` channel for every remote session.

## Messages

A session channel carries messages, each one is:

| Field | Size |
|---|---|
| type | 1 byte |
| length | 4 bytes, big endian, length of the payload |
| payload | length bytes |

Casvisor relays the messages between the channel and the viewer's WebSocket without decoding the video, one message per WebSocket binary frame, in the same format. Multi-byte integers are big endian.

| Type | Name | Direction | Payload |
|---|---|---|---|
| 1 | hello | Casvisor → device | JSON, see below, always the first message of a channel |
| 2 | accept | device → Casvisor | JSON `{"accepted": bool, "message": string}` |
| 16 | video-config | device → viewer | JSON `{"codec": "avc1.42e028", "width": 1920, "height": 1080}`, sent before the first key frame and when the size changes |
| 17 | video | device → viewer | flags (1 byte, bit 0 = key frame), timestamp (8 bytes, microseconds), H.264 Annex B access unit |
| 18 | cursor | device → viewer | x (4 bytes, signed), y (4 bytes, signed), visible (1 byte), in video pixels |
| 19 | displays | device → viewer | JSON `{"displays": [{"index", "name", "left", "top", "width", "height", "primary"}], "current": 0}` |
| 32 | mouse | viewer → device | action (1 byte: 0 move, 1 down, 2 up, 3 wheel), button (1 byte: 0 left, 1 middle, 2 right, 3 back, 4 forward), x (2 bytes), y (2 bytes), delta x (2 bytes, signed), delta y (2 bytes, signed); x and y are in video pixels, deltas in wheel units of 120 per notch |
| 33 | key | viewer → device | scan code (2 bytes, 0xE0xx for extended keys), down (1 byte) |
| 34 | control | viewer → device | JSON `{"type": "keyframe"}`, `{"type": "display", "index": 1}` or `{"type": "ctrl-alt-del"}` |
| 48 | clipboard | both | UTF-8 text |
| 64 | notice | device or Casvisor → viewer | JSON `{"type": string, "message": string}` |

### hello

```json
{
  "version": 1,
  "session": "org/session-name",
  "user": "org/alice",
  "userDisplayName": "Alice",
  "clientIp": "203.0.113.5",
  "permissions": ["Connect", "Copy", "Paste"]
}
```

Unless the device is set to unattended, it asks the person in front of it whether to allow the session and answers with `accept`. No answer within 30 seconds is a refusal. After `accepted: true` it starts sending video.

### What Casvisor enforces

Casvisor doesn't trust the device or the viewer to keep to the permissions:

- From the viewer it only forwards mouse, key, control and clipboard messages, and clipboard only with the `Paste` permission.
- From the device it only forwards video-config, video, cursor, displays, clipboard and notice messages, and clipboard only with the `Copy` permission.
- Clipboard text in both directions goes into the audit log.
