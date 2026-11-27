# HTTP contract

The service has two listeners: the public one (`Proxy.ListenUrl`) and the admin one (`127.0.0.1:Proxy.AdminPort`).

On the public listener, everything under `/_comptroller/` belongs to the service and is never forwarded. All other paths are proxied to the upstream.

## Proxy checks

Applied in this order:

1. **Client address.** Outside `Access.AllowedCidrs` → 403. An empty list allows any address.
2. **Token** (`Identity.TokenHeaderName`), on every proxied request except `HEAD /api/hello`, which Claude Code sends without custom headers.
3. The token header is removed before forwarding. `Authorization`, `x-api-key` and `anthropic-*` are never touched.

| Situation | Status | `error.type` |
| - | - | - |
| Token missing or unknown | 401 | `authentication_error` |
| Device suspended or revoked, user revoked, token expired | 403 | `permission_error` |
| Address not allowed | 403 | `permission_error` |

Errors use Anthropic's shape, so Claude Code shows the message:

```json
{ "type": "error", "error": { "type": "permission_error", "message": "Disconnected from the tray on LAPTOP-1. Click Reconnect." } }
```

## Endpoints

Every `/_comptroller/` request must send `X-Comptroller-Client` (any value). Without it the answer is 400. This stops a web page from using the developer's Windows sign-in.

"Token" means the token header. "Windows" means Negotiate authentication, used when `Identity.Mode` is `windows`.

| Endpoint | Auth | Purpose |
| - | - | - |
| `GET /_comptroller/connect?device=NAME` | Windows (+ token if the device has one) | Windows mode: issue or refresh the device token |
| `POST /_comptroller/connect` `{"code": "XXXX-XXXX-XXXX"}` | one-time code | Manual mode: redeem a code for a token |
| `POST /_comptroller/connect` (no body) | token | Manual mode: refresh |
| `GET /_comptroller/info` | none | How to sign in: see below |
| `GET /_comptroller/preflight/stream` | none | Five server-sent events 200 ms apart, for the admin preflight test |
| `GET /_comptroller/me` | token | Who and what this token is. Works while suspended or expired |
| `POST /_comptroller/disconnect` | token | Suspend this device |
| `POST /_comptroller/reconnect?device=NAME` | Windows (+ token) or, in manual mode, token | Reactivate a device the user disconnected |

`GET /_comptroller/info` needs no sign-in: it tells a new client which mode the server uses.

### Session response

`connect` and `reconnect` return:

```json
{ "token": "…", "user": "CORP\\jdoe", "device": "LAPTOP-1", "status": "active", "expiresUtc": "2026-12-04T09:00:00.000Z" }
```

**Refreshing keeps the same token** and moves its expiry, so Claude Code sessions already running carry on. A new token is issued only when the request doesn't present this device's current one.

Refreshing never undoes a disconnect or a suspension. In windows mode, a refresh that fails the group check expires all of that user's tokens at once.

In manual mode an expired token can't be refreshed; the admin issues a new code.

### `info` response

```json
{ "organizationName": "", "mode": "windows", "allowedGroup": "Claude Code Users", "tokenHeader": "X-Comptroller-Token" }
```

`allowedGroup` is null in manual mode.

### `me` response

```json
{
  "user": "CORP\\jdoe", "device": "LAPTOP-1",
  "status": "active", "suspendedBy": null, "expired": false,
  "expiresUtc": "2026-12-04T09:00:00.000Z",
  "today": { "requests": 12, "limitHits": 1, "lastRequestUtc": "2026-11-27T14:02:11.120Z" }
}
```

`today` counts this computer's requests since midnight on the server. `limitHits` is how many were answered 429, which on a subscription seat usually means a plan limit. `lastRequestUtc` is null before the first one.

`status` is `active`, `suspended` or `revoked`. A revoked user reports `revoked`.

### Errors

Same shape as the proxy errors. 400 bad request, 401 not signed in or unknown token, 403 not allowed, 404 unknown endpoint.

## Health

`GET /health` on the admin listener only, with no authentication. Everything else there is 404. On the public listener `/health` is an ordinary proxied path.

```json
{
  "version": "0.1.0",
  "startedUtc": "2026-11-27T09:00:00.000Z",
  "uptimeSeconds": 273600,
  "windowsService": true,
  "config": { "status": "ok", "error": null, "path": "C:\\…\\comptroller.json", "loadedUtc": "2026-11-27T09:00:00.000Z" },
  "schemaVersion": 3,
  "listenUrl": "http://localhost:8080",
  "upstreamUrl": "https://api.anthropic.com",
  "inFlight": {
    "count": 1,
    "requests": [
      { "user": "CORP\\jdoe", "device": "LAPTOP-1", "method": "POST", "path": "/v1/messages", "startedUtc": "2026-11-30T09:12:04.512Z" }
    ]
  },
  "recording": { "queued": 0, "dropped": 0, "error": null },
  "retention": {
    "days": 14,
    "nextPruneUtc": "2026-11-28T07:00:00.000Z",
    "lastPrune": { "finishedUtc": "2026-11-27T09:00:02.140Z", "removed": 1204, "freedBytes": 393216, "error": null }
  }
}
```

- `inFlight` lists up to 50 requests being forwarded now, oldest first. `count` is all of them.
- `retention` is the black box's: `days` kept (today and the days before it, by the server's calendar), when the nightly prune runs next, and what the last one did. `lastPrune` is null until the first, which runs a few seconds after the service starts.
- `recording.queued` is finished requests waiting to be written to the database, `dropped` how many were never written because the database fell far behind, and `error` the last write failure, if any.

- `config.status` is `error` when the file on disk was rejected. `error` says why, and the last good config stays in use.
- `listenUrl` is what the service is listening on now. If the file says something else, a restart is needed.
- `windowsService` is false for a console run.
- The admin terminal reads this every 2 seconds; it's not logged.
