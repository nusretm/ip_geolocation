# Working Rules

1. User approval gates implementation scope and merges.
2. Use feature branches and PRs; no direct application commits to main.
3. Prioritize working Rust code, realistic executions, and focused tests.
4. Keep future providers independently addable/removable and responsible for their own database updates.
5. Cross-platform core; Windows Service will be the first deployment target. Linux support should remain possible.
6. Both trusted TCP source IP and API key are required. Never trust unsanitized forwarded headers.
7. Do not commit API keys, credentials or downloaded runtime databases.
8. Update `docs/continuity/CURRENT_TARGET.md` on each development checkpoint.
