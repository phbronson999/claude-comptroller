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
| Tray app for developer computers | Works. There is no installer yet |
| Recording each request with its token usage | Works against the fake upstream; not yet checked against the real API |
| Admin terminal: install, start, stop, status, preflight test, settings, users, radar, dashboard, cost | Works |
| Admin terminal: black box, archives and baseline | Not built |

What has been tried against the real API is in [docs/testing-status.md](docs/testing-status.md).

## How it works

1. A developer's computer signs in to the service and receives a device token.
2. Claude Code is pointed at the service with `ANTHROPIC_BASE_URL`, and sends the token in `X-Comptroller-Token` through `ANTHROPIC_CUSTOM_HEADERS`.
3. The service checks the token, removes it, and forwards the request with the developer's own login untouched.
4. When the reply has finished, the service records who sent it, when, the path, status, time taken and size, and for model replies the model and token counts. It reads those from the reply as it streams past, without holding it up. It never stores a prompt or a reply.

Never set `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY` or `apiKeyHelper` on those computers: any of them switches Claude Code to API billing.

The endpoints and error messages are described in [contracts/api.md](contracts/api.md).

## Try it on one computer

Needs Windows, the [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0) and Claude Code.

```powershell
.\dev.ps1          # fake upstream: uses no seat
.\dev.ps1 -Seed    # the same, after adding three weeks of traffic from four example people
.\dev.ps1 -Real    # the real api.anthropic.com: uses your seat
```

Then run the lines it prints in a second terminal. They set environment variables for that terminal only, so closing it puts everything back.

If you put these variables in a Claude Code or VS Code settings file instead, remove them before you stop the service. Claude Code can't work while it points at a service that isn't running.

Run the tests with `dotnet test service/Comptroller.slnx`.

## The admin terminal

`claude-comptroller.exe` is the only command an admin types. It has no subcommands: every action is a key on its home screen. It has the service exe built in, so on a new server it's the only file you need.

| Key | Does |
| - | - |
| `i` | Install: writes the service to `%ProgramFiles%\claude-comptroller`, a default `comptroller.json` (an existing one is kept), registers the service (delayed automatic start, restart on crash, virtual account `NT SERVICE\claude-comptroller`), creates the data and log folders, starts it, and runs the preflight test |
| `t` · `x` | Takeoff (start) and Terminate (stop; requests in flight finish first) |
| `d` | Dashboard: the same cards as Radar for today or the last 7 or 14 days (`p`), requests per hour or day against the period before, each person's share with their plan-limit hits and p95, latency, and the latest requests |
| `4` | Cost: what each person's seat costs against what their recorded tokens would cost at API list prices, for this billing month (`m`), the last 30 days (`3`) or the seat year (`y`), with a projection to the end of the period, a breakdown by model and the token mix. `s` edits the seat plans and their prices |
| `r` | Radar: today's requests, slowest replies (p95), plan limits hit (429s) and errors, above a live feed of requests. Requests in flight show first. `f` picks a person, `e` shows only errors and plan limits, space pauses, enter shows one request |
| `s` | Status: the service, the proxy and health checks, with the commands they read |
| `p` | Preflight test: config, service, proxy, a request through to Anthropic, and that replies stream without buffering. It sends no credentials, so it uses no seat |
| `u` | Uninstall: stops and removes the service; data, logs and config are kept unless you choose otherwise |
| `c` | Settings: every value in `comptroller.json` with its default and limits. Changes are saved together with `s`; the old file is kept as `comptroller.json.bak`, and comments and anything else in the file are kept. Settings the service reads only at startup are marked, and saving one offers to restart the service |
| `v` | Users: each person and the computers they connected, with status, requests today, last seen and token expiry. `p` puts a person on a seat plan. Suspend or resume a computer, revoke it, or remove it so it can connect again as new; revoke or remove a person. In `manual` mode, `n` creates a one-time code. Changes apply to the next request |

Installing, starting, stopping, saving settings and changing users need an administrator terminal; if it isn't one, it offers to reopen itself as administrator. Each step shows the `sc.exe`, `icacls` or `sqlite3` command that does the same by hand.

A computer the developer disconnected from the tray app can only be reconnected by them. An admin suspension can only be lifted by an admin.

With no service installed, it finds a service running in a console (for example from `dev.ps1`) through `/health` and says so.

To build it with the service inside, publish the service first (see below), then `cargo build --release --manifest-path tui\Cargo.toml`. `build.ps1` will do both in a later step.

## Running as a Windows Service

The service runs in a console or under Windows Service Control, and detects which. Publish it as one self-contained exe:

```powershell
dotnet publish service/Comptroller.Service -c Release -r win-x64 --self-contained -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true -o publish/service
```

The admin terminal installs it. To check by hand that the service works on a machine, run this in an administrator terminal. It installs a temporary service, tests it against the fake upstream and removes it:

```powershell
.\dev\service-test.ps1
```

While running, `http://127.0.0.1:8081/health` reports the version, uptime and whether the config file was accepted. It listens on the server itself only.

Stopping waits for replies in flight, up to `Proxy.ShutdownTimeoutSeconds`.

## The tray app

`claude-comptroller-connect` runs on each developer computer. On first run it asks for the server address, signs in (silently with Windows, or with a one-time code), and writes two settings files:

- `%USERPROFILE%\.claude\settings.json`: the `env` block, used by the CLI and the Desktop Code tab
- VS Code's user `settings.json`: `claudeCode.environmentVariables`, used by the VS Code extension

Only `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS` are written. Everything else in those files, including comments, is kept, and the originals are backed up under `%APPDATA%\claude-comptroller\backups`. Restart VS Code once after connecting.

**Disconnect** blocks Claude on that computer until you reconnect; it doesn't remove the settings, because that would send Claude Code straight to Anthropic. **Remove comptroller settings** in the right-click menu puts the original files back, so the computer talks to Anthropic directly again. Do that before uninstalling, or whenever a test server is stopped.

Build and try it without touching your real settings:

```powershell
cargo run --manifest-path connect\Cargo.toml -- --settings-root dev\sandbox --server http://localhost:8080
```

`--device-name <name>` registers the same computer as another device. Tests: `cargo test --manifest-path connect/Cargo.toml`, after building the service.

## Configuration

One file, `comptroller.json`. The admin terminal's Settings screen edits it; by hand, start from [contracts/comptroller.sample.json](contracts/comptroller.sample.json) and pass it with `--config <path>`. Changes apply while running, except the listen URL, admin port, shutdown timeout, data folder and logging. An invalid file is ignored and the last good one stays in use.

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
| `General.OrganizationName` | empty | Shown in the tray app |
| `Logging.Folder` · `Level` · `RetainedFiles` · `FileSizeLimitMb` | `logs` · `Information` · 14 · 20 | Level: `Verbose` to `Fatal`. Files kept 1–365, size 1–1024 MB |

The admin terminal also reads `Cost` (currency symbol, the day a billing month starts, the pricing file, and the seat plans with their billing period, price and renewal date) and `Dashboard` (refresh, and when latency and errors show as a warning). The service ignores both.

**API prices** come from [contracts/pricing.json](contracts/pricing.json), copied from [Anthropic's pricing page](https://platform.claude.com/docs/en/about-claude/pricing) on the date it gives; the Cost screen warns when it's over 90 days old. To change them, save a copy as `pricing.json` beside `comptroller.json`. Models are matched by family and version (`claude-opus-5-5-20261001` is Opus 5.5); tokens on a model that isn't in the table are listed, not priced.

In `manual` mode, create one-time codes on the admin terminal's Users screen (`v`, then `n`). Without the terminal, the service can print one:

```powershell
claude-comptroller-service.exe --config comptroller.json --create-code jdoe LAPTOP-1
```

## Known limits

- Only Claude Code traffic goes through the service. Claude Desktop's Chat and Cowork tabs don't.
- Remote Control is unavailable while `ANTHROPIC_BASE_URL` points anywhere but Anthropic.
- The Claude Desktop Code tab hasn't been tested.
- Claude Code sends some non-model traffic (version checks, telemetry, sign-in) directly to Anthropic.

## License

[MIT](LICENSE)
