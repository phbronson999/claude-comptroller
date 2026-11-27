# Testing status

What has been verified against the real Anthropic API, and what hasn't.

| Client | Status | Notes |
| - | - | - |
| Claude Code CLI | Verified (step 1) | Streaming, subscription login, usage meters, 1M context, custom header arrives. |
| Claude Code in VS Code | Verified (step 1) | Via `claudeCode.environmentVariables` in the user `settings.json`. |
| Windows sign-in and device tokens | Verified (step 2) | Real API, signed in over loopback with a built-in group. A real AD group from a second computer is **untested**. |
| VS Code with a real device token | **Untested** | Step 1 used a placeholder header. |
| Running as a Windows Service | Verified (step 3) | Under a virtual account on a domain-joined machine, against the mock: `/health`, Windows sign-in over loopback, stop with a reply in flight, restart. Starting at boot and crash recovery are **untested**. |
| Tray app sign-in, settings files, disconnect and reconnect | Verified (step 4) | Automated tests and by hand, in a sandbox folder against the mock upstream: popup states, tray icons, menu, Windows sign-in through WinHTTP. Writing the **real** settings files by hand: **untested**. |
| Recording and token usage | **Untested against the real API** (step 7) | Automated tests against the mock: streamed, plain JSON and gzip-compressed replies. Whether real replies are compressed, and whether every real reply's usage is read, needs a `dev.ps1 -Real` session; Status shows how many replies today couldn't be read. |
| Claude Desktop, Code tab | **Untested** | Expected to work through the `env` block in `~/.claude/settings.json`, the same file the CLI reads. Not tried yet. |
