# claude-comptroller design

The detailed design, including parts not built yet. The hard requirements, component overview and build order are in [CLAUDE.md](../CLAUDE.md); what exists today is in the [README](../README.md).

## Identity: users and devices, not IPs

People use multiple computers, so identity is **user + device**.

- **Default mode, `windows`:** for on-prem AD or hybrid-joined machines.
  1. The connect app calls `GET /_comptroller/connect` using WinHTTP with automatic logon, so Kerberos/Negotiate signs in silently with the current Windows login and there is no prompt.
  2. The service authenticates it with ASP.NET Core Negotiate, reads `DOMAIN\user` and group SIDs, and checks membership of `Identity.AllowedGroup`.
  3. It issues a **device token**: 256-bit random. The server stores only its SHA-256 hash, plus user, device name, created, last seen, expires, and status (`active` / `suspended` / `revoked`).
- **Why a token at all:** Claude Code's own requests can't do Windows auth, because the `Authorization` header carries the developer's seat login. The token rides in `X-Comptroller-Token` via `ANTHROPIC_CUSTOM_HEADERS`.
- **Proxy check, on every request:**
  - Look up the token hash.
  - If the token is missing or unknown, return **401**. If it's suspended or expired, return **403**. Write both in Anthropic's error JSON (`{"type":"error","error":{"type":"permission_error","message":"…"}}`) so Claude Code shows a readable reason, for example: *"This computer isn't connected to claude-comptroller. Open claude-comptroller-connect."* or *"Disconnected from the tray on LAPTOP-1. Click Reconnect."*
  - **Strip `X-Comptroller-Token` before forwarding.**
  - `HEAD /api/hello` is the one proxied request that needs no token: Claude Code sends it without custom headers.
- **Refresh keeps the same token** and moves its expiry, so running sessions don't break. It never undoes a disconnect or suspension. The tray can reconnect only from its own disconnect, not an admin suspension. A failed group check expires all that user's tokens at once.
- `/_comptroller/*` calls require an `X-Comptroller-Client` header, so a web page can't use someone's Windows sign-in. Details: `contracts/api.md`.
- **Lifetime:** tokens expire after `Identity.TokenLifetimeDays` (default 7). The tray app refreshes silently at logon and daily, and each refresh rechecks the group. Removing someone from the group, or disabling their account, cuts them off at the next refresh or expiry.
- **Other modes** (design the interface now, implement after v1, so companies without AD can use it):
  - `oidc`: the tray app opens a browser sign-in (Entra, Okta or Google) and the group comes from a claim.
  - `manual` (**built in v1**, for offline testing and companies without AD; see Local testing): an admin creates the user and device in the TUI and pastes a one-time code into the tray app.
- All own endpoints live under the reserved `/_comptroller/` prefix, so they never collide with Anthropic API paths. Everything else is the proxy catch-all.

## Service (C#)

- **Stack:** .NET 10 (LTS; pin Microsoft packages to 10.x, since `dotnet add` picks 11), ASP.NET Core, **YARP** (`Yarp.ReverseProxy`, github.com/dotnet/yarp; use the latest stable), `Microsoft.AspNetCore.Authentication.Negotiate`, `Microsoft.Extensions.Hosting.WindowsServices`, `Microsoft.Data.Sqlite`, Serilog writing compact JSON to a rolling file.
- **Publish command:**
  ```
  dotnet publish -c Release -r win-x64 --self-contained -p:PublishSingleFile=true -p:IncludeNativeLibrariesForSelfExtract=true
  ```
- **Proxy:**
  - One route, `{**catch-all}`, to `Proxy.UpstreamUrl`. Outbound Host is the upstream host.
  - SSE must not be buffered. Set `ActivityTimeout` from config (default 10 min).
  - Use the `"X-Forwarded": "Off"` transform.
- **Middleware order:**
  1. CIDR allow-list (outer network fence)
  2. token check
  3. strip token header
  4. recording
  5. YARP
- **Recording:**
  - Captures: UTC time, user, device, client IP, method, path, status, duration ms, request/response bytes, upstream error flag.
  - **Usage too:**
    - Fields: model, `input_tokens`, `output_tokens`, `cache_creation_input_tokens` (split by 5 m / 1 h TTL when present), `cache_read_input_tokens`.
    - Read them **passively as the response streams through**, using a pass-through stream wrapper that never buffers or delays bytes:
      - for SSE, `message_start` carries the model and input/cache usage, and the final `message_delta` carries `output_tokens`
      - for non-streaming JSON, read `usage` from the body
    - Only the `usage` and `model` fields are parsed. Content is never stored or logged.
    - Skip `/v1/messages/count_tokens`.
    - If the usage can't be parsed, record nulls and count it on Status. Never fail the request.
  - Rows go through a `Channel<T>` to a background batch writer, so a request never waits on SQLite.
- **SQLite:**
  - `data/comptroller.db`, with WAL and `auto_vacuum=INCREMENTAL`.
  - Tables: `requests`, `users`, `devices`, `schema_version`.
  - Nightly prune at the configured time, then `PRAGMA incremental_vacuum`.
- **Config:**
  - `comptroller.json`, loaded with `IOptionsMonitor` and `reloadOnChange`. YARP reloads from the same file.
  - On an invalid file, keep the last good config and report it on `/health`.
- **Endpoints:**
  - `/_comptroller/connect`: Negotiate, issues or refreshes the device token.
  - `/_comptroller/me`: token auth. Returns user, device, status, expiry, and today's counts (requests, 429s, last request) for the tray popup.
  - `/_comptroller/disconnect`: token auth, suspends the device.
  - `/_comptroller/reconnect`: Negotiate. Rechecks the group, then reactivates.
  - `/_comptroller/download`: serves the connect installer and a one-page setup guide.
  - `GET /health` on a separate listener bound to `127.0.0.1:<adminPort>`: version, uptime, config status, schema version, listen URL.

## TUI (Rust, server)

- **Crates:** `ratatui` + `crossterm`, `rusqlite` (bundled), `serde`/`serde_json`, `windows-service` (service-manager client), `ipnet`, and `ureq` for `/health` and the preflight test.
- **Reuse Code Keepsake's TUI code directly** from https://github.com/phbronson999/Code-Keepsake → `crates/code-keepsake/src/tui/`:
  - `theme.rs`: copy as-is
  - `widgets.rs`: `allocate`, `stacked_bar`, `ratio_bar`, `keycap`, `section`, `spinner`, `centered`
  - the frame from `ui.rs`: header · cards · body · footer
- **One command for humans:** `claude-comptroller` opens the TUI. There are no subcommands; every action is on the home screen with a single-letter key.
- Each screen shows the command or config key it applies, so it doubles as documentation.

### Screens

The theme is used only where anyone understands it without aviation knowledge; everything else is plain. Mockups are in `docs/mockups/`.

| Key | Name | What it does |
| - | - | - |
| | **Terminal** | The home screen. Header: `claude-comptroller · Terminal` |
| `d` | Dashboard | Metric cards, requests per hour vs baseline, share by **user**, latency, recent requests |
| `r` | Radar | Live feed. In-flight requests show a spinner; 4xx/5xx are pink. Filter by user. |
| `$` | Cost | Subscription vs API pricing (see below) |
| `t` | Takeoff | Start the service |
| `x` | Terminate | Stop gracefully (in-flight requests finish), after a confirm |
| `s` | Status | Service, proxy and health checks, with the commands they use |
| `b` | Black box | The rolling recorded data: per-day size and requests, prune now |
| `a` | Archives | Create from a date range (summary by default, raw optional), set the baseline, delete. Summaries include token totals by user and model, and the API-equivalent and seat cost **with the pricing table snapshot used**, so old archives stay comparable after prices change. |
| `c` | Settings | Every configurable value |
| `v` | Users | Users with their devices: status, last seen, requests today. Suspend or revoke a device, revoke a user. Replaces the IP-based "Developers" mockup. |
| `p` | Preflight test | End-to-end check, including that streaming isn't buffered |
| `i` | Install | Write the embedded service exe, register it (delayed-auto start + crash recovery), default config, data folders. Shows each `sc.exe` equivalent. |
| `u` | Uninstall | Stop, delete the service, optionally remove data |

These keys are suggestions; finalize them so none clash.

- **Cost screen** (`docs/mockups/png/Cost.png`) answers "are our seats worth it compared with paying API prices?"
  - **Cards:**
    - seat cost for the period, prorated per user from their plan price
    - API-equivalent cost, meaning the same recorded tokens at list API prices
    - the difference, with an "N× cheaper" ratio
    - total tokens and the cache-read share
  - **Per user:** seat vs API bars, and a verdict ("seat saves $X" in lime, or "API cheaper by $X" in orange, which flags a light user who might be better off on the API).
  - **Breakdowns:** by model, and the token mix (input / output / cache write / cache read).
  - **Periods:** this month, last 30 days, **seat year to date** (from the annual renewal date), any archive range, plus the delta vs baseline.
  - **Annual seats** are paid up front. Spread their cost evenly per day across the seat year so any period gets a fair share, and label it, for example "annual billing · $X/mo effective".
  - The seat-year view also shows the whole year's seat cost against API-equivalent so far, plus a simple projection to renewal at the current daily rate. That's the number to look at before renewing.
  - A user added partway through the year is prorated from their start date.
  - The screen always states two caveats: it covers **Claude Code traffic only** (Chat and Cowork use the same seats but bypass the proxy), and the API figure is an **estimate at list prices** (no discounts, batch or priority tiers).
  - The Dashboard gets an "API equivalent today" figure, and `/_comptroller/me` returns today's API-equivalent so the tray popup can show it if enabled.
- **Holding pattern** is the dashboard card for 429s, shown per user. On subscription seats, a 429 usually means a plan limit.
- **Status indicator:** no banner. The header always shows a small colored dot and word:
  - lime `● running`
  - pink `● stopped`
  - orange spinner `starting…` / `stopping…` / `installing…`
  - faint `○ not installed`
  - orange `● config error`

## Connect tray app (Tauri v2, developer PCs)

- **Icon:** concept **A, "Radar C"** (`docs/mockups/png/TrayIcons.png`): an open radar ring forming a C, with a sweep line and a status-colored blip.
  - Colors: lime = connected, orange = refreshing or expiring soon, pink = disconnected or server unreachable, gray = not connected yet.
  - Disconnected also adds a slash, so it doesn't rely on color alone.
  - Ship `.ico` files at 16/20/24/32/48 px for each state.
- **Popup** (a small frameless window anchored above the tray; Keepsake palette; Segoe UI for text, JetBrains Mono for values) shows:
  - user, device and server
  - today's requests, holding pattern count and last request (from `/_comptroller/me`)
  - token expiry with **Refresh now**
  - checks that both settings files are in place
  - one large **Disconnect / Reconnect** button
  - States: connected, disconnected, server unreachable (with **Retry now**), and first run. See the mockups.
- **Disconnect** calls `/_comptroller/disconnect`, so the server suspends the token. **It never removes `ANTHROPIC_BASE_URL`**, because that would send Claude Code straight to Anthropic. A disconnected or unreachable server means Claude doesn't work on that machine.
- **Reconnect** re-runs the silent Windows sign-in, which rechecks the group.
- **Settings it writes and keeps correct:**
  - `%USERPROFILE%\.claude\settings.json` → the `env` block with `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS` (`X-Comptroller-Token: …`). This covers the CLI, background agents and Desktop Code-tab local sessions.
  - VS Code user `settings.json` → `claudeCode.environmentVariables` with the same two entries. The VS Code extension needs this one; the docs say values in `~/.claude/settings.json` don't reach the extension's own login check.
  - Merge into both files without overwriting anything else, and keep a backup.
  - Tell the user to restart VS Code once after first connect.
- **Startup:** HKCU Run key (on by default, toggle in the right-click menu). Quitting the tray app does **not** disconnect.
- **Right-click menu:** Open · Refresh token · Copy diagnostics · Open log folder · Start with Windows ✓ · Disconnect · Quit.
- **Silent auth:** WinHTTP via the `windows` crate with automatic logon (Negotiate) against the server's hostname. Explain clearly when the machine isn't domain-joined.

## Known limits (document them in the README)

- **Claude Desktop Chat and Cowork tabs don't go through the proxy.** Desktop routes those only through its own third-party inference setting, which is meant for gateway credentials rather than seats. Only Claude Code traffic (VS Code, CLI, Desktop Code tab) is covered.
- **Remote Control is unavailable** while `ANTHROPIC_BASE_URL` points at a non-Anthropic host. This is intended.
- **Optional hardening:** deliver `ANTHROPIC_BASE_URL` plus `allowedProviders: ["customEndpoint"]` through managed settings (Intune or GPO), so Claude Code refuses any other destination even if someone edits their settings. Requires Claude Code v2.1.285 or later. Document it; it's not required.
- Claude Code sends some non-model traffic (version checks, telemetry, sign-in) directly to Anthropic. That's expected.

## Configuration

Rule: **nothing operational or company-specific is hard-coded.**

- Everything lives in `comptroller.json`. The TUI edits it with validation, keeps a `.bak`, and writes atomically.
- Settings that need a restart are marked, and the TUI offers to restart the service.
- Each setting shows its current value, default, and a one-line explanation, with reset to default.

| Group | Setting | Default | Notes |
| - | - | - | - |
| General | Organization display name | empty | shown in the tray popup and TUI header if set |
| Proxy | Public URL | `https://comptroller.example.internal` | what clients use; restart |
| Proxy | Listen URL | `http://localhost:8080` | restart |
| Proxy | Admin (health) port | 8081 | 127.0.0.1 only; restart |
| Proxy | Upstream URL | `https://api.anthropic.com` | locked behind a confirm |
| Proxy | Activity timeout | 10 min | 1–60 |
| Proxy | Strip X-Forwarded headers | on | |
| Access | Allowed client CIDRs | `127.0.0.1/32`, `::1/128` | outer fence; warn if empty |
| TLS | Certificate (store thumbprint or .pfx) | none | restart |
| Identity | Mode | `windows` | `windows` now; `oidc` and `manual` later |
| Identity | Allowed group | `Claude Code Users` | name or SID |
| Identity | Token lifetime | 7 days | 1–90 |
| Identity | Token header name | `X-Comptroller-Token` | |
| Identity | One-time code lifetime | 24 hours | 1–168, manual mode |
| Cost | Pricing table | **none shipped as fact** | `pricing.json`, per model: input, output, cache write (5 m and 1 h), cache read, per million tokens, plus optional long-context tier thresholds and rates. Includes "last updated" and a source URL. The TUI warns when it's older than 90 days. Fill it from Anthropic's published pricing page at build time; never invent or remember prices. Editable in Settings. |
| Cost | Seat plans | Team standard, annual | named plans (for example Team standard, Team premium, Pro, Max). Each has a **billing period**, `annual` (the default) or `monthly`, a price for that period, and for annual plans a **renewal date**. Prices are entered by the admin. Plans are assigned per user on the Users screen. |
| Cost | Currency symbol · month start day | `$` · 1 | display only |
| Data | Data folder · Retention days · Nightly prune time | `.\data` · 14 · 02:00 | 1–365 days |
| Archives | Archive folder · Default content · Baseline | `.\data\archives` · summary · none | |
| Logging | Folder · Level · File retention | `.\logs` · Information · 14 × 20 MB | |
| Dashboard | Refresh · Radar rows · p95 warning · error-rate warning | 2 s · 20 · 10 s · 5% | |
| Service | Name / display name · Crash restart delays | `claude-comptroller` · 5 s, 5 s, 30 s | |

The mockups use sample names (`CORP\jdoe`, `comptroller.internal`, `dev-01`). Docs and defaults use neutral examples such as `example.internal`.

## Local testing: the whole suite on one laptop

Requirement: the service, TUI and tray app must all run and be testable on a single Windows laptop, offline from the office where possible. They must also be testable without burning seat usage or breaking the developer's own Claude Code setup.

- **Run modes.**
  - The service runs in a plain console (`dotnet run`, or the exe without `--service`) as well as installed as a Windows Service.
  - The TUI works against either. When no service is installed, it detects the console instance through `/health`, and shows that it's in "console mode" rather than failing.
- **Loopback works.** On `http://localhost:8080` there's no TLS, cert or firewall rule to deal with.
  - Negotiate on loopback falls back to NTLM, which is fine.
  - `windows` identity mode must work on a domain-joined laptop against localhost, including the group check. Allow a **local** group, for example a `ComptrollerTest` local group, as `Identity.AllowedGroup`.
- **`manual` identity mode is built in v1, not later.** It's the offline and no-AD fallback: the admin creates a user and device in the TUI, and the tray app takes a one-time code. That covers testing on a laptop away from the domain, and small companies without AD.
- **Mock upstream.** Ship `tools/mock-anthropic`, a tiny local server that:
  - answers `/v1/messages` with a realistic SSE stream (including pauses, keep-alive pings, and `usage` in `message_start` / `message_delta`), a non-streaming JSON mode, `/v1/models`, and configurable 401/429/5xx responses
  - echoes the request headers it received
  
  Point `Proxy.UpstreamUrl` at it so tests never touch Anthropic or use any seat's limits. Only the final manual check uses the real API.
- **Sandboxed client settings.** The tray app takes `--settings-root <dir>` (or an env var) so it writes the Claude Code and VS Code settings files into a test folder instead of the real `%USERPROFILE%` and `%APPDATA%`. When it does write the real files:
  - it backs them up first
  - the right-click menu gets **Remove comptroller settings**, which restores the originals, so the laptop can always go back to talking to Anthropic directly
  - the README spells this out, because a stopped test service means Claude Code on that laptop stops working until the settings are removed
- **Fake extra devices.** `--device-name <name>` lets one laptop register as several devices, so the Users screen and per-device disconnect can be tested.
- **Seed data.** `tools/seed-db` fills the SQLite DB with realistic multi-user, multi-day traffic, so the Dashboard, Radar, Black box, Archives and baseline can be checked without waiting two weeks.
- **One script.** `dev.ps1` starts the mock upstream, runs the service in a console against `dev/comptroller.json`, optionally seeds the DB, and prints the commands for the TUI and the tray app in sandbox mode. `dev.ps1 -Real` uses the real upstream for the final manual check.
- **Automated end-to-end tests** (`tests/e2e`, run in CI on a Windows runner and locally) against the mock upstream:
  - streaming chunks arrive unbuffered
  - `Authorization` and `anthropic-beta` reach the upstream untouched
  - `X-Comptroller-Token` is stripped
  - a missing token gets 401, and a suspended token gets 403 in Anthropic error format
  - disconnect then reconnect works
  - rows are recorded
  - token usage is parsed correctly from both streaming and non-streaming responses
  - the API-equivalent cost math matches a hand-computed fixture
  - retention prunes old rows
  - an invalid config keeps the last good one
- **Unit tests:** the Rust TUI tests run against `contracts/` samples. The tray app's settings writer is tested against temp folders, including merging with existing settings and restoring the backup.

## Open decisions

- Using "Claude" in a publicly published project name: check Anthropic's brand guidelines before publishing, and include a "not affiliated with Anthropic" line in the README.
