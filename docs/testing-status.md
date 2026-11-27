# Testing status

What has been verified against the real Anthropic API, and what hasn't.

| Client | Status | Notes |
| - | - | - |
| Claude Code CLI | Verified (step 1) | Streaming, subscription login, usage meters, 1M context, custom header arrives. |
| Claude Code in VS Code | Verified (step 1) | Via `claudeCode.environmentVariables` in the user `settings.json`. |
| Windows sign-in and device tokens | Verified (step 2) | Real API, signed in over loopback with a built-in group. A real AD group from a second computer is **untested**. |
| VS Code with a real device token | **Untested** | Step 1 used a placeholder header. |
| Claude Desktop, Code tab | **Untested** | Expected to work through the `env` block in `~/.claude/settings.json`, the same file the CLI reads. Not tried yet. |
