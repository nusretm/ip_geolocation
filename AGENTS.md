# Repository instructions

Read `docs/WORKING_RULES.md` and `docs/continuity/CURRENT_TARGET.md` before making changes.

- Explicit approval is required before implementing new scopes or merging.
- Keep Rust provider implementations isolated; do not accumulate provider-specific branches in one helper.
- Favor runnable behavior and focused evidence over excessive speculative tests.
- Make changes on feature branches and use pull requests. Do not merge without approval.
- Protect API keys and credentials; never commit runtime data or secrets.
- Keep documentation/continuity updated with each checkpoint.
