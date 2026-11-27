# claude-comptroller

A self-hosted control tower for a small team's Claude Code traffic. ("Comptroller" is pronounced "controller".)

An on-prem Windows service sits between your developers' Claude Code and Anthropic. It knows who each request comes from (Windows user and device), and gives you one place to watch it and one thing to unplug. Developers keep using their own Claude Team, Pro or Max seats: the service holds no Anthropic credential and never logs prompt or response bodies.

Not affiliated with or endorsed by Anthropic.

## Status

Early development. **Not ready to roll out to a team.**

| Part | State |
| - | - |
| Proxy to `api.anthropic.com` | Works |
| Identity: Windows sign-in or one-time codes, device tokens, disconnect and reconnect | Works |
| Running as a Windows Service, health endpoint | Works. There is no installer yet |
| Tray app for developer computers | Not built |
| Admin terminal UI, usage recording, cost view, archives | Not built |

What has been tried against the real API is in [docs/testing-status.md](docs/testing-status.md).

## How it works

1. A developer's computer signs in to the service and receives a device token.
2. Claude Code is pointed at the service with `ANTHROPIC_BASE_URL`, and sends the token in `X-Comptroller-Token` through `ANTHROPIC_CUSTOM_HEADERS`.
3. The service checks the token, removes it, and forwards the request with the developer's own login untouched.

Never set `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY` or `apiKeyHelper` on those computers: any of them switches Claude Code to API billing.

The endpoints and error messages are described in [contracts/api.md](contracts/api.md).

## Try it on one computer

Needs Windows, the [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0) and Claude Code.

```powershell
.\dev.ps1          # fake upstream: uses no seat
.\dev.ps1 -Real    # the real api.anthropic.com: uses your seat
```

Then run the lines it prints in a second terminal. They set environment variables for that terminal only, so closing it puts everything back.

If you put these variables in a Claude Code or VS Code settings file instead, remove them before you stop the service. Claude Code can't work while it points at a service that isn't running.

Run the tests with `dotnet test service/Comptroller.slnx`.

## Running as a Windows Service

The service runs in a console or under Windows Service Control, and detects which. Publish it as one self-contained exe:

```powershell
dotnet publish service/Comptroller.Service -c Release -r win-x64 --self-contained -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -o publish/service
```

Installing is a job for the admin UI, which isn't built yet. To check that the service works on a machine, run this in an administrator terminal. It installs a temporary service, tests it against the fake upstream and removes it:

```powershell
.\dev\service-test.ps1
```

While running, `http://127.0.0.1:8081/health` reports the version, uptime and whether the config file was accepted. It listens on the server itself only.

Stopping waits for replies in flight, up to `Proxy.ShutdownTimeoutSeconds`.

## Configuration

One file, `comptroller.json`. Start from [contracts/comptroller.sample.json](contracts/comptroller.sample.json) and pass it with `--config <path>`. Changes apply while running, except the listen URL, admin port, shutdown timeout, data folder and logging. An invalid file is ignored and the last good one stays in use.

| Setting | Default | Meaning |
| - | - | - |
| `Proxy.ListenUrl` | `http://localhost:8080` | Where the service listens |
| `Proxy.AdminPort` | 8081 | Port for `/health`, on `127.0.0.1` only |
| `Proxy.ShutdownTimeoutSeconds` | 60 | How long a stop waits for replies in flight (5–600) |
| `Proxy.UpstreamUrl` | `https://api.anthropic.com` | Where requests go |
| `Proxy.ActivityTimeoutMinutes` | 10 | How long a silent request may last (1–60) |
| `Proxy.StripForwardedHeaders` | true | Don't add `X-Forwarded-*` |
| `Access.AllowedCidrs` | this computer only | Networks that may connect. Empty allows any |
| `Identity.Mode` | `windows` | `windows`, or `manual` for one-time codes |
| `Identity.AllowedGroup` | `Claude Code Users` | Windows group name or SID that may connect |
| `Identity.TokenLifetimeDays` | 7 | 1–90 |
| `Identity.CodeLifetimeHours` | 24 | How long a one-time code works (1–168) |
| `Identity.TokenHeaderName` | `X-Comptroller-Token` | |
| `Data.Folder` | `data` | Database folder, relative to the config file |
| `Logging.Folder` · `Level` · `RetainedFiles` · `FileSizeLimitMb` | `logs` · `Information` · 14 · 20 | |

In `manual` mode, create a code for a user and device with:

```powershell
Comptroller.Service.exe --config comptroller.json --create-code jdoe LAPTOP-1
```

## Known limits

- Only Claude Code traffic goes through the service. Claude Desktop's Chat and Cowork tabs don't.
- Remote Control is unavailable while `ANTHROPIC_BASE_URL` points anywhere but Anthropic.
- The Claude Desktop Code tab hasn't been tested.
- Claude Code sends some non-model traffic (version checks, telemetry, sign-in) directly to Anthropic.

## License

[MIT](LICENSE)
