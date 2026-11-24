# Security

Please report a vulnerability privately through this repository's
**Security → Report a vulnerability** (GitHub private advisory), not in a public issue.

CashCraft keeps your data in a local SQLite file and makes no network requests. The MCP
server (`cashcraft-cli mcp`) only reads files inside `--automation-read-root` and only writes
inside `--automation-write-root`; a way around that is a security bug.
