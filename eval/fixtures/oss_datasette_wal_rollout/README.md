# SQLite event store

The service opens a small writable SQLite database from multiple worker
processes. Deployments currently rely on SQLite's default rollback journal.
