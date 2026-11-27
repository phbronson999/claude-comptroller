# claude-comptroller

A self-hosted "control tower" for a small team's Claude Code traffic.

- An on-prem Windows service reverse-proxies every developer's Claude Code requests to Anthropic.
- It knows **who** each request belongs to: the developer's signed-in Windows user, per device.
- A server TUI installs, watches and configures it.
- A tray app on each developer computer connects that machine and can disconnect it.
- One channel to monitor, one thing to unplug.

"Comptroller" is pronounced "controller". It controls the traffic and keeps an eye on the books (usage and plan limits).

## Where things are written down

- `README.md`: what exists, how to run and configure it. Keep it current with each step.
- [`docs/design.md`](docs/design.md): the detailed design for identity, service, TUI, tray app, configuration and local testing. **Read the relevant section before building or changing that part**, and update it when a decision changes.
- `contracts/`: the database schema, sample config and HTTP contract shared by all components.
- `docs/testing-status.md`: what has been verified against the real API.

**Company-agnostic.** This is built for any small company to use, not one specific org. Nothing about a particular company (domain, hostnames, group names, branding) is hard-coded. All of it comes from config with neutral defaults, and the docs are written for a stranger setting it up for the first time.

## Hard requirements

- **Subscription billing only.** Developers use Claude **Team / Pro / Max seats** (their own claude.ai login) and stay on subscription billing, never API billing.
  - The proxy holds **no Anthropic credential**. It never adds, replaces or strips `Authorization`, `x-api-key` or any `anthropic-*` header; the OAuth beta value in `anthropic-beta` must reach Anthropic.
  - Developer machines set `ANTHROPIC_BASE_URL` and `ANTHROPIC_CUSTOM_HEADERS` only. Never set `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY` or `apiKeyHelper`, because any of them switches the session to API billing.
  - Verified in step 1 (Claude Code 2.1.330): subscription, `/status` meters and 1M context all work through a non-Anthropic base URL with the extra header. Re-check after major Claude Code upgrades; see https://github.com/anthropics/claude-code/issues/87876.
- **Single upstream:** `https://api.anthropic.com`. No multi-provider routing, and no LLM gateway products.
- **Self-hosted.** The only outbound connections are proxy → api.anthropic.com, plus the company's own directory (on-prem AD in the default mode).
- **Never log prompt or response bodies.** Record metadata only.
- **Windows-first.** The server runs as a native Windows Service and developer machines run Windows. macOS and Linux clients are out of scope for v1, but don't design them out.

## Three components

```
 developer PC                               server (Windows)
 ┌───────────────────────────────┐          ┌────────────────────────────────────────────┐
 │ claude-comptroller-connect    │ /_comptroller/connect (Windows auth, Negotiate)       │
 │ (Tauri v2 tray app)           │────────▶ │ claude-comptroller-service (C#, YARP)      │
 │  writes settings files        │          │  /_comptroller/* endpoints                 │
 │  disconnect / reconnect       │          │  catch-all proxy → api.anthropic.com       │
 └───────────────────────────────┘          │  token check · recording · retention       │
 ┌───────────────────────────────┐          │  /health on 127.0.0.1 only                 │
 │ Claude Code (VS Code, CLI,    │ every model request + X-Comptroller-Token             │
 │ Desktop Code tab)             │────────▶ │                                            │
 └───────────────────────────────┘          └────────────────────────────────────────────┘
                                             ┌────────────────────────────────────────────┐
                                             │ claude-comptroller (Rust, ratatui TUI)     │
                                             │  edits comptroller.json · SQLite: users    │
                                             │  controls service via SCM · /health        │
                                             └────────────────────────────────────────────┘
```

| Component | Language | Runs on | Exe |
| - | - | - | - |
| Service (proxy) | C# .NET 10, ASP.NET Core, YARP | server, as a Windows Service | `claude-comptroller-service.exe`, embedded in the TUI exe |
| TUI | Rust, ratatui | server, for admins | `claude-comptroller.exe`, the only command admins type |
| Connect (tray) | Rust + Tauri v2 | each developer PC | `claude-comptroller-connect.exe` |

The service and the TUI never call each other directly. They share `comptroller.json`, the SQLite DB, Windows Service Control, and the localhost `/health` endpoint. Contracts live in `contracts/` and are tested from all sides:
- `schema.sql`, with a `schema_version` table owned by the service
- `comptroller.sample.json`
- `api.md`, describing the `/_comptroller/*` endpoints and the error bodies

## Repo layout

```
service/          C# solution: Comptroller.Service
tui/              Rust crate: claude-comptroller (embeds the published service exe via include_bytes!)
connect/          Tauri v2 app: claude-comptroller-connect
contracts/        schema.sql, comptroller.sample.json, api.md
docs/mockups/     design exports: *.html (open in a browser) and png/*.png. The targets for look and copy.
tools/            mock-anthropic (fake upstream), seed-db
tests/e2e/        end-to-end tests against the mock upstream
dev/              dev comptroller.json
dev.ps1           one-command local test environment
build.ps1         publish service → build tui → build connect
README.md         for any company setting this up
LICENSE           MIT
```

## Build order

Each step should be shippable on its own.

- **Branches track progress:** one branch per step (for example `step/01-prove-path`, `step/04-connect-tray`). When its step works end to end, open a pull request. Changes reach `main` only through a PR, never a local merge. Label each PR with its type (`enhancement`, `bug` or `documentation`) and, for build steps, `step-NN`.
- **GitHub issues are only for real problems:** bugs found after something is built, or things noticed along the way that can wait. Don't open issues for planned steps.
- When you spot something that can wait, note it in your summary so it can become an issue. Don't fix it mid-step.

1. **Prove the path.** C# YARP pass-through from `comptroller.json`, run in a console on localhost:8080. From VS Code, the CLI and a Desktop Code-tab session, with `ANTHROPIC_BASE_URL` and a hand-set `ANTHROPIC_CUSTOM_HEADERS`, confirm:
   - streaming works
   - the subscription works
   - `/status` shows the usage meters
   - the header arrives
   Also build `tools/mock-anthropic` and `dev.ps1` in this step.
2. Identity: the devices and users tables, `/_comptroller/connect` with Negotiate **and** `manual` mode, the token check, stripping the header, Anthropic-format errors, `/me`, `/disconnect`, `/reconnect`, and the e2e tests.
3. Service mode + `/health`.
4. Connect tray app: first run, the settings writers, all popup states, disconnect/reconnect, refresh, start with Windows, icons.
5. TUI frame (Keepsake theme) + Terminal + status indicator + Install/Uninstall/Takeoff/Terminate/Status/Preflight.
6. Settings + Users.
7. Recording + Radar.
8. Dashboard (with Holding pattern) and the Cost screen. Token parsing lands in step 7 with recording.
9. Black box retention.
10. Archives + baseline.
11. `build.ps1`, release packaging, and a README written for any company.
12. `oidc` identity mode.

## Working rules

- .NET 10 LTS: pin Microsoft packages to 10.x, since `dotnet add` picks 11.
- Tests: `dotnet test service/Comptroller.slnx`. Local run: `dev.ps1` (mock upstream) or `dev.ps1 -Real`.
- This repo is public: no company names, hostnames or real user names in files.
- `docs/mockups/` is local only (gitignored).
