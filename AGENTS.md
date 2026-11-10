# Repository instructions

Read `docs/WORKING_RULES.md` and `docs/continuity/CURRENT_TARGET.md` before making changes.

- Explicit approval is required before implementing new scopes or merging.
- Keep Rust provider implementations isolated; do not accumulate provider-specific branches in one helper.
- Favor runnable behavior and focused evidence over excessive speculative tests.
- Make changes on feature branches and use pull requests. Do not merge without approval.
- Protect API keys and credentials; never commit runtime data or secrets.
- Keep documentation/continuity updated with each checkpoint.

## Codex visual-acceptance boundary (2026-11-10)

- **Do not delegate visual/UI acceptance to Codex.** Codex must not use computer-use, screenshots, remote desktop, GUI automation, or manual window inspection to perform or claim visual verification. Do not request permission to do so.
- **The user performs visual acceptance.** If visual inspection is required, stop at that gate, give the user concise manual verification steps, and record the result as **NOT VISUALLY VERIFIED** until the user explicitly reports acceptance.
- Non-visual work remains allowed: source review, implementation, automated tests (including widget/integration tests), static analysis, builds, CLI/API checks, artifact integrity, executable metadata, and logs. These checks are not substitutes for user-observed UI acceptance.
- Never assert that a window rendered correctly, an interaction looked right, or visual acceptance passed solely from test results, process existence, screenshots, or tool output.
